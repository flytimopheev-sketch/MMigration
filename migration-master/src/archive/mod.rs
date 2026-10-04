//! Работа с архивами миграции (формат `.rmm`).
//!
//! Структура архива:
//! - `manifest.json` — метаданные (первая запись tar);
//! - файлы профиля с относительными путями;
//! - сжатие zstd, шифрование age (если задан пароль).
//!
//! Все пути проверяются на path traversal, запись за пределы целевого
//! каталога запрещена, файлы записываются атомарно.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::cancel::CancelToken;
use crate::config::ComponentType;
use crate::conflict_resolver::{ConflictInfo, ConflictKind, ConflictResolver, ConflictStrategy};
use crate::error::{MigrationError, Result};
use crate::file_transfer::{self, ProgressObserver, TransferItem};
use crate::platform;
use crate::security;

/// Сведения об операционной системе источника.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OsInfo {
    /// Название ОС
    pub name: String,
    /// Версия ОС
    pub version: String,
    /// Архитектура
    pub architecture: String,
}

impl OsInfo {
    /// Собрать информацию о текущей системе.
    pub fn from_system() -> Self {
        let release = platform::os_release();
        Self {
            name: release.pretty_name,
            version: release.version,
            architecture: platform::architecture(),
        }
    }
}

/// Запись о файле внутри архива.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    /// Относительный путь внутри архива
    pub relative_path: String,
    /// Размер в байтах
    pub size: u64,
    /// SHA-256 хеш содержимого
    pub hash: String,
    /// Права доступа (Unix mode)
    pub mode: u32,
    /// UID владельца на источнике
    pub uid: u32,
    /// GID владельца на источнике
    pub gid: u32,
    /// Символическая ссылка
    pub is_symlink: bool,
    /// Цель символической ссылки
    pub symlink_target: Option<String>,
    /// Компонент миграции
    pub component: Option<ComponentType>,
}

impl FileEntry {
    /// Элемент передачи для использования в `file_transfer`.
    pub fn to_transfer_item(&self, base: &Path) -> TransferItem {
        TransferItem {
            source: base.join(&self.relative_path),
            relative: PathBuf::from(&self.relative_path),
            size: self.size,
            is_symlink: self.is_symlink,
            symlink_target: self.symlink_target.as_ref().map(PathBuf::from),
            mode: if self.mode == 0 {
                None
            } else {
                Some(self.mode)
            },
        }
    }

    /// Относится ли файл к указанному компоненту.
    pub fn matches_component(&self, component: &ComponentType) -> bool {
        if self.component == Some(*component) {
            return true;
        }
        match component.archive_prefix() {
            Some(prefix) => self.relative_path.starts_with(prefix),
            None => false,
        }
    }
}

/// Манифест архива (первая запись внутри tar).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchiveManifest {
    /// Версия формата архива
    pub format_version: u32,
    /// Дата создания (RFC 3339)
    pub created_at: String,
    /// Имя хоста источника
    pub source_hostname: String,
    /// Пользователь источника
    pub source_user: String,
    /// UID пользователя источника
    pub source_uid: u32,
    /// GID пользователя источника
    pub source_gid: u32,
    /// Версия приложения
    pub app_version: String,
    /// Сведения об ОС источника
    pub os_info: OsInfo,
    /// Компоненты, включённые в архив
    pub components: Vec<ComponentType>,
    /// Список файлов
    pub files: Vec<FileEntry>,
    /// Общий размер данных
    pub total_size: u64,
    /// Количество файлов
    pub total_files: usize,
    /// Зашифрован ли архив
    pub encrypted: bool,
    /// Использованное сжатие
    pub compression: String,
    /// Необязательная подпись архива
    pub signature: Option<String>,
    /// SHA-256 хеш манифеста (без этого поля)
    pub manifest_hash: Option<String>,
}

impl ArchiveManifest {
    /// Вычислить хеш манифеста (поле `manifest_hash` не учитывается).
    pub fn compute_hash(&self) -> Result<String> {
        let mut copy = self.clone();
        copy.manifest_hash = None;
        let json = serde_json::to_vec(&copy)?;
        Ok(security::compute_sha256_from_data(&json))
    }

    /// Проверить целостность манифеста.
    pub fn verify_hash(&self) -> Result<bool> {
        let expected = match &self.manifest_hash {
            Some(hash) => hash,
            None => return Ok(false),
        };
        Ok(self.compute_hash()?.eq_ignore_ascii_case(expected))
    }

    /// Файлы, относящиеся к указанным компонентам.
    pub fn files_for_components(&self, components: &[ComponentType]) -> Vec<&FileEntry> {
        self.files
            .iter()
            .filter(|file| {
                components
                    .iter()
                    .any(|component| file.matches_component(component))
            })
            .collect()
    }
}

