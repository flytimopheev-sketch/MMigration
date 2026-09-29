//! Migration Master - Мастер миграции пользователя для РЕД ОС Linux
//!
//! Приложение предназначено для безопасного переноса пользовательского профиля
//! со старого компьютера или старой установки на новый компьютер.
//!
//! Уровни архитектуры:
//! 1. UI (`ui`, `cli`) — интерфейс командной строки, консольный и GTK4 мастер.
//! 2. Application services (`wizard`, `archive`, `ssh_transfer`, ...) — сценарии миграции.
//! 3. Domain models (`config`, `profile_scanner`, `ssh_keys`, ...) — модели данных.
//! 4. Infrastructure (`database`, `logging`, `platform`, `security`, `polkit`).
//! 5. Privileged backend (`migration-master-helper`) — минимальные root-операции по беому списку.

// --- Уровень 4: инфраструктура ---
pub mod database;
pub mod error;
pub mod logging;
pub mod platform;
pub mod polkit;
pub mod security;

// --- Уровень 3: доменные модели и сервисы-компоненты ---
pub mod applications;
pub mod backup;
pub mod compatibility;
pub mod config;
pub mod conflict_resolver;
pub mod packages;
pub mod printers;
pub mod profile_scanner;
pub mod ssh_keys;
pub mod system_settings;

// --- Уровень 2: сервисы приложения ---
pub mod archive;
pub mod file_transfer;
pub mod report;
pub mod ssh_transfer;
pub mod wizard;

// --- Уровень 1: интерфейс ---
pub mod cli;
pub mod ui;

// Re-exports
pub use config::{AppConfig, ComponentType, MigrationMode, RiskLevel};
pub use error::{MigrationError, Result};

/// Версия приложения
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Название приложения
pub const APP_NAME: &str = "Migration Master";
/// Идентификатор приложения (desktop/AppStream/polkit)
pub const APP_ID: &str = "com.redos.migration-master";
