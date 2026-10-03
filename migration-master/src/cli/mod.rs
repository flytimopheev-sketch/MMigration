//! Консольный интерфейс (clap): инспекция, создание, восстановление, миграция.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use clap::{Parser, Subcommand};

use crate::archive::{ArchiveManager, CreateArchiveOptions, RestoreOptions};
use crate::config::{self, ComponentType, MigrationMode};
use crate::conflict_resolver::ConflictStrategy;
use crate::database::DatabaseManager;
use crate::error::{MigrationError, Result};
use crate::file_transfer::ProgressObserver;
use crate::packages::PackageManager;
use crate::printers::PrinterManager;
use crate::profile_scanner::ProfileScanner;
use crate::report::{render_history_html, render_history_json, render_history_text, ReportFormat};
use crate::ssh_transfer::{SshTarget, SshTransfer};
use crate::wizard::{MigrationWizard, WizardConfig};

/// Разбор интерфейса командной строки.
#[derive(Debug, Parser)]
#[command(
    name = "migration-master",
    version,
    about = "Мастер миграции пользователя для РЕД ОС Linux"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Команды.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Просмотреть содержимое архива
    Inspect {
        /// Путь к архиву
        archive: PathBuf,
        /// Пароль шифрования
        #[arg(long)]
        passphrase: Option<String>,
    },
    /// Создать архив из пользовательского профиля
    Create {
        /// Куда записать архив
        #[arg(long, short)]
        output: PathBuf,
        /// Компоненты через запятую (documents,ssh_keys,...)
        #[arg(long, default_value = "default")]
        components: String,
        /// Домашний каталог-источник
        #[arg(long)]
        home: Option<PathBuf>,
        /// Пароль шифрования
        #[arg(long)]
        passphrase: Option<String>,
        /// Уровень сжатия (0-22)
        #[arg(long, default_value_t = 3)]
        compression: i32,
        /// Только оценить, без записи
        #[arg(long)]
        dry_run: bool,
    },
    /// Восстановить архив в целевой каталог
    Restore {
        /// Путь к архиву
        archive: PathBuf,
        /// Целевой домашний каталог
        #[arg(long, short)]
        target: PathBuf,
        /// Компоненты через запятую
        #[arg(long, default_value = "all")]
        components: String,
        /// Пароль шифрования
        #[arg(long)]
        passphrase: Option<String>,
        /// Стратегия конфликтов: skip|replace|newer|keep-both|rename-old|ask
        #[arg(long, default_value = "ask")]
        strategy: String,
        /// Проверять хеши после восстановления
        #[arg(long, default_value_t = true)]
        verify: bool,
        /// Только оценить, без изменений
        #[arg(long)]
        dry_run: bool,
    },
    /// Проверить целостность архива
    Verify {
        /// Путь к архиву
        archive: PathBuf,
        /// Пароль шифрования
        #[arg(long)]
        passphrase: Option<String>,
        /// Проверять хеши всех файлов
        #[arg(long, default_value_t = true)]
        deep: bool,
    },
    /// Просканировать профиль и показать состав
    Scan {
        /// Домашний каталог (по умолчанию — текущий пользователь)
        #[arg(long)]
        home: Option<PathBuf>,
        /// Компоненты через запятую (default, all или список ключей)
        #[arg(long, default_value = "default")]
        components: String,
        /// Быстрая оценка размера без детального разбора
        #[arg(long)]
        quick: bool,
        /// Вывести результат в формате JSON
        #[arg(long)]
        json: bool,
    },
    /// Показать список установленных пакетов
    ListPackages {
        /// Вывести результат в формате JSON
        #[arg(long)]
        json: bool,
    },
    /// Показать список принтеров (CUPS)
    ListPrinters {
        /// Вывести результат в формате JSON
        #[arg(long)]
        json: bool,
    },
    /// Прямая миграция на другой компьютер по SSH
    MigrateSsh {
        /// Адрес вида `[user@]host[:port]`
        target: String,
        /// Порт (перекрывает значение из адреса)
        #[arg(long)]
        port: Option<u16>,
        /// Домашний каталог-источник
        #[arg(long)]
        home: Option<PathBuf>,
        /// Домашний каталог на удалённом хосте
        #[arg(long)]
        remote_home: Option<String>,
        /// Компоненты через запятую (default, all или список ключей)
        #[arg(long, default_value = "default")]
        components: String,
        /// Файл приватного ключа SSH
        #[arg(long)]
        identity: Option<PathBuf>,
        /// Только проверка соединения и план (без передачи)
        #[arg(long)]
        dry_run: bool,
    },
    /// Отчёт по истории миграций (JSON/HTML/TXT)
    Report {
        /// Каталог для файла отчёта (по умолчанию — каталог данных приложения)
        #[arg(long, short)]
        output: Option<PathBuf>,
        /// Формат: json|html|txt
        #[arg(long, default_value = "txt")]
        format: String,
        /// Сколько последних операций включить
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Настройки приложения
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Правила миграции настроек приложений (§5)
    AppRules {
        #[command(subcommand)]
        action: AppRulesAction,
    },
}