/// Опции создания архива.
#[derive(Debug, Clone)]
pub struct CreateArchiveOptions {
    /// Каталог для выходного файла
    pub output: PathBuf,
    /// Домашний каталог источника
    pub source_home: PathBuf,
    /// Элементы для упаковки
    pub items: Vec<TransferItem>,
    /// Компоненты, включённые в архив
    pub components: Vec<ComponentType>,
    /// Пароль шифрования (None — без шифрования)
    pub passphrase: Option<String>,
    /// Уровень сжатия zstd (0-22)
    pub compression_level: i32,
    /// Режим без создания файла
    pub dry_run: bool,
    /// Токен отмены длительной операции
    pub cancel: Option<CancelToken>,
}

impl Default for CreateArchiveOptions {
    fn default() -> Self {
        Self {
            output: PathBuf::new(),
            source_home: PathBuf::new(),
            items: Vec::new(),
            components: Vec::new(),
            passphrase: None,
            compression_level: 3,
            dry_run: false,
            cancel: None,
        }
    }
}
/// Результат создания архива.
#[derive(Debug, Clone)]
pub struct CreateArchiveResult {
    /// Путь к архиву
    pub path: PathBuf,
    /// Манифест
    pub manifest: ArchiveManifest,
    /// Размер архива в байтах (0 в режиме dry-run)
    pub archive_size: u64,
    /// Зашифрован ли архив
    pub encrypted: bool,
    /// Длительность операции в миллисекундах
    pub duration_ms: u128,
}

/// Информация об архиве без восстановления.
#[derive(Debug, Clone)]
pub struct ArchiveInfo {
    /// Манифест
    pub manifest: ArchiveManifest,
    /// Путь к архиву
    pub archive_path: PathBuf,
    /// Размер файла архива
    pub archive_size: u64,
    /// Результат проверки целостности манифеста
    pub integrity_verified: bool,
}

/// Опции восстановления.
#[derive(Debug, Clone)]
pub struct RestoreOptions {
    /// Путь к архиву
    pub archive: PathBuf,
    /// Пароль (для зашифрованных архивов)
    pub passphrase: Option<String>,
    /// Каталог восстановления
    pub target_root: PathBuf,
    /// Компоненты (None — все)
    pub components: Option<Vec<ComponentType>>,
    /// Стратегия разрешения конфликтов
    pub strategy: ConflictStrategy,
    /// Проверять хеши после восстановления
    pub verify_hash: bool,
    /// Режим без изменений
    pub dry_run: bool,
    /// Токен отмены длительной операции
    pub cancel: Option<CancelToken>,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            archive: PathBuf::new(),
            passphrase: None,
            target_root: PathBuf::new(),
            components: None,
            strategy: ConflictStrategy::Ask,
            verify_hash: false,
            dry_run: false,
            cancel: None,
        }
    }
}

/// Результат восстановления.
#[derive(Debug, Clone, Default)]
pub struct RestoreResult {
    /// Восстановлено файлов
    pub restored_files: u64,
    /// Восстановлено байт
    pub restored_bytes: u64,
    /// Пропущено файлов
    pub skipped_files: u64,
    /// Проверено хешей
    pub verified_files: u64,
    /// Обнаруженные конфликты
    pub conflicts: Vec<ConflictInfo>,
    /// Ошибки
    pub errors: Vec<String>,
}

