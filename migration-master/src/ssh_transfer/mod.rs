//! Модуль ssh_transfer - Прямая миграция по SSH

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::io::{Read, Write};
use std::fs;
use serde::{Deserialize, Serialize};
use crate::logging::LogManager;
use crate::config::ComponentType as MigrationComponent;

/// Конфигурация SSH подключения
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: SshAuthMethod,
    pub proxy_jump: Option<String>,
    pub timeout_secs: u32,
    pub compression: bool,
    pub limit_rate: Option<u32>, // бит/сек
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SshAuthMethod {
    Password(String),
    KeyFile(PathBuf),
    Agent,
}

/// Результат проверки SSH подключения
#[derive(Debug)]
pub struct SshConnectionResult {
    pub is_connected: bool,
    pub fingerprint: String,
    pub remote_hostname: String,
    pub remote_os_info: String,
    pub available_disk_space: u64,
    pub error_message: Option<String>,
}

/// Результат анализа удаленной системы
#[derive(Debug)]
pub struct RemoteSystemInfo {
    pub hostname: String,
    pub os_name: String,
    pub os_version: String,
    pub architecture: String,
    pub home_dir: PathBuf,
    pub total_disk_space: u64,
    pub free_disk_space: u64,
    pub available_components: Vec<MigrationComponent>,
}

/// Менеджер SSH передачи
pub struct SshTransferManager {
    config: SshConfig,
    logger: LogManager,
}

impl SshTransferManager {
    /// Создать новый менеджер SSH передачи
    pub fn new(config: SshConfig, logger: LogManager) -> Self {
        Self { config, logger }
    }

    /// Проверить SSH подключение
    pub fn check_connection(&self) -> Result<SshConnectionResult, Box<dyn std::error::Error>> {
        self.logger.info(&format!("Проверка SSH подключения к {}:{}", self.config.host, self.config.port));

        // Проверяем доступность порта
        let addr = format!("{}:{}", self.config.host, self.config.port);
        match TcpStream::connect(&addr) {
            Ok(_) => self.logger.info("Порт SSH доступен"),
            Err(e) => {
                return Ok(SshConnectionResult {
                    is_connected: false,
                    fingerprint: String::new(),
                    remote_hostname: String::new(),
                    remote_os_info: String::new(),
                    available_disk_space: 0,
                    error_message: Some(format!("Не удалось подключиться к порту {}: {}", self.config.port, e)),
                });
            }
        }

        // Получаем fingerprint через ssh-keyscan
        let fingerprint = self.get_host_fingerprint()?;

        // Пробуем подключиться и получить информацию
        let output = self.run_ssh_command("hostname")?;
        let remote_hostname = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Получаем информацию об ОС
        let os_output = self.run_ssh_command("cat /etc/os-release | grep PRETTY_NAME")?;
        let remote_os_info = String::from_utf8_lossy(&os_output.stdout)
            .trim()
            .trim_start_matches("PRETTY_NAME=")
            .trim_matches('"')
            .to_string();

        // Проверяем доступное место
        let df_output = self.run_ssh_command("df -h ~ | tail -1 | awk '{print $4}'")?;
        let free_space_str = String::from_utf8_lossy(&df_output.stdout).trim().to_string();
        let available_disk_space = parse_human_readable_size(&free_space_str).unwrap_or(0);

        self.logger.info(&format!("SSH подключение успешно. Хост: {}, ОС: {}", remote_hostname, remote_os_info));

        Ok(SshConnectionResult {
            is_connected: true,
            fingerprint,
            remote_hostname,
            remote_os_info,
            available_disk_space,
            error_message: None,
        })
    }

