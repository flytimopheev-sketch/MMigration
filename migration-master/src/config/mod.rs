//! Конфигурация приложения

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use std::path::Path;

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
        let data_dir = get_data_dir();
        let temp_dir = std::env::temp_dir().join("migration-master");

        Self {
            database_path: data_dir.join("migrations.db"),
            log_path: data_dir.join("migration.log"),
            temp_dir,
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationMode {
    /// Локальный архив
    #[default]
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

    /// Ключ компонента для CLI и сериализации.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Documents => "documents",
            Self::Downloads => "downloads",
            Self::Pictures => "pictures",
            Self::Videos => "videos",
            Self::Music => "music",
            Self::Templates => "templates",
            Self::CustomDirs => "custom_dirs",
            Self::AppConfigs => "app_configs",
            Self::AppData => "app_data",
            Self::LocalBin => "local_bin",
            Self::LocalApps => "local_apps",
            Self::Themes => "themes",
            Self::Icons => "icons",
            Self::Fonts => "fonts",
            Self::SshKeys => "ssh_keys",
            Self::Printers => "printers",
            Self::Packages => "packages",
            Self::SystemSettings => "system_settings",
            Self::CronJobs => "cron_jobs",
            Self::UserServices => "user_services",
        }
    }

    /// Разбор ключа компонента (с поддержкой распространённых синонимов).
    pub fn from_key(key: &str) -> Option<Self> {
        let normalized = key.trim().to_lowercase().replace('-', "_");
        match normalized.as_str() {
            "desktop" => Some(Self::Desktop),
            "documents" | "docs" => Some(Self::Documents),
            "downloads" => Some(Self::Downloads),
            "pictures" | "images" => Some(Self::Pictures),
            "videos" => Some(Self::Videos),
            "music" => Some(Self::Music),
            "templates" => Some(Self::Templates),
            "custom_dirs" | "custom" => Some(Self::CustomDirs),
            "app_configs" | "config" | "configs" | ".config" => Some(Self::AppConfigs),
            "app_data" | "local_data" | "data" => Some(Self::AppData),
            "local_bin" | "bin" => Some(Self::LocalBin),
            "local_apps" | "applications" => Some(Self::LocalApps),
            "themes" => Some(Self::Themes),
            "icons" => Some(Self::Icons),
            "fonts" => Some(Self::Fonts),
            "ssh_keys" | "ssh" => Some(Self::SshKeys),
            "printers" | "cups" => Some(Self::Printers),
            "packages" | "rpm" => Some(Self::Packages),
            "system_settings" | "system" => Some(Self::SystemSettings),
            "cron_jobs" | "cron" => Some(Self::CronJobs),
            "user_services" | "services" => Some(Self::UserServices),
            _ => None,
        }
    }

    /// Разбор списка компонентов из строки (`documents,ssh_keys,printers`).
    pub fn parse_list(list: &str) -> Result<Vec<Self>, String> {
        let mut result = Vec::new();
        for raw in list.split(',') {
            let item = raw.trim();
            if item.is_empty() {
                continue;
            }
            match Self::from_key(item) {
                Some(component) => result.push(component),
                None => return Err(format!("неизвестный компонент: {}", item)),
            }
        }
        Ok(result)
    }

    /// Пути компонента относительно домашнего каталога пользователя.
    pub fn default_paths(&self, home: &std::path::Path) -> Vec<std::path::PathBuf> {
        match self {
            Self::Desktop => vec![home.join("Desktop"), home.join("Рабочий стол")],
            Self::Documents => vec![home.join("Documents"), home.join("Документы")],
            Self::Downloads => vec![home.join("Downloads"), home.join("Загрузки")],
            Self::Pictures => vec![home.join("Pictures"), home.join("Изображения")],
            Self::Videos => vec![home.join("Videos"), home.join("Видео")],
            Self::Music => vec![home.join("Music"), home.join("Музыка")],
            Self::Templates => vec![home.join("Templates"), home.join("Шаблоны")],
            Self::CustomDirs => Vec::new(),
            Self::AppConfigs => vec![home.join(".config")],
            Self::AppData => vec![home.join(".local").join("share")],
            Self::LocalBin => vec![home.join(".local").join("bin")],
            Self::LocalApps => vec![home.join(".local").join("share").join("applications")],
            Self::Themes => vec![home.join(".themes")],
            Self::Icons => vec![home.join(".icons")],
            Self::Fonts => vec![home.join(".fonts"), home.join(".local/share/fonts")],
            Self::SshKeys => vec![home.join(".ssh")],
            Self::Printers => vec![home.join(".cups")],
            Self::Packages => vec![home.join(".local/share/migration-master/package-list.json")],
            Self::SystemSettings => vec![home.join(".config/migration-master/system-settings.json")],
            Self::CronJobs => vec![home.join(".config/migration-master/crontab.txt")],
            Self::UserServices => vec![home.join(".config/systemd/user")],
        }
    }

    /// Основной префикс пути в архиве (для фильтрации при восстановлении).
    pub fn archive_prefix(&self) -> Option<&'static str> {
        match self {
            Self::Desktop => Some("Desktop"),
            Self::Documents => Some("Documents"),
            Self::Downloads => Some("Downloads"),
            Self::Pictures => Some("Pictures"),
            Self::Videos => Some("Videos"),
            Self::Music => Some("Music"),
            Self::Templates => Some("Templates"),
            Self::AppConfigs => Some(".config"),
            Self::AppData => Some(".local/share"),
            Self::LocalBin => Some(".local/bin"),
            Self::LocalApps => Some(".local/share/applications"),
            Self::Themes => Some(".themes"),
            Self::Icons => Some(".icons"),
            Self::Fonts => Some(".fonts"),
            Self::SshKeys => Some(".ssh"),
            Self::Printers => Some(".cups"),
            _ => None,
        }
    }

    /// Требует ли компонент привилегированных операций (root).
    pub fn requires_root(&self) -> bool {
        matches!(self, Self::Packages | Self::Printers | Self::SystemSettings)
    }

    /// Опасный ли компонент (требует отдельного подтверждения пользователя).
    pub fn is_sensitive(&self) -> bool {
        matches!(
            self,
            Self::SshKeys | Self::Printers | Self::Packages | Self::SystemSettings
        )
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
pub fn load_config() -> crate::error::Result<AppConfig> {
    load_config_from(config_file_path())
}

/// Путь к файлу конфигурации
pub fn config_file_path() -> PathBuf {
    get_config_dir().join("config.toml")
}

/// Загрузка конфигурации из указанного файла
pub fn load_config_from(path: impl AsRef<Path>) -> crate::error::Result<AppConfig> {
    let config_path = path.as_ref().to_path_buf();
    
    if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        let config: AppConfig = toml::from_str(&content)?;
        Ok(config)
    } else {
        Ok(AppConfig::default())
    }
}

