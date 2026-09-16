//! Модуль работы с архивами миграции (.rmm формат)
//! 
//! Формат архива:
//! - manifest.json (метаданные)
//! - зашифрованные данные (age)
//! - SHA-256 контрольная сумма

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use anyhow::{Context, Result, bail, ensure};
use crate::config::ComponentType;
use crate::security::{encrypt_data, decrypt_data};

/// Версия формата архива
const ARCHIVE_FORMAT_VERSION: u32 = 1;
/// Расширение файлов архива
const ARCHIVE_EXTENSION: &str = "rmm";

/// Метаданные архива миграции
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveManifest {
    /// Версия формата архива
    pub format_version: u32,
    /// Имя хоста источника
    pub source_hostname: String,
    /// Имя пользователя источника
    pub source_user: String,
    /// UID пользователя источника
    pub source_uid: u32,
    /// GID пользователя источника
    pub source_gid: u32,
    /// Дата создания архива (Unix timestamp)
    pub created_at: u64,
    /// Версия приложения, создавшего архив
    pub app_version: String,
    /// Сведения об ОС источника
    pub os_info: OsInfo,
    /// Список переносимых компонентов
    pub components: Vec<ComponentType>,
    /// Список файлов в архиве
    pub files: Vec<FileEntry>,
    /// Общий размер данных до сжатия
    pub total_size: u64,
    /// SHA-256 хеш манифеста (для проверки целостности)
    pub manifest_hash: Option<String>,
}

/// Сведения об операционной системе
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsInfo {
    /// Название ОС
    pub name: String,
    /// Версия ОС
    pub version: String,
    /// Архитектура
    pub architecture: String,
}

/// Информация о файле в архиве
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Относительный путь файла
    pub relative_path: String,
    /// Полный путь источника
    pub source_path: String,
    /// Размер файла в байтах
    pub size: u64,
    /// SHA-256 хеш файла
    pub hash: String,
    /// Права доступа (Unix mode)
    pub mode: u32,
    /// UID владельца
    pub uid: u32,
    /// GID владельца
    pub gid: u32,
    /// Является ли файл символьной ссылкой
    pub is_symlink: bool,
    /// Цель символической ссылки (если применимо)
    pub symlink_target: Option<String>,
}

/// Контекст создания архива
#[derive(Debug, Clone)]
pub struct CreateArchiveContext {
    /// Путь к выходному файлу архива
    pub output_path: PathBuf,
    /// Пароль для шифрования
    pub passphrase: String,
    /// Выбранные компоненты для миграции
    pub components: Vec<ComponentType>,
    /// Список файлов для включения
    pub files: Vec<FileEntry>,
    /// Путь к домашнему каталогу источника
    pub source_home: PathBuf,
    /// Dry-run режим (только анализ, без создания)
    pub dry_run: bool,
}

/// Контекст восстановления из архива
#[derive(Debug, Clone)]
pub struct RestoreArchiveContext {
    /// Путь к файлу архива
    pub archive_path: PathBuf,
    /// Пароль для расшифровки
    pub passphrase: String,
    /// Целевой путь для восстановления
    pub target_path: PathBuf,
    /// Компоненты для восстановления (None = все)
    pub components: Option<Vec<ComponentType>>,
    /// Dry-run режим
    pub dry_run: bool,
    /// Перезаписывать существующие файлы
    pub overwrite: bool,
}

/// Результат анализа архива
#[derive(Debug, Clone)]
pub struct ArchiveInfo {
    /// Манифест архива
    pub manifest: ArchiveManifest,
    /// Путь к файлу архива
    pub archive_path: PathBuf,
    /// Размер файла архива
    pub archive_size: u64,
    /// Статус проверки целостности
    pub integrity_verified: bool,
}

/// Менеджер архивов миграции
pub struct ArchiveManager;

