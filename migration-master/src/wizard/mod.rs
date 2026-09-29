//! Мастер миграции: сканирование → план → выполнение → отчёт.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::archive::{
    ArchiveManager, CreateArchiveOptions, RestoreOptions,
};
use crate::config::{ComponentType, MigrationMode};
use crate::conflict_resolver::ConflictStrategy;
use crate::error::{MigrationError, Result};
use crate::file_transfer::{ProgressObserver, TransferItem};
use crate::report::MigrationReport;
use crate::ssh_transfer::{SshTarget, SshTransfer, SshTransferOptions};

/// Конфигурация мастера.
#[derive(Debug, Clone)]
pub struct WizardConfig {
    /// Режим миграции
    pub mode: MigrationMode,
    /// Домашний каталог-источник
    pub source_home: PathBuf,
    /// Домашний каталог-приёмник (для SSH)
    pub target_home: PathBuf,
    /// Компоненты
    pub components: Vec<ComponentType>,
    /// Путь к архиву (LocalArchive — куда писать, Restore — откуда читать)
    pub archive_path: Option<PathBuf>,
    /// Пароль шифрования архива
    pub passphrase: Option<String>,
    /// Подключение SSH (для SshDirect)
    pub ssh: Option<SshTarget>,
    /// Стратегия конфликтов
    pub conflict_strategy: ConflictStrategy,
    /// Режим без реальных изменений
    pub dry_run: bool,
    /// Проверять хеши после восстановления
    pub verify: bool,
    /// Уровень сжатия архива
    pub compression_level: i32,
}

impl WizardConfig {
    /// Конфигурация по умолчанию для указанного режима.
    pub fn new(mode: MigrationMode, source_home: impl Into<PathBuf>) -> Self {
        Self {
            mode,
            source_home: source_home.into(),
            target_home: PathBuf::new(),
            components: ComponentType::default_components(),
            archive_path: None,
            passphrase: None,
            ssh: None,
            conflict_strategy: ConflictStrategy::Ask,
            dry_run: false,
            verify: true,
            compression_level: 3,
        }
    }
}

/// Сцена мастера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WizardStage {
    /// Создан
    Created,
    /// Профиль просканирован
    Scanned,
    /// Выполняется
    Running,
    /// Завершено успешно
    Completed,
    /// Завершено с ошибкой
    Failed,
}

/// Инвентарь файлов профиля (результат сканирования мастера).
#[derive(Debug, Clone, Default)]
pub struct ScanInventory {
    /// Элементы для переноса
    pub items: Vec<TransferItem>,
    /// Всего файлов
    pub total_files: u64,
    /// Всего байт
    pub total_size: u64,
    /// Ошибки сканирования (недоступные пути)
    pub errors: Vec<String>,
}

/// Мастер миграции пользователя.
pub struct MigrationWizard {
    config: WizardConfig,
    stage: WizardStage,
    inventory: Option<ScanInventory>,
}

impl MigrationWizard {
    /// Создать мастер.
    pub fn new(config: WizardConfig) -> Self {
        Self {
            config,
            stage: WizardStage::Created,
            inventory: None,
        }
    }

    /// Текущая сцена.
    pub fn stage(&self) -> WizardStage {
        self.stage
    }

    /// Результат сканирования (после `scan`/`run`).
    pub fn inventory(&self) -> Option<&ScanInventory> {
        self.inventory.as_ref()
    }

    /// Конфигурация.
    pub fn config(&self) -> &WizardConfig {
        &self.config
    }

    /// Просканировать компоненты профиля.
    pub fn scan(&mut self) -> Result<&ScanInventory> {
        let mut inventory = ScanInventory::default();

        for component in &self.config.components {
            for root in component.default_paths(&self.config.source_home) {
                if !root.exists() {
                    continue;
                }
                if let Err(error) = collect_items(&root, &self.config.source_home, &mut inventory) {
                    inventory.errors.push(format!("{}: {}", root.display(), error));
                }
            }
        }

        inventory.items.sort_by(|a, b| a.source.cmp(&b.source));
        inventory.items.dedup_by(|a, b| a.source == b.source);
        inventory.total_files = inventory.items.len() as u64;
        inventory.total_size = inventory.items.iter().map(|item| item.size).sum();
        self.stage = WizardStage::Scanned;
        self.inventory = Some(inventory);
        Ok(self.inventory.as_ref().expect("inventory set")
        )
    }