    /// Получить fingerprint хоста
    fn get_host_fingerprint(&self) -> Result<String, Box<dyn std::error::Error>> {
        let mut cmd = Command::new("ssh-keyscan");
        cmd.args(&[
            "-p", &self.config.port.to_string(),
            "-t", "ed25519,rsa",
            &self.config.host,
        ]);
        cmd.stderr(Stdio::null());

        let output = cmd.output()?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Берем первый ключ
            if let Some(line) = stdout.lines().next() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    let key_type = parts[1];
                    let key_hash = self.hash_fingerprint(parts[2])?;
                    return Ok(format!("{}:{}", key_type, key_hash));
                }
            }
        }

        Ok("unknown".to_string())
    }

    /// Хешировать fingerprint для отображения
    fn hash_fingerprint(&self, key: &str) -> Result<String, Box<dyn std::error::Error>> {
        use sha2::{Sha256, Digest};
        let decoded = base64::decode(key)?;
        let hash = Sha256::digest(&decoded);
        Ok(hex::encode(&hash[..16])) // Первые 16 байт как у OpenSSH
    }

    /// Выполнить команду по SSH
    fn run_ssh_command(&self, command: &str) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut args = Vec::new();

        // Порт
        args.push("-p".to_string());
        args.push(self.config.port.to_string());

        // Compression
        if self.config.compression {
            args.push("-C".to_string());
        }

        // ProxyJump
        if let Some(ref proxy) = self.config.proxy_jump {
            args.push("-J".to_string());
            args.push(proxy.clone());
        }

        // Timeout
        args.push("-o".to_string());
        args.push(format!("ConnectTimeout={}", self.config.timeout_secs));

        // StrictHostKeyChecking
        args.push("-o".to_string());
        args.push("StrictHostKeyChecking=ask".to_string());

        // Auth method
        match &self.config.auth_method {
            SshAuthMethod::Password(_) => {
                // Пароль будет передан через sshpass если доступен
                // Или через интерактивный диалог
            }
            SshAuthMethod::KeyFile(path) => {
                args.push("-i".to_string());
                args.push(path.to_string_lossy().to_string());
            }
            SshAuthMethod::Agent => {
                // Используем ssh-agent по умолчанию
            }
        }

        // Host и команда
        let user_host = format!("{}@{}", self.config.username, self.config.host);
        args.push(user_host);
        args.push(command.to_string());

        self.logger.debug(&format!("Выполнение SSH команды: ssh {}", args.join(" ")));

        let mut cmd = Command::new("ssh");
        cmd.args(&args);
        cmd.stdin(Stdio::null());
        cmd.stderr(Stdio::piped());
        cmd.stdout(Stdio::piped());

        // Если используется пароль, пытаемся использовать sshpass
        if let SshAuthMethod::Password(password) = &self.config.auth_method {
            if which::which("sshpass").is_ok() {
                let mut pass_cmd = Command::new("sshpass");
                pass_cmd.arg("-p").arg(password);
                pass_cmd.arg("ssh");
                pass_cmd.args(&args);
                pass_cmd.stdin(Stdio::null());
                pass_cmd.stderr(Stdio::piped());
                pass_cmd.stdout(Stdio::piped());
                return Ok(pass_cmd.output()?);
            }
        }

        Ok(cmd.output()?)
    }

    /// Получить информацию о удаленной системе
    pub fn analyze_remote_system(&self) -> Result<RemoteSystemInfo, Box<dyn std::error::Error>> {
        self.logger.info("Анализ удаленной системы...");

        // Hostname
        let hostname_output = self.run_ssh_command("hostname")?;
        let hostname = String::from_utf8_lossy(&hostname_output.stdout).trim().to_string();

        // OS Info
        let os_release = self.run_ssh_command("cat /etc/os-release")?;
        let os_release_str = String::from_utf8_lossy(&os_release.stdout);
        let os_name = extract_os_field(&os_release_str, "PRETTY_NAME");
        let os_version = extract_os_field(&os_release_str, "VERSION_ID");

        // Architecture
        let arch_output = self.run_ssh_command("uname -m")?;
        let architecture = String::from_utf8_lossy(&arch_output.stdout).trim().to_string();

        // Home directory
        let home_output = self.run_ssh_command("echo $HOME")?;
        let home_dir = PathBuf::from(String::from_utf8_lossy(&home_output.stdout).trim());

        // Disk space
        let df_output = self.run_ssh_command("df -B1 ~ | tail -1 | awk '{print $2, $4}'")?;
        let df_line = String::from_utf8_lossy(&df_output.stdout);
        let mut total_space = 0u64;
        let mut free_space = 0u64;
        
        for part in df_line.trim().split_whitespace() {
            if let Ok(val) = part.parse::<u64>() {
                if total_space == 0 {
                    total_space = val;
                } else {
                    free_space = val;
                    break;
                }
            }
        }

        // Доступные компоненты
        let mut available_components = Vec::new();
        for component in &[
            MigrationComponent::Desktop,
            MigrationComponent::Documents,
            MigrationComponent::Downloads,
            MigrationComponent::Pictures,
            MigrationComponent::Videos,
            MigrationComponent::Music,
            MigrationComponent::Templates,
            MigrationComponent::Config,
            MigrationComponent::SshKeys,
            MigrationComponent::LocalData,
        ] {
            let path = component.get_default_path().unwrap_or_default();
            let check_cmd = format!("[ -e '{}' ] && echo 'exists'", path.display());
            let result = self.run_ssh_command(&check_cmd)?;
            if String::from_utf8_lossy(&result.stdout).contains("exists") {
                available_components.push(component.clone());
            }
        }

        self.logger.info(&format!("Найдено {} компонентов на удаленной системе", available_components.len()));

        Ok(RemoteSystemInfo {
            hostname,
            os_name,
            os_version,
            architecture,
            home_dir,
            total_disk_space: total_space,
            free_disk_space: free_space,
            available_components,
        })
    }

    /// Передать файлы через rsync
    pub fn transfer_files_rsync(
        &self,
        source_path: &Path,
        dest_path: &Path,
        components: &[MigrationComponent],
        dry_run: bool,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        self.logger.info(&format!("Передача файлов через rsync: {:?} -> {:?}", source_path, dest_path));

        let mut args = vec![
            "-avz".to_string(),           // archive, verbose, compress
            "--progress".to_string(),      // показывать прогресс
            "--partial".to_string(),       // сохранять частичные файлы
            "--delete".to_string(),        // удалять лишние файлы на приемнике
        ];

        if dry_run {
            args.push("--dry-run".to_string());
            self.logger.info("Режим Dry-Run активирован");
        }

        // Compression
        if self.config.compression {
            args.push("-C".to_string());
        }

        // Rate limit
        if let Some(rate) = self.config.limit_rate {
            args.push(format!("--bwlimit={}", rate / 1024)); // rsync использует KB/s
        }

        // SSH опции
        let mut ssh_opts = format!(
            "-p {} -o ConnectTimeout={} -o StrictHostKeyChecking=ask",
            self.config.port,
            self.config.timeout_secs
        );

        if let SshAuthMethod::KeyFile(ref path) = self.config.auth_method {
            ssh_opts.push_str(&format!(" -i {}", path.display()));
        }

        args.push(format!("--rsh=ssh {}", ssh_opts));

        // Формируем пути
        let user_host = format!("{}@{}", self.config.username, self.config.host);
        
        // Для каждого компонента создаем правило rsync
        for component in components {
            if let Some(rel_path) = component.get_default_path() {
                let source_full = source_path.join(&rel_path);
                let dest_full = format!("{}:{}", user_host, dest_path.join(&rel_path).display());

                if source_full.exists() {
                    args.push(source_full.to_string_lossy().to_string());
                    args.push(dest_full);
                }
            }
        }

        self.logger.debug(&format!("rsync команда: {}", args.join(" ")));

        let mut cmd = Command::new("rsync");
        cmd.args(&args);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = cmd.output()?;
        
        if !output.status.success() {
            return Err(format!("Ошибка rsync: {}", String::from_utf8_lossy(&output.stderr)).into());
        }

        // Парсим вывод rsync для получения размера переданных данных
        let transferred_size = parse_rsync_output(&String::from_utf8_lossy(&output.stdout));

        self.logger.info(&format!("Передача завершена. Передано {} байт", transferred_size));

        Ok(transferred_size)
    }

    /// Передать файлы через tar + ssh (альтернатива rsync)
    pub fn transfer_files_tar_ssh(
        &self,
        source_path: &Path,
        dest_path: &Path,
        components: &[MigrationComponent],
        dry_run: bool,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        self.logger.info("Передача файлов через tar+ssh...");

        let mut total_size = 0u64;

        for component in components {
            if let Some(rel_path) = component.get_default_path() {
                let source_full = source_path.join(&rel_path);
                
                if !source_full.exists() {
                    continue;
                }

                // Создаем tar архив в памяти и передаем по ssh
                let user_host = format!("{}@{}", self.config.username, self.config.host);
                let dest_full = dest_path.join(&rel_path);

                // Команда для создания tar и отправки по ssh
                let tar_cmd = format!(
                    "tar czf - -C {} {}",
                    source_path.display(),
                    rel_path.display()
                );

                let ssh_mkdir = format!("ssh {} mkdir -p {}", 
                    self.get_ssh_base_args(),
                    dest_full.parent().unwrap_or(dest_path).display()
                );

                if dry_run {
                    self.logger.info(&format!("Dry-run: {}", tar_cmd));
                    continue;
                }

                // Создаем директорию назначения
                Command::new("ssh")
                    .args(self.get_ssh_base_args().split_whitespace().collect::<Vec<_>>())
                    .arg(&user_host)
                    .arg("mkdir")
                    .arg("-p")
                    .arg(dest_full.parent().unwrap_or(dest_path))
                    .output()?;

                // Передаем архив
                let mut tar_process = Command::new("tar")
                    .args(&["czf", "-", "-C"])
                    .arg(source_path)
                    .arg(&rel_path)
                    .stdout(Stdio::piped())
                    .spawn()?;

                let mut ssh_process = Command::new("ssh")
                    .args(self.get_ssh_base_args().split_whitespace().collect::<Vec<_>>())
                    .arg(&user_host)
                    .arg("tar")
                    .args(&["xzf", "-", "-C"])
                    .arg(dest_full.parent().unwrap_or(dest_path))
                    .stdin(Stdio::piped())
                    .spawn()?;

                // Соединяем вывод tar с вводом ssh
                if let Some(tar_stdout) = tar_process.stdout.take() {
                    if let Some(ssh_stdin) = ssh_process.stdin.take() {
                        let mut tar_reader = tar_stdout;
                        let mut ssh_writer = ssh_stdin;
                        
                        std::io::copy(&mut tar_reader, &mut ssh_writer)?;
                    }
                }

                tar_process.wait()?;
                let ssh_result = ssh_process.wait_with_output()?;

                if !ssh_result.status.success() {
                    return Err(format!("Ошибка SSH: {}", String::from_utf8_lossy(&ssh_result.stderr)).into());
                }

                // Вычисляем размер переданных данных
                let metadata = fs::metadata(&source_full)?;
                total_size += if metadata.is_file() {
                    metadata.len()
                } else {
                    get_dir_size(&source_full)?
                };
            }
        }

        self.logger.info(&format!("Передача tar+ssh завершена. Всего {} байт", total_size));

        Ok(total_size)
    }

    /// Получить базовые аргументы SSH
    fn get_ssh_base_args(&self) -> String {
        let mut args = format!(
            "-p {} -o ConnectTimeout={} -o StrictHostKeyChecking=ask",
            self.config.port,
            self.config.timeout_secs
        );

        if self.config.compression {
            args.push_str(" -C");
        }

        if let Some(ref proxy) = self.config.proxy_jump {
            args.push_str(&format!(" -J {}", proxy));
        }

        match &self.config.auth_method {
            SshAuthMethod::KeyFile(path) => {
                args.push_str(&format!(" -i {}", path.display()));
            }
            SshAuthMethod::Password(_) => {
                // Обработка пароля отдельно
            }
            _ => {}
        }

        args
    }

    /// Проверить контрольные суммы после передачи
    pub fn verify_checksums(
        &self,
        source_path: &Path,
        dest_path: &Path,
        components: &[MigrationComponent],
    ) -> Result<bool, Box<dyn std::error::Error>> {
        self.logger.info("Проверка контрольных сумм...");

        for component in components {
            if let Some(rel_path) = component.get_default_path() {
                let source_full = source_path.join(&rel_path);
                
                if !source_full.exists() {
                    continue;
                }

                // Генерируем хеш источника
                let source_hash = if source_full.is_file() {
                    self.compute_file_hash(&source_full)?
                } else {
                    self.compute_dir_hash(&source_full)?
                };

                // Получаем хеш назначения через SSH
                let user_host = format!("{}@{}", self.config.username, self.config.host);
                let dest_full = dest_path.join(&rel_path);
                
                let hash_cmd = if dest_full.is_file() {
                    format!("sha256sum {} | cut -d' ' -f1", dest_full.display())
                } else {
                    format!("find {} -type f -exec sha256sum {{}} \\; | sort | sha256sum | cut -d' ' -f1", dest_full.display())
                };

                let output = self.run_ssh_command(&hash_cmd)?;
                let dest_hash = String::from_utf8_lossy(&output.stdout).trim().to_string();

                if source_hash != dest_hash {
                    self.logger.error(&format!(
                        "Несоответствие хешей для {:?}: источник={}, назначение={}",
                        rel_path, source_hash, dest_hash
                    ));
                    return Ok(false);
                }

                self.logger.debug(&format!("Хеши совпадают для {:?}", rel_path));
            }
        }

        self.logger.info("Все контрольные суммы совпадают");
        Ok(true)
    }

    /// Вычислить хеш файла
    fn compute_file_hash(&self, path: &Path) -> Result<String, Box<dyn std::error::Error>> {
        use sha2::{Sha256, Digest};
        use std::fs::File;
        use std::io::BufReader;

        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];

        loop {
            let count = std::io::Read::read(&mut reader, &mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }

        Ok(hex::encode(hasher.finalize()))
    }

    /// Вычислить хеш директории
    fn compute_dir_hash(&self, dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();

        for entry in walkdir::WalkDir::new(dir) {
            let entry = entry?;
            if entry.file_type().is_file() {
                if let Ok(hash) = self.compute_file_hash(entry.path()) {
                    hasher.update(hash.as_bytes());
                }
            }
        }

        Ok(hex::encode(hasher.finalize()))
    }
}