impl ArchiveManager {
    /// Создать новый архив миграции
    pub fn create_archive(ctx: CreateArchiveContext) -> Result<PathBuf> {
        // Проверка пути на path traversal
        Self::validate_archive_path(&ctx.output_path)?;
        
        if ctx.dry_run {
            log::info!("Dry-run: создание архива отменено");
            log::info!("Путь: {}", ctx.output_path.display());
            log::info!("Компонентов: {}", ctx.components.len());
            log::info!("Файлов: {}", ctx.files.len());
            let total_size: u64 = ctx.files.iter().map(|f| f.size).sum();
            log::info!("Общий размер: {} байт", total_size);
            return Ok(ctx.output_path);
        }
        
        // Создание манифеста
        let hostname = hostname::get()
            .context("Не удалось получить имя хоста")?
            .to_string_lossy()
            .to_string();
        
        let os_info = Self::detect_os_info()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("Ошибка системного времени")?
            .as_secs();
        
        let total_size: u64 = ctx.files.iter().map(|f| f.size).sum();
        
        let mut manifest = ArchiveManifest {
            format_version: ARCHIVE_FORMAT_VERSION,
            source_hostname: hostname,
            source_user: whoami::username(),
            source_uid: uzers::get_current_uid(),
            source_gid: uzers::get_current_gid(),
            created_at: now,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            os_info,
            components: ctx.components,
            files: ctx.files,
            total_size,
            manifest_hash: None,
        };
        
        // Вычисление хеша манифеста
        let manifest_json = serde_json::to_string_pretty(&manifest)
            .context("Ошибка сериализации манифеста")?;
        let manifest_hash = Self::compute_hash(manifest_json.as_bytes());
        manifest.manifest_hash = Some(manifest_hash.clone());
        
        // Обновлённый JSON манифеста
        let manifest_json = serde_json::to_string_pretty(&manifest)
            .context("Ошибка сериализации манифеста с хешем")?;
        
        // Создание временного каталога для упаковки
        let temp_dir = tempfile::tempdir()
            .context("Не удалось создать временный каталог")?;
        let temp_archive_path = temp_dir.path().join("archive.tmp");
        
        // Упаковка данных в tar+zstd
        log::info!("Упаковка данных в архив...");
        Self::create_tar_archive(
            &temp_archive_path,
            &manifest_json,
            &ctx.files,
            &ctx.source_home,
        )?;
        
        // Шифрование архива
        log::info!("Шифрование архива...");
        let encrypted_data = encrypt_data(&temp_archive_path, &ctx.passphrase)?;
        
        // Запись зашифрованного архива
        let output_file = File::create(&ctx.output_path)
            .with_context(|| format!("Не удалось создать файл {}", ctx.output_path.display()))?;
        let mut writer = BufWriter::new(output_file);
        writer.write_all(&encrypted_data)
            .context("Ошибка записи зашифрованных данных")?;
        writer.flush()?;
        
        log::info!("Архив создан: {}", ctx.output_path.display());
        log::info!("Размер архива: {} байт", fs::metadata(&ctx.output_path)?.len());
        
        Ok(ctx.output_path)
    }
    
    /// Восстановить данные из архива
    pub fn restore_archive(ctx: RestoreArchiveContext) -> Result<()> {
        // Проверка существования архива
        ensure!(
            ctx.archive_path.exists(),
            "Архив не найден: {}",
            ctx.archive_path.display()
        );
        
        // Проверка пути назначения на path traversal
        Self::validate_restore_path(&ctx.target_path)?;
        
        if ctx.dry_run {
            log::info!("Dry-run: восстановление отменено");
            log::info!("Архив: {}", ctx.archive_path.display());
            log::info!("Цель: {}", ctx.target_path.display());
            return Ok(());
        }
        
        // Чтение и расшифровка архива
        log::info!("Чтение и расшифровка архива...");
        let decrypted_data = decrypt_data(&ctx.archive_path, &ctx.passphrase)?;
        
        // Временный каталог для расшифрованных данных
        let temp_dir = tempfile::tempdir()
            .context("Не удалось создать временный каталог")?;
        let temp_archive_path = temp_dir.path().join("decrypted.tar.zst");
        
        // Запись расшифрованных данных
        let mut temp_file = File::create(&temp_archive_path)?;
        temp_file.write_all(&decrypted_data)?;
        drop(temp_file);
        
        // Извлечение манифеста и данных
        log::info!("Извлечение данных из архива...");
        let manifest = Self::extract_archive(
            &temp_archive_path,
            &ctx.target_path,
            ctx.components.as_ref(),
            ctx.overwrite,
        )?;
        
        // Фильтрация по компонентам если указано
        let files_to_restore: Vec<&FileEntry> = if let Some(components) = &ctx.components {
            manifest.files.iter()
                .filter(|f| components.iter().any(|c| {
                    // Простая логика: проверяем префикс пути
                    f.relative_path.starts_with(&Self::component_to_prefix(c))
                }))
                .collect()
        } else {
            manifest.files.iter().collect()
        };
        
        log::info!("Восстановлено файлов: {}", files_to_restore.len());
        
        Ok(())
    }
    
