//! Консольный интерфейс (clap): инспекция, создание, восстановление, миграция.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use clap::{Parser, Subcommand};

use crate::archive::{ArchiveManager, CreateArchiveOptions, RestoreOptions};
use crate::config::{ComponentType, MigrationMode};
use crate::conflict_resolver::ConflictStrategy;
use crate::error::{MigrationError, Result};
use crate::file_transfer::ProgressObserver;
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
                let source_home = home.unwrap_or_else(|| {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("."))
                });
                let items = scan_items(&source_home, &components)?;
                let options = CreateArchiveOptions {
                    output,
                    source_home,
                    items,
                    components,
                    passphrase,
                    compression_level: compression,
                    dry_run,
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
        }
    }
}

/// Собрать элементы профиля для упаковки.
fn scan_items(home: &Path, components: &[ComponentType]) -> Result<Vec<TransferItem>> {
    let config = WizardConfig {
        components: components.to_vec(),
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
