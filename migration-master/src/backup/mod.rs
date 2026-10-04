//! Резервное копирование пользовательских данных перед изменениями.
//!
//! Копии создаются как `tar.zst` с манифестом внутри и sidecar-файлом
//! `<id>.json` для быстрого перечисления без распаковки.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{MigrationError, Result};
use crate::security;

/// Описание резервной копии.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackupEntry {
    /// Идентификатор копии
    pub id: String,
    /// Путь к файлу архива
    pub path: PathBuf,
    /// Дата создания (RFC 3339)
    pub created_at: String,
    /// Размер архива в байтах
    pub size: u64,
    /// Количество файлов
    pub files_count: u64,
    /// Описание
    pub description: String,
    /// Базовый каталог, относительно которого сохранены пути
    pub base: String,
    /// Сохранённые относительные пути
    pub paths: Vec<String>,
}

/// Манифест внутри архива резервной копии.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackupManifest {
    /// Версия формата
    pub format_version: u32,
    /// Идентификатор копии
    pub id: String,
    /// Дата создания
    pub created_at: String,
    /// Базовый каталог
    pub base: String,
    /// Относительные пути
    pub relative_paths: Vec<String>,
    /// Описание
    pub description: String,
    /// Версия приложения
    pub app_version: String,
}

/// Менеджер резервного копирования.
pub struct BackupManager {
    root: PathBuf,
}

impl BackupManager {
    /// Создать менеджер с указанным каталогом хранения копий.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Каталог резервных копий по умолчанию.
    pub fn default_root() -> PathBuf {
        crate::config::get_data_dir().join("backups")
    }

    /// Менеджер с каталогом по умолчанию.
    pub fn with_default_root() -> Self {
        Self::new(Self::default_root())
    }

    /// Каталог хранения копий.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Создать резервную копию указанных путей (относительно `base`).
    pub fn create_backup(
        &self,
        base: &Path,
        paths: &[PathBuf],
        description: &str,
    ) -> Result<BackupEntry> {
        std::fs::create_dir_all(&self.root)?;

        let id = format!(
            "{}-{}",
            chrono::Utc::now().format("%Y%m%d-%H%M%S"),
            uuid::Uuid::new_v4().simple()
        );
        let archive_path = self.root.join(format!("{}.backup.tar.zst", id));

        let mut relative_paths = Vec::new();
        for path in paths {
            let relative = security::make_relative(base, path)?;
            relative_paths.push(relative.to_string_lossy().to_string());
        }

        let created_at = chrono::Utc::now().to_rfc3339();
        let manifest = BackupManifest {
            format_version: 1,
            id: id.clone(),
            created_at: created_at.clone(),
            base: base.to_string_lossy().to_string(),
            relative_paths: relative_paths.clone(),
            description: description.to_string(),
            app_version: crate::VERSION.to_string(),
        };

        let files_count = self.write_archive(&archive_path, base, &manifest)?;
        let size = std::fs::metadata(&archive_path)?.len();

        let entry = BackupEntry {
            id,
            path: archive_path,
            created_at,
            size,
            files_count,
            description: description.to_string(),
            base: base.to_string_lossy().to_string(),
            paths: relative_paths,
        };

        self.write_sidecar(&entry)?;
        Ok(entry)
    }

    fn write_archive(
        &self,
        archive_path: &Path,
        base: &Path,
        manifest: &BackupManifest,
    ) -> Result<u64> {
        let file = File::create(archive_path)?;
        let encoder = zstd::stream::write::Encoder::new(file, 3)
            .map_err(|e| MigrationError::Unknown(e.to_string()))?;
        let mut builder = tar::Builder::new(encoder);

        let manifest_json = serde_json::to_vec_pretty(manifest)?;
        let mut header = tar::Header::new_gnu();
        header.set_path("manifest.json").map_err(|e| {
            MigrationError::Unknown(format!("не удалось записать манифест копии: {}", e))
        })?;
        header.set_size(manifest_json.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder.append_data(&mut header, "manifest.json", manifest_json.as_slice())?;

        let mut files_count = 0u64;

        for relative in &manifest.relative_paths {
            let relative_path = PathBuf::from(relative);
            let source = base.join(&relative_path);

            if !source.exists() {
                continue;
            }

            if source.is_dir() {
                for entry in walkdir::WalkDir::new(&source).follow_links(false) {
                    let entry = entry?;
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let rel = security::make_relative(base, entry.path())?;
                    builder.append_path_with_name(entry.path(), rel)?;
                    files_count += 1;
                }
            } else {
                builder.append_path_with_name(&source, &relative_path)?;
                files_count += 1;
            }
        }

        let encoder = builder.into_inner()?;
        let mut file = encoder.finish()?;
        file.flush()?;

        Ok(files_count)
    }

    fn sidecar_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{}.json", id))
    }