    /// Получить информацию об архиве без расшифровки
    pub fn inspect_archive(archive_path: &Path, passphrase: &str) -> Result<ArchiveInfo> {
        // Чтение и расшифровка только манифеста
        let decrypted_data = decrypt_data(archive_path, passphrase)?;
        
        // Временный файл для анализа
        let temp_dir = tempfile::tempdir()?;
        let temp_path = temp_dir.path().join("inspect.tar.zst");
        File::create(&temp_path)?.write_all(&decrypted_data)?;
        
        // Извлечение манифеста
        let manifest = Self::extract_manifest_only(&temp_path)?;
        
        // Проверка целостности
        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        let computed_hash = Self::compute_hash(manifest_json.as_bytes());
        let integrity_verified = manifest.manifest_hash.as_ref() == Some(&computed_hash);
        
        let archive_size = fs::metadata(archive_path)?.len();
        
        Ok(ArchiveInfo {
            manifest,
            archive_path: archive_path.to_path_buf(),
            archive_size,
            integrity_verified,
        })
    }
    
    /// Проверить контрольную сумму архива
    pub fn verify_archive(archive_path: &Path, passphrase: &str) -> Result<bool> {
        let info = Self::inspect_archive(archive_path, passphrase)?;
        Ok(info.integrity_verified)
    }
    
    // === Приватные методы ===
    