impl RestoreResult {
    /// Завершилось ли восстановление без ошибок.
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Отчёт проверки архива.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct VerifyReport {
    /// Манифест целостен
    pub manifest_ok: bool,
    /// Проверено файлов
    pub checked_files: u64,
    /// Файлы с несовпадающими хешами
    pub mismatched: Vec<String>,
    /// Отсутствующие в архиве файлы
    pub missing: Vec<String>,
}

impl VerifyReport {
    /// Полностью ли корректен архив.
    pub fn is_valid(&self) -> bool {
        self.manifest_ok && self.mismatched.is_empty() && self.missing.is_empty()
    }
}

/// Определить компонент по относительному пути (по префиксу в архиве).
fn component_for_path(relative: &str) -> Option<ComponentType> {
    let mut best: Option<(usize, ComponentType)> = None;

    for component in ComponentType::all_components() {
        if let Some(prefix) = component.archive_prefix() {
            if relative.starts_with(prefix) {
                let is_better = best.map(|(len, _)| prefix.len() > len).unwrap_or(true);
                if is_better {
                    best = Some((prefix.len(), component));
                }
            }
        }
    }

    best.map(|(_, component)| component)
}

/// Собрать манифест для набора элементов (с вычислением хешей).
fn build_manifest(
    options: &CreateArchiveOptions,
    observer: &dyn ProgressObserver,
) -> Result<ArchiveManifest> {
    let total_bytes: u64 = options.items.iter().map(|item| item.size).sum();
    let mut files = Vec::with_capacity(options.items.len());
    let mut done = 0u64;

    for item in &options.items {
        if let Some(cancel) = &options.cancel {
            cancel.check()?;
        }

        let relative = item.relative.to_string_lossy().to_string();
        let component = component_for_path(&relative);

        if item.is_symlink {
            let target = item
                .symlink_target
                .as_ref()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default();

            files.push(FileEntry {
                relative_path: relative,
                size: item.size,
                hash: security::compute_sha256_from_data(target.as_bytes()),
                mode: item.mode.unwrap_or(0o777),
                uid: platform::current_uid(),
                gid: platform::current_gid(),
                is_symlink: true,
                symlink_target: Some(target),
                component,
            });
        } else {
            let hash = security::compute_sha256(&item.source)?;
            let (uid, gid) = platform::owner_uid_gid(&item.source);

            files.push(FileEntry {
                relative_path: relative,
                size: hash.size,
                hash: hash.sha256,
                mode: item
                    .mode
                    .or_else(|| platform::file_mode(&item.source))
                    .unwrap_or(0o644),
                uid: uid.unwrap_or_else(platform::current_uid),
                gid: gid.unwrap_or_else(platform::current_gid),
                is_symlink: false,
                symlink_target: None,
                component,
            });
        }

        done += item.size;
        observer.on_progress(done, total_bytes, &item.source);
        observer.on_file_done(&item.source, item.size);
    }

    let total_size = files.iter().map(|file| file.size).sum();
    let mut manifest = ArchiveManifest {
        format_version: crate::config::ARCHIVE_FORMAT_VERSION,
        created_at: chrono::Utc::now().to_rfc3339(),
        source_hostname: platform::hostname(),
        source_user: platform::username(),
        source_uid: platform::current_uid(),
        source_gid: platform::current_gid(),
        app_version: crate::VERSION.to_string(),
        os_info: OsInfo::from_system(),
        components: options.components.clone(),
        files,
        total_size,
        total_files: 0,
        encrypted: options.passphrase.is_some(),
        compression: format!("zstd-{}", options.compression_level),
        signature: None,
        manifest_hash: None,
    };

    manifest.total_files = manifest.files.len();
    let hash = manifest.compute_hash()?;
    manifest.manifest_hash = Some(hash);

    Ok(manifest)
}

/// Менеджер архивов миграции.
pub struct ArchiveManager;

impl ArchiveManager {
    /// Создать архив из элементов передачи.
    pub fn create(
        options: &CreateArchiveOptions,
        observer: &dyn ProgressObserver,
    ) -> Result<CreateArchiveResult> {
        let started = std::time::Instant::now();

        if options.items.is_empty() {
            return Err(MigrationError::InvalidInput(
                "нечего упаковывать: список файлов пуст".to_string(),
            ));
        }

        let manifest = build_manifest(options, observer)?;
        let encrypted = manifest.encrypted;

        let mut result = CreateArchiveResult {
            path: options.output.clone(),
            manifest,
            archive_size: 0,
            encrypted,
            duration_ms: started.elapsed().as_millis(),
        };

        if options.dry_run {
            return Ok(result);
        }

        if let Some(parent) = options.output.parent() {
            std::fs::create_dir_all(parent)?;
            file_transfer::ensure_space(parent, result.manifest.total_size)?;
        }

        // При шифровании сначала пишем обычный tar.zst во временный файл.
        let temp = if options.passphrase.is_some() {
            Some(tempfile::NamedTempFile::new()?)
        } else {
            None
        };

        let target_path = match &temp {
            Some(file) => file.path().to_path_buf(),
            None => options.output.clone(),
        };

        if let Err(error) = Self::write_archive(&target_path, options, &result.manifest, observer) {
            // При отмене не оставляем «недозаписанный» архив на диске.
            if error.is_cancelled() && target_path == options.output {
                let _ = std::fs::remove_file(&target_path);
            }
            return Err(error);
        }

        if let (Some(passphrase), Some(temp)) = (&options.passphrase, &temp) {
            security::encrypt_file(temp.path(), &options.output, passphrase)?;
        }

        result.archive_size = std::fs::metadata(&options.output)?.len();
        result.duration_ms = started.elapsed().as_millis();
        Ok(result)
    }

