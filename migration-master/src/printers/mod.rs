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
    #[serde(default)]
    pub model: String,
    /// Адрес устройства
    #[serde(default)]
    pub device_uri: String,
    /// Общедоступный ли принтер
    #[serde(default)]
    pub shared: bool,
    /// Принтер по умолчанию
    #[serde(default)]
    pub default: bool,
}

/// Работа с принтерами системы.
pub struct PrinterManager;

impl PrinterManager {
    /// Список принтеров через `lpstat -p -d` и `lpstat -v`.
    ///
    /// Если CUPS не установлен, возвращается пустой список: это не ошибка,
    /// компонент просто не даст данных. URI нужен для восстановления через
    /// `lpadmin`.
    pub fn list() -> Result<Vec<PrinterEntry>> {
        let printers = Command::new("lpstat")
            .args(["-p", "-d"])
            .stdin(Stdio::null())
            .output();
        let devices = Command::new("lpstat")
            .arg("-v")
            .stdin(Stdio::null())
            .output();

        let (Ok(printers), Ok(devices)) = (printers, devices) else {
            return Ok(Vec::new());
        };
        if !printers.status.success() && !devices.status.success() {
            return Ok(Vec::new());
        }

        Ok(parse_lpstat_with_uris(
            &String::from_utf8_lossy(&printers.stdout),
            &String::from_utf8_lossy(&devices.stdout),
        ))
    }

    /// Строка для безопасной передачи привилегированному помощнику.
    pub fn helper_spec(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}",
            self.name,
            self.device_uri,
            self.model,
            if self.default { "1" } else { "0" }
        )
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
                .unwrap_or_else(|_| {
                    PathBuf::from(source.to_string_lossy().trim_start_matches('/'))
                });
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
        let mut commands = Vec::new();
        for printer in entries {
            if printer.name.trim().is_empty() || printer.device_uri.trim().is_empty() {
                continue;
            }
            let mut command = vec![
                "lpadmin".to_string(),
                "-p".to_string(),
                printer.name.clone(),
                "-v".to_string(),
                printer.device_uri.clone(),
                "-E".to_string(),
            ];
            if !printer.model.trim().is_empty() {
                command.push("-m".to_string());
                command.push(printer.model.clone());
            }
            commands.push(command);
            commands.push(vec!["cupsenable".to_string(), printer.name.clone()]);
            commands.push(vec!["cupsaccept".to_string(), printer.name.clone()]);
            if printer.default {
                commands.push(vec![
                    "lpadmin".to_string(),
                    "-d".to_string(),
                    printer.name.clone(),
                ]);
            }
        }
        commands
    }
}

/// Разобрать вывод `lpstat -p -d`.
pub fn parse_lpstat(text: &str) -> Vec<PrinterEntry> {
    parse_lpstat_with_uris(text, "")
}

/// Разобрать `lpstat -p -d` вместе с `lpstat -v`.
pub fn parse_lpstat_with_uris(text: &str, devices: &str) -> Vec<PrinterEntry> {
    let default_printer = text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("system default destination:")
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    });
    let uris: std::collections::HashMap<String, String> = devices
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("device for ")?;
            let (name, uri) = rest.split_once(':')?;
            Some((name.trim().to_string(), uri.trim().to_string()))
        })
        .collect();

    let mut entries: Vec<PrinterEntry> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("printer ") {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let disabled = line.contains("disabled");
            if let Some(existing) = entries.iter_mut().find(|entry| entry.name == name) {
                existing.shared |= !disabled;
                continue;
            }
            entries.push(PrinterEntry {
                default: default_printer.as_deref() == Some(name.as_str()),
                device_uri: uris.get(&name).cloned().unwrap_or_default(),
                name,
                model: String::new(),
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
            default: true,
        }];
        let commands = PrinterManager::install_commands(&entries);
        assert_eq!(commands.len(), 4);
        assert_eq!(commands[0][0], "lpadmin");
        assert!(commands[0].contains(&"HP".to_string()));
        assert!(commands
            .iter()
            .any(|command| command[0] == "lpadmin" && command.contains(&"-d".to_string())));
    }

    #[test]
    fn test_parse_lpstat_with_uris_and_default() {
        let text = "system default destination: HP\nprinter HP is idle\nprinter EPSON disabled since Mon\n";
        let devices =
            "device for HP: ipp://printer.local/ipp/print\ndevice for EPSON: usb://EPSON/L3250\n";
        let entries = parse_lpstat_with_uris(text, devices);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].default);
        assert_eq!(entries[0].device_uri, "ipp://printer.local/ipp/print");
        assert!(!entries[1].shared);
    }
}