    fn write_sidecar(&self, entry: &BackupEntry) -> Result<()> {
        let json = serde_json::to_string_pretty(entry)?;
        std::fs::write(self.sidecar_path(&entry.id), json)?;
        Ok(())
    }
}

/// Статистика восстановления резервной копии.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BackupRestoreStats {
    /// Восстановлено файлов
    pub restored_files: u64,
    /// Восстановлено байт
    pub restored_bytes: u64,
    /// Пропущено записей
    pub skipped: u64,
}

impl BackupManager {
    /// Прочитать манифест резервной копии, не распаковывая данные целиком.
    pub fn read_manifest(archive_path: &Path) -> Result<BackupManifest> {
        let file = File::open(archive_path)
            .map_err(|_| crate::error::MigrationError::not_found(archive_path))?;
        let decoder = zstd::stream::read::Decoder::new(file)
            .map_err(|e| MigrationError::CorruptedArchive(e.to_string()))?;
        let mut archive = tar::Archive::new(decoder);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();
            if path == "manifest.json" {
                let mut content = String::new();
                std::io::Read::read_to_string(&mut entry, &mut content)?;
                return serde_json::from_str(&content).map_err(MigrationError::from);
            }
        }

        Err(MigrationError::CorruptedArchive(
            "в резервной копии отсутствует манифест".to_string(),
        ))
    }

    /// Перечислить все резервные копии (сначала новые).
    pub fn list(&self) -> Result<Vec<BackupEntry>> {
        let mut entries = Vec::new();

        if !self.root.exists() {
            return Ok(entries);
        }

        for item in std::fs::read_dir(&self.root)? {
            let item = item?;
            let path = item.path();
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(entry) = serde_json::from_str::<BackupEntry>(&content) {
                        entries.push(entry);
                    }
                }
            }
        }

        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(entries)
    }

    /// Найти копию по идентификатору.
    pub fn find(&self, id: &str) -> Result<BackupEntry> {
        self.list()?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| {
                MigrationError::FileNotFound(format!("резервная копия не найдена: {}", id))
            })
    }

    /// Восстановить резервную копию в указанный каталог.
    pub fn restore(
        &self,
        entry: &BackupEntry,
        target_root: &Path,
        dry_run: bool,
    ) -> Result<BackupRestoreStats> {
        let file = File::open(&entry.path)
            .map_err(|_| crate::error::MigrationError::not_found(&entry.path))?;
        let decoder = zstd::stream::read::Decoder::new(file)
            .map_err(|e| MigrationError::CorruptedArchive(e.to_string()))?;
        let mut archive = tar::Archive::new(decoder);

        let mut stats = BackupRestoreStats::default();

        for entry_result in archive.entries()? {
            let mut tar_entry = entry_result?;
            let raw_path = tar_entry.path()?.to_string_lossy().to_string();

            if raw_path == "manifest.json" {
                stats.skipped += 1;
                continue;
            }

            let relative = security::sanitize_relative_path(Path::new(&raw_path))?;
            let destination = security::safe_join(target_root, &relative)?;

            if dry_run {
                stats.restored_files += 1;
                stats.restored_bytes += tar_entry.size();
                continue;
            }

            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }

            tar_entry.unpack(&destination)?;

            if let Ok(mode) = tar_entry.header().mode() {
                let _ = crate::platform::set_mode(&destination, mode);
            }

            stats.restored_files += 1;
            stats.restored_bytes += tar_entry.size();
        }

        Ok(stats)
    }

    /// Проверить целостность резервной копии (манифест и отсутствие path traversal).
    pub fn verify(&self, entry: &BackupEntry) -> Result<bool> {
        let manifest = Self::read_manifest(&entry.path)?;
        if manifest.relative_paths.len() != entry.paths.len() {
            return Ok(false);
        }

        for relative in &manifest.relative_paths {
            if security::sanitize_relative_path(Path::new(relative)).is_err() {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Удалить резервную копию.
    pub fn delete(&self, entry: &BackupEntry) -> Result<()> {
        if entry.path.exists() {
            std::fs::remove_file(&entry.path)?;
        }
        let sidecar = self.sidecar_path(&entry.id);
        if sidecar.exists() {
            std::fs::remove_file(sidecar)?;
        }
        Ok(())
    }

    /// Удалить старые копии, оставив последние `keep` штук.
    pub fn cleanup(&self, keep: usize) -> Result<usize> {
        let entries = self.list()?;
        let mut removed = 0;

        for entry in entries.into_iter().skip(keep) {
            self.delete(&entry)?;
            removed += 1;
        }

        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup() -> (tempfile::TempDir, PathBuf, BackupManager) {
        let dir = tempdir().expect("tempdir");
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("Documents")).expect("mkdir");
        std::fs::write(home.join("Documents/report.txt"), b"important data").expect("write");
        std::fs::write(home.join("notes.md"), b"# notes").expect("write");

        let manager = BackupManager::new(dir.path().join("backups"));
        (dir, home, manager)
    }

    #[test]
    fn test_create_and_list_backup() {
        let (_dir, home, manager) = setup();
        let entry = manager
            .create_backup(
                &home,
                &[home.join("Documents"), home.join("notes.md")],
                "тестовая копия",
            )
            .expect("backup");

        assert!(entry.path.exists());
        assert_eq!(entry.files_count, 2);
        assert!(entry.size > 0);
        assert_eq!(entry.paths.len(), 2);

        let listed = manager.list().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, entry.id);
        assert!(manager.verify(&entry).expect("verify"));
    }

    #[test]
    fn test_restore_backup_recreates_files() {
        let (dir, home, manager) = setup();
        let entry = manager
            .create_backup(&home, &[home.join("Documents")], "копия")
            .expect("backup");

        std::fs::remove_file(home.join("Documents/report.txt")).expect("remove");
        assert!(!home.join("Documents/report.txt").exists());

        let target = dir.path().join("restored");
        let stats = manager.restore(&entry, &target, false).expect("restore");

        assert_eq!(stats.restored_files, 1);
        assert_eq!(
            std::fs::read(target.join("Documents/report.txt")).expect("read"),
            b"important data"
        );
    }

    #[test]
    fn test_restore_dry_run_does_not_write() {
        let (dir, home, manager) = setup();
        let entry = manager
            .create_backup(&home, &[home.join("Documents")], "копия")
            .expect("backup");

        let target = dir.path().join("dry");
        let stats = manager.restore(&entry, &target, true).expect("restore");

        assert_eq!(stats.restored_files, 1);
        assert!(!target.exists());
    }

    #[test]
    fn test_manifest_round_trip() {
        let (_dir, home, manager) = setup();
        let entry = manager
            .create_backup(&home, &[home.join("notes.md")], "копия")
            .expect("backup");

        let manifest = BackupManager::read_manifest(&entry.path).expect("manifest");
        assert_eq!(manifest.id, entry.id);
        assert_eq!(manifest.format_version, 1);
        assert_eq!(manifest.description, "копия");
        assert_eq!(manifest.relative_paths, vec!["notes.md".to_string()]);
    }

    #[test]
    fn test_find_and_delete_and_cleanup() {
        let (_dir, home, manager) = setup();

        let first = manager
            .create_backup(&home, &[home.join("notes.md")], "первая")
            .expect("backup");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        manager
            .create_backup(&home, &[home.join("Documents")], "вторая")
            .expect("backup");

        assert_eq!(manager.list().expect("list").len(), 2);
        assert_eq!(manager.find(&first.id).expect("find").description, "первая");

        let removed = manager.cleanup(1).expect("cleanup");
        assert_eq!(removed, 1);

        let remaining = manager.list().expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].description, "вторая");

        manager.delete(&remaining[0]).expect("delete");
        assert!(manager.list().expect("list").is_empty());
    }

    #[test]
    fn test_create_backup_rejects_paths_outside_base() {
        let (_dir, home, manager) = setup();
        let result = manager.create_backup(&home, &[PathBuf::from("/etc/passwd")], "копия");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_manifest_of_broken_archive_fails() {
        let dir = tempdir().expect("tempdir");
        let broken = dir.path().join("broken.backup.tar.zst");
        std::fs::write(&broken, b"not a zstd stream").expect("write");

        assert!(BackupManager::read_manifest(&broken).is_err());
    }
}