    /// Записать несжатый поток: tar + zstd (+ manifest первым элементом).
    fn write_archive(
        path: &Path,
        options: &CreateArchiveOptions,
        manifest: &ArchiveManifest,
        observer: &dyn ProgressObserver,
    ) -> Result<()> {
        let file = File::create(path)?;
        let encoder = zstd::stream::write::Encoder::new(file, options.compression_level)
            .map_err(|e| MigrationError::Unknown(format!("ошибка сжатия: {}", e)))?;
        let mut builder = tar::Builder::new(encoder);

        let json = serde_json::to_vec_pretty(manifest)?;
        let mut header = tar::Header::new_gnu();
        header
            .set_path("manifest.json")
            .map_err(|e| MigrationError::Unknown(format!("манифест: {}", e)))?;
        header.set_size(json.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder.append_data(&mut header, "manifest.json", json.as_slice())?;

        let total = manifest.total_size;
        let mut done = 0u64;

        for file_entry in &manifest.files {
            if let Some(cancel) = &options.cancel {
                cancel.check()?;
            }

            let relative = Path::new(&file_entry.relative_path);
            let source = options.source_home.join(relative);
            let mut header = tar::Header::new_gnu();

            if file_entry.is_symlink {
                header.set_entry_type(tar::EntryType::Symlink);
                header
                    .set_path(&file_entry.relative_path)
                    .map_err(|e| MigrationError::Unknown(format!("ссылка: {}", e)))?;
                header.set_size(0);
                header.set_mode(file_entry.mode);
                header.set_uid(file_entry.uid as u64);
                header.set_gid(file_entry.gid as u64);
                let target = file_entry.symlink_target.clone().unwrap_or_default();
                header
                    .set_link_name(&target)
                    .map_err(|e| MigrationError::Unknown(format!("цель ссылки: {}", e)))?;
                header.set_cksum();
                builder.append(&header, std::io::empty())?;
            } else {
                let source_file = File::open(&source)?;
                let metadata = std::fs::symlink_metadata(&source)?;

                header.set_size(file_entry.size);
                header.set_mode(file_entry.mode);
                header.set_uid(file_entry.uid as u64);
                header.set_gid(file_entry.gid as u64);
                if let Ok(mtime) = metadata.modified() {
                    if let Ok(secs) = mtime.duration_since(std::time::UNIX_EPOCH) {
                        header.set_mtime(secs.as_secs());
                    }
                }
                header.set_cksum();
                builder.append_data(&mut header, relative, source_file)?;
            }

            done += file_entry.size;
            observer.on_progress(done, total, &source);
        }

        let encoder = builder.into_inner()?;
        let mut file = encoder.finish()?;
        file.flush()?;

        Ok(())
    }
}

impl ArchiveManager {
    /// Подготовить читаемый tar.zst (расшифровать при необходимости).
    ///
    /// Возвращает путь к потоку и, если выполнялась расшифровка, временный
    /// файл (его нужно держать живым на время чтения).
    fn prepare_source(
        archive: &Path,
        passphrase: Option<&str>,
    ) -> Result<(PathBuf, Option<tempfile::TempPath>)> {
        match passphrase {
            Some(pass) => {
                let tmp = tempfile::NamedTempFile::new()?;
                security::decrypt_file(archive, tmp.path(), pass)?;
                let temp_path = tmp.into_temp_path();
                let path = temp_path.to_path_buf();
                Ok((path, Some(temp_path)))
            }
            None => Ok((archive.to_path_buf(), None)),
        }
    }

    /// Открыть tar-архив поверх zstd-потока.
    fn open_tar(path: &Path) -> Result<tar::Archive<Box<dyn Read>>> {
        let file = File::open(path)?;
        let decoder = zstd::stream::read::Decoder::new(file)
            .map_err(|e| MigrationError::CorruptedArchive(format!("декодирование: {}", e)))?;
        Ok(tar::Archive::new(Box::new(decoder)))
    }

    /// Прочитать и проверить манифест архива.
    pub fn read_manifest(archive: &Path, passphrase: Option<&str>) -> Result<ArchiveManifest> {
        let (source, _keep) = Self::prepare_source(archive, passphrase)?;
        let mut tar_reader = Self::open_tar(&source)?;

        for entry in tar_reader.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();
            if path != "manifest.json" {
                continue;
            }

            let mut content = String::new();
            entry.read_to_string(&mut content)?;
            let manifest: ArchiveManifest = serde_json::from_str(&content)
                .map_err(|e| MigrationError::CorruptedArchive(format!("манифест: {}", e)))?;

            if manifest.format_version != crate::config::ARCHIVE_FORMAT_VERSION {
                return Err(MigrationError::Unsupported(format!(
                    "версия формата архива {} (поддерживается {})",
                    manifest.format_version,
                    crate::config::ARCHIVE_FORMAT_VERSION
                )));
            }

            if !manifest.verify_hash()? {
                return Err(MigrationError::ChecksumMismatch(
                    "манифест архива повреждён (хеш не совпадает)".to_string(),
                ));
            }

            return Ok(manifest);
        }