    /// Обязательный путь к архиву.
    fn require_archive(&self) -> Result<&PathBuf> {
        self.config
            .archive_path
            .as_ref()
            .ok_or_else(|| MigrationError::InvalidInput("не задан путь к архиву".to_string()))
    }

    /// Выполнить миграцию целиком.
    pub fn run(&mut self, observer: &dyn ProgressObserver) -> Result<MigrationReport> {
        let started = Instant::now();
        self.stage = WizardStage::Running;

        let id = uuid::Uuid::new_v4().to_string();
        let mut report = MigrationReport::new(
            id,
            self.config.mode,
            self.config.source_home.to_string_lossy().to_string(),
            self.config
                .target_home
                .to_string_lossy()
                .to_string(),
        );
        report.components = self.config.components.clone();

        let result = self.execute(observer, &mut report);
        report.stats.duration_ms = started.elapsed().as_millis();

        match result {
            Ok(()) => self.stage = WizardStage::Completed,
            Err(error) => {
                self.stage = WizardStage::Failed;
                report.errors.push(error.to_string());
            }
        }

        Ok(report)
    }

    fn execute(
        &mut self,
        observer: &dyn ProgressObserver,
        report: &mut MigrationReport,
    ) -> Result<()> {
        match self.config.mode {
            MigrationMode::LocalArchive => self.run_create_archive(observer, report),
            MigrationMode::Restore => self.run_restore(observer, report),
            MigrationMode::SshDirect => self.run_ssh_direct(observer, report),
        }
    }

    /// Создание архива из профиля.
    fn run_create_archive(
        &mut self,
        observer: &dyn ProgressObserver,
        report: &mut MigrationReport,
    ) -> Result<()> {
        let archive = self.require_archive()?.clone();
        let inventory = self.scan()?.clone();

        for error in &inventory.errors {
            report.warnings.push(error.clone());
        }

        let options = CreateArchiveOptions {
            output: archive,
            source_home: self.config.source_home.clone(),
            items: inventory.items,
            components: self.config.components.clone(),
            passphrase: self.config.passphrase.clone(),
            compression_level: self.config.compression_level,
            dry_run: self.config.dry_run,
        };

        let created = ArchiveManager::create(&options, observer)?;
        report.stats.files_copied = created.manifest.total_files as u64;
        report.stats.bytes_copied = created.manifest.total_size;
        Ok(())
    }

    /// Восстановление из архива.
    fn run_restore(
        &mut self,
        observer: &dyn ProgressObserver,
        report: &mut MigrationReport,
    ) -> Result<()> {
        if self.config.target_home.as_os_str().is_empty() {
            return Err(MigrationError::InvalidInput(
                "не задан целевой домашний каталог".to_string(),
            ));
        }
        let archive = self.require_archive()?.clone();

        let options = RestoreOptions {
            archive,
            passphrase: self.config.passphrase.clone(),
            target_root: self.config.target_home.clone(),
            components: Some(self.config.components.clone()),
            strategy: self.config.conflict_strategy,
            verify_hash: self.config.verify,
            dry_run: self.config.dry_run,
        };

        let result = ArchiveManager::restore(&options, observer)?;
        report.stats.files_copied = result.restored_files;
        report.stats.bytes_copied = result.restored_bytes;
        report.stats.files_skipped = result.skipped_files;
        report.stats.conflicts_resolved = result.conflicts.len() as u64;

        for conflict in &result.conflicts {
            report.warnings.push(conflict.describe());
        }
        report.errors.extend(result.errors);
        Ok(())
    }

    /// Прямая миграция по SSH (rsync/scp каталогов компонентов).
    fn run_ssh_direct(
        &mut self,
        observer: &dyn ProgressObserver,
        report: &mut MigrationReport,
    ) -> Result<()> {
        if self.config.target_home.as_os_str().is_empty() {
            return Err(MigrationError::InvalidInput(
                "не задан целевой домашний каталог".to_string(),
            ));
        }
        let target = self.config.ssh.clone().ok_or_else(|| {
            MigrationError::InvalidInput("не задано SSH-подключение".to_string())
        })?;

        let inventory = self.scan()?.clone();
        for error in &inventory.errors {
            report.warnings.push(error.clone());
        }

        let transfer = SshTransfer::new(target);
        let options = SshTransferOptions {
            dry_run: self.config.dry_run,
            ..Default::default()
        };

        if !self.config.dry_run {
            transfer.check_connection()?;
        }

        for component in &self.config.components {
            for root in component.default_paths(&self.config.source_home) {
                if !root.exists() {
                    continue;
                }

                let relative = crate::security::make_relative(&self.config.source_home, &root)?;
                let remote = format!(
                    "{}/{}",
                    self.config.target_home.display(),
                    relative.display()
                );
                observer.on_message(&format!("передача {}", root.display()));

                let result = transfer.push(&root, &remote, &options)?;
                report.stats.files_copied += result.files;
                report.stats.bytes_copied += result.bytes;
            }
        }

        Ok(())
    }
}

