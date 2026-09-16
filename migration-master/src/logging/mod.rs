//! Модуль логирования операций

use std::path::{Path, PathBuf};
use std::fs::{File, OpenOptions};
use std::io::{Write, BufWriter};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::error::{MigrationError, Result};

/// Уровни логирования
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Отладочная информация
    Debug,
    /// Информационное сообщение
    Info,
    /// Предупреждение
    Warning,
    /// Ошибка
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::Error => "ERROR",
        }
    }
}

/// Запись журнала
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Временная метка
    pub timestamp: DateTime<Utc>,
    /// Уровень логирования
    pub level: LogLevel,
    /// Категория события
    pub category: String,
    /// Сообщение
    pub message: String,
    /// Дополнительные данные (JSON)
    pub data: Option<serde_json::Value>,
}

impl LogEntry {
    pub fn new(level: LogLevel, category: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            level,
            category: category.into(),
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn format(&self) -> String {
        let timestamp = self.timestamp.format("%Y-%m-%d %H:%M:%S%.3f");
        let data_str = self.data.as_ref()
            .map(|d| format!(" | {}", d))
            .unwrap_or_default();
        
        format!(
            "[{}] [{}] [{}] {}{}",
            timestamp,
            self.level.as_str(),
            self.category,
            self.message,
            data_str
        )
    }
}

/// Менеджер журнала операций
pub struct LogManager {
    log_path: PathBuf,
    max_size_bytes: u64,
    writer: Option<BufWriter<File>>,
}

impl LogManager {
    /// Создание нового менеджера журнала
    pub fn new(log_path: impl AsRef<Path>, max_size_mb: u64) -> Result<Self> {
        let log_path = log_path.as_ref().to_path_buf();
        
        // Создаём директорию если нужно
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let max_size_bytes = max_size_mb * 1024 * 1024;

        let mut manager = Self {
            log_path,
            max_size_bytes,
            writer: None,
        };

        // Проверяем размер и ротируем если нужно
        manager.rotate_if_needed()?;
        
        Ok(manager)
    }

    /// Ротация журнала если размер превышен
    fn rotate_if_needed(&mut self) -> Result<()> {
        if !self.log_path.exists() {
            return Ok(());
        }

        let metadata = std::fs::metadata(&self.log_path)?;
        if metadata.len() > self.max_size_bytes {
            self.rotate()?;
        }

        Ok(())
    }

    /// Ротация журнала
    fn rotate(&mut self) -> Result<()> {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let rotated_path = self.log_path.with_extension(format!("{}.log", timestamp));
        
        std::fs::rename(&self.log_path, &rotated_path)?;
        
        // Удаляем старые логи (оставляем последние 5)
        self.cleanup_old_logs()?;
        
        Ok(())
    }

    /// Очистка старых логов
    fn cleanup_old_logs(&self) -> Result<()> {
        if let Some(parent) = self.log_path.parent() {
            let pattern = self.log_path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("migration");
            
            let mut log_files: Vec<_> = std::fs::read_dir(parent)?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with(pattern) && n.ends_with(".log"))
                        .unwrap_or(false)
                })
                .filter_map(|e| {
                    e.path()
                        .metadata()
                        .ok()
                        .map(|m| (e.path(), m.modified().ok()))
                })
                .collect();

            // Сортируем по времени модификации
            log_files.sort_by(|a, b| {
                let time_a = a.1.and_then(|x| x).unwrap_or(std::time::UNIX_EPOCH);
                let time_b = b.1.and_then(|x| x).unwrap_or(std::time::UNIX_EPOCH);
                time_b.cmp(&time_a)
            });