        Err(MigrationError::CorruptedArchive(
            "manifest.json отсутствует в архиве".to_string(),
        ))
    }

    /// Получить информацию об архиве без восстановления.
    pub fn inspect(archive: &Path, passphrase: Option<&str>) -> Result<ArchiveInfo> {
        let manifest = Self::read_manifest(archive, passphrase)?;
        let integrity_verified = manifest.verify_hash()?;
        let archive_size = std::fs::metadata(archive)?.len();

        Ok(ArchiveInfo {
            manifest,
            archive_path: archive.to_path_buf(),
            archive_size,
            integrity_verified,
        })
    }

    /// Список относительных путей внутри архива.
    pub fn list_contents(archive: &Path, passphrase: Option<&str>) -> Result<Vec<String>> {
        let manifest = Self::read_manifest(archive, passphrase)?;
        Ok(manifest
            .files
            .into_iter()
            .map(|file| file.relative_path)
            .collect())
    }

    /// Проверка архива: манифест и, при `deep`, хеши файлов.
    pub fn verify(archive: &Path, passphrase: Option<&str>, deep: bool) -> Result<VerifyReport> {
        let manifest = Self::read_manifest(archive, passphrase)?;
        let mut report = VerifyReport {
            manifest_ok: manifest.verify_hash()?,
            ..Default::default()
        };

        if !deep {
            return Ok(report);
        }

        let (source, _keep) = Self::prepare_source(archive, passphrase)?;
        let mut tar_reader = Self::open_tar(&source)?;

        // Пути манифеста и tar нормализуются к '/' (как и при восстановлении):
        // на Windows сканер даёт '\', а tar хранит '/'.
        let by_path: HashMap<String, &FileEntry> = manifest
            .files
            .iter()
            .map(|file| (file.relative_path.replace('\\', "/"), file))
            .collect();
        let mut seen: HashSet<String> = HashSet::new();

        for entry in tar_reader.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().to_string();
            if path == "manifest.json" {
                continue;
            }

            let normalized = path.replace('\\', "/");
            let expected = match by_path.get(&normalized) {
                Some(file) => *file,
                None => continue,
            };
            seen.insert(normalized);

            let actual = if expected.is_symlink {
                let target = entry
                    .link_name()?
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                security::compute_sha256_from_data(target.as_bytes())
            } else {
                security::hash_reader(&mut entry)?
            };

            report.checked_files += 1;
            if !actual.eq_ignore_ascii_case(&expected.hash) {
                report.mismatched.push(path);
            }
        }

        for file in &manifest.files {
            if !seen.contains(&file.relative_path.replace('\\', "/")) {
                report.missing.push(file.relative_path.clone());
            }
        }

        Ok(report)
    }
}

/// Определить конфликт для файла архива относительно целевого пути.
fn detect_conflict_for(
    file_entry: &FileEntry,
    destination: &Path,
    relative: &Path,
    source_mtime: Option<u64>,
) -> Result<Option<ConflictInfo>> {
    let metadata = match std::fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(None),
    };

    let target_size = metadata.len();
    let target_mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());

    // Идентичность определяем по содержимому (SHA-256), а не только по размеру:
    // иначе разные файлы одного размера ошибочно считались бы идентичными
    // и пропускались (потеря данных при стратегии Replace).
    let identical = !file_entry.is_symlink
        && file_entry.size == target_size
        && security::compute_sha256(destination)
            .map(|hash| hash.sha256.eq_ignore_ascii_case(&file_entry.hash))
            .unwrap_or(false);

    let kind = if identical {
        ConflictKind::Identical
    } else {
        let source_newer = match (source_mtime, target_mtime) {
            (Some(source), Some(target)) => source > target,
            _ => true,
        };
        if source_newer {
            ConflictKind::SourceNewer
        } else {
            ConflictKind::TargetNewer
        }
    };

    Ok(Some(ConflictInfo {
        target_path: destination.to_path_buf(),
        relative_path: relative.to_path_buf(),
        kind,
        source_size: file_entry.size,
        target_size,
        decision: None,
    }))
}

/// Путь «второй копии» для стратегии KeepBoth.
fn alternate_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    for counter in 0..1000 {
        let candidate_name = if counter == 0 {
            format!("{}.from-archive", name)
        } else {
            format!("{}.from-archive-{}", name, counter)
        };
        let candidate = destination.with_file_name(&candidate_name);
        if std::fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }

    destination.with_file_name(format!("{}.from-archive-{}", name, uuid::Uuid::new_v4()))
}