    /// Создать tar+zstd архив из файлов
    fn create_tar_archive(
        archive_path: &Path,
        manifest_json: &str,
        files: &[FileEntry],
        source_home: &Path,
    ) -> Result<()> {
        use zstd::stream::write::Encoder;
        
        let file = File::create(archive_path)?;
        let encoder = Encoder::new(file, 3)?; // Уровень сжатия 3
        let mut tar_builder = tar::Builder::new(encoder);
        
        // Добавление манифеста первым файлом
        let mut header = tar::Header::new_gnu();
        header.set_path("manifest.json")?;
        header.set_size(manifest_json.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(SystemTime::now()
            .duration_since(UNIX_EPOCH)?.as_secs());
        header.set_cksum();
        
        tar_builder.append_data(&mut header, "manifest.json", manifest_json.as_bytes())?;
        
        // Добавление файлов данных
        for file_entry in files {
            let full_path = source_home.join(&file_entry.source_path);
            
            if !full_path.exists() {
                log::warn!("Файл не найден: {}", full_path.display());
                continue;
            }
            
            if file_entry.is_symlink {
                // Символическая ссылка
                if let Some(target) = &file_entry.symlink_target {
                    tar_builder.append_link(&mut tar::Header::new_gnu(), 
                                           &file_entry.relative_path, target)?;
                }
            } else {
                // Обычный файл
                let mut file = File::open(&full_path)?;
                tar_builder.append_path_with_name(&full_path, &file_entry.relative_path)?;
            }
        }
        
        // Завершение архива
        let encoder = tar_builder.into_inner()?;
        encoder.finish()?.flush()?;
        
        Ok(())
    }
    
    /// Извлечь архив в целевую директорию
    fn extract_archive(
        archive_path: &Path,
        target_path: &Path,
        components: Option<&[ComponentType]>,
        overwrite: bool,
    ) -> Result<ArchiveManifest> {
        use zstd::stream::read::Decoder;
        
        let file = File::open(archive_path)?;
        let decoder = Decoder::new(file)?;
        let mut tar_archive = tar::Archive::new(decoder);
        
        // Сначала извлекаем манифест
        let entries = tar_archive.entries()?;
        let mut manifest: Option<ArchiveManifest> = None;
        let mut other_entries = Vec::new();
        
        for entry_result in entries {
            let mut entry = entry_result?;
            let path = entry.path()?.to_string_lossy().to_string();
            
            if path == "manifest.json" {
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                manifest = Some(serde_json::from_str(&content)?);
            } else {
                other_entries.push(path);
            }
        }
        
        let manifest = manifest.context("Манифест не найден в архиве")?;
        
        // Создание целевой директории
        fs::create_dir_all(target_path)?;
        
        // Извлечение остальных файлов
        let mut tar_archive = tar::Archive::new(Decoder::new(File::open(archive_path)?)?);
        for mut entry in tar_archive.entries()? {
            let entry_path = entry.path()?.to_string_lossy().to_string();
            
            if entry_path == "manifest.json" {
                continue; // Пропускаем манифест
            }
            
            // Проверка компонента если указана фильтрация
            if let Some(components) = components {
                if !components.iter().any(|c| entry_path.starts_with(&Self::component_to_prefix(c))) {
                    continue;
                }
            }
            
            // Защита от path traversal при извлечении
            let safe_path = Self::sanitize_path(&entry_path)?;
            let full_target_path = target_path.join(safe_path);
            
            // Проверка конфликта файлов
            if full_target_path.exists() && !overwrite {
                log::warn!("Файл уже существует: {}", full_target_path.display());
                // TODO: Реализовать стратегию разрешения конфликтов
                continue;
            }
            
            // Создание родительских директорий
            if let Some(parent) = full_target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            
            // Извлечение файла
            entry.unpack(&full_target_path)?;
            
            // Восстановление прав доступа
            if let Ok(metadata) = entry.header().mode() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&full_target_path, 
                                       fs::Permissions::from_mode(metadata))?;
                }
            }
        }
        
        Ok(manifest)
    }
    
    /// Извлечь только манифест из архива
    fn extract_manifest_only(archive_path: &Path) -> Result<ArchiveManifest> {
        use zstd::stream::read::Decoder;
        
        let file = File::open(archive_path)?;
        let decoder = Decoder::new(file)?;
        let mut tar_archive = tar::Archive::new(decoder);
        
        for mut entry in tar_archive.entries()? {
            let path = entry.path()?.to_string_lossy().to_string();
            if path == "manifest.json" {
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                return Ok(serde_json::from_str(&content)?);
            }
        }
        
        bail!("Манифест не найден в архиве")
    }
    
    /// Получить префикс пути для компонента
    fn component_to_prefix(component: &ComponentType) -> String {
        match component {
            ComponentType::Desktop => "Desktop".to_string(),
            ComponentType::Documents => "Documents".to_string(),
            ComponentType::Downloads => "Downloads".to_string(),
            ComponentType::Pictures => "Pictures".to_string(),
            ComponentType::Videos => "Videos".to_string(),
            ComponentType::Music => "Music".to_string(),
            ComponentType::Templates => "Templates".to_string(),
            ComponentType::AppConfigs => ".config".to_string(),
            ComponentType::AppData => ".local/share".to_string(),
            ComponentType::SshKeys => ".ssh".to_string(),
            ComponentType::Fonts => ".fonts".to_string(),
            ComponentType::Themes => ".themes".to_string(),
            ComponentType::Icons => ".icons".to_string(),
            ComponentType::LocalApps => ".local/share/applications".to_string(),
            ComponentType::CustomDirs => String::new(),
            _ => String::new(),
        }
    }
    
    /// Определить информацию об ОС
    fn detect_os_info() -> Result<OsInfo> {
        let os_release = fs::read_to_string("/etc/os-release")
            .unwrap_or_else(|_| "NAME=\"RED OS\"\nVERSION=\"7.3\"".to_string());
        
        let mut name = "RED OS".to_string();
        let mut version = "unknown".to_string();
        
        for line in os_release.lines() {
            if line.starts_with("NAME=") {
                name = line.trim_start_matches("NAME=").trim_matches('"').to_string();
            }
            if line.starts_with("VERSION=") {
                version = line.trim_start_matches("VERSION=").trim_matches('"').to_string();
            }
        }
        
        let architecture = std::env::consts::ARCH.to_string();
        
        Ok(OsInfo {
            name,
            version,
            architecture,
        })
    }
    
    /// Вычислить SHA-256 хеш данных
    fn compute_hash(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        format!("{:x}", result)
    }
    
    /// Проверить путь архива на безопасность
    fn validate_archive_path(path: &Path) -> Result<()> {
        // Проверка на абсолютный путь
        if !path.is_absolute() {
            bail!("Путь архива должен быть абсолютным");
        }
        
        // Проверка расширения
        if path.extension().map_or(true, |ext| ext != ARCHIVE_EXTENSION) {
            bail!("Неверное расширение файла. Ожидается .{}", ARCHIVE_EXTENSION);
        }
        
        Ok(())
    }
    
    /// Проверить путь восстановления на безопасность
    fn validate_restore_path(path: &Path) -> Result<()> {
        if !path.is_absolute() {
            bail!("Путь восстановления должен быть абсолютным");
        }
        
        // Нормализация пути и проверка на выход за пределы
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        Self::sanitize_path(canonical.to_str().unwrap_or(""))?;
        
        Ok(())
    }
    
    /// Санитизировать путь (защита от path traversal)
    fn sanitize_path(path_str: &str) -> Result<PathBuf> {
        let path = PathBuf::from(path_str);
        
        // Проверка на наличие ".."
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                bail!("Обнаружен path traversal: {}", path_str);
            }
        }
        
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[test]
    fn test_validate_archive_path() {
        // Допустимый путь
        let valid_path = PathBuf::from("/home/user/backup.rmm");
        assert!(ArchiveManager::validate_archive_path(&valid_path).is_ok());
        
        // Неверное расширение
        let invalid_ext = PathBuf::from("/home/user/backup.tar");
        assert!(ArchiveManager::validate_archive_path(&invalid_ext).is_err());
        
        // Относительный путь
        let relative_path = PathBuf::from("backup.rmm");
        assert!(ArchiveManager::validate_archive_path(&relative_path).is_err());
    }
    
    #[test]
    fn test_sanitize_path() {
        // Нормальный путь
        let normal = ArchiveManager::sanitize_path("Documents/file.txt").unwrap();
        assert_eq!(normal, PathBuf::from("Documents/file.txt"));
        
        // Path traversal
        let traversal = ArchiveManager::sanitize_path("../etc/passwd");
        assert!(traversal.is_err());
        
        // Сложный path traversal
        let complex = ArchiveManager::sanitize_path("Documents/../../../etc/passwd");
        assert!(complex.is_err());
    }
    
    #[test]
    fn test_component_to_prefix() {
        assert_eq!(ArchiveManager::component_to_prefix(&MigrationComponent::Desktop), "Desktop");
        assert_eq!(ArchiveManager::component_to_prefix(&MigrationComponent::SshKeys), ".ssh");
        assert_eq!(ArchiveManager::component_to_prefix(&MigrationComponent::Config), ".config");
    }
    
    #[test]
    fn test_compute_hash() {
        let data = b"test data";
        let hash1 = ArchiveManager::compute_hash(data);
        let hash2 = ArchiveManager::compute_hash(data);
        assert_eq!(hash1, hash2);
        
        let different_data = b"different data";
        let hash3 = ArchiveManager::compute_hash(different_data);
        assert_ne!(hash1, hash3);
    }
}