/// Действия с правилами миграции приложений (§5).
#[derive(Debug, Subcommand)]
pub enum AppRulesAction {
    /// Показать правила и объём данных каждого приложения
    List {
        /// Домашний каталог для оценки размера
        #[arg(long)]
        home: Option<PathBuf>,
        /// Вывести результат в формате JSON
        #[arg(long)]
        json: bool,
    },
    /// Экспортировать действующие правила в YAML
    Export {
        /// Куда записать файл (по умолчанию — каталог настроек)
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Импортировать правила из YAML-файла
    Import {
        /// Файл с правилами
        path: PathBuf,
    },
    /// Выполнить post-migration hooks (требует подтверждения)
    RunHooks {
        /// Явное подтверждение выполнения hooks
        #[arg(long)]
        confirm: bool,
        /// Домашний каталог источника
        #[arg(long)]
        home: Option<PathBuf>,
    },
}

/// Действия с конфигурацией приложения.
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Показать текущие настройки
    List,
    /// Сбросить настройки к значениям по умолчанию
    Reset,
    /// Показать путь к файлу настроек
    Path,
}

use std::path::Path;

use crate::file_transfer::TransferItem;

/// Консольный индикатор прогресса.
pub struct ConsoleProgress {
    last_percent: AtomicU64,
}

impl ConsoleProgress {
    /// Создать индикатор.
    pub fn new() -> Self {
        Self {
            last_percent: AtomicU64::new(0),
        }
    }
}

impl Default for ConsoleProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressObserver for ConsoleProgress {
    fn on_progress(&self, done_bytes: u64, total_bytes: u64, current: &Path) {
        if total_bytes == 0 {
            return;
        }
        let percent = done_bytes.saturating_mul(100) / total_bytes;
        let last = self.last_percent.load(Ordering::Relaxed);
        if percent >= last + 10 || percent >= 100 {
            self.last_percent.store(percent, Ordering::Relaxed);
            eprintln!("[{:>3}%] {}", percent, current.display());
        }
    }

    fn on_message(&self, message: &str) {
        eprintln!("{}", message);
    }
}

/// Разбор стратегии конфликтов.
fn parse_strategy(strategy: &str) -> Result<ConflictStrategy> {
    ConflictStrategy::from_key(strategy).map_err(MigrationError::InvalidInput)
}

/// Домашний каталог по умолчанию (переменная `HOME` либо системный).
fn default_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Разобрать строку подключения `[user@]host[:port]`.
fn parse_ssh_target(
    spec: &str,
    port: Option<u16>,
    identity: Option<PathBuf>,
) -> Result<SshTarget> {
    let spec = spec.trim();
    let (user, host_port) = match spec.split_once('@') {
        Some((user, rest)) => (user.to_string(), rest.to_string()),
        None => (String::new(), spec.to_string()),
    };

    let (host, parsed_port) = match host_port.rsplit_once(':') {
        Some((host, value)) => match value.parse::<u16>() {
            Ok(parsed) => (host.to_string(), parsed),
            Err(_) => (host_port, 22),
        },
        None => (host_port, 22),
    };

    if host.trim().is_empty() {
        return Err(MigrationError::InvalidInput(
            "не задан хост для SSH-миграции".to_string(),
        ));
    }

    Ok(SshTarget {
        host: host.trim().to_string(),
        user,
        port: port.unwrap_or(parsed_port),
        identity_file: identity,
        known_hosts_file: None,
    })
}

