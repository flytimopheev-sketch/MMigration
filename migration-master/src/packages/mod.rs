//! Списки установленных пакетов: dpkg/rpm через подпроцессы.

use std::process::{Command, Stdio};

use crate::error::{MigrationError, Result};

/// Пакетный менеджер системы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageManager {
    /// Debian/Ubuntu (dpkg)
    Dpkg,
    /// РЕД ОС / RHEL (rpm)
    Rpm,
}

/// Запись об установленном пакете.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackageEntry {
    /// Имя пакета
    pub name: String,
    /// Версия
    pub version: String,
}

impl PackageManager {
    /// Определить пакетный менеджер по наличию утилиты.
    pub fn detect() -> Self {
        let dpkg = Command::new("dpkg-query")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);

        if dpkg {
            Self::Dpkg
        } else {
            Self::Rpm
        }
    }

    /// Имя утилиты списка пакетов.
    pub fn list_tool(&self) -> &'static str {
        match self {
            Self::Dpkg => "dpkg-query",
            Self::Rpm => "rpm",
        }
    }

    /// Список установленных пакетов.
    pub fn list_installed(&self) -> Result<Vec<PackageEntry>> {
        let output = match self {
            Self::Dpkg => Command::new("dpkg-query")
                .args(["-W", "-f=${Package}\\t${Version}\\n"])
                .stdin(Stdio::null())
                .output(),
            Self::Rpm => Command::new("rpm")
                .args(["-qa", "--qf", "%{NAME}\\t%{VERSION}-%{RELEASE}\\n"])
                .stdin(Stdio::null())
                .output(),
        }
        .map_err(|_| MigrationError::missing_tool(self.list_tool()))?;

        if !output.status.success() {
            return Err(MigrationError::Command {
                command: self.list_tool().to_string(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        let text = String::from_utf8_lossy(&output.stdout);
        Ok(parse_package_list(&text))
    }

    /// Список явно установленных пользователем пакетов.
    ///
    /// Для РЕД ОС используется `dnf repoquery --userinstalled`, с откатом на
    /// `dnf history userinstalled`. Список `rpm -qa` намеренно не применяется:
    /// он содержит ядра, драйверы и системные пакеты, установка которых на
    /// другую машину опасна.
    pub fn list_user_installed(&self) -> Result<Vec<PackageEntry>> {
        match self {
            Self::Dpkg => {
                let manual = Command::new("apt-mark")
                    .arg("showmanual")
                    .stdin(Stdio::null())
                    .output()
                    .map_err(|_| MigrationError::missing_tool("apt-mark"))?;
                if !manual.status.success() {
                    return Err(MigrationError::Command {
                        command: "apt-mark".to_string(),
                        message: String::from_utf8_lossy(&manual.stderr).trim().to_string(),
                    });
                }
                let wanted: std::collections::HashSet<String> =
                    String::from_utf8_lossy(&manual.stdout)
                        .lines()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .collect();
                Ok(self
                    .list_installed()?
                    .into_iter()
                    .filter(|entry| wanted.contains(&entry.name))
                    .collect())
            }
            Self::Rpm => {
                let output = Command::new("dnf")
                    .args([
                        "-q",
                        "repoquery",
                        "--userinstalled",
                        "--qf",
                        "%{name}\\t%{version}-%{release}\\n",
                    ])
                    .stdin(Stdio::null())
                    .output();
                if let Ok(output) = output {
                    if output.status.success() {
                        let entries = parse_package_list(&String::from_utf8_lossy(&output.stdout));
                        if !entries.is_empty() {
                            return Ok(entries);
                        }
                    }
                }

                // Резервный путь для старых dnf: только имена без версий.
                let history = Command::new("dnf")
                    .arg("history")
                    .arg("userinstalled")
                    .stdin(Stdio::null())
                    .output()
                    .map_err(|_| MigrationError::missing_tool("dnf"))?;
                if !history.status.success() {
                    return Err(MigrationError::Command {
                        command: "dnf".to_string(),
                        message: String::from_utf8_lossy(&history.stderr).trim().to_string(),
                    });
                }
                Ok(parse_userinstalled_history(&String::from_utf8_lossy(
                    &history.stdout,
                )))
            }
        }
    }

    /// Экспорт списка пакетов (по одному в строке: имя<TAB>версия).
    pub fn export_list(&self) -> Result<String> {
        let entries = self.list_installed()?;
        Ok(export_entries(&entries))
    }

    /// Экспорт только пользовательских пакетов.
    pub fn export_user_installed(&self) -> Result<String> {
        Ok(export_entries(&self.list_user_installed()?))
    }

    /// Пакеты из списка источника, которых нет в текущей системе.
    pub fn missing_from_system(&self, wanted: &[PackageEntry]) -> Result<Vec<PackageEntry>> {
        let installed: std::collections::HashSet<String> = self
            .list_installed()?
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        Ok(wanted
            .iter()
            .filter(|entry| !installed.contains(&entry.name))
            .cloned()
            .collect())
    }

    /// Команда установки пакетов (для выполнения на целевой системе).
    pub fn install_command(&self, packages: &[String]) -> Vec<String> {
        match self {
            Self::Dpkg => {
                let mut args = vec!["install".to_string(), "-y".to_string()];
                args.extend(packages.iter().cloned());
                let mut command = vec!["apt-get".to_string()];
                command.extend(args);
                command
            }
            Self::Rpm => {
                let mut args = vec!["install".to_string(), "-y".to_string()];
                args.extend(packages.iter().cloned());
                let mut command = vec!["dnf".to_string()];
                command.extend(args);
                command
            }
        }
    }
}

fn export_entries(entries: &[PackageEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("{}\t{}", entry.name, entry.version))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Разобрать `dnf history userinstalled`.
///
/// В разных версиях dnf вывод содержит либо просто имена, либо заголовок и
/// строки вида `package : name`. Поддерживаются оба варианта.
pub fn parse_userinstalled_history(text: &str) -> Vec<PackageEntry> {
    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with("Last metadata")
            || line.starts_with("Command line")
            || line.starts_with("Available")
            || line.starts_with("Installed")
        {
            continue;
        }
        let name = match line.split_once(':') {
            Some((left, right))
                if left.trim().eq_ignore_ascii_case("package")
                    || left.trim().eq_ignore_ascii_case("name") =>
            {
                right.trim()
            }
            Some((left, _)) => left.trim(),
            None => line,
        }
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim();
        // Пропускаем строки-заголовки и явно не пакетные значения.
        if name.is_empty()
            || !name.chars().any(|c| c.is_ascii_alphabetic())
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.' | ':'))
        {
            continue;
        }
        if seen.insert(name.to_string()) {
            entries.push(PackageEntry {
                name: name.to_string(),
                version: String::new(),
            });
        }
    }
    entries
}

/// Разобрать вывод `dpkg-query`/`rpm -qa` (строки `имя<TAB>версия`).
pub fn parse_package_list(text: &str) -> Vec<PackageEntry> {
    text.lines()
        .filter_map(|line| {
            let (name, version) = line.split_once('\t')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(PackageEntry {
                name: name.to_string(),
                version: version.trim().to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package_list() {
        let text = "bash\t5.1.16-1\nglibc\t2.36-9\n\nнет-табуляции\n";
        let entries = parse_package_list(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            PackageEntry {
                name: "bash".into(),
                version: "5.1.16-1".into()
            }
        );
        assert_eq!(entries[1].name, "glibc");
    }

    #[test]
    fn test_install_command_shapes() {
        let packages = vec!["htop".to_string()];
        let dpg = PackageManager::Dpkg.install_command(&packages);
        assert_eq!(dpg[0], "apt-get");
        assert!(dpg.contains(&"install".to_string()));

        let rpm = PackageManager::Rpm.install_command(&packages);
        assert_eq!(rpm[0], "dnf");
        assert!(rpm.contains(&"install".to_string()));
        assert!(rpm.contains(&"htop".to_string()));
    }

    #[test]
    fn test_parse_userinstalled_history() {
        let text = "\
Last metadata expiration check: 0:01:00 ago.
htop
vim-enhanced
htop
package : curl
";
        let entries = parse_userinstalled_history(text);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["htop", "vim-enhanced", "curl"]);
    }
}