/// Сохранение конфигурации в файл
pub fn save_config(config: &AppConfig) -> crate::error::Result<()> {
    save_config_to(config_file_path(), config)
}

/// Сохранение конфигурации в указанный файл
pub fn save_config_to(path: impl AsRef<Path>, config: &AppConfig) -> crate::error::Result<()> {
    let config_path = path.as_ref().to_path_buf();
    let config_dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_config_dir);
    std::fs::create_dir_all(&config_dir)?;
    
    let content = toml::to_string_pretty(config)?;
    std::fs::write(config_path, content)?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_dirs_are_valid() {
        let config = AppConfig::default();
        assert!(config
            .database_path
            .to_string_lossy()
            .contains("migration-master"));
        assert_eq!(config.language, "ru");
        assert!(config.ssh_timeout_secs > 0);
    }

    #[test]
    fn test_component_keys_round_trip() {
        for component in ComponentType::all_components() {
            let key = component.key();
            assert_eq!(
                ComponentType::from_key(key),
                Some(component),
                "компонент {} не восстанавливается по ключу",
                key
            );
        }
    }

    #[test]
    fn test_parse_component_list() {
        let parsed = ComponentType::parse_list("documents, ssh_keys ,printers").expect("parse");
        assert_eq!(
            parsed,
            vec![
                ComponentType::Documents,
                ComponentType::SshKeys,
                ComponentType::Printers
            ]
        );
        assert!(ComponentType::parse_list("documents,unknown").is_err());
    }

    #[test]
    fn test_component_aliases() {
        assert_eq!(ComponentType::from_key("ssh"), Some(ComponentType::SshKeys));
        assert_eq!(
            ComponentType::from_key("cups"),
            Some(ComponentType::Printers)
        );
        assert_eq!(ComponentType::from_key("rpm"), Some(ComponentType::Packages));
    }

    #[test]
    fn test_default_paths_are_inside_home() {
        let home = PathBuf::from("/home/user");
        for component in ComponentType::all_components() {
            for path in component.default_paths(&home) {
                assert!(
                    path.starts_with(&home),
                    "{:?} вне домашнего каталога: {}",
                    component,
                    path.display()
                );
            }
        }
    }

    #[test]
    fn test_risk_levels() {
        assert_eq!(ComponentType::Documents.risk_level(), RiskLevel::Low);
        assert_eq!(ComponentType::SshKeys.risk_level(), RiskLevel::High);
        assert_eq!(
            ComponentType::SystemSettings.risk_level(),
            RiskLevel::Critical
        );
    }

    #[test]
    fn test_config_save_and_load_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let config = AppConfig {
            history_days: 42,
            ..AppConfig::default()
        };

        save_config_to(&path, &config).expect("save");
        let loaded = load_config_from(&path).expect("load");
        assert_eq!(loaded.history_days, 42);
    }

    #[test]
    fn test_load_missing_config_returns_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let loaded = load_config_from(dir.path().join("nope.toml")).expect("load");
        assert_eq!(loaded.language, "ru");
    }
}