impl Cli {
    /// Выполнить команду.
    pub fn run(self) -> Result<()> {
        match self.command {
            Commands::Inspect {
                archive,
                passphrase,
            } => {
                let info = ArchiveManager::inspect(&archive, passphrase.as_deref())?;
                println!("Архив: {}", archive.display());
                println!(
                    "Формат: v{}; создан: {}",
                    info.manifest.format_version, info.manifest.created_at
                );
                println!(
                    "Источник: {}@{} ({} {})",
                    info.manifest.source_user,
                    info.manifest.source_hostname,
                    info.manifest.os_info.name,
                    info.manifest.os_info.architecture
                );
                println!(
                    "Зашифрован: {}",
                    if info.manifest.encrypted { "да" } else { "нет" }
                );
                println!(
                    "Файлов: {} ({} байт); размер архива: {} байт",
                    info.manifest.total_files, info.manifest.total_size, info.archive_size
                );
                println!(
                    "Компоненты: {}",
                    info.manifest
                        .components
                        .iter()
                        .map(|component| component.key())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                for file in &info.manifest.files {
                    println!("  {} ({} байт)", file.relative_path, file.size);
                }
                Ok(())
            }
            Commands::Create {
                output,
                components,
                home,
                passphrase,
                compression,
                dry_run,
            } => {
                let components = parse_components(&components)?;
                let source_home = home.unwrap_or_else(default_home);
                let items = scan_items(&source_home, &components)?;
                let options = CreateArchiveOptions {
                    output,
                    source_home,
                    items,
                    components,
                    passphrase,
                    compression_level: compression,
                    dry_run,
                    cancel: Some(crate::cancel::global()),
                };
                let progress = ConsoleProgress::new();
                let created = ArchiveManager::create(&options, &progress)?;
                println!("Архив: {}", created.path.display());
                println!(
                    "Файлов: {}, данных: {} байт, зашифрован: {}, {} мс",
                    created.manifest.total_files,
                    created.manifest.total_size,
                    created.encrypted,
                    created.duration_ms
                );
                Ok(())
            }
            Commands::Restore {
                archive,
                target,
                components,
                passphrase,
                strategy,
                verify,
                dry_run,
            } => {
                let components = if components == "all" {
                    None
                } else {
                    Some(parse_components(&components)?)
                };
                let options = RestoreOptions {
                    archive,
                    passphrase,
                    target_root: target,
                    components,
                    strategy: parse_strategy(&strategy)?,
                    verify_hash: verify,
                    dry_run,
                    cancel: Some(crate::cancel::global()),
                };
                let progress = ConsoleProgress::new();
                let result = ArchiveManager::restore(&options, &progress)?;

                println!(
                    "Восстановлено: {} ({} байт); пропущено: {}; проверено хешей: {}",
                    result.restored_files, result.restored_bytes, result.skipped_files,
                    result.verified_files
                );
                for conflict in &result.conflicts {
                    println!("  конфликт: {}", conflict.describe());
                }
                for error in &result.errors {
                    eprintln!("  ошибка: {}", error);
                }

                if result.is_success() {
                    Ok(())
                } else {
                    Err(MigrationError::Conflict(format!(
                        "восстановление завершено с ошибками: {}",
                        result.errors.len()
                    )))
                }
            }
            Commands::Verify {
                archive,
                passphrase,
                deep,
            } => {
                let report = ArchiveManager::verify(&archive, passphrase.as_deref(), deep)?;
                println!(
                    "Манифест: {}",
                    if report.manifest_ok {
                        "корректен"
                    } else {
                        "ПОВРЕЖДЁН"
                    }
                );
                println!("Проверено файлов: {}", report.checked_files);
                for path in &report.mismatched {
                    println!("  хеш не совпадает: {}", path);
                }
                for path in &report.missing {
                    println!("  отсутствует в архиве: {}", path);
                }

                if report.is_valid() {
                    println!("Архив корректен.");
                    Ok(())
                } else {
                    Err(MigrationError::CorruptedArchive(
                        "архив повреждён".to_string(),
                    ))
                }
            }
            Commands::Scan {
                home,
                components,
                quick,
                json,
            } => {
                let components = parse_components(&components)?;
                let source_home = home.unwrap_or_else(default_home);
                let scanner = ProfileScanner::new()?
                    .with_home(source_home)
                    .with_components(components);

                if quick {
                    let (files, bytes) = scanner.quick_estimate()?;
                    if json {
                        println!("{}", serde_json::json!({ "files": files, "bytes": bytes }));
                    } else {
                        println!(
                            "Файлов: {}, размер: {}",
                            files,
                            human_bytes::human_bytes(bytes as f64)
                        );
                    }
                    return Ok(());
                }

                let result = scanner.scan()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                } else {
                    println!("Домашний каталог: {}", result.home_dir.display());
                    println!(
                        "Пользователь: {} (uid {}, gid {})",
                        result.username, result.uid, result.gid
                    );
                    println!(
                        "Файлов: {}, размер: {}",
                        result.total_files,
                        result.total_size_human()
                    );

                    let mut components: Vec<_> = result.files_by_component.iter().collect();
                    components.sort_by(|a, b| a.0.cmp(b.0));
                    for (component, count) in components {
                        let bytes = result.size_by_component.get(component).copied().unwrap_or(0);
                        println!(
                            "  {}: {} файлов, {}",
                            component,
                            count,
                            human_bytes::human_bytes(bytes as f64)
                        );
                    }
                    if !result.excluded_paths.is_empty() {
                        println!("  исключено по правилам: {}", result.excluded_paths.len());
                    }
                    for error in &result.errors {
                        eprintln!("  ошибка: {}", error);
                    }
                }
                Ok(())
            }
            Commands::ListPackages { json } => {
                let manager = PackageManager::detect();
                let entries = manager.list_installed()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                } else {
                    println!("Менеджер пакетов: {}", manager.list_tool());
                    println!("Установлено пакетов: {}", entries.len());
                    for entry in entries.iter().take(50) {
                        println!("  {} {}", entry.name, entry.version);
                    }
                    if entries.len() > 50 {
                        println!("  … и ещё {}", entries.len() - 50);
                    }
                }
                Ok(())
            }
            Commands::ListPrinters { json } => {
                let printers = PrinterManager::list()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&printers)?);
                } else {
                    println!("Принтеров: {}", printers.len());
                    for printer in &printers {
                        println!(
                            "  {} (URI: {}, общий: {})",
                            printer.name,
                            if printer.device_uri.is_empty() {
                                "—"
                            } else {
                                printer.device_uri.as_str()
                            },
                            printer.shared
                        );
                    }
                }
                Ok(())
            }
            Commands::MigrateSsh {
                target,
                port,
                home,
                remote_home,
                components,
                identity,
                dry_run,
            } => {
                let components = parse_components(&components)?;
                let source_home = home.unwrap_or_else(default_home);
                let ssh = parse_ssh_target(&target, port, identity)?;

                println!(
                    "Подключение: {}@{}:{}",
                    if ssh.user.is_empty() {
                        "<текущий пользователь>"
                    } else {
                        ssh.user.as_str()
                    },
                    ssh.host,
                    ssh.port
                );
                println!("Проверьте отпечаток узла (host key) перед тем, как доверять хосту.");

                let remote_home = remote_home.unwrap_or_else(|| {
                    if ssh.user.is_empty() {
                        "~".to_string()
                    } else {
                        format!("/home/{}", ssh.user)
                    }
                });

                if dry_run {
                    println!("[dry-run] передача не выполняется; цель: {}", remote_home);
                } else {
                    SshTransfer::new(ssh.clone()).check_connection()?;
                    println!("Соединение установлено. Начинаю передачу…");
                }

                let mut wizard_config = WizardConfig::new(MigrationMode::SshDirect, source_home);
                wizard_config.target_home = PathBuf::from(remote_home);
                wizard_config.components = components;
                wizard_config.app_rules = crate::applications::load_effective_rules();
                wizard_config.ssh = Some(ssh);
                wizard_config.dry_run = dry_run;
                wizard_config.cancel = Some(crate::cancel::global());

                let progress = ConsoleProgress::new();
                let mut wizard = MigrationWizard::new(wizard_config);
                let report = wizard.run(&progress)?;

                println!(
                    "Передача: файлов {}, байт {}; статус: {}",
                    report.stats.files_copied,
                    report.stats.bytes_copied,
                    report.status()
                );
                for error in &report.errors {
                    eprintln!("  ошибка: {}", error);
                }
                Ok(())
            }
            Commands::Report {
                output,
                format,
                limit,
            } => {
                let current = config::load_config()?;
                let db_path = current.database_path.clone();
                if !db_path.exists() {
                    println!(
                        "История недоступна: база данных не найдена ({})",
                        db_path.display()
                    );
                    return Ok(());
                }

                let database = DatabaseManager::new(db_path)?;
                let records = database.get_migrations(limit, 0)?;
                if records.is_empty() {
                    println!("История операций пуста.");
                    return Ok(());
                }

                let directory = output.unwrap_or_else(config::get_data_dir);
                std::fs::create_dir_all(&directory)?;

                let format =
                    ReportFormat::from_key(&format).map_err(MigrationError::InvalidInput)?;
                let (content, extension) = match format {
                    ReportFormat::Json => (render_history_json(&records)?, "json"),
                    ReportFormat::Html => (render_history_html(&records), "html"),
                    _ => (render_history_text(&records), "txt"),
                };

                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let path = directory.join(format!("history-{stamp}.{extension}"));
                std::fs::write(&path, content)?;

                println!(
                    "Отчёт по истории: {} операций → {}",
                    records.len(),
                    path.display()
                );
                Ok(())
            }
            Commands::Config { action } => {
                match action {
                    ConfigAction::Path => {
                        println!("{}", config::config_file_path().display());
                    }
                    ConfigAction::List => {
                        let current = config::load_config()?;
                        println!("Файл настроек: {}", config::config_file_path().display());
                        println!("{}", toml::to_string_pretty(&current)?);
                    }
                    ConfigAction::Reset => {
                        config::save_config(&config::AppConfig::default())?;
                        println!(
                            "Настройки сброшены к значениям по умолчанию: {}",
                            config::config_file_path().display()
                        );
                    }
                }
                Ok(())
            }
            Commands::AppRules { action } => {
                use crate::applications;
                match action {
                    AppRulesAction::List { home, json } => {
                        let home = home.unwrap_or_else(default_home);
                        let rules = applications::load_effective_rules();
                        if json {
                            let entries: Vec<_> = rules
                                .iter()
                                .map(|rule| {
                                    serde_json::json!({
                                        "app_name": rule.app_name,
                                        "enabled": rule.enabled,
                                        "transfer_mode": rule.transfer_mode,
                                        "requires_restart": rule.requires_restart,
                                        "bytes": rule.size_in(&home),
                                        "warnings": rule.warnings,
                                        "has_hook": rule.post_migration_hook.is_some(),
                                    })
                                })
                                .collect();
                            println!("{}", serde_json::to_string_pretty(&entries)?);
                        } else {
                            println!("Правил приложений: {}", rules.len());
                            for rule in &rules {
                                let bytes = rule.size_in(&home);
                                println!(
                                    "  [{}] {} — {} (режим: {}{})",
                                    if rule.enabled { "вкл" } else { "выкл" },
                                    rule.app_name,
                                    human_bytes::human_bytes(bytes as f64),
                                    rule.transfer_mode,
                                    if rule.post_migration_hook.is_some() {
                                        ", hook"
                                    } else {
                                        ""
                                    }
                                );
                            }
                        }
                        Ok(())
                    }
                    AppRulesAction::Export { output } => {
                        let rules = applications::load_effective_rules();
                        let yaml = applications::export_rules_yaml(&rules);
                        let path = output.unwrap_or_else(applications::default_rules_path);
                        if let Some(parent) = path.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        std::fs::write(&path, yaml)?;
                        println!("Правила экспортированы: {}", path.display());
                        Ok(())
                    }
                    AppRulesAction::Import { path } => {
                        let rules = applications::load_rules_file(&path)?;
                        let saved = applications::save_rules(&rules)?;
                        println!("Импортировано правил: {} → {}", rules.len(), saved.display());
                        Ok(())
                    }
                    AppRulesAction::RunHooks { confirm, home } => {
                        let mut wizard_config = WizardConfig::new(
                            MigrationMode::LocalArchive,
                            home.unwrap_or_else(default_home),
                        );
                        wizard_config.app_rules = applications::load_effective_rules();
                        let wizard = MigrationWizard::new(wizard_config);
                        let results = wizard.run_app_hooks(confirm)?;
                        if results.is_empty() {
                            println!("Post-migration hooks не заданы.");
                        } else {
                            for (app, output) in results {
                                println!("{}: {}", app, output.trim());
                            }
                        }
                        Ok(())
                    }
                }
            }
        }
    }
}

/// Собрать элементы профиля для упаковки.
fn scan_items(home: &Path, components: &[ComponentType]) -> Result<Vec<TransferItem>> {
    let config = WizardConfig {
        components: components.to_vec(),
        app_rules: crate::applications::load_effective_rules(),
        ..WizardConfig::new(MigrationMode::LocalArchive, home)
    };
    let mut wizard = MigrationWizard::new(config);
    let inventory = wizard.scan()?.clone();
    Ok(inventory.items)
}

/// Разбор компонентов из строки (`default`, `all` или список ключей).
fn parse_components(list: &str) -> Result<Vec<ComponentType>> {
    match list {
        "default" => Ok(ComponentType::default_components()),
        "all" => Ok(ComponentType::all_components()),
        other => ComponentType::parse_list(other).map_err(MigrationError::InvalidInput),
    }
}
