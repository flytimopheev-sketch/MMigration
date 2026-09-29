//! Принтеры: сбор и восстановление конфигурации CUPS.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{MigrationError, Result};

/// Описание принтера.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrinterEntry {
    /// Имя принтера
    pub name: String,
    /// Модель/драйвер
    pub model: String,
    /// Адрес устройства
    pub device_uri: String,
    /// Общедоступный ли принтер
    pub shared: bool,
}

/// Работа с принтерами системы.
pub struct PrinterManager;

impl PrinterManager {
    /// Список принтеров через `lpstat -p -d` (пусто, если CUPS не установлен).
    pub fn list() -> Result<Vec<PrinterEntry>> {
        let output = Command::new("lpstat")
            .args(["-p", "-d"])
            .stdin(Stdio::null())
            .output()
            .map_err(|_| MigrationError::missing_tool("lpstat"))?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        let text = String::from_utf8_lossy(&output.stdout);
        Ok(parse_lpstat(&text))
    }

    /// Пути файлов конфигурации CUPS в профиле/системе.
    pub fn config_files(home: &Path) -> Vec<PathBuf> {
        vec![
            home.join(".cups/printers.conf"),
            home.join(".cups/ppd"),
            PathBuf::from("/etc/cups/printers.conf"),
            PathBuf::from("/etc/cups/ppd"),
        ]
    }

    /// Собрать доступные файлы конфигурации в каталог назначения.
    pub fn collect(home: &Path, destination: &Path) -> Result<usize> {
        let mut copied = 0;

        for source in Self::config_files(home) {
            if !source.exists() {
                continue;
            }

            let relative = source
                .strip_prefix(home)
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|_| PathBuf::from(source.to_string_lossy().trim_start_matches('/')));
            let target = destination.join(&relative);

            if source.is_dir() {
                copy_dir(&source, &target)?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&source, &target)?;
            }
            copied += 1;
        }

        Ok(copied)
    }

    /// Установить список принтеров из записей (генерация команд `lpadmin`).
    pub fn install_commands(entries: &[PrinterEntry]) -> Vec<Vec<String>> {
        entries
            .iter()
            .map(|printer| {
                vec![
                    "lpadmin".to_string(),
                    "-p".to_string(),
                    printer.name.clone(),
                    "-v".to_string(),
                    printer.device_uri.clone(),
                    "-E".to_string(),
                ]
            })
            .collect()
    }
}

/// Разобрать вывод `lpstat -p -d`.
pub fn parse_lpstat(text: &str) -> Vec<PrinterEntry> {
    let mut entries = Vec::new();

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("printer ") {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let disabled = line.contains("disabled");
            entries.push(PrinterEntry {
                name,
                model: String::new(),
                device_uri: String::new(),
                shared: !disabled,
            });
        }
    }

    entries
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
    fn test_parse_lpstat() {
        let text = "printer HP_LaserJet is idle\nprinter EPSON disabled since Mon\n";
        let entries = parse_lpstat(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "HP_LaserJet");
        assert!(entries[0].shared);
        assert!(!entries[1].shared);
    }

    #[test]
    fn test_collect_user_cups_config() {
        let dir = tempdir().expect("tempdir");
        let cups = dir.path().join(".cups");
        std::fs::create_dir_all(&cups).expect("mkdir");
        std::fs::write(cups.join("printers.conf"), "# conf\n").expect("write");

        let dest = dir.path().join("out");
        let copied = PrinterManager::collect(dir.path(), &dest).expect("collect");
        assert_eq!(copied, 1);
        assert!(dest.join(".cups/printers.conf").exists());
    }

    #[test]
    fn test_install_commands() {
        let entries = vec![PrinterEntry {
            name: "HP".into(),
            model: "HP LaserJet".into(),
            device_uri: "ipp://printer".into(),
            shared: true,
        }];
        let commands = PrinterManager::install_commands(&entries);
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0][0], "lpadmin");
        assert!(commands[0].contains(&"HP".to_string()));
    }
}