/// Путь сохранённой старой версии для стратегии RenameOld.
fn old_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    for counter in 0..1000 {
        let candidate_name = if counter == 0 {
            format!("{}.old", name)
        } else {
            format!("{}.old-{}", name, counter)
        };
        let candidate = destination.with_file_name(&candidate_name);
        if std::fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }

    destination.with_file_name(format!("{}.old-{}", name, uuid::Uuid::new_v4()))
}

impl ArchiveManager {
    /// Записать одну запись архива в целевой путь (атомарно).
    fn extract_entry(
        entry: &mut tar::Entry<'_, Box<dyn Read>>,
        file_entry: &FileEntry,
        destination: &Path,
    ) -> Result<()> {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if file_entry.is_symlink {
            file_transfer::remove_path(destination)?;
            let target = file_entry.symlink_target.clone().unwrap_or_default();
            create_symlink(Path::new(&target), destination)?;
            return Ok(());
        }

        let temp = file_transfer::temp_path_for(destination);
        {
            let mut out = File::create(&temp)?;
            std::io::copy(entry, &mut out)?;
            out.flush()?;
        }

        if let Ok(mode) = entry.header().mode() {
            let _ = platform::set_mode(&temp, mode);
        }

        file_transfer::replace_atomically(&temp, destination)
    }
}

/// Создать символическую ссылку (на Windows — с эвристикой файл/каталог).
fn create_symlink(target: &Path, destination: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, destination)?;
        Ok(())
    }

    #[cfg(windows)]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, destination)?;
        } else {
            std::os::windows::fs::symlink_file(target, destination)?;
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, destination);
        Err(MigrationError::Unsupported(
            "символические ссылки не поддерживаются".to_string(),
        ))
    }
}

