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

        if let Some(dump) = Self::dump_dconf_to_string()? {
            std::fs::write(destination, dump)?;
        }
        Ok(())
    }

    /// Выгрузить dconf в строку (для манифеста архива).
    ///
    /// `Ok(None)` означает, что `dconf` недоступен: это не ошибка, настройки
    /// просто не переносятся.
    pub fn dump_dconf_to_string() -> Result<Option<String>> {
        let output = match std::process::Command::new("dconf")
            .args(["dump", "/"])
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(output) => output,
            Err(_) => return Ok(None),
        };

        if output.status.success() {
            Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
        } else {
            Ok(None)
        }
    }

    /// Применить dconf-дамп к текущей пользовательской сессии.
    ///
    /// Это пользовательская операция, root не требуется. Пустой дамп
    /// пропускается, чтобы не сбрасывать настройки.
    pub fn apply_dconf_dump(dump: &str) -> Result<()> {
        use std::io::Write;

        if dump.trim().is_empty() {
            return Ok(());
        }

        let mut child = std::process::Command::new("dconf")
            .args(["load", "/"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|_| crate::error::MigrationError::missing_tool("dconf"))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(dump.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(crate::error::MigrationError::Command {
                command: "dconf".to_string(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    /// Обновить кэш шрифтов после восстановления (`fc-cache -f`).
    pub fn refresh_font_cache() -> Result<()> {
        run_tool("fc-cache", &["-f"])
    }

    /// Обновить базу `.desktop`-файлов пользователя.
    pub fn refresh_desktop_database(home: &Path) -> Result<()> {
        let applications = home.join(".local/share/applications");
        if applications.is_dir() {
            let path = applications.to_string_lossy().to_string();
            run_tool("update-desktop-database", &[path.as_str()])?;
        }
        Ok(())
    }

    /// Перечитать пользовательские systemd-юниты.
    pub fn reload_user_systemd() -> Result<()> {
        run_tool("systemctl", &["--user", "daemon-reload"])
    }

    /// Список включённых пользовательских юнитов (для отчёта).
    pub fn enabled_user_units() -> Vec<String> {
        let output = std::process::Command::new("systemctl")
            .args([
                "--user",
                "list-unit-files",
                "--state=enabled",
                "--no-legend",
            ])
            .stdin(std::process::Stdio::null())
            .output();
        match output {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }
}

fn run_tool(tool: &str, args: &[&str]) -> Result<()> {
    let output = std::process::Command::new(tool)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| crate::error::MigrationError::missing_tool(tool))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(crate::error::MigrationError::Command {
            command: tool.to_string(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
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
