//! Конфигурация приложения

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Версия формата архива
pub const ARCHIVE_FORMAT_VERSION: u32 = 1;
/// Расширение файлов архивов
pub const ARCHIVE_EXTENSION: &str = "rmm";

/// Настройки приложения
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Путь к базе данных
    pub database_path: PathBuf,
    /// Путь к журналу операций
    pub log_path: PathBuf,
    /// Путь к временным файлам
    pub temp_dir: PathBuf,
    /// Максимальный размер лога в МБ
    pub max_log_size_mb: u64,
    /// Таймаут SSH соединения в секундах
    pub ssh_timeout_secs: u32,
    /// Количество попыток подключения SSH
    pub ssh_retry_count: u32,
    /// Скорость ограничения для SSH передачи (байт/сек), 0 = без ограничений
    pub ssh_rate_limit: u64,
    /// Использовать сжатие при SSH передаче
    pub ssh_compression: bool,
    /// Уровень сжатия zstd (0-22)
    pub compression_level: i32,
    /// Алгоритм шифрования по умолчанию
    pub encryption_algorithm: EncryptionAlgorithm,
    /// Язык интерфейса
    pub language: String,
    /// Тема оформления
    pub theme: Theme,
    /// Автоматически создавать резервные копии
    pub auto_backup: bool,
    /// Хранить историю операций (дней)
    pub history_days: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"));
        let data_dir = PathBuf::from("/var/local")
            .join("migration-master");

        Self {
            database_path: data_dir.join("migrations.db"),
            log_path: data_dir.join("migration.log"),
            temp_dir: PathBuf::from("/tmp/migration-master"),
            max_log_size_mb: 10,
            ssh_timeout_secs: 30,
            ssh_retry_count: 3,
            ssh_rate_limit: 0,
            ssh_compression: true,
            compression_level: 3,
            encryption_algorithm: EncryptionAlgorithm::Age,
            language: "ru".to_string(),
            theme: Theme::System,
            auto_backup: true,
            history_days: 90,
        }
    }
}

/// Алгоритм шифрования
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EncryptionAlgorithm {
    /// Age encryption
    Age,
    /// GPG encryption
    Gpg,
}

/// Тема оформления
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Системная тема
    System,
    /// Светлая тема
    Light,
    /// Тёмная тема
    Dark,
}

/// Режим миграции
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationMode {
    /// Локальный архив
    LocalArchive,
    /// Прямая миграция по SSH
    SshDirect,
    /// Восстановление из архива
    Restore,
}

/// Профиль миграции
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationProfile {
    pub name: String,
    pub mode: MigrationMode,
    pub components: Vec<ComponentType>,
    pub exclusions: Vec<String>,
    pub include_hidden: bool,
    pub compression_level: i32,
    pub encrypt: bool,
}

/// Типы компонентов для переноса
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentType {
    /// Рабочий стол
    Desktop,
    /// Документы
    Documents,
    /// Загрузки
    Downloads,
    /// Изображения
    Pictures,
    /// Видео
    Videos,
    /// Музыка
    Music,
    /// Шаблоны
    Templates,
    /// Пользовательские каталоги
    CustomDirs,
    /// Конфигурация приложений
    AppConfigs,
    /// Локальные данные приложений
    AppData,
    /// Локальные бинарники
    LocalBin,
    /// Локальные приложения
    LocalApps,
    /// Темы оформления
    Themes,
    /// Иконки
    Icons,
    /// Шрифты
    Fonts,
    /// SSH ключи и настройки
    SshKeys,
    /// Настройки принтеров
    Printers,
    /// Список установленных пакетов
    Packages,
    /// Системные настройки
    SystemSettings,
    /// Пользовательские cron задачи
    CronJobs,
    /// Пользовательские systemd сервисы
    UserServices,
}

impl ComponentType {
    pub fn default_components() -> Vec<Self> {
        vec![
            Self::Desktop,
            Self::Documents,
            Self::Downloads,
            Self::Pictures,
            Self::Videos,
            Self::Music,
            Self::Templates,
            Self::AppConfigs,
            Self::SshKeys,
        ]
    }

