//! Платформенные операции: информация о пользователе, системе, правах доступа.
//!
//! Модуль изолирует всё, что зависит от конкретной ОС. Целевая платформа —
//! РЕД ОС Linux (x86_64), но код собирается и на других платформах (для
//! разработки и тестирования), linux-специфичные участки скрыты за `cfg(unix)`.

use std::path::{Path, PathBuf};

use crate::error::{MigrationError, Result};

/// Домашний каталог текущего пользователя.
pub fn home_dir() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Ok(PathBuf::from(home));
        }
    }

    dirs::home_dir().ok_or_else(|| {
        MigrationError::FileNotFound("не удалось определить домашний каталог".to_string())
    })
}

/// Имя текущего пользователя.
pub fn username() -> String {
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() {
            return user;
        }
    }
    if let Ok(user) = std::env::var("LOGNAME") {
        if !user.is_empty() {
            return user;
        }
    }

    whoami::fallible::username().unwrap_or_else(|_| "user".to_string())
}

/// UID текущего пользователя (0 на платформах без UID).
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata("/proc/self") {
            return meta.uid();
        }
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(meta) = std::fs::metadata(cwd) {
                return meta.uid();
            }
        }
        0
    }

    #[cfg(not(unix))]
    {
        0
    }
}

/// GID текущего пользователя (0 на платформах без GID).
pub fn current_gid() -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata("/proc/self") {
            return meta.gid();
        }
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(meta) = std::fs::metadata(cwd) {
                return meta.gid();
            }
        }
        0
    }

    #[cfg(not(unix))]
    {
        0
    }
}

/// Запущено ли приложение от имени root.
pub fn is_root() -> bool {
    current_uid() == 0
}

/// Имя хоста.
pub fn hostname() -> String {
    hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "localhost".to_string())
}

/// Архитектура системы.
pub fn architecture() -> String {
    std::env::consts::ARCH.to_string()
}

/// Является ли платформа Linux.
pub fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

/// Сведения о дистрибутиве из `/etc/os-release`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OsRelease {
    /// Идентификатор дистрибутива (`ID`)
    pub id: String,
    /// Читаемое название (`PRETTY_NAME`)
    pub pretty_name: String,
    /// Версия (`VERSION_ID`)
    pub version: String,
}

/// Чтение сведений о дистрибутиве.
pub fn os_release() -> OsRelease {
    let mut release = OsRelease {
        id: std::env::consts::OS.to_string(),
        pretty_name: std::env::consts::OS.to_string(),
        version: "unknown".to_string(),
    };

    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        for line in content.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').to_string();
            match key.trim() {
                "ID" => release.id = value,
                "PRETTY_NAME" => release.pretty_name = value,
                "VERSION_ID" => release.version = value,
                _ => {}
            }
        }
    }

    release
}

/// Доступное место на диске, содержащем указанный путь (байты).
pub fn free_space(path: &Path) -> u64 {
    with_disk(path, |disk| disk.available_space(), 0)
}

/// Общий размер диска, содержащего указанный путь (байты).
pub fn total_space(path: &Path) -> u64 {
    with_disk(path, |disk| disk.total_space(), 0)
}

/// Тип файловой системы для указанного пути.
pub fn filesystem_for(path: &Path) -> Option<String> {
    with_disk(
        path,
        |disk| Some(disk.file_system().to_string_lossy().to_string()),
        None,
    )
}

/// Найти диск для пути и применить к нему функцию (sysinfo::Disk не Clone).
fn with_disk<R>(path: &Path, apply: impl FnOnce(&sysinfo::Disk) -> R, default: R) -> R {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let disks = sysinfo::Disks::new_with_refreshed_list();

    let mut best: Option<&sysinfo::Disk> = None;
    let mut best_len = 0usize;

    for disk in disks.list() {
        let mount = disk.mount_point();
        if canonical.starts_with(mount) {
            let len = mount.as_os_str().len();
            if len >= best_len {
                best_len = len;
                best = Some(disk);
            }
        }
    }

    match best.or_else(|| disks.list().first()) {
        Some(disk) => apply(disk),
        None => default,
    }
}

/// Проверить наличие исполняемой команды в `PATH`.
pub fn command_exists(command: &str) -> bool {
    which::which(command).is_ok()
}

/// Полный путь к команде, если она доступна.
pub fn command_path(command: &str) -> Option<PathBuf> {
    which::which(command).ok()
}

/// Права доступа файла в виде Unix mode.
pub fn file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .ok()
            .map(|meta| meta.permissions().mode())
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Владелец файла: (UID, GID).
pub fn owner_uid_gid(path: &Path) -> (Option<u32>, Option<u32>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(path) {
            Ok(meta) => (Some(meta.uid()), Some(meta.gid())),
            Err(_) => (None, None),
        }
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        (None, None)
    }
}

/// Установить права доступа (Unix mode). На других платформах — no-op.
pub fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }

    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }

    Ok(())
}

/// Изменить владельца файла. Требует прав root (или совпадения с текущим пользователем).
pub fn set_owner(path: &Path, uid: u32, gid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(|e| {
            MigrationError::PermissionDenied(format!(
                "не удалось изменить владельца {}: {}",
                path.display(),
                e
            ))
        })?;
    }

    #[cfg(not(unix))]
    {
        let _ = (path, uid, gid);
    }

    Ok(())
}

/// Права `0600` для файла (приватные ключи, конфигурация SSH).
pub fn ensure_private_file(path: &Path) -> Result<()> {
    set_mode(path, 0o600)
}

/// Права `0644` для файла (публичные ключи).
pub fn ensure_public_file(path: &Path) -> Result<()> {
    set_mode(path, 0o644)
}

/// Права `0700` для каталога (`~/.ssh`).
pub fn ensure_private_dir(path: &Path) -> Result<()> {
    set_mode(path, 0o700)
}

/// Является ли путь символической ссылкой и куда она указывает.
pub fn symlink_target(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        std::fs::read_link(path).ok()
    } else {
        None
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_home_dir_resolves() {
        let home = home_dir().expect("домашний каталог должен определяться");
        assert!(!home.as_os_str().is_empty());
    }

    #[test]
    fn test_username_is_not_empty() {
        assert!(!username().is_empty());
    }

    #[test]
    fn test_hostname_is_not_empty() {
        assert!(!hostname().is_empty());
    }

    #[test]
    fn test_os_release_has_pretty_name() {
        let release = os_release();
        assert!(!release.pretty_name.is_empty());
    }

    #[test]
    fn test_total_space_of_temp_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(total_space(dir.path()) > 0);
    }

    #[test]
    fn test_command_exists() {
        // `cargo` доступен, раз выполняются тесты
        assert!(command_exists("cargo"));
        assert!(!command_exists("migration-master-nonexistent-tool"));
    }

    #[test]
    fn test_architecture_is_known() {
        assert!(!architecture().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn test_ensure_private_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("secret");
        std::fs::write(&file, "data").expect("write");
        ensure_private_file(&file).expect("chmod");
        assert_eq!(file_mode(&file).map(|m| m & 0o777), Some(0o600));
    }

    #[cfg(unix)]
    #[test]
    fn test_owner_lookup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (uid, gid) = owner_uid_gid(dir.path());
        assert!(uid.is_some());
        assert!(gid.is_some());
    }
}

