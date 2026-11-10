//! Консольный интерфейс (clap): инспекция, создание, восстановление, миграция.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use clap::{Args, Parser, Subcommand};

use crate::archive::ArchiveManager;
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

/// Общие параметры парольной фразы архива.
#[derive(Debug, Clone, Args)]
pub struct PassphraseArgs {
    /// Пароль шифрования (не рекомендуется: виден в списке процессов)
    #[arg(long)]
    pub passphrase: Option<String>,
    /// Файл с парольной фразой (первая строка; безопаснее аргумента)
    #[arg(long = "passphrase-file")]
    pub passphrase_file: Option<PathBuf>,
    /// Запросить парольную фразу интерактивно (без эха)
    #[arg(long = "ask-passphrase")]
    pub ask_passphrase: bool,
}

impl PassphraseArgs {
    /// Получить пароль с учётом файла/интерактивного ввода.
    ///
    /// `confirm` запрашивает подтверждение (для создания нового архива).
    fn resolve(&self, confirm: bool) -> Result<Option<String>> {
        let sources = [
            self.passphrase.is_some(),
            self.passphrase_file.is_some(),
            self.ask_passphrase,
        ]
        .into_iter()
        .filter(|value| *value)
        .count();
        if sources > 1 {
            return Err(MigrationError::InvalidInput(
                "укажите только один способ пароля: --passphrase, --passphrase-file \
                 или --ask-passphrase"
                    .to_string(),
            ));
        }

        if let Some(passphrase) = &self.passphrase {
            if passphrase.is_empty() {
                return Err(MigrationError::InvalidInput(
                    "парольная фраза не может быть пустой".to_string(),
                ));
            }
            eprintln!(
                "предупреждение: --passphrase виден в списке процессов; \
                 используйте --passphrase-file или --ask-passphrase"
            );
            return Ok(Some(passphrase.clone()));
        }

        if let Some(path) = &self.passphrase_file {
            let content = std::fs::read_to_string(path).map_err(|error| {
                MigrationError::Io(std::io::Error::new(
                    error.kind(),
                    format!("не удалось прочитать {}: {}", path.display(), error),
                ))
            })?;
            let passphrase = content.trim_end_matches(['\r', '\n']).to_string();
            if passphrase.is_empty() {
                return Err(MigrationError::InvalidInput(format!(
                    "файл {} пуст",
                    path.display()
                )));
            }
            return Ok(Some(passphrase));
        }

        if self.ask_passphrase {
            let first = rpassword::prompt_password("Парольная фраза архива: ")?;
            if first.is_empty() {
                return Err(MigrationError::InvalidInput(
                    "парольная фраза не может быть пустой".to_string(),
                ));
            }
            if confirm {
                let second = rpassword::prompt_password("Повторите парольную фразу: ")?;
                if first != second {
                    return Err(MigrationError::InvalidInput(
                        "парольные фразы не совпадают".to_string(),
                    ));
                }
            }
            return Ok(Some(first));
        }

        Ok(None)
    }
}

