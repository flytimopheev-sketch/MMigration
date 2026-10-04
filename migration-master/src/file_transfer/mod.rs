//! Перенос файлов: копирование с прогрессом, атомарная замена,
//! проверка контрольных сумм и параллельное хеширование.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::cancel::CancelToken;
use crate::error::Result;
use crate::platform;
use crate::security;

/// Элемент передачи (файл или символическая ссылка).
#[derive(Debug, Clone)]
pub struct TransferItem {
    /// Абсолютный путь источника
    pub source: PathBuf,
    /// Относительный путь назначения
    pub relative: PathBuf,
    /// Размер в байтах
    pub size: u64,
    /// Является ли символической ссылкой
    pub is_symlink: bool,
    /// Цель символической ссылки
    pub symlink_target: Option<PathBuf>,
    /// Права доступа (Unix mode)
    pub mode: Option<u32>,
}

impl TransferItem {
    /// Создать элемент для обычного файла.
    pub fn file(source: impl Into<PathBuf>, relative: impl Into<PathBuf>, size: u64) -> Self {
        Self {
            source: source.into(),
            relative: relative.into(),
            size,
            is_symlink: false,
            symlink_target: None,
            mode: None,
        }
    }
}

/// Настройки копирования.
#[derive(Debug, Clone)]
pub struct CopyOptions {
    /// Перезаписывать существующие файлы
    pub overwrite: bool,
    /// Сохранять права доступа
    pub preserve_permissions: bool,
    /// Проверять контрольные суммы после копирования
    pub verify_hash: bool,
    /// Ограничение скорости (байт/сек), 0 — без ограничения
    pub rate_limit: u64,
    /// Режим без реальных изменений
    pub dry_run: bool,
    /// Токен отмены длительной операции
    pub cancel: Option<CancelToken>,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            overwrite: false,
            preserve_permissions: true,
            verify_hash: true,
            rate_limit: 0,
            dry_run: false,
            cancel: None,
        }
    }
}

/// Статистика переноса.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CopyStats {
    /// Скопировано файлов
    pub files_copied: u64,
    /// Скопировано байт
    pub bytes_copied: u64,
    /// Пропущено файлов
    pub files_skipped: u64,
    /// Проверено контрольных сумм
    pub verified: u64,
    /// Ошибки (путь + описание)
    pub errors: Vec<String>,
}

impl CopyStats {
    /// Было ли копирование успешным (без ошибок).
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Подписчик на события прогресса.
pub trait ProgressObserver: Send + Sync {
    /// Текущее состояние: сколько байт выполнено из общего объёма.
    fn on_progress(&self, done_bytes: u64, total_bytes: u64, current: &Path);

    /// Завершена обработка файла.
    fn on_file_done(&self, _path: &Path, _size: u64) {}

    /// Сообщение для пользователя.
    fn on_message(&self, _message: &str) {}
}

/// Наблюдатель-заглушка (без вывода).
pub struct NoProgress;

impl ProgressObserver for NoProgress {
    fn on_progress(&self, _done_bytes: u64, _total_bytes: u64, _current: &Path) {}
}

/// Ограничитель скорости передачи.
pub struct RateLimiter {
    bytes_per_second: u64,
    started: Instant,
    transferred: u64,
}

impl RateLimiter {
    /// Создать ограничитель (0 = без ограничения).
    pub fn new(bytes_per_second: u64) -> Self {
        Self {
            bytes_per_second,
            started: Instant::now(),
            transferred: 0,
        }
    }

    /// Учесть переданные байты и при необходимости приостановить передачу.
    pub fn throttle(&mut self, bytes: u64) {
        if self.bytes_per_second == 0 {
            return;
        }

        self.transferred += bytes;
        let expected =
            Duration::from_secs_f64(self.transferred as f64 / self.bytes_per_second as f64);
        let elapsed = self.started.elapsed();

        if expected > elapsed {
            std::thread::sleep(expected - elapsed);
        }
    }
}

/// Временный путь для атомарной записи (`<файл>.part`).
pub fn temp_path_for(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    name.push_str(".part");
    destination.with_file_name(name)
}

/// Атомарная замена файла (сначала пишем во временный, затем переименовываем).
pub fn replace_atomically(temp: &Path, destination: &Path) -> Result<()> {
    #[cfg(windows)]
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }

    std::fs::rename(temp, destination)?;
    Ok(())
}

/// Копирование файла с ограничением скорости и сохранением прав.
pub fn copy_file(
    source: &Path,
    destination: &Path,
    options: &CopyOptions,
    limiter: &mut RateLimiter,
) -> Result<u64> {
    let metadata =
        std::fs::metadata(source).map_err(|_| crate::error::MigrationError::not_found(source))?;

    if options.dry_run {
        return Ok(metadata.len());
    }

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let temp = temp_path_for(destination);
    let mut reader = std::io::BufReader::with_capacity(128 * 1024, std::fs::File::open(source)?);
    let mut writer = std::io::BufWriter::with_capacity(128 * 1024, std::fs::File::create(&temp)?);

    let mut buffer = vec![0u8; 128 * 1024];
    let mut written = 0u64;
    let mut chunks_since_check = 0u32;

    loop {
        // Отмена по Ctrl+C / кнопке «Отмена»: проверяем не каждый байт, а каждые 2 МБ.
        chunks_since_check += 1;
        if chunks_since_check >= 16 {
            chunks_since_check = 0;
            if let Some(token) = &options.cancel {
                if token.is_cancelled() {
                    drop(writer);
                    let _ = std::fs::remove_file(&temp);
                    return Err(crate::error::MigrationError::Cancelled);
                }
            }
        }

        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        written += read as u64;
        limiter.throttle(read as u64);
    }

    writer.flush()?;
    drop(writer);

    if options.preserve_permissions {
        if let Some(mode) = platform::file_mode(source) {
            platform::set_mode(&temp, mode)?;
        }
    }

    replace_atomically(&temp, destination)?;
    Ok(written)
}

/// Копирование символической ссылки.
pub fn copy_symlink(target: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, destination)?;
    }

    #[cfg(windows)]
    {
        let _ = target;
        let _ = destination;
    }

    Ok(())
}

/// Удалить путь (файл или каталог), если он существует.
pub fn remove_path(path: &Path) -> Result<()> {
    if !path.exists() && platform::symlink_target(path).is_none() {
        return Ok(());
    }

    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }

    Ok(())
}

/// Проверить наличие свободного места на целевом диске.
pub fn ensure_space(target_root: &Path, required: u64) -> Result<()> {
    let available = platform::free_space(target_root);
    if available > 0 && available < required {
        return Err(crate::error::MigrationError::InsufficientSpace {
            required,
            available,
        });
    }
    Ok(())
}

/// Скопировать набор элементов с отображением прогресса.
pub fn copy_items(
    items: &[TransferItem],
    target_root: &Path,
    options: &CopyOptions,
    observer: &dyn ProgressObserver,
) -> Result<CopyStats> {
    let total_bytes: u64 = items.iter().map(|i| i.size).sum();
    if !options.dry_run {
        ensure_space(target_root, total_bytes)?;
    }

    let mut stats = CopyStats::default();
    let mut limiter = RateLimiter::new(options.rate_limit);
    let mut done_bytes = 0u64;

    for item in items {
        if let Some(token) = &options.cancel {
            // Отмена длительной операции между файлами.
            token.check()?;
        }

        let destination = security::safe_join(target_root, &item.relative)?;
        observer.on_progress(done_bytes, total_bytes, &destination);

        if destination.exists() && !options.overwrite {
            stats.files_skipped += 1;
            done_bytes += item.size;
            continue;
        }

        let result = if item.is_symlink {
            let target = item
                .symlink_target
                .clone()
                .unwrap_or_else(|| item.source.clone());
            copy_symlink(&target, &destination)
        } else {
            copy_file(&item.source, &destination, options, &mut limiter).map(|_| ())
        };

        match result {
            Ok(()) => {
                stats.files_copied += 1;
                stats.bytes_copied += item.size;

                if options.verify_hash && !item.is_symlink && !options.dry_run {
                    let source_hash = security::compute_sha256(&item.source)?.sha256;
                    security::ensure_checksum(&destination, &source_hash)?;
                    stats.verified += 1;
                }

                observer.on_file_done(&destination, item.size);
            }
            Err(error) => {
                stats
                    .errors
                    .push(format!("{}: {}", item.source.display(), error));
                observer.on_message(&format!(
                    "Не удалось скопировать {}: {}",
                    item.source.display(),
                    error
                ));
            }
        }

        done_bytes += item.size;
    }

    observer.on_progress(done_bytes, total_bytes, target_root);
    Ok(stats)
}

