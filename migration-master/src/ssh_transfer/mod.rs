//! Передача данных по SSH (подпроцессы `ssh`, `scp`, `rsync`).
//!
//! Выбрана реализация через системные утилиты: на РЕД ОС они уже установлены,
//! поддерживают все способы аутентификации (ключ, агент, пароль) и не требуют
//! нативных зависимостей (libssh2) в дереве сборки.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::error::{MigrationError, Result};

/// Подключение к удалённому хосту.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SshTarget {
    /// Имя хоста или IP-адрес
    pub host: String,
    /// Пользователь
    pub user: String,
    /// Порт SSH
    pub port: u16,
    /// Файл приватного ключа (None — агент/по умолчанию)
    pub identity_file: Option<PathBuf>,
    /// Файл known_hosts (None — системный)
    pub known_hosts_file: Option<PathBuf>,
}

impl Default for SshTarget {
    fn default() -> Self {
        Self {
            host: String::new(),
            user: String::new(),
            port: 22,
            identity_file: None,
            known_hosts_file: None,
        }
    }
}

impl SshTarget {
    /// Создать подключение.
    pub fn new(host: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            user: user.into(),
            ..Default::default()
        }
    }

    /// Строка `user@host` для scp/rsync.
    pub fn destination(&self) -> String {
        if self.user.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }

    /// Общие опции ssh (порт, ключ, known_hosts, неинтерактивность).
    fn ssh_base_args(&self) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(),
            self.port.to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
        ];

        if let Some(identity) = &self.identity_file {
            args.push("-i".to_string());
            args.push(identity.to_string_lossy().to_string());
        }
        if let Some(known_hosts) = &self.known_hosts_file {
            args.push("-o".to_string());
            args.push(format!("UserKnownHostsFile={}", known_hosts.display()));
        }

        args
    }
}

/// Опции передачи.
#[derive(Debug, Clone)]
pub struct SshTransferOptions {
    /// Режим без реальных изменений
    pub dry_run: bool,
    /// Сжатие при передаче
    pub compress: bool,
    /// Ограничение скорости (КБ/с), 0 — без ограничения
    pub rate_limit_kbps: u64,
    /// Удалять на приёмнике файлы, отсутствующие в источнике (rsync --delete)
    pub delete_extra: bool,
}

impl Default for SshTransferOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            compress: true,
            rate_limit_kbps: 0,
            delete_extra: false,
        }
    }
}

/// Результат передачи.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SshTransferResult {
    /// Оценочное количество файлов
    pub files: u64,
    /// Оценочное количество байт
    pub bytes: u64,
    /// Длительность в миллисекундах
    pub duration_ms: u128,
    /// Стандартный поток ошибок
    pub stderr: String,
}

/// Передача данных по SSH.
pub struct SshTransfer {
    target: SshTarget,
}

impl SshTransfer {
    /// Создать передачу для указанного подключения.
    pub fn new(target: SshTarget) -> Self {
        Self { target }
    }

    /// Подключение.
    pub fn target(&self) -> &SshTarget {
        &self.target
    }

