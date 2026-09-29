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

    /// Экспорт списка пакетов (по одному в строке: имя<TAB>версия).
    pub fn export_list(&self) -> Result<String> {
        let entries = self.list_installed()?;
        Ok(entries
            .iter()
            .map(|entry| format!("{}\t{}", entry.name, entry.version))
            .collect::<Vec<_>>()
            .join("\n"))
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
                let mut args = vec!["-y".to_string(), "-i".to_string()];
                args.extend(packages.iter().cloned());
                let mut command = vec!["dnf".to_string()];
                command.extend(args);
                command
            }
        }
    }
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
        assert_eq!(entries[0], PackageEntry { name: "bash".into(), version: "5.1.16-1".into() });
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
        assert!(rpm.contains(&"htop".to_string()));
    }
}