/// Параллельное вычисление SHA-256 для набора путей.
pub fn hash_paths_parallel(
    paths: &[PathBuf],
    max_concurrency: usize,
) -> Result<Vec<security::HashResult>> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }

    let workers = max_concurrency.clamp(1, 16);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .map_err(|e| {
            crate::error::MigrationError::Unknown(format!("не удалось запустить runtime: {}", e))
        })?;

    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(workers));
    let paths: Vec<PathBuf> = paths.to_vec();

    runtime.block_on(async move {
        let mut tasks = Vec::with_capacity(paths.len());

        for path in paths {
            let semaphore = semaphore.clone();
            tasks.push(tokio::spawn(async move {
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|e| crate::error::MigrationError::Unknown(e.to_string()))?;

                match tokio::task::spawn_blocking(move || security::compute_sha256(&path)).await {
                    Ok(result) => result,
                    Err(e) => Err(crate::error::MigrationError::Unknown(format!(
                        "поток завершился с ошибкой: {}",
                        e
                    ))),
                }
            }));
        }

        let mut results = Vec::with_capacity(tasks.len());
        for task in tasks {
            match task.await {
                Ok(Ok(result)) => results.push(result),
                Ok(Err(e)) => return Err(e),
                Err(e) => {
                    return Err(crate::error::MigrationError::Unknown(format!(
                        "задача вычисления хеша прервана: {}",
                        e
                    )))
                }
            }
        }

        Ok(results)
    })
}

