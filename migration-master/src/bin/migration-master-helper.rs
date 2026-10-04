//! Привилегированный помощник `migration-master-helper`.
//!
//! Выполняет строго ограниченный набор операций от имени root:
//! установка пакетов, копирование системных конфигураций, установка
//! прав и файлов политик. Каждый аргумент валидируется до запуска команд.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use clap::{Parser, Subcommand};

/// Интерфейс помощника.
#[derive(Debug, Parser)]
#[command(
    name = "migration-master-helper",
    version,
    about = "Привилегированный помощник Migration Master (корневые операции по белому списку)"
)]
struct HelperCli {
    #[command(subcommand)]
    command: HelperCommand,
}

/// Разрешённые операции.
#[derive(Debug, Subcommand)]
enum HelperCommand {
    /// Установить пакеты (apt-get/dnf install -y)
    InstallPackages { packages: Vec<String> },
    /// Установить файл политики PolicyKit
    InstallPolicy { path: PathBuf },
    /// Скопировать системный файл или каталог (белый список источников)
    CopySystem {
        source: PathBuf,
        destination: PathBuf,
    },
    /// Установить права (только для системных путей)
    SetMode { path: PathBuf, mode: u32 },
}

/// Префиксы каталогов, из которых разрешено копирование.
const ALLOWED_SYSTEM_PREFIXES: &[&str] = &["/etc/cups", "/etc/skel", "/usr/share/migration-master"];

/// Префиксы путей, для которых разрешено менять права.
const ALLOWED_MODE_PREFIXES: &[&str] = &["/etc/cups", "/etc/skel", "/home"];

fn main() {
    let cli = HelperCli::parse();

    if let Err(error) = run(cli.command) {
        eprintln!("helper: {}", error);
        std::process::exit(1);
    }
}

fn run(command: HelperCommand) -> Result<(), String> {
    // Все операции, кроме чтения версии, требуют root
    if migration_master::platform::current_uid() != 0 {
        return Err("операция требует прав root (запустите через pkexec)".to_string());
    }

    match command {
        HelperCommand::InstallPackages { packages } => {
            for package in &packages {
                validate_package(package)?;
            }

            let manager = migration_master::packages::PackageManager::detect();
            let mut command = match manager {
                migration_master::packages::PackageManager::Dpkg => {
                    let mut command = Command::new("apt-get");
                    command.args(["install", "-y"]);
                    command
                }
                migration_master::packages::PackageManager::Rpm => {
                    let mut command = Command::new("dnf");
                    command.args(["install", "-y"]);
                    command
                }
            };

            command.args(&packages);
            exec(command)
        }
        HelperCommand::InstallPolicy { path } => {
            if !path.is_file() {
                return Err(format!("файл не найден: {}", path.display()));
            }
            let target = "/usr/share/polkit-1/actions/com.redos.migration-master.policy";
            let mut command = Command::new("install");
            command.args(["-m", "644"]);
            command.arg(&path);
            command.arg(target);
            exec(command)
        }
        HelperCommand::CopySystem {
            source,
            destination,
        } => {
            require_prefix(&source, ALLOWED_SYSTEM_PREFIXES, "источник")?;
            require_prefix(&destination, ALLOWED_SYSTEM_PREFIXES, "назначение")?;
            copy_path(&source, &destination).map_err(|error| error.to_string())
        }
        HelperCommand::SetMode { path, mode } => {
            require_prefix(&path, ALLOWED_MODE_PREFIXES, "путь")?;
            if mode & 0o7777 != mode {
                return Err(format!("недопустимые права: {:o}", mode));
            }
            #[cfg(unix)]
            {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                    .map_err(|error| error.to_string())
            }
            #[cfg(not(unix))]
            {
                let _ = (path, mode);
                Ok(())
            }
        }
    }
}

fn exec(mut command: Command) -> Result<(), String> {
    command.stdin(Stdio::null());
    let status = command
        .status()
        .map_err(|error| format!("не удалось запустить команду: {}", error))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("команда завершилась с кодом {}", status))
    }
}

fn validate_package(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() < 200
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.' | ':'));

    if valid {
        Ok(())
    } else {
        Err(format!("недопустимое имя пакета: {:?}", name))
    }
}

fn require_prefix(path: &Path, prefixes: &[&str], what: &str) -> Result<(), String> {
    let text = path.to_string_lossy();
    if prefixes.iter().any(|prefix| text.starts_with(prefix)) {
        Ok(())
    } else {
        Err(format!("{} вне белого списка: {}", what, path.display()))
    }
}

fn copy_path(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(source, destination)?;
        Ok(())
    }
}