    /// Проверить доступность хоста (`ssh ... true`).
    pub fn check_connection(&self) -> Result<()> {
        let output = self.run_ssh(&["true"])?;
        if !output.status.success() {
            return Err(MigrationError::Ssh(format!(
                "хост {} недоступен: {}",
                self.target.host,
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Выполнить удалённую команду и вернуть её stdout.
    pub fn run_remote(&self, command: &str) -> Result<String> {
        let output = self.run_ssh(&[command])?;
        if !output.status.success() {
            return Err(MigrationError::Ssh(format!(
                "команда завершилась с ошибкой: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Отпечаток ключа хоста (`ssh-keyscan`).
    pub fn host_fingerprint(&self) -> Result<Option<String>> {
        let output = Command::new("ssh-keyscan")
            .args(["-p", &self.target.port.to_string(), &self.target.host])
            .stdin(Stdio::null())
            .output()
            .map_err(|_| MigrationError::missing_tool("ssh-keyscan"))?;

        if !output.status.success() {
            return Ok(None);
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|line| !line.starts_with('#'))
            .map(|line| line.to_string()))
    }

    fn run_ssh(&self, remote_args: &[&str]) -> Result<Output> {
        let mut command = Command::new("ssh");
        command.args(self.target.ssh_base_args());
        command.arg(self.target.destination());
        command.args(remote_args);
        command.stdin(Stdio::null());

        command
            .output()
            .map_err(|_| MigrationError::missing_tool("ssh"))
    }

    fn scp_base_args(&self, compress: bool) -> Vec<String> {
        let mut args = vec!["-P".to_string(), self.target.port.to_string()];
        if compress {
            args.push("-C".to_string());
        }
        if let Some(identity) = &self.target.identity_file {
            args.push("-i".to_string());
            args.push(identity.to_string_lossy().to_string());
        }
        args.push("-o".to_string());
        args.push("BatchMode=yes".to_string());
        args.push("-o".to_string());
        args.push("StrictHostKeyChecking=accept-new".to_string());
        args
    }

    fn ssh_transport_string(&self) -> String {
        let mut transport = format!("ssh -p {}", self.target.port);
        if let Some(identity) = &self.target.identity_file {
            transport.push_str(&format!(" -i {}", identity.display()));
        }
        transport.push_str(" -o BatchMode=yes");
        transport
    }

    fn finish(&self, mut command: Command) -> Result<Output> {
        command
            .stdin(Stdio::null())
            .output()
            .map_err(|_| MigrationError::missing_tool("ssh"))
    }
}

impl SshTransfer {
    /// Оценить объём локального пути (для отчёта).
    fn estimate_local(path: &Path) -> (u64, u64) {
        let mut files = 0u64;
        let mut bytes = 0u64;

        for entry in walkdir::WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                files += 1;
                bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }

        (files, bytes)
    }

    /// Отправить локальный путь на удалённый хост (rsync, иначе scp).
    pub fn push(
        &self,
        local: &Path,
        remote: &str,
        options: &SshTransferOptions,
    ) -> Result<SshTransferResult> {
        let started = std::time::Instant::now();
        let (files, bytes) = Self::estimate_local(local);

        let mut result = SshTransferResult {
            files,
            bytes,
            ..Default::default()
        };

        if options.dry_run {
            return Ok(result);
        }

        let use_rsync = which_available("rsync");
        let output = if use_rsync {
            let mut command = Command::new("rsync");
            command.arg("-a");
            if options.compress {
                command.arg("-z");
            }
            if options.delete_extra {
                command.arg("--delete");
            }
            if options.rate_limit_kbps > 0 {
                command.arg(format!("--bwlimit={}", options.rate_limit_kbps));
            }
            command.arg("-e");
            command.arg(self.ssh_transport_string());
            command.arg(local);
            command.arg(format!("{}:{}", self.target.destination(), remote));
            self.finish(command)?
        } else {
            let mut command = Command::new("scp");
            command.args(self.scp_base_args(options.compress));
            command.arg("-r");
            command.arg(local);
            command.arg(format!("{}:{}", self.target.destination(), remote));
            self.finish(command)?
        };

        result.duration_ms = started.elapsed().as_millis();
        result.stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            return Err(MigrationError::Ssh(format!(
                "передача {} не удалась: {}",
                local.display(),
                result.stderr.trim()
            )));
        }

        Ok(result)
    }

    /// Получить удалённый путь в локальный каталог.
    pub fn pull(
        &self,
        remote: &str,
        local: &Path,
        options: &SshTransferOptions,
    ) -> Result<SshTransferResult> {
        let started = std::time::Instant::now();

        if options.dry_run {
            return Ok(SshTransferResult::default());
        }

        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut command = Command::new("scp");
        command.args(self.scp_base_args(options.compress));
        command.arg("-r");
        command.arg(format!("{}:{}", self.target.destination(), remote));
        command.arg(local);

        let output = self.finish(command)?;
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let duration_ms = started.elapsed().as_millis();

        if !output.status.success() {
            return Err(MigrationError::Ssh(format!(
                "получение {} не удалось: {}",
                remote,
                stderr.trim()
            )));
        }

        Ok(SshTransferResult {
            stderr,
            duration_ms,
            ..Default::default()
        })
    }
}

/// Доступна ли утилита в PATH.
fn which_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_destination_formatting() {
        let mut target = SshTarget::new("10.0.0.1", "alice");
        assert_eq!(target.destination(), "alice@10.0.0.1");

        target.user = String::new();
        assert_eq!(target.destination(), "10.0.0.1");
    }

    #[test]
    fn test_ssh_base_args_include_port_and_options() {
        let mut target = SshTarget::new("host.example", "bob");
        target.port = 2222;
        let args = target.ssh_base_args();
        assert!(args.contains(&"2222".to_string()));
        assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
        assert!(!args.contains(&"-i".to_string()));
    }

    #[test]
    fn test_push_dry_run_never_touches_network() {
        let dir = tempdir().expect("tempdir");
        let local = dir.path().join("data");
        std::fs::create_dir_all(&local).expect("mkdir");
        std::fs::write(local.join("a.txt"), b"12345").expect("write");

        let transfer = SshTransfer::new(SshTarget::new("192.0.2.1", "test"));
        let options = SshTransferOptions {
            dry_run: true,
            ..Default::default()
        };

        let result = transfer
            .push(&local, "/remote/data", &options)
            .expect("dry push");
        assert_eq!(result.files, 1);
        assert_eq!(result.bytes, 5);

        let result = transfer
            .pull("/remote/x", &dir.path().join("in"), &options)
            .expect("dry pull");
        assert_eq!(result.bytes, 0);
    }

    #[test]
    fn test_ssh_target_serde_round_trip() {
        let target = SshTarget::new("host", "user");
        let json = serde_json::to_string(&target).expect("serialize");
        let back: SshTarget = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.host, "host");
        assert_eq!(back.user, "user");
        assert_eq!(back.port, 22);
    }
}