/// Команды.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Просмотреть содержимое архива
    Inspect {
        /// Путь к архиву
        archive: PathBuf,
        #[command(flatten)]
        passphrase: PassphraseArgs,
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
        #[command(flatten)]
        passphrase: PassphraseArgs,
        /// Уровень сжатия (0-22)
        #[arg(long, default_value_t = 3)]
        compression: i32,
        /// Перенести приватные SSH-ключи (явное согласие)
        #[arg(long)]
        include_private_ssh_keys: bool,
        /// Перенести authorized_keys (явное согласие)
        #[arg(long)]
        include_authorized_keys: bool,
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
        #[command(flatten)]
        passphrase: PassphraseArgs,
        /// Стратегия конфликтов: skip|replace|newer|keep-both|rename|ask
        #[arg(long, default_value = "ask")]
        strategy: String,
        /// Проверять хеши после восстановления
        #[arg(long, default_value_t = true)]
        verify: bool,
        /// Установить пользовательские пакеты из архива (через polkit)
        #[arg(long)]
        install_packages: bool,
        /// Настроить принтеры из архива (через polkit)
        #[arg(long)]
        configure_printers: bool,
        /// Применить dconf и crontab из архива
        #[arg(long)]
        apply_system_settings: bool,
        /// Не создавать резервную копию перед восстановлением
        #[arg(long)]
        no_backup: bool,
        /// Только оценить, без изменений
        #[arg(long)]
        dry_run: bool,
    },
    /// Проверить целостность архива
    Verify {
        /// Путь к архиву
        archive: PathBuf,
        #[command(flatten)]
        passphrase: PassphraseArgs,
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
        /// Показать все пакеты, включая системные (по умолчанию — userinstalled)
        #[arg(long)]
        all: bool,
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
        /// Файл known_hosts (для проверки отпечатка)
        #[arg(long)]
        known_hosts: Option<PathBuf>,
        /// Передавать приватные SSH-ключи (явное согласие)
        #[arg(long)]
        include_private_ssh_keys: bool,
        /// Передавать authorized_keys (явное согласие)
        #[arg(long)]
        include_authorized_keys: bool,
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
    /// Запустить графический мастер (GTK4 + libadwaita, требует фичу `gui`)
    Gui,
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
fn parse_ssh_target(spec: &str, port: Option<u16>, identity: Option<PathBuf>) -> Result<SshTarget> {
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
            Commands::Gui => crate::ui::run_gui(),
            Commands::Inspect {
                archive,
                passphrase,
            } => {
                let passphrase = passphrase.resolve(false)?;
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
                    if info.manifest.encrypted {
                        "да"
                    } else {
                        "нет"
                    }
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
                let metadata = &info.manifest.metadata;
                println!("Метаданные: {}", metadata.summary());
                if !metadata.packages.is_empty() {
                    println!(
                        "  пакеты: {}",
                        metadata
                            .packages
                            .iter()
                            .map(|package| package.name.as_str())
                            .take(20)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    if metadata.packages.len() > 20 {
                        println!("  … и ещё {}", metadata.packages.len() - 20);
                    }
                }
                for printer in &metadata.printers {
                    println!(
                        "  принтер: {} (URI: {}, по умолчанию: {})",
                        printer.name,
                        if printer.device_uri.is_empty() {
                            "—"
                        } else {
                            printer.device_uri.as_str()
                        },
                        if printer.default { "да" } else { "нет" }
                    );
                }
                if metadata
                    .dconf
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("")
                    .is_empty()
                {
                    println!("  dconf: нет");
                } else {
                    println!("  dconf: есть");
                }
                for warning in &metadata.warnings {
                    eprintln!("  предупреждение: {}", warning);
                }
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
                include_private_ssh_keys,
                include_authorized_keys,
                dry_run,
            } => {
                let components = parse_components(&components)?;
                let source_home = home.unwrap_or_else(default_home);
                let passphrase = passphrase.resolve(true)?;

                let mut wizard_config =
                    WizardConfig::new(MigrationMode::LocalArchive, source_home.clone());
                wizard_config.components = components;
                wizard_config.archive_path = Some(output.clone());
                wizard_config.passphrase = passphrase;
                wizard_config.compression_level = compression;
                wizard_config.include_private_ssh_keys = include_private_ssh_keys;
                wizard_config.include_authorized_keys = include_authorized_keys;
                wizard_config.app_rules = crate::applications::load_effective_rules();
                wizard_config.dry_run = dry_run;
                wizard_config.cancel = Some(crate::cancel::global());

                // Не включать сам будущий архив, если он лежит внутри HOME.
                if let Ok(relative) = output.strip_prefix(&source_home) {
                    let relative = relative.to_string_lossy().replace('\\', "/");
                    if !relative.is_empty() {
                        wizard_config.exclusions.push(relative);
                    }
                }

                let progress = ConsoleProgress::new();
                let mut wizard = MigrationWizard::new(wizard_config);
                let report = wizard.run(&progress)?;

                println!("Архив: {}", output.display());
                println!(
                    "Файлов: {}, данных: {} байт, статус: {}",
                    report.stats.files_copied,
                    report.stats.bytes_copied,
                    report.status()
                );
                if !report.warnings.is_empty() {
                    for warning in &report.warnings {
                        eprintln!("  предупреждение: {}", warning);
                    }
                }
                if !report.is_success() {
                    return Err(MigrationError::Unknown(format!(
                        "создание архива завершилось с ошибками: {}",
                        report.errors.join("; ")
                    )));
                }
                Ok(())
            }
            Commands::Restore {
                archive,
                target,
                components,
                passphrase,
                strategy,
                verify,
                install_packages,
                configure_printers,
                apply_system_settings,
                no_backup,
                dry_run,
            } => {
                use std::io::IsTerminal;

                let components = if components == "all" {
                    ComponentType::all_components()
                } else {
                    parse_components(&components)?
                };
                let strategy = parse_strategy(&strategy)?;
                if strategy == ConflictStrategy::Ask && !std::io::stdin().is_terminal() {
                    return Err(MigrationError::InvalidInput(
                        "стратегия ask требует терминала; укажите --strategy rename, \
                         replace, skip, keep-both или newer"
                            .to_string(),
                    ));
                }
                let passphrase = passphrase.resolve(false)?;

                let mut wizard_config = WizardConfig::new(MigrationMode::Restore, default_home());
                wizard_config.target_home = target;
                wizard_config.components = components;
                wizard_config.archive_path = Some(archive);
                wizard_config.passphrase = passphrase;
                wizard_config.conflict_strategy = strategy;
                wizard_config.interactive_conflicts = strategy == ConflictStrategy::Ask;
                wizard_config.verify = verify;
                wizard_config.dry_run = dry_run;
                wizard_config.install_packages = install_packages;
                wizard_config.configure_printers = configure_printers;
                wizard_config.apply_system_settings = apply_system_settings;
                wizard_config.auto_backup = !no_backup;
                wizard_config.cancel = Some(crate::cancel::global());
                if let Ok(current) = config::load_config() {
                    wizard_config.database_path = Some(current.database_path);
                }

                let progress = ConsoleProgress::new();
                let mut wizard = MigrationWizard::new(wizard_config);
                let report = wizard.run(&progress)?;

                println!(
                    "Восстановлено: {} ({} байт); пропущено: {}; конфликтов: {}",
                    report.stats.files_copied,
                    report.stats.bytes_copied,
                    report.stats.files_skipped,
                    report.stats.conflicts_resolved
                );
                if let Some(backup) = &report.backup_path {
                    println!("Резервная копия: {}", backup);
                }
                if !report.installed_packages.is_empty() {
                    println!(
                        "Установлены пакеты: {}",
                        report.installed_packages.join(", ")
                    );
                }
                if !report.restored_printers.is_empty() {
                    println!("Принтеры: {}", report.restored_printers.join(", "));
                }
                if !report.applied_settings.is_empty() {
                    println!(
                        "Системные настройки: {}",
                        report.applied_settings.join(", ")
                    );
                }
                for skipped in &report.skipped_files {
                    eprintln!("  пропущено: {}", skipped);
                }
                for warning in &report.warnings {
                    eprintln!("  предупреждение: {}", warning);
                }
                for error in &report.errors {
                    eprintln!("  ошибка: {}", error);
                }
                for recommendation in &report.recommendations {
                    println!("  рекомендация: {}", recommendation);
                }

                if report.is_success() && !report.cancelled {
                    Ok(())
                } else {
                    Err(MigrationError::Conflict(format!(
                        "восстановление завершилось с ошибками: {}",
                        report.errors.len()
                    )))
                }
            }
            Commands::Verify {
                archive,
                passphrase,
                deep,
            } => {
                let passphrase = passphrase.resolve(false)?;
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

                if quick {
                    let scanner = ProfileScanner::new()?
                        .with_home(source_home)
                        .with_components(components);
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

                // То же сканирование, что и при создании архива: правила
                // приложений, XDG-каталоги и метаданные не расходятся.
                let mut wizard_config =
                    WizardConfig::new(MigrationMode::LocalArchive, &source_home);
                wizard_config.components = components;
                wizard_config.app_rules = crate::applications::load_effective_rules();
                wizard_config.cancel = Some(crate::cancel::global());
                let mut wizard = MigrationWizard::new(wizard_config);
                let inventory = wizard.scan()?.clone();

                if json {
                    let files: Vec<_> = inventory
                        .items
                        .iter()
                        .map(|item| {
                            serde_json::json!({
                                "source": item.source,
                                "relative": item.relative,
                                "size": item.size,
                                "is_symlink": item.is_symlink,
                            })
                        })
                        .collect();
                    let by_component: Vec<_> = inventory
                        .by_component
                        .iter()
                        .map(|(component, files, bytes)| {
                            serde_json::json!({
                                "component": component.key(),
                                "files": files,
                                "bytes": bytes,
                            })
                        })
                        .collect();
                    let applications: Vec<_> = inventory
                        .applications
                        .iter()
                        .map(|app| {
                            serde_json::json!({
                                "name": app.name,
                                "files": app.files,
                                "bytes": app.bytes,
                            })
                        })
                        .collect();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "home": source_home,
                            "files": files,
                            "total_files": inventory.total_files,
                            "total_size": inventory.total_size,
                            "by_component": by_component,
                            "applications": applications,
                            "metadata": inventory.metadata,
                            "metadata_summary": inventory.metadata.summary(),
                            "excluded": inventory.excluded,
                            "private_keys": inventory.private_keys,
                            "errors": inventory.errors,
                        }))?
                    );
                } else {
                    println!("Домашний каталог: {}", source_home.display());
                    println!(
                        "Файлов: {}, размер: {}",
                        inventory.total_files,
                        human_bytes::human_bytes(inventory.total_size as f64)
                    );

                    let mut by_component = inventory.by_component.clone();
                    by_component.sort_by_key(|component| component.0);
                    for (component, count, bytes) in by_component {
                        println!(
                            "  {}: {} файлов, {}",
                            component.description(),
                            count,
                            human_bytes::human_bytes(bytes as f64)
                        );
                    }
                    if !inventory.excluded.is_empty() {
                        println!("  исключено по правилам: {}", inventory.excluded.len());
                    }
                    println!("Метаданные: {}", inventory.metadata.summary());
                    for warning in &inventory.metadata.warnings {
                        eprintln!("  предупреждение: {}", warning);
                    }
                    for error in &inventory.errors {
                        eprintln!("  ошибка: {}", error);
                    }
                }
                Ok(())
            }
            Commands::ListPackages { all, json } => {
                let manager = PackageManager::detect();
                let entries = if all {
                    manager.list_installed()?
                } else {
                    manager.list_user_installed()?
                };
                if json {
                    println!("{}", serde_json::to_string_pretty(&entries)?);
                } else {
                    println!("Менеджер пакетов: {}", manager.list_tool());
                    println!(
                        "{} пакетов: {}",
                        if all { "Всего" } else { "Userinstalled" },
                        entries.len()
                    );
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
                known_hosts,
                include_private_ssh_keys,
                include_authorized_keys,
                dry_run,
            } => {
                let components = parse_components(&components)?;
                let source_home = home.unwrap_or_else(default_home);
                let mut ssh = parse_ssh_target(&target, port, identity)?;
                ssh.known_hosts_file = known_hosts;

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
                match SshTransfer::new(ssh.clone()).host_fingerprint() {
                    Ok(Some(fingerprint)) => println!(
                        "Отпечаток узла (проверьте по доверенному каналу): {}",
                        fingerprint
                    ),
                    Ok(None) => println!(
                        "Отпечаток узла получить не удалось; проверьте host key в known_hosts"
                    ),
                    Err(error) => eprintln!("  отпечаток: {}", error),
                }

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
                wizard_config.include_private_ssh_keys = include_private_ssh_keys;
                wizard_config.include_authorized_keys = include_authorized_keys;
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
                        println!(
                            "Импортировано правил: {} → {}",
                            rules.len(),
                            saved.display()
                        );
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

/// Разбор компонентов из строки (`default`, `all` или список ключей).
fn parse_components(list: &str) -> Result<Vec<ComponentType>> {
    match list {
        "default" => Ok(ComponentType::default_components()),
        "all" => Ok(ComponentType::all_components()),
        other => ComponentType::parse_list(other).map_err(MigrationError::InvalidInput),
    }
}