/// Собрать файлы каталога в инвентарь (относительно домашнего каталога).
fn collect_items(root: &Path, home: &Path, inventory: &mut ScanInventory) -> Result<()> {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }

        let path = entry.path();
        let relative = crate::security::make_relative(home, path)?;
        let metadata = entry.metadata()?;
        let is_symlink = entry.file_type().is_symlink();

        inventory.items.push(TransferItem {
            source: path.to_path_buf(),
            relative,
            size: metadata.len(),
            is_symlink,
            symlink_target: if is_symlink {
                std::fs::read_link(path).ok()
            } else {
                None
            },
            mode: crate::platform::file_mode(path),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_transfer::NoProgress;
    use tempfile::tempdir;

    fn make_home(dir: &Path) -> PathBuf {
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("Documents")).expect("mkdir");
        std::fs::create_dir_all(home.join(".ssh")).expect("mkdir");
        std::fs::write(home.join("Documents/doc.txt"), b"doc").expect("write");
        std::fs::write(home.join(".ssh/config"), b"Host *\n").expect("write");
        home
    }

    #[test]
    fn test_scan_builds_inventory() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());

        let mut config = WizardConfig::new(MigrationMode::LocalArchive, &home);
        config.components = vec![ComponentType::Documents, ComponentType::SshKeys];

        let mut wizard = MigrationWizard::new(config);
        let inventory = wizard.scan().expect("scan").clone();
        assert_eq!(wizard.stage(), WizardStage::Scanned);
        assert_eq!(inventory.total_files, 2);
        assert_eq!(inventory.total_size, 10);
        assert!(inventory.items.iter().any(|i| i.relative == Path::new("Documents/doc.txt")));
        assert!(inventory.items.iter().any(|i| i.relative == Path::new(".ssh/config")));
    }

    #[test]
    fn test_local_archive_round_trip_via_wizard() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let archive = dir.path().join("out.rmm");

        let mut config = WizardConfig::new(MigrationMode::LocalArchive, &home);
        config.components = vec![ComponentType::Documents];
        config.archive_path = Some(archive.clone());

        let mut wizard = MigrationWizard::new(config);
        let report = wizard.run(&NoProgress).expect("run");
        assert_eq!(wizard.stage(), WizardStage::Completed);
        assert!(report.is_success(), "errors: {:?}", report.errors);
        assert_eq!(report.stats.files_copied, 1);
        assert!(archive.exists());

        // Восстановление
        let target = dir.path().join("target");
        let mut config = WizardConfig::new(MigrationMode::Restore, &home);
        config.components = vec![ComponentType::Documents];
        config.archive_path = Some(archive);
        config.target_home = target.clone();
        config.conflict_strategy = ConflictStrategy::Replace;

        let mut wizard = MigrationWizard::new(config);
        let report = wizard.run(&NoProgress).expect("restore");
        assert!(report.is_success(), "errors: {:?}", report.errors);
        assert_eq!(report.stats.files_copied, 1);
        assert_eq!(
            std::fs::read(target.join("Documents/doc.txt")).expect("read"),
            b"doc"
        );
    }

    #[test]
    fn test_missing_archive_path_fails() {
        let dir = tempdir().expect("tempdir");
        let config = WizardConfig::new(MigrationMode::LocalArchive, dir.path());
        let mut wizard = MigrationWizard::new(config);
        let report = wizard.run(&NoProgress).expect("run");
        assert_eq!(wizard.stage(), WizardStage::Failed);
        assert!(!report.is_success());
    }

    #[test]
    fn test_restore_requires_target_home() {
        let dir = tempdir().expect("tempdir");
        let mut config = WizardConfig::new(MigrationMode::Restore, dir.path());
        config.archive_path = Some(dir.path().join("missing.rmm"));
        let mut wizard = MigrationWizard::new(config);
        let report = wizard.run(&NoProgress).expect("run");
        assert_eq!(wizard.stage(), WizardStage::Failed);
        assert!(!report.is_success());
    }
}


