//! Migration Master - Мастер миграции пользователя для РЕД ОС Linux
//! 
//! Приложение предназначено для безопасного переноса пользовательского профиля
//! со старого компьютера или старой установки на новый компьютер.

pub mod config;
pub mod database;
pub mod logging;
pub mod security;
pub mod profile_scanner;
pub mod archive;
pub mod ssh_transfer;
pub mod file_transfer;
pub mod conflict_resolver;
pub mod ssh_keys;
pub mod applications;
pub mod printers;
pub mod packages;
pub mod system_settings;
pub mod compatibility;
pub mod backup;
pub mod polkit;
pub mod report;
pub mod wizard;
pub mod cli;

#[cfg(feature = "gui")]
pub mod ui;

// Re-exports
pub use config::{AppConfig, ComponentType, MigrationMode, RiskLevel};
pub use error::{MigrationError, Result};

pub mod error {
    use thiserror::Error;

    #[derive(Error, Debug)]
    pub enum MigrationError {
        #[error("Ошибка ввода-вывода: {0}")]
        Io(#[from] std::io::Error),

        #[error("Ошибка сериализации JSON: {0}")]
        Json(#[from] serde_json::Error),

        #[error("Ошибка сериализации TOML: {0}")]
        Toml(#[from] toml::de::Error),

        #[error("Ошибка базы данных: {0}")]
        Database(#[from] rusqlite::Error),

        #[error("Ошибка SSH: {0}")]
        Ssh(#[from] ssh2::Error),

        #[error("Ошибка шифрования: {0}")]
        Encryption(String),

        #[error("Ошибка хеширования: {0}")]
        Hashing(String),

        #[error("Файл не найден: {0}")]
        FileNotFound(String),

        #[error("Недостаточно прав доступа: {0}")]
        PermissionDenied(String),

        #[error("Недостаточно места на диске: требуется {required}, доступно {available}")]
        InsufficientSpace { required: u64, available: u64 },

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

        #[error("Ошибка проверки контрольной суммы")]
        ChecksumMismatch,

        #[error("Повреждённый архив: {0}")]
        CorruptedArchive(String),

        #[error("Path traversal атака обнаружена: {0}")]
        PathTraversal(String),

        #[error("Неизвестная ошибка: {0}")]
        Unknown(String),
    }

    pub type Result<T> = std::result::Result<T, MigrationError>;
}

/// Версия приложения
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Название приложения
pub const APP_NAME: &str = "Migration Master";