            // Удаляем всё кроме последних 5
            for (path, _) in log_files.iter().skip(5) {
                let _ = std::fs::remove_file(path);
            }
        }

        Ok(())
    }

    /// Получение писателя для журнала
    fn get_writer(&mut self) -> Result<&mut BufWriter<File>> {
        if self.writer.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log_path)?;
            self.writer = Some(BufWriter::new(file));
        }
        Ok(self.writer.as_mut().unwrap())
    }

    /// Запись записи в журнал
    pub fn log(&mut self, entry: LogEntry) -> Result<()> {
        let writer = self.get_writer()?;
        writeln!(writer, "{}", entry.format())?;
        writer.flush()?;
        Ok(())
    }

    /// Логирование отладочного сообщения
    pub fn debug(&mut self, category: impl Into<String>, message: impl Into<String>) -> Result<()> {
        self.log(LogEntry::new(LogLevel::Debug, category, message))
    }

    /// Логирование информационного сообщения
    pub fn info(&mut self, category: impl Into<String>, message: impl Into<String>) -> Result<()> {
        self.log(LogEntry::new(LogLevel::Info, category, message))
    }

    /// Логирование предупреждения
    pub fn warning(&mut self, category: impl Into<String>, message: impl Into<String>) -> Result<()> {
        self.log(LogEntry::new(LogLevel::Warning, category, message))
    }

    /// Логирование ошибки
    pub fn error(&mut self, category: impl Into<String>, message: impl Into<String>) -> Result<()> {
        self.log(LogEntry::new(LogLevel::Error, category, message))
    }

    /// Логирование ошибки с данными
    pub fn error_with_data(
        &mut self,
        category: impl Into<String>,
        message: impl Into<String>,
        data: serde_json::Value,
    ) -> Result<()> {
        let entry = LogEntry::new(LogLevel::Error, category, message).with_data(data);
        self.log(entry)
    }

    /// Чтение журнала
    pub fn read_logs(&self, lines: usize) -> Result<Vec<String>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&self.log_path)?;
        let lines: Vec<String> = content
            .lines()
            .rev()
            .take(lines)
            .map(|s| s.to_string())
            .collect();
        
        Ok(lines.into_iter().rev().collect())
    }

    /// Очистка журнала
    pub fn clear(&mut self) -> Result<()> {
        if self.log_path.exists() {
            std::fs::remove_file(&self.log_path)?;
        }
        self.writer = None;
        Ok(())
    }

    /// Путь к файлу журнала
    pub fn path(&self) -> &Path {
        &self.log_path
    }
}

impl Drop for LogManager {
    fn drop(&mut self) {
        if let Some(ref mut writer) = self.writer {
            let _ = writer.flush();
        }
    }
}

/// Глобальный экземпляр логгера (ленивая инициализация)
use std::sync::Mutex;
use once_cell::sync::Lazy;

static GLOBAL_LOGGER: Lazy<Mutex<Option<LogManager>>> = Lazy::new(|| Mutex::new(None));

/// Инициализация глобального логгера
pub fn init_logger(log_path: impl AsRef<Path>, max_size_mb: u64) -> Result<()> {
    let logger = LogManager::new(log_path, max_size_mb)?;
    *GLOBAL_LOGGER.lock().unwrap() = Some(logger);
    Ok(())
}

/// Получение доступа к глобальному логгеру
pub fn with_logger<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut LogManager) -> R,
{
    let mut guard = GLOBAL_LOGGER.lock().unwrap();
    guard.as_mut().map(f)
}

/// Макросы для удобного логирования
#[macro_export]
macro_rules! log_debug {
    ($category:expr, $message:expr $(, $args:expr)*) => {
        $crate::logging::with_logger(|logger| {
            let _ = logger.debug($category, format!($message $(, $args)*));
        });
    };
}

#[macro_export]
macro_rules! log_info {
    ($category:expr, $message:expr $(, $args:expr)*) => {
        $crate::logging::with_logger(|logger| {
            let _ = logger.info($category, format!($message $(, $args)*));
        });
    };
}

#[macro_export]
macro_rules! log_warning {
    ($category:expr, $message:expr $(, $args:expr)*) => {
        $crate::logging::with_logger(|logger| {
            let _ = logger.warning($category, format!($message $(, $args)*));
        });
    };
}

#[macro_export]
macro_rules! log_error {
    ($category:expr, $message:expr $(, $args:expr)*) => {
        $crate::logging::with_logger(|logger| {
            let _ = logger.error($category, format!($message $(, $args)*));
        });
    };
}