    pub fn all_components() -> Vec<Self> {
        vec![
            Self::Desktop,
            Self::Documents,
            Self::Downloads,
            Self::Pictures,
            Self::Videos,
            Self::Music,
            Self::Templates,
            Self::CustomDirs,
            Self::AppConfigs,
            Self::AppData,
            Self::LocalBin,
            Self::LocalApps,
            Self::Themes,
            Self::Icons,
            Self::Fonts,
            Self::SshKeys,
            Self::Printers,
            Self::Packages,
            Self::SystemSettings,
            Self::CronJobs,
            Self::UserServices,
        ]
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Desktop => "Файлы рабочего стола",
            Self::Documents => "Документы",
            Self::Downloads => "Загрузки",
            Self::Pictures => "Изображения",
            Self::Videos => "Видео",
            Self::Music => "Музыка",
            Self::Templates => "Шаблоны",
            Self::CustomDirs => "Пользовательские каталоги",
            Self::AppConfigs => "Настройки приложений (.config)",
            Self::AppData => "Данные приложений (.local/share)",
            Self::LocalBin => "Локальные бинарники (.local/bin)",
            Self::LocalApps => "Локальные приложения (.local/share/applications)",
            Self::Themes => "Темы оформления (.themes)",
            Self::Icons => "Иконки (.icons)",
            Self::Fonts => "Шрифты (.fonts)",
            Self::SshKeys => "SSH ключи и настройки",
            Self::Printers => "Настройки принтеров",
            Self::Packages => "Список установленных пакетов",
            Self::SystemSettings => "Системные настройки",
            Self::CronJobs => "Cron задачи пользователя",
            Self::UserServices => "Пользовательские systemd сервисы",
        }
    }

    pub fn risk_level(&self) -> RiskLevel {
        match self {
            Self::Desktop
            | Self::Documents
            | Self::Downloads
            | Self::Pictures
            | Self::Videos
            | Self::Music
            | Self::Templates => RiskLevel::Low,
            Self::CustomDirs
            | Self::AppConfigs
            | Self::AppData
            | Self::LocalBin
            | Self::LocalApps
            | Self::Themes
            | Self::Icons
            | Self::Fonts => RiskLevel::Medium,
            Self::SshKeys | Self::Printers | Self::Packages | Self::CronJobs | Self::UserServices => {
                RiskLevel::High
            }
            Self::SystemSettings => RiskLevel::Critical,
        }
    }
}

/// Уровень риска операции
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    /// Низкий риск
    Low,
    /// Средний риск
    Medium,
    /// Высокий риск
    High,
    /// Критический риск
    Critical,
}

impl RiskLevel {
    pub fn color(&self) -> &'static str {
        match self {
            Self::Low => "success",
            Self::Medium => "warning",
            Self::High => "error",
            Self::Critical => "danger",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Low => "Безопасная операция",
            Self::Medium => "Требуется внимание",
            Self::High => "Высокий риск, требуется подтверждение",
            Self::Critical => "Критическая операция, требуется явное подтверждение",
        }
    }
}

/// Исключения по умолчанию
pub const DEFAULT_EXCLUSIONS: &[&str] = &[
    "**/.cache/**",
    "**/Trash/**",
    "**/.trash/**",
    "**/*.tmp",
    "**/*.temp",
    "**/*.swp",
    "**/*.swo",
    "**/*~",
    "**/.git/**",
    "**/node_modules/**",
    "**/__pycache__/**",
    "**/*.pyc",
    "**/.venv/**",
    "**/venv/**",
    "**/.mozilla/firefox/*/cache/**",
    "**/.mozilla/firefox/*/Cache/**",
    "**/.cache/mozilla/**",
    "**/.chromium/**/Cache/**",
    "**/.config/chromium/**/Cache/**",
    "**/.config/google-chrome/**/Cache/**",
    "**/.local/share/Trash/**",
];

/// Получение пути к конфигурации
pub fn get_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/etc"))
        .join("migration-master")
}

/// Получение пути к данным приложения
pub fn get_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/var/local"))
        .join("migration-master")
}

/// Загрузка конфигурации из файла
pub fn load_config() -> Result<AppConfig, Box<dyn std::error::Error>> {
    let config_path = get_config_dir().join("config.toml");
    
    if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        let config: AppConfig = toml::from_str(&content)?;
        Ok(config)
    } else {
        Ok(AppConfig::default())
    }
}

/// Сохранение конфигурации в файл
pub fn save_config(config: &AppConfig) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = get_config_dir();
    std::fs::create_dir_all(&config_dir)?;
    
    let config_path = config_dir.join("config.toml");
    let content = toml::to_string_pretty(config)?;
    std::fs::write(config_path, content)?;
    
    Ok(())
}