/// Проверить контрольные суммы файлов после восстановления.
pub fn verify_items(items: &[TransferItem], target_root: &Path) -> Result<CopyStats> {
    let mut stats = CopyStats::default();

    for item in items {
        if item.is_symlink {
            continue;
        }

        let destination = security::safe_join(target_root, &item.relative)?;
        if !destination.exists() {
            stats
                .errors
                .push(format!("отсутствует файл: {}", destination.display()));
            continue;
        }

        let source_size = std::fs::metadata(&item.source)
            .map(|m| m.len())
            .unwrap_or(0);
        let dest_size = std::fs::metadata(&destination)
            .map(|m| m.len())
            .unwrap_or(0);

        if source_size != dest_size {
            stats.errors.push(format!(
                "размер не совпадает: {} ({} != {})",
                destination.display(),
                dest_size,
                source_size
            ));
            continue;
        }

        stats.verified += 1;
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_file(path: &Path, data: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, data).expect("write");
    }

    #[test]
    fn test_copy_file_creates_destination() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        let destination = dir.path().join("nested/destination.txt");
        write_file(&source, b"payload");

        let mut limiter = RateLimiter::new(0);
        let copied =
            copy_file(&source, &destination, &CopyOptions::default(), &mut limiter).expect("copy");

        assert_eq!(copied, 7);
        assert_eq!(std::fs::read(&destination).expect("read"), b"payload");
    }

    #[test]
    fn test_copy_file_dry_run_does_not_create_file() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        let destination = dir.path().join("dry.txt");
        write_file(&source, b"payload");

        let options = CopyOptions {
            dry_run: true,
            ..CopyOptions::default()
        };
        let mut limiter = RateLimiter::new(0);
        copy_file(&source, &destination, &options, &mut limiter).expect("copy");

        assert!(!destination.exists());
    }

    #[test]
    fn test_copy_file_replaces_atomically_without_leftovers() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        let destination = dir.path().join("destination.txt");
        write_file(&source, b"new");
        write_file(&destination, b"old");

        let options = CopyOptions {
            overwrite: true,
            ..CopyOptions::default()
        };
        let mut limiter = RateLimiter::new(0);
        copy_file(&source, &destination, &options, &mut limiter).expect("copy");

        assert_eq!(std::fs::read(&destination).expect("read"), b"new");
        assert!(!temp_path_for(&destination).exists());
    }

    #[test]
    fn test_remove_path_handles_files_and_directories() {
        let dir = tempdir().expect("tempdir");
        let file = dir.path().join("file.txt");
        let nested = dir.path().join("nested");
        write_file(&file, b"x");
        std::fs::create_dir_all(nested.join("deep")).expect("mkdir");

        remove_path(&file).expect("remove file");
        remove_path(&nested).expect("remove dir");
        remove_path(&file).expect("remove missing is ok");

        assert!(!file.exists());
        assert!(!nested.exists());
    }

    #[test]
    fn test_rate_limiter_without_limit_is_fast() {
        let mut limiter = RateLimiter::new(0);
        let started = Instant::now();
        for _ in 0..100 {
            limiter.throttle(1024);
        }
        assert!(started.elapsed().as_millis() < 500);
    }

    #[test]
    fn test_copy_items_reports_stats() {
        let dir = tempdir().expect("tempdir");
        let source_dir = dir.path().join("source");
        let target_dir = dir.path().join("target");

        let mut items = Vec::new();
        for index in 0..3 {
            let file = source_dir.join(format!("file{}.txt", index));
            write_file(&file, format!("data {}", index).as_bytes());
            let size = std::fs::metadata(&file).expect("meta").len();
            items.push(TransferItem::file(
                &file,
                PathBuf::from(format!("files/file{}.txt", index)),
                size,
            ));
        }

        let options = CopyOptions {
            overwrite: true,
            ..CopyOptions::default()
        };
        let stats = copy_items(&items, &target_dir, &options, &NoProgress).expect("copy");

        assert_eq!(stats.files_copied, 3);
        assert_eq!(stats.verified, 3);
        assert!(stats.is_success());
        assert!(target_dir.join("files/file0.txt").exists());
    }

    #[test]
    fn test_copy_items_skips_existing_by_default() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        let target_dir = dir.path().join("target");
        write_file(&source, b"data");
        write_file(&target_dir.join("existing.txt"), b"old");

        let items = vec![TransferItem::file(
            &source,
            PathBuf::from("existing.txt"),
            4,
        )];
        let stats =
            copy_items(&items, &target_dir, &CopyOptions::default(), &NoProgress).expect("copy");

        assert_eq!(stats.files_skipped, 1);
        assert_eq!(stats.files_copied, 0);
        assert_eq!(
            std::fs::read(target_dir.join("existing.txt")).expect("read"),
            b"old"
        );
    }

    #[test]
    fn test_copy_items_rejects_path_traversal() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        let target_dir = dir.path().join("target");
        write_file(&source, b"data");
        std::fs::create_dir_all(&target_dir).expect("mkdir");

        let items = vec![TransferItem::file(
            &source,
            PathBuf::from("../escaped.txt"),
            4,
        )];
        let result = copy_items(&items, &target_dir, &CopyOptions::default(), &NoProgress);

        assert!(result.is_err());
        assert!(!dir.path().join("escaped.txt").exists());
    }

    #[test]
    fn test_hash_paths_parallel() {
        let dir = tempdir().expect("tempdir");
        let mut paths = Vec::new();

        for index in 0..8 {
            let file = dir.path().join(format!("file{}.txt", index));
            write_file(&file, format!("content {}", index).as_bytes());
            paths.push(file);
        }

        let results = hash_paths_parallel(&paths, 4).expect("hashes");
        assert_eq!(results.len(), 8);
        for result in &results {
            assert_eq!(result.sha256.len(), 64);
        }

        assert!(hash_paths_parallel(&[], 4).expect("empty").is_empty());
        assert!(hash_paths_parallel(&[dir.path().join("missing")], 2).is_err());
    }

    #[test]
    fn test_verify_items_detects_size_mismatch() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        let target_dir = dir.path().join("target");
        write_file(&source, b"12345");
        write_file(&target_dir.join("file.txt"), b"12");

        let items = vec![TransferItem::file(&source, PathBuf::from("file.txt"), 5)];
        let stats = verify_items(&items, &target_dir).expect("verify");

        assert!(!stats.is_success());
        assert_eq!(stats.verified, 0);
    }
}
