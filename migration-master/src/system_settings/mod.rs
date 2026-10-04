//! Системные настройки пользователя: сбор и восстановление.

use std::path::{Path, PathBuf};

use crate::error::Result;

/// Известные файлы и каталоги настроек.
pub struct SystemSettings;

impl SystemSettings {
    /// Относительные пути настроек, которые переносятся с профилем.
    pub fn profile_paths() -> Vec<&'static str> {
        vec![
            ".config/gtk-3.0",
            ".config/gtk-4.0",
            ".config/kdeglobals",
            ".config/plasmarc",
            ".config/autostart",
            ".config/systemd/user",
            ".dconf/user",
            ".local/share/backgrounds",
            ".config/Mimeapps.list",
        ]
    }

    /// Пути, существующие в указанном домашнем каталоге.
    pub fn present_paths(home: &Path) -> Vec<PathBuf> {
        Self::profile_paths()
            .iter()
            .map(|relative| home.join(relative))
            .filter(|path| path.exists())
            .collect()
    }

    /// Общий размер путей настроек.
    pub fn total_size(home: &Path) -> u64 {
        Self::present_paths(home)
            .iter()
            .map(|path| size_of(path))
            .sum()
    }

    /// Собрать настройки в каталог назначения (сохраняя структуру профиля).
    pub fn collect(home: &Path, destination: &Path) -> Result<usize> {
        let mut copied = 0;

        for path in Self::present_paths(home) {
            let relative = path
                .strip_prefix(home)
                .map_err(|error| crate::error::MigrationError::InvalidInput(error.to_string()))?;
            let target = destination.join(relative);

            if path.is_dir() {
                copy_dir(&path, &target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&path, &target)?;
            }
            copied += 1;
        }

        Ok(copied)
    }

    /// Команды применения dconf-дампа (выполняются на целевой системе).
    pub fn dconf_restore_command(dump_path: &Path) -> Vec<String> {
        vec![
            "dconf".to_string(),
            "load".to_string(),
            "/".to_string(),
            dump_path.to_string_lossy().to_string(),
        ]
    }

    /// Выгрузить настройки dconf в файл.
    pub fn dump_dconf(destination: &Path) -> Result<()> {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let output = std::process::Command::new("dconf")
            .args(["dump", "/"])
            .stdin(std::process::Stdio::null())
            .output();

        match output {
            Ok(output) if output.status.success() => {
                std::fs::write(destination, &output.stdout)?;
            }
            // dconf отсутствует — это не ошибка, настройки просто не переносятся
            _ => {}
        }

        Ok(())
    }
}

fn size_of(path: &Path) -> u64 {
    if path.is_file() {
        return std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }

    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            total += size_of(&entry.path());
        }
    }
    total
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination)?;

    for entry in std::fs::read_dir(source)?.flatten() {
        let path = entry.path();
        let target = destination.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_present_paths_filters_missing() {
        let dir = tempdir().expect("tempdir");
        assert!(SystemSettings::present_paths(dir.path()).is_empty());

        std::fs::create_dir_all(dir.path().join(".config/gtk-3.0")).expect("mkdir");
        let present = SystemSettings::present_paths(dir.path());
        assert_eq!(present.len(), 1);
        assert!(present[0].ends_with(".config/gtk-3.0"));
    }

    #[test]
    fn test_collect_and_size() {
        let dir = tempdir().expect("tempdir");
        let gtk = dir.path().join(".config/gtk-3.0");
        std::fs::create_dir_all(&gtk).expect("mkdir");
        std::fs::write(gtk.join("settings.ini"), b"[Settings]\n").expect("write");

        assert!(SystemSettings::total_size(dir.path()) > 0);

        let dest = dir.path().join("out");
        let copied = SystemSettings::collect(dir.path(), &dest).expect("collect");
        assert_eq!(copied, 1);
        assert!(dest.join(".config/gtk-3.0/settings.ini").exists());
    }

    #[test]
    fn test_dconf_command_shape() {
        let command = SystemSettings::dconf_restore_command(Path::new("/tmp/dump"));
        assert_eq!(command[0], "dconf");
        assert_eq!(command[1], "load");
    }
}