/// Ближайший существующий каталог для `path` (сам путь либо его родитель).
///
/// Нужен для оценки свободного места в dry-run, когда целевой каталог ещё
/// не создан: `free_space` для несуществующего пути вернул бы 0.
fn nearest_existing_ancestor(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    while !current.is_dir() {
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    current
}

impl ArchiveManager {
    /// Восстановить архив в целевой каталог.
    pub fn restore(
        options: &RestoreOptions,
        observer: &dyn ProgressObserver,
    ) -> Result<RestoreResult> {
        let manifest = Self::read_manifest(&options.archive, options.passphrase.as_deref())?;
        let mut result = RestoreResult::default();

        let wanted: Vec<&FileEntry> = match &options.components {
            Some(components) => manifest.files_for_components(components),
            None => manifest.files.iter().collect(),
        };
        let required: u64 = wanted.iter().map(|file| file.size).sum();

        // Dry-run не должен оставлять следов: каталог назначения не создаётся.
        // Проверка места идёт по ближайшему существующему родителю.
        if options.dry_run {
            file_transfer::ensure_space(
                &nearest_existing_ancestor(&options.target_root),
                required,
            )?;
        } else {
            std::fs::create_dir_all(&options.target_root)?;
            file_transfer::ensure_space(&options.target_root, required)?;
        }

        let (source_path, _keep) =
            Self::prepare_source(&options.archive, options.passphrase.as_deref())?;
        let mut tar_reader = Self::open_tar(&source_path)?;

        // Ключи манифеста нормализуются к '/' — tar хранит пути с '/',
        // а сканер на Windows может дать '\'.
        let by_path: HashMap<String, &FileEntry> = manifest
            .files
            .iter()
            .map(|file| (file.relative_path.replace('\\', "/"), file))
            .collect();
        let mut resolver = ConflictResolver::new(options.strategy);
        let mut done_bytes = 0u64;

        for entry in tar_reader.entries()? {
            if let Some(cancel) = &options.cancel {
                cancel.check()?;
            }

            let mut entry = entry?;
            let raw_path = entry.path()?.to_string_lossy().to_string();
            if raw_path == "manifest.json" {
                continue;
            }

            let file_entry = match by_path.get(&raw_path.replace('\\', "/")) {
                Some(file) => *file,
                None => continue,
            };

            if let Some(components) = &options.components {
                if !components.iter().any(|c| file_entry.matches_component(c)) {
                    continue;
                }
            }

            // Защита от path traversal: путь берётся только из манифеста
            let relative =
                security::sanitize_relative_path(Path::new(&file_entry.relative_path))
                    .map_err(|_| MigrationError::PathTraversal(file_entry.relative_path.clone()))?;
            let mut destination = security::safe_join(&options.target_root, &relative)?;

            let source_mtime = entry.header().mtime().ok();

            if let Some(conflict) =
                detect_conflict_for(file_entry, &destination, &relative, source_mtime)?
            {
                let decision = resolver.resolve(conflict)?;
                match decision {
                    ConflictStrategy::Skip => {
                        result.skipped_files += 1;
                        continue;
                    }
                    ConflictStrategy::Replace => {
                        if !options.dry_run {
                            file_transfer::remove_path(&destination)?;
                        }
                    }
                    ConflictStrategy::RenameOld => {
                        if !options.dry_run {
                            let backup = old_path(&destination);
                            std::fs::rename(&destination, &backup)?;
                        }
                    }
                    ConflictStrategy::KeepBoth => {
                        destination = alternate_path(&destination);
                    }
                    _ => {
                        result.skipped_files += 1;
                        continue;
                    }
                }
            }

            if options.dry_run {
                result.restored_files += 1;
                result.restored_bytes += file_entry.size;
                continue;
            }

            match Self::extract_entry(&mut entry, file_entry, &destination) {
                Ok(()) => {
                    result.restored_files += 1;
                    result.restored_bytes += file_entry.size;

                    if options.verify_hash && !file_entry.is_symlink {
                        if let Ok(actual) = security::compute_sha256(&destination) {
                            result.verified_files += 1;
                            if !actual.sha256.eq_ignore_ascii_case(&file_entry.hash) {
                                result
                                    .errors
                                    .push(format!("хеш не совпадает: {}", raw_path));
                            }
                        }
                    }
                }
                Err(error) => result.errors.push(format!("{}: {}", raw_path, error)),
            }

            done_bytes += file_entry.size;
            observer.on_progress(done_bytes, required, &destination);
            observer.on_file_done(&destination, file_entry.size);
        }

        result.conflicts = resolver.conflicts().to_vec();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_transfer::NoProgress;
    use crate::platform;
    use tempfile::tempdir;

    fn make_home(dir: &Path) -> PathBuf {
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("Documents")).expect("mkdir");
        std::fs::create_dir_all(home.join(".config")).expect("mkdir");
        std::fs::write(home.join("Documents/report.txt"), b"hello archive").expect("write");
        std::fs::write(home.join(".config/app.conf"), b"key=value").expect("write");
        home
    }

    fn make_items(home: &Path) -> Vec<TransferItem> {
        vec![
            TransferItem::file(
                home.join("Documents/report.txt"),
                "Documents/report.txt",
                13,
            ),
            TransferItem::file(home.join(".config/app.conf"), ".config/app.conf", 9),
        ]
    }

    fn create_options(
        home: &Path,
        output: &Path,
        passphrase: Option<&str>,
    ) -> CreateArchiveOptions {
        CreateArchiveOptions {
            output: output.to_path_buf(),
            source_home: home.to_path_buf(),
            items: make_items(home),
            components: vec![ComponentType::Documents, ComponentType::AppConfigs],
            passphrase: passphrase.map(str::to_string),
            compression_level: 3,
            dry_run: false,
            cancel: None,
        }
    }

    fn restore_options(archive: &Path, target: &Path) -> RestoreOptions {
        RestoreOptions {
            archive: archive.to_path_buf(),
            passphrase: None,
            target_root: target.to_path_buf(),
            components: None,
            strategy: ConflictStrategy::Replace,
            verify_hash: true,
            dry_run: false,
            cancel: None,
        }
    }

    #[test]
    fn test_create_inspect_verify_round_trip() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let output = dir.path().join("profile.rmm");

        let created = ArchiveManager::create(&create_options(&home, &output, None), &NoProgress)
            .expect("create");

        assert!(created.archive_size > 0);
        assert!(!created.encrypted);
        assert_eq!(created.manifest.total_files, 2);
        assert!(created.manifest.manifest_hash.is_some());
        assert_eq!(created.manifest.source_uid, platform::current_uid());

        let info = ArchiveManager::inspect(&output, None).expect("inspect");
        assert!(info.integrity_verified);
        assert_eq!(info.manifest.total_files, 2);

        let contents = ArchiveManager::list_contents(&output, None).expect("list");
        assert_eq!(contents.len(), 2);
        assert!(contents.iter().any(|path| path.ends_with("report.txt")));

        let report = ArchiveManager::verify(&output, None, true).expect("verify");
        assert!(report.is_valid(), "отчёт: {:?}", report);
        assert_eq!(report.checked_files, 2);

        let shallow = ArchiveManager::verify(&output, None, false).expect("shallow");
        assert!(shallow.is_valid());
        assert_eq!(shallow.checked_files, 0);
    }

    #[test]
    fn test_restore_and_conflicts() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let output = dir.path().join("profile.rmm");
        ArchiveManager::create(&create_options(&home, &output, None), &NoProgress).expect("create");

        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("Documents")).expect("mkdir");
        std::fs::write(target.join("Documents/report.txt"), b"old content").expect("write");

        // Конфликт + фильтр компонентов: под заменой ничего не должно быть
        let mut options = restore_options(&output, &target);
        options.components = Some(vec![ComponentType::Documents]);
        options.strategy = ConflictStrategy::Skip;

        let result = ArchiveManager::restore(&options, &NoProgress).expect("restore");
        assert!(result.is_success(), "ошибки: {:?}", result.errors);
        assert_eq!(result.skipped_files, 1);
        assert_eq!(result.restored_files, 0);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(
            std::fs::read(target.join("Documents/report.txt")).expect("read"),
            b"old content"
        );

        // Полное восстановление с заменой
        let result = ArchiveManager::restore(&restore_options(&output, &target), &NoProgress)
            .expect("restore");
        assert!(result.is_success(), "ошибки: {:?}", result.errors);
        assert_eq!(result.restored_files, 2);
        assert_eq!(result.verified_files, 2);
        assert_eq!(
            std::fs::read(target.join("Documents/report.txt")).expect("read"),
            b"hello archive"
        );
        assert_eq!(
            std::fs::read(target.join(".config/app.conf")).expect("read"),
            b"key=value"
        );
    }

    #[test]
    fn test_dry_run_create_and_restore() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let output = dir.path().join("profile.rmm");

        let mut options = create_options(&home, &output, None);
        options.dry_run = true;
        let created = ArchiveManager::create(&options, &NoProgress).expect("dry-run");
        assert!(!output.exists());
        assert_eq!(created.archive_size, 0);
        assert_eq!(created.manifest.total_files, 2);

        ArchiveManager::create(&create_options(&home, &output, None), &NoProgress).expect("create");

        let target = dir.path().join("target-dry");
        let mut restore = restore_options(&output, &target);
        restore.dry_run = true;
        let result = ArchiveManager::restore(&restore, &NoProgress).expect("restore dry");
        assert_eq!(result.restored_files, 2);
        assert!(!target.join("Documents/report.txt").exists());
        // Dry-run не должен оставлять следов: целевой каталог не создаётся.
        assert!(
            !target.exists(),
            "dry-run создал каталог назначения: {}",
            target.display()
        );
        assert!(
            result.errors.is_empty(),
            "ошибки dry-run: {:?}",
            result.errors
        );
    }

    /// Проверка свободного места в dry-run работает по существующему родителю
    /// даже когда целевой каталог ещё не создан.
    #[test]
    fn test_dry_run_space_check_uses_existing_ancestor() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let output = dir.path().join("profile.rmm");
        ArchiveManager::create(&create_options(&home, &output, None), &NoProgress).expect("create");

        let target = dir.path().join("missing-parent").join("target");
        let mut restore = restore_options(&output, &target);
        restore.dry_run = true;

        let result = ArchiveManager::restore(&restore, &NoProgress).expect("dry-run");
        assert_eq!(result.restored_files, 2);
        assert!(!target.exists());
        assert!(!target.parent().expect("parent").exists());
    }

    #[test]
    fn test_encrypted_archive_round_trip() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let output = dir.path().join("secret.rmm");
        let passphrase = "correct horse battery";

        let created = ArchiveManager::create(
            &create_options(&home, &output, Some(passphrase)),
            &NoProgress,
        )
        .expect("create");
        assert!(created.encrypted);

        // Без пароля прочитать нельзя
        assert!(ArchiveManager::inspect(&output, None).is_err());

        let info = ArchiveManager::inspect(&output, Some(passphrase)).expect("inspect");
        assert!(info.integrity_verified);

        // Неверный пароль
        assert!(ArchiveManager::inspect(&output, Some("wrong")).is_err());

        let target = dir.path().join("target-enc");
        let mut options = restore_options(&output, &target);
        options.passphrase = Some(passphrase.to_string());
        let result = ArchiveManager::restore(&options, &NoProgress).expect("restore");
        assert!(result.is_success(), "ошибки: {:?}", result.errors);
        assert_eq!(
            std::fs::read(target.join("Documents/report.txt")).expect("read"),
            b"hello archive"
        );
    }

    #[test]
    fn test_create_rejects_empty_items() {
        let dir = tempdir().expect("tempdir");
        let options = CreateArchiveOptions {
            output: dir.path().join("empty.rmm"),
            source_home: dir.path().to_path_buf(),
            items: Vec::new(),
            components: Vec::new(),
            passphrase: None,
            compression_level: 3,
            dry_run: false,
            cancel: None,
        };
        assert!(ArchiveManager::create(&options, &NoProgress).is_err());
    }

    #[test]
    fn test_component_for_path() {
        assert_eq!(
            component_for_path("Documents/x.txt"),
            Some(ComponentType::Documents)
        );
        assert_eq!(
            component_for_path(".config/app.conf"),
            Some(ComponentType::AppConfigs)
        );
        assert_eq!(component_for_path("unknown/file"), None);
    }
}