/// Извлечь поле из /etc/os-release
fn extract_os_field(content: &str, field: &str) -> String {
    for line in content.lines() {
        if line.starts_with(field) {
            return line
                .trim_start_matches(field)
                .trim_start_matches('=')
                .trim_matches('"')
                .to_string();
        }
    }
    "unknown".to_string()
}

/// Распарсить человеческий размер (K, M, G)
fn parse_human_readable_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let num_part: String = s.chars().filter(|c| c.is_numeric()).collect();
    let unit = s.chars().last()?.to_ascii_uppercase();

    let num: u64 = num_part.parse().ok()?;

    match unit {
        'K' => Some(num * 1024),
        'M' => Some(num * 1024 * 1024),
        'G' => Some(num * 1024 * 1024 * 1024),
        'T' => Some(num * 1024 * 1024 * 1024 * 1024),
        _ => Some(num),
    }
}

/// Распарсить вывод rsync
fn parse_rsync_output(output: &str) -> u64 {
    // Ищем строку вида "sent X bytes ..."
    for line in output.lines() {
        if line.contains("sent") && line.contains("bytes") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            for (i, part) in parts.iter().enumerate() {
                if *part == "sent" && i + 1 < parts.len() {
                    if let Ok(bytes) = parts[i + 1].parse::<u64>() {
                        return bytes;
                    }
                }
            }
        }
    }
    0
}

/// Получить размер директории
fn get_dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry?;
        if entry.file_type().is_file() {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_human_readable_size() {
        assert_eq!(parse_human_readable_size("100K"), Some(102400));
        assert_eq!(parse_human_readable_size("10M"), Some(10485760));
        assert_eq!(parse_human_readable_size("1G"), Some(1073741824));
        assert_eq!(parse_human_readable_size("500"), Some(500));
    }

    #[test]
    fn test_extract_os_field() {
        let content = r#"PRETTY_NAME="Red OS 7.3"
VERSION_ID="7.3"
ID=redos"#;
        assert_eq!(extract_os_field(content, "PRETTY_NAME"), "Red OS 7.3");
        assert_eq!(extract_os_field(content, "VERSION_ID"), "7.3");
        assert_eq!(extract_os_field(content, "UNKNOWN"), "unknown");
    }
}
