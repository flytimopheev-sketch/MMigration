//! Единый тип ошибок приложения.
//!
//! Все ошибки содержат понятное пользователю описание (на русском языке) и
//! технические детали, которые можно показать в подробном журнале.

use std::path::Path;

use thiserror::Error;

/// Категория ошибки — используется интерфейсом для выбора реакции
/// (повторить, показать предупреждение, запросить действие пользователя).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    /// Ошибка ввода пользователя — нужно исправить данные и повторить.
    UserInput,
    /// Ошибка окружения (нет команды, нет прав, нет места).
    Environment,
    /// Сетевая ошибка (SSH, CUPS, репозитории).
    Network,
    /// Внутренняя ошибка приложения.
    Internal,
    /// Операция отменена пользователем.
    Cancelled,
}

/// Общий тип ошибок Migration Master.
#[derive(Error, Debug)]
pub enum MigrationError {
    #[error("Ошибка ввода-вывода: {0}")]
    Io(#[from] std::io::Error),

    #[error("Ошибка формата JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Ошибка разбора TOML: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("Ошибка записи TOML: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("Ошибка разбора YAML: {0}")]
    Yaml(String),

    #[error("Ошибка базы данных: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Ошибка обхода каталогов: {0}")]
    WalkDir(#[from] walkdir::Error),

    #[error("Ошибка SSH: {0}")]
    Ssh(String),

    #[error("Ошибка шифрования: {0}")]
    Encryption(String),

    #[error("Ошибка расшифрования: {0}")]
    Decryption(String),

    #[error("Ошибка хеширования: {0}")]
    Hashing(String),

    #[error("Файл или каталог не найден: {0}")]
    FileNotFound(String),

    #[error("Некорректные данные: {0}")]
    InvalidInput(String),

    #[error("Недостаточно прав доступа: {0}")]
    PermissionDenied(String),

    #[error("Недостаточно места на диске: требуется {required}, доступно {available}")]
    InsufficientSpace {
        /// Сколько байт требуется
        required: u64,
        /// Сколько байт доступно
        available: u64,
    },

    #[error("Ошибка сети: {0}")]
    Network(String),

    #[error("Таймаут операции: {0}")]
    Timeout(String),

    #[error("Операция отменена пользователем")]
    Cancelled,

    #[error("Конфликт файлов: {0}")]
    Conflict(String),

    #[error("Несовместимость: {0}")]
    Incompatible(String),

    #[error("Контрольная сумма не совпадает: {0}")]
    ChecksumMismatch(String),

    #[error("Повреждённый архив: {0}")]
    CorruptedArchive(String),

    #[error("Обнаружена попытка выхода за пределы целевого каталога (path traversal): {0}")]
    PathTraversal(String),

    #[error("Ошибка выполнения внешней команды ({command}): {message}")]
    Command {
        /// Имя команды
        command: String,
        /// Сообщение об ошибке (вывод команды)
        message: String,
    },

    #[error("Требуется внешний инструмент, но он недоступен: {0}")]
    Unsupported(String),

    #[error("Требуются права администратора: {0}")]
    PrivilegedRequired(String),

    #[error("Неизвестная ошибка: {0}")]
    Unknown(String),
}

impl MigrationError {
    /// Категория ошибки для интерфейса.
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::InvalidInput(_) | Self::Conflict(_) => ErrorCategory::UserInput,
            Self::Network(_) | Self::Ssh(_) | Self::Timeout(_) => ErrorCategory::Network,
            Self::InsufficientSpace { .. }
            | Self::PermissionDenied(_)
            | Self::Unsupported(_)
            | Self::PrivilegedRequired(_)
            | Self::FileNotFound(_) => ErrorCategory::Environment,
            Self::Cancelled => ErrorCategory::Cancelled,
            _ => ErrorCategory::Internal,
        }
    }

    /// Понятное пользователю сообщение (без технических деталей).
    pub fn user_message(&self) -> String {
        match self {
            Self::Io(e) => format!("Ошибка работы с файлами: {}", e),
            Self::InsufficientSpace {
                required,
                available,
            } => format!(
                "Недостаточно места: требуется {}, доступно {}",
                human_bytes::human_bytes(*required as f64),
                human_bytes::human_bytes(*available as f64)
            ),
            Self::PrivilegedRequired(action) => format!(
                "Для действия «{}» нужны права администратора. Запрос будет выполнен через polkit.",
                action
            ),
            other => other.to_string(),
        }
    }

    /// Технические детали для подробного журнала.
    pub fn technical_details(&self) -> String {
        format!("{:#?}", self)
    }

    /// Является ли ошибка отменой операции.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// Создать ошибку «файл не найден».
    pub fn not_found(path: impl AsRef<Path>) -> Self {
        Self::FileNotFound(path.as_ref().display().to_string())
    }

    /// Создать ошибку path traversal.
    pub fn traversal(path: impl AsRef<Path>) -> Self {
        Self::PathTraversal(path.as_ref().display().to_string())
    }

    /// Ошибка отсутствующей функции окружения (нет утилиты).
    pub fn missing_tool(tool: &str) -> Self {
        Self::Unsupported(tool.to_string())
    }
}

/// Результат операций Migration Master.
pub type Result<T> = std::result::Result<T, MigrationError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_categories() {
        assert_eq!(
            MigrationError::Cancelled.category(),
            ErrorCategory::Cancelled
        );
        assert_eq!(
            MigrationError::Ssh("нет связи".into()).category(),
            ErrorCategory::Network
        );
        assert_eq!(
            MigrationError::InsufficientSpace {
                required: 10,
                available: 1
            }
            .category(),
            ErrorCategory::Environment
        );
    }

    #[test]
    fn test_user_message_is_human_readable() {
        let err = MigrationError::InsufficientSpace {
            required: 1024,
            available: 512,
        };
        let message = err.user_message();
        assert!(message.starts_with("Недостаточно места:"));
        assert!(!message.contains("InsufficientSpace"));
    }

    #[test]
    fn test_helpers() {
        let err = MigrationError::not_found("/tmp/missing.rmm");
        assert!(err.to_string().contains("/tmp/missing.rmm"));
        assert!(MigrationError::Cancelled.is_cancelled());
    }
}

