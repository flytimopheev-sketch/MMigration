//! Передача данных по SSH (подпроцессы `ssh`, `scp`, `rsync`).
//!
//! Выбрана реализация через системные утилиты: на РЕД ОС они уже установлены,
//! поддерживают все способы аутентификации (ключ, агент, пароль) и не требуют
//! нативных зависимостей (libssh2) в дереве сборки.

use std::fmt;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use zeroize::Zeroizing;

use crate::error::{MigrationError, Result};

/// Секретная строка, которая очищается при уничтожении.
///
/// Используется для пароля sudo: он не должен попадать в журналы, отчёты,
/// состояние мастера или список процессов.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    /// Создать секрет.
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Получить значение для передачи в stdin дочернего процесса.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Пустой ли секрет.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(***)")
    }
}

/// Параметры повышения прав через sudo.
#[derive(Debug, Clone)]
pub struct SudoOptions {
    /// Пользователь-владелец файлов на приёмнике.
    pub target_user: String,
    /// Пароль sudo (только в памяти). `None` — беспарольный sudo.
    pub password: Option<SecretString>,
}

/// Режим доступа к целевому каталогу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshAccessMode {
    /// Администратор может писать сам и владелец совпадает с ним.
    Direct,
    /// Нужен root: staging + apply-скрипт через sudo.
    Sudo,
}

/// Результат предварительной проверки удалённого хоста.
#[derive(Debug, Clone)]
pub struct SshPreflight {
    /// Пользователь SSH-подключения.
    pub login_user: String,
    /// Домашний каталог пользователя SSH-подключения.
    pub login_home: String,
    /// Пользователь-владелец файлов на приёмнике.
    pub target_user: String,
    /// Домашний каталог целевого пользователя по данным `getent passwd`.
    pub target_user_home: Option<String>,
    /// Совпадает ли целевой каталог с домашним каталогом пользователя
    /// (с учётом символических ссылок).
    pub target_home_matches_user: bool,
    /// Целевой домашний каталог.
    pub target_home: String,
    /// Существует ли целевой пользователь.
    pub target_user_exists: bool,
    /// Существует ли целевой каталог.
    pub target_home_exists: bool,
    /// Может ли администратор писать в целевой каталог.
    pub target_home_writable: bool,
    /// Владелец целевого каталога, если он существует.
    pub target_home_owner: Option<String>,
    /// Доступен ли sudo.
    pub sudo_available: bool,
    /// Настроен ли sudo без пароля.
    pub sudo_passwordless: bool,
    /// Есть ли rsync на приёмнике (нужен для apply-этапа).
    pub rsync_available: bool,
    /// Есть ли restorecon (SELinux).
    pub restorecon_available: bool,
    /// Свободно байт на целевом разделе.
    pub target_available_bytes: Option<u64>,
    /// Свободно байт в домашнем каталоге администратора (staging).
    pub staging_available_bytes: Option<u64>,
    /// Находятся ли staging и цель на одном разделе.
    pub same_filesystem: bool,
    /// Рекомендуемый режим.
    pub access_mode: SshAccessMode,
}

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

/// Предложить удалённый домашний каталог по логину SSH и целевому пользователю.
///
/// Если целевой пользователь не задан или совпадает с пользователем SSH,
/// возвращается `~`: каталог разрешается на удалённой стороне и не зависит от
/// соглашения `/home/<user>`. Для другого пользователя используется
/// `/home/<target>`, а для `root` — `/root`. Точный путь можно передать через
/// `--remote-home`.
pub fn default_remote_home(login_user: &str, target_user: Option<&str>) -> String {
    match target_user {
        Some("root") => "/root".to_string(),
        Some(target) if target != login_user => format!("/home/{}", target),
        _ => "~".to_string(),
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
            "-T".to_string(),
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
    /// Проверять содержимое по контрольным суммам (rsync --checksum)
    pub verify: bool,
    /// Использовать rsync, если он доступен и локально, и на приёмнике
    pub use_rsync: bool,
}

impl Default for SshTransferOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            compress: true,
            rate_limit_kbps: 0,
            delete_extra: false,
            verify: true,
            use_rsync: true,
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

/// Параметры привилегированного применения staging.
#[derive(Debug, Clone)]
pub struct ApplyStagedOptions {
    /// Абсолютный целевой домашний каталог.
    pub target_home: String,
    /// Пользователь-владелец файлов.
    pub target_user: String,
    /// Каталог резервной копии (None — без backup).
    pub backup_dir: Option<String>,
    /// Проверять содержимое по контрольным суммам.
    pub verify: bool,
}

/// Результат привилегированного применения staging.
#[derive(Debug, Clone, Default)]
pub struct ApplyStagedResult {
    /// Диагностический вывод apply-скрипта.
    pub stderr: String,
    /// Каталог backup, если он создавался.
    pub backup_dir: Option<String>,
}

/// Передача данных по SSH.
pub struct SshTransfer {
    target: SshTarget,
    sudo: Option<SudoOptions>,
}

impl SshTransfer {
    /// Создать передачу для указанного подключения.
    pub fn new(target: SshTarget) -> Self {
        Self { target, sudo: None }
    }

    /// Разрешить повышение прав через sudo.
    pub fn with_sudo(mut self, sudo: SudoOptions) -> Self {
        self.sudo = Some(sudo);
        self
    }

    /// Подключение.
    pub fn target(&self) -> &SshTarget {
        &self.target
    }

    /// Параметры sudo, если они заданы.
    pub fn sudo_options(&self) -> Option<&SudoOptions> {
        self.sudo.as_ref()
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

    /// Развернуть `~`/`~/путь` в абсолютный домашний каталог SSH-пользователя.
    ///
    /// Остальные пути проверяются как абсолютные. Это позволяет сохранить
    /// привычное значение `--remote-home ~`, но дальнейшие операции работают
    /// только с абсолютным путём.
    pub fn resolve_remote_home(&self, path: &str) -> Result<String> {
        let path = path.trim_end_matches('/');
        if path == "~" {
            let output = self.run_remote("printf '%s\\n' \"$HOME\"")?;
            let home = output.trim();
            validate_remote_path(home)?;
            return Ok(home.to_string());
        }
        if let Some(rest) = path.strip_prefix("~/") {
            let output = self.run_remote("printf '%s\\n' \"$HOME\"")?;
            let home = output.trim().trim_end_matches('/');
            validate_remote_path(home)?;
            let resolved = format!("{}/{}", home, rest.trim_start_matches('/'));
            validate_remote_path(&resolved)?;
            return Ok(resolved);
        }
        validate_remote_path(path)?;
        Ok(path.to_string())
    }

    /// Предварительная проверка удалённого хоста без sudo.
    ///
    /// Проверяет ключ, целевого пользователя, право записи, наличие sudo,
    /// rsync и свободное место. Пароль sudo здесь не передаётся.
    pub fn preflight(
        &self,
        target_home: &str,
        requested_user: Option<&str>,
    ) -> Result<SshPreflight> {
        validate_remote_path(target_home)?;
        if let Some(user) = requested_user {
            validate_remote_user(user)?;
        }

        let target = shell_quote(target_home);
        let script = format!(
            r#"set -u
TARGET={target}
PARENT=$(dirname -- "$TARGET")
printf 'login_user=%s\n' "$(id -un)"
printf 'login_home=%s\n' "$HOME"
printf 'staging_available_kb=%s\n' "$(df -Pk -- "$HOME" 2>/dev/null | awk 'NR==2 {{print $4}}')"
if [ -e "$TARGET" ]; then
  printf 'target_exists=1\n'
  printf 'target_owner=%s\n' "$(stat -c %U -- "$TARGET" 2>/dev/null || true)"
  if [ -d "$TARGET" ] && [ -w "$TARGET" ]; then printf 'target_writable=1\n'; else printf 'target_writable=0\n'; fi
  printf 'target_available_kb=%s\n' "$(df -Pk -- "$TARGET" 2>/dev/null | awk 'NR==2 {{print $4}}')"
  TARGET_DEV=$(stat -c %d -- "$TARGET" 2>/dev/null || echo '')
else
  printf 'target_exists=0\n'
  printf 'target_owner=\n'
  if [ -d "$PARENT" ] && [ -w "$PARENT" ]; then printf 'target_writable=1\n'; else printf 'target_writable=0\n'; fi
  printf 'target_available_kb=%s\n' "$(df -Pk -- "$PARENT" 2>/dev/null | awk 'NR==2 {{print $4}}')"
  TARGET_DEV=$(stat -c %d -- "$PARENT" 2>/dev/null || echo '')
fi
HOME_DEV=$(stat -c %d -- "$HOME" 2>/dev/null || echo '')
if [ -n "$TARGET_DEV" ] && [ "$TARGET_DEV" = "$HOME_DEV" ]; then printf 'same_filesystem=1\n'; else printf 'same_filesystem=0\n'; fi
if [ -x /usr/bin/sudo ]; then
  printf 'sudo_available=1\n'
  if /usr/bin/sudo -n true >/dev/null 2>&1; then printf 'sudo_passwordless=1\n'; else printf 'sudo_passwordless=0\n'; fi
else
  printf 'sudo_available=0\n'
  printf 'sudo_passwordless=0\n'
fi
if command -v rsync >/dev/null 2>&1; then printf 'rsync=1\n'; else printf 'rsync=0\n'; fi
if command -v restorecon >/dev/null 2>&1; then printf 'restorecon=1\n'; else printf 'restorecon=0\n'; fi
"#,
            target = target
        );
        let output = self.run_remote(&script)?;
        let mut values = std::collections::HashMap::new();
        for line in output.lines() {
            if let Some((key, value)) = line.split_once('=') {
                values.insert(key.trim().to_string(), value.trim().to_string());
            }
        }

        let login_user = values
            .get("login_user")
            .filter(|value| !value.is_empty())
            .cloned()
            .ok_or_else(|| MigrationError::Ssh("не удалось определить пользователя SSH".into()))?;
        let login_home = values
            .get("login_home")
            .filter(|value| !value.is_empty())
            .cloned()
            .ok_or_else(|| {
                MigrationError::Ssh("не удалось определить домашний каталог SSH".into())
            })?;
        let target_home_exists = values.get("target_exists").map(String::as_str) == Some("1");
        let target_home_owner = values
            .get("target_owner")
            .filter(|value| !value.is_empty())
            .cloned();
        let target_user = requested_user
            .map(str::to_string)
            .or_else(|| target_home_owner.clone())
            .unwrap_or_else(|| login_user.clone());
        validate_remote_user(&target_user)?;
        let target_user_home = self.remote_user_home(&target_user)?;
        let target_home_matches_user = match target_user_home.as_deref() {
            Some(user_home) => self.remote_home_matches(target_home, user_home)?,
            None => false,
        };
        let target_user_exists = target_user == login_user || target_user_home.is_some();
        let target_home_writable = values.get("target_writable").map(String::as_str) == Some("1");
        let target_available_bytes = parse_kib(values.get("target_available_kb"));
        let staging_available_bytes = parse_kib(values.get("staging_available_kb"));
        let same_filesystem = values.get("same_filesystem").map(String::as_str) == Some("1");
        let sudo_available = values.get("sudo_available").map(String::as_str) == Some("1");
        let sudo_passwordless = values.get("sudo_passwordless").map(String::as_str) == Some("1");
        let rsync_available = values.get("rsync").map(String::as_str) == Some("1");
        let restorecon_available = values.get("restorecon").map(String::as_str) == Some("1");
        let access_mode = if target_home_writable && target_user == login_user {
            SshAccessMode::Direct
        } else {
            SshAccessMode::Sudo
        };

        Ok(SshPreflight {
            login_user,
            login_home,
            target_user,
            target_user_home,
            target_home_matches_user,
            target_home: target_home.to_string(),
            target_user_exists,
            target_home_exists,
            target_home_writable,
            target_home_owner,
            sudo_available,
            sudo_passwordless,
            rsync_available,
            restorecon_available,
            target_available_bytes,
            staging_available_bytes,
            same_filesystem,
            access_mode,
        })
    }

    /// Получить домашний каталог пользователя по `getent passwd`.
    fn remote_user_home(&self, user: &str) -> Result<Option<String>> {
        validate_remote_user(user)?;
        let output = self.run_remote(&format!(
            "if command -v getent >/dev/null 2>&1; then \
             getent passwd {} | cut -d: -f6; fi",
            shell_quote(user)
        ))?;
        let home = output.trim();
        Ok((!home.is_empty()).then(|| home.to_string()))
    }

    /// Совпадают ли удалённые пути с учётом символических ссылок.
    fn remote_home_matches(&self, target_home: &str, user_home: &str) -> Result<bool> {
        validate_remote_path(target_home)?;
        validate_remote_path(user_home)?;
        let output = self.run_remote(&format!(
            "if command -v realpath >/dev/null 2>&1 && \
             [ \"$(realpath -m -- {target})\" = \"$(realpath -m -- {home})\" ]; \
             then echo 1; else echo 0; fi",
            target = shell_quote(target_home),
            home = shell_quote(user_home)
        ))?;
        Ok(output.trim() == "1")
    }

    /// Проверить пароль sudo до начала передачи.
    ///
    /// Пароль передаётся только в stdin SSH-процесса. В аргументах командной
    /// строки, журналах и удалённых файлах его нет.
    pub fn check_sudo_password(&self, password: &SecretString) -> Result<()> {
        let mut input = Zeroizing::new(Vec::with_capacity(password.expose().len() + 1));
        input.extend_from_slice(password.expose().as_bytes());
        input.push(b'\n');
        let output =
            self.run_ssh_with_stdin(&["LC_ALL=C /usr/bin/sudo -S -p '' true"], input.as_slice())?;
        if !output.status.success() {
            return Err(sudo_error(&String::from_utf8_lossy(&output.stderr)));
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

    /// Отпечаток ключа хоста (`ssh-keyscan` + `ssh-keygen -lf -`).
    ///
    /// Показывается пользователю до подключения. `accept-new` принимает новый
    /// ключ только при первом подключении; изменение известного ключа
    /// по-прежнему блокируется (защита от MITM).
    pub fn host_fingerprint(&self) -> Result<Option<String>> {
        let scan = Command::new("ssh-keyscan")
            .args(["-p", &self.target.port.to_string(), &self.target.host])
            .stdin(Stdio::null())
            .output()
            .map_err(|_| MigrationError::missing_tool("ssh-keyscan"))?;

        if !scan.status.success() || scan.stdout.is_empty() {
            return Ok(None);
        }

        let mut child = Command::new("ssh-keygen")
            .args(["-lf", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| MigrationError::missing_tool("ssh-keygen"))?;
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(&scan.stdout)?;
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Ok(None);
        }

        let fingerprint = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string);
        Ok(fingerprint)
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

    /// Выполнить удалённую команду, передав данные в stdin.
    ///
    /// Используется только для пароля sudo: данные не попадают в argv и не
    /// сохраняются на удалённом хосте.
    fn run_ssh_with_stdin(&self, remote_args: &[&str], input: &[u8]) -> Result<Output> {
        let mut command = Command::new("ssh");
        command.args(self.target.ssh_base_args());
        command.arg(self.target.destination());
        command.args(remote_args);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|_| MigrationError::missing_tool("ssh"))?;
        let write_result = child
            .stdin
            .take()
            .map(|mut stdin| stdin.write_all(input))
            .transpose();
        if let Err(error) = write_result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        child
            .wait_with_output()
            .map_err(|error| MigrationError::Ssh(format!("не удалось завершить ssh: {error}")))
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
        if let Some(known_hosts) = &self.target.known_hosts_file {
            args.push("-o".to_string());
            args.push(format!("UserKnownHostsFile={}", known_hosts.display()));
        }
        args
    }

    fn ssh_transport_string(&self) -> String {
        let mut transport = format!("ssh -p {}", self.target.port);
        if let Some(identity) = &self.target.identity_file {
            transport.push_str(&format!(" -i {}", shell_quote(&identity.to_string_lossy())));
        }
        if let Some(known_hosts) = &self.target.known_hosts_file {
            transport.push_str(&format!(
                " -o UserKnownHostsFile={}",
                shell_quote(&known_hosts.to_string_lossy())
            ));
        }
        transport.push_str(" -o BatchMode=yes -o StrictHostKeyChecking=accept-new");
        transport
    }

    fn finish(&self, mut command: Command) -> Result<Output> {
        command
            .stdin(Stdio::null())
            .output()
            .map_err(|_| MigrationError::missing_tool("ssh"))
    }
}

/// Проверить наличие `rsync` в локальном `PATH`.
pub fn local_rsync_available() -> bool {
    which_available("rsync")
}

impl SshTransfer {
    /// Источник для передачи: для каталога — его содержимое, а не сам каталог.
    fn transfer_source(local: &Path, local_is_dir: bool, use_rsync: bool) -> PathBuf {
        if !local_is_dir {
            return local.to_path_buf();
        }
        if use_rsync {
            let mut source = local.as_os_str().to_os_string();
            source.push(std::path::MAIN_SEPARATOR.to_string());
            PathBuf::from(source)
        } else {
            local.join(".")
        }
    }

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

        // Для каталога копируем содержимое, а не сам каталог: иначе при
        // существующем каталоге на приёмнике rsync/scp создадут вложенный
        // каталог (Desktop/Desktop, .ssh/.ssh).
        let local_is_dir = local
            .symlink_metadata()
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false);
        if local_is_dir {
            self.ensure_remote_dir(remote)?;
        } else if let Some((parent, _)) = remote.rsplit_once('/') {
            if !parent.is_empty() {
                self.ensure_remote_dir(parent)?;
            }
        }

        let use_rsync = options.use_rsync && which_available("rsync");
        let output = if use_rsync {
            let mut command = Command::new("rsync");
            command.arg("-a");
            command.arg("--partial");
            if options.verify {
                command.arg("--checksum");
            }
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
            command.arg(Self::transfer_source(local, local_is_dir, true));
            command.arg(format!("{}:{}", self.target.destination(), remote));
            self.finish(command)?
        } else {
            let mut command = Command::new("scp");
            command.args(self.scp_base_args(options.compress));
            command.arg("-r");
            // `.` заставляет scp копировать содержимое каталога, а не
            // создавать одноимённый вложенный каталог.
            command.arg(Self::transfer_source(local, local_is_dir, false));
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

    /// Исправить права `.ssh` в прямом режиме (без root).
    ///
    /// Символическая ссылка `.ssh` не разыменовывается.
    pub fn fix_ssh_permissions(&self, home: &str) -> Result<()> {
        validate_remote_path(home)?;
        let ssh = shell_quote(&format!("{}/.ssh", home.trim_end_matches('/')));
        self.run_remote(&format!(
            "if [ -d {ssh} ] && [ ! -L {ssh} ]; then \
             chmod 700 -- {ssh} && \
             {{ find {ssh} -maxdepth 1 -type f -exec chmod 600 -- {{}} + 2>/dev/null || true; }}; \
             fi",
            ssh = ssh
        ))?;
        Ok(())
    }

    /// Создать удалённый каталог (если его нет).
    pub fn ensure_remote_dir(&self, path: &str) -> Result<()> {
        validate_remote_path(path)?;
        self.run_remote(&format!("mkdir -p -- {}", shell_quote(path)))?;
        Ok(())
    }

    /// Создать удалённый staging-каталог с доступом только владельцу.
    pub fn prepare_staging(&self, stage_root: &str) -> Result<()> {
        validate_remote_path(stage_root)?;
        self.run_remote(&format!(
            "umask 077; mkdir -p -- {stage}; chmod 700 -- {stage}",
            stage = shell_quote(stage_root)
        ))?;
        Ok(())
    }

    /// Отправить локальный путь в staging на удалённом хосте.
    pub fn push_staged(
        &self,
        local: &Path,
        relative: &Path,
        stage_root: &str,
        options: &SshTransferOptions,
    ) -> Result<SshTransferResult> {
        validate_remote_path(stage_root)?;
        let relative = remote_relative(relative)?;
        let remote = if relative.is_empty() {
            stage_root.to_string()
        } else {
            format!("{}/{}", stage_root.trim_end_matches('/'), relative)
        };
        self.push(local, &remote, options)
    }

    /// Применить staging к целевому каталогу от имени root.
    ///
    /// Пароль sudo передаётся только в stdin SSH. Скрипт создаёт backup,
    /// копирует данные, выставляет владельца и, при необходимости, проверяет
    /// содержимое по контрольным суммам.
    pub fn apply_staged(
        &self,
        stage_root: &str,
        options: &ApplyStagedOptions,
        password: Option<&SecretString>,
    ) -> Result<ApplyStagedResult> {
        validate_remote_path(stage_root)?;
        validate_remote_path(&options.target_home)?;
        validate_remote_user(&options.target_user)?;
        if let Some(backup_dir) = &options.backup_dir {
            validate_remote_path(backup_dir)?;
            let prefix = format!("{}/", options.target_home.trim_end_matches('/'));
            let has_parent_component = backup_dir
                .split('/')
                .any(|component| component == ".." || component == ".");
            if !backup_dir.starts_with(&prefix) || has_parent_component {
                return Err(MigrationError::InvalidInput(
                    "каталог резервной копии должен находиться внутри целевого каталога"
                        .to_string(),
                ));
            }
        }

        let script = build_apply_script(stage_root, options);
        let mut local_script = tempfile::NamedTempFile::new()?;
        local_script.write_all(script.as_bytes())?;
        local_script.flush()?;

        let script_remote = format!("{}/apply.sh", stage_root.trim_end_matches('/'));
        let script_options = SshTransferOptions {
            dry_run: false,
            compress: true,
            rate_limit_kbps: 0,
            delete_extra: false,
            verify: false,
            use_rsync: true,
        };
        self.push(local_script.path(), &script_remote, &script_options)?;

        let remote_script = shell_quote(&script_remote);
        let command = password
            .filter(|value| !value.is_empty())
            .map(|_| {
                format!(
                    "LC_ALL=C /usr/bin/sudo -S -p '' /bin/bash {}",
                    remote_script
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "LC_ALL=C /usr/bin/sudo -n -p '' /bin/bash {}",
                    remote_script
                )
            });

        let output = if let Some(password) = password.filter(|value| !value.is_empty()) {
            let mut input = Zeroizing::new(Vec::with_capacity(password.expose().len() + 1));
            input.extend_from_slice(password.expose().as_bytes());
            input.push(b'\n');
            self.run_ssh_with_stdin(&[command.as_str()], input.as_slice())?
        } else {
            self.run_ssh(&[command.as_str()])?
        };
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            let lower = stderr.to_ascii_lowercase();
            if lower.contains("sudo:") && !lower.contains("rsync") {
                return Err(sudo_error(&stderr));
            }
            return Err(MigrationError::Ssh(format!(
                "не удалось применить staging в {}: {}",
                options.target_home,
                stderr.trim()
            )));
        }

        Ok(ApplyStagedResult {
            stderr,
            backup_dir: options.backup_dir.clone(),
        })
    }

    /// Удалить staging-каталог (например, после ошибки).
    pub fn cleanup_staging(&self, stage_root: &str) -> Result<()> {
        validate_remote_path(stage_root)?;
        self.run_remote(&format!("rm -rf -- {}", shell_quote(stage_root)))?;
        Ok(())
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

/// Собрать apply-скрипт, который выполняется от root.
///
/// Скрипт не читает stdin (там пароль sudo) и не содержит секретов.
fn build_apply_script(stage_root: &str, options: &ApplyStagedOptions) -> String {
    let target_home = options.target_home.trim_end_matches('/');
    let backup = options.backup_dir.clone().unwrap_or_default();
    let verify = if options.verify { "1" } else { "0" };
    let template = r#"#!/bin/bash
set -euo pipefail
umask 077

STAGE=@@STAGE@@
DEST=@@DEST@@
OWNER=@@OWNER@@
BACKUP=@@BACKUP@@
VERIFY=@@VERIFY@@

if [ ! -d "$STAGE" ]; then
  printf 'staging не найден: %s\n' "$STAGE" >&2
  exit 2
fi
if [ -z "$DEST" ] || [ "$DEST" = "/" ]; then
  printf 'некорректный целевой каталог: %s\n' "$DEST" >&2
  exit 2
fi
if ! id -u "$OWNER" >/dev/null 2>&1; then
  printf 'целевой пользователь не найден: %s\n' "$OWNER" >&2
  exit 3
fi
USER_HOME=$(getent passwd "$OWNER" 2>/dev/null | cut -d: -f6)
if [ -z "$USER_HOME" ] || ! command -v realpath >/dev/null 2>&1; then
  printf 'не удалось проверить домашний каталог пользователя %s\n' "$OWNER" >&2
  exit 3
fi
DEST_REAL=$(realpath -m -- "$DEST")
USER_HOME_REAL=$(realpath -m -- "$USER_HOME")
if [ "$DEST_REAL" != "$USER_HOME_REAL" ]; then
  printf 'целевой каталог %s не совпадает с домашним каталогом %s (%s)\n' \
    "$DEST" "$OWNER" "$USER_HOME" >&2
  exit 3
fi
case "$USER_HOME_REAL" in
  /|/etc|/etc/*|/usr|/usr/*|/var|/var/*|/boot|/boot/*|/dev|/dev/*|/proc|/proc/*|/sys|/sys/*|/bin|/bin/*|/sbin|/sbin/*|/lib|/lib/*|/lib64|/lib64/*|/opt|/opt/*|/srv|/srv/*|/tmp|/tmp/*|/run|/run/*|/media|/media/*|/mnt|/mnt/*|/lost+found|/lost+found/*|/home)
    printf 'небезопасный домашний каталог пользователя %s: %s\n' \
      "$OWNER" "$USER_HOME_REAL" >&2
    exit 3
    ;;
esac
if [ "$USER_HOME_REAL" = "/root" ] && [ "$OWNER" != "root" ]; then
  printf 'домашний каталог /root разрешён только пользователю root\n' >&2
  exit 3
fi
if ! command -v rsync >/dev/null 2>&1; then
  printf 'на приёмнике не найден rsync\n' >&2
  exit 4
fi
if [ -n "$BACKUP" ]; then
  BACKUP_REAL=$(realpath -m -- "$BACKUP")
  case "$BACKUP_REAL" in
    "$DEST_REAL"/*) BACKUP="$BACKUP_REAL" ;;
    *)
      printf 'каталог резервной копии вне целевого каталога: %s\n' "$BACKUP" >&2
      exit 3
      ;;
  esac
fi

GROUP=$(id -gn "$OWNER")
if [ -d "$DEST" ]; then
  DEST_MODE=$(stat -c %a -- "$DEST")
else
  DEST_MODE=700
fi
mkdir -p -- "$DEST"
chown "$OWNER:$GROUP" "$DEST"

ARGS=(-aHAX --exclude=/apply.sh "--chown=$OWNER:$GROUP")
if [ "$VERIFY" = "1" ]; then
  ARGS+=(--checksum)
fi
if [ -n "$BACKUP" ]; then
  mkdir -p -- "$BACKUP"
  printf 'backup=%s\n' "$BACKUP"
  ARGS+=(--backup "--backup-dir=$BACKUP")
fi

rsync "${ARGS[@]}" "$STAGE"/ "$DEST"/

# Вернуть режим домашнего каталога: rsync мог перенести на него режим staging.
chmod "$DEST_MODE" "$DEST"
chown "$OWNER:$GROUP" "$DEST"

# Символическая ссылка .ssh не должна приводить к chmod/chown за пределами home.
if [ -d "$DEST/.ssh" ] && [ ! -L "$DEST/.ssh" ]; then
  chmod 700 "$DEST/.ssh" 2>/dev/null || true
  find "$DEST/.ssh" -maxdepth 1 -type f -exec chmod 600 {} + 2>/dev/null || true
fi
if command -v restorecon >/dev/null 2>&1; then
  restorecon -R "$DEST" >/dev/null 2>&1 || true
fi

rm -rf -- "$STAGE"
printf 'applied=1\n'
"#;

    let stage = shell_quote(stage_root);
    let dest = shell_quote(target_home);
    let owner = shell_quote(&options.target_user);
    let backup = shell_quote(&backup);
    render_apply_script(
        template,
        &[
            ("@@STAGE@@", stage.as_str()),
            ("@@DEST@@", dest.as_str()),
            ("@@OWNER@@", owner.as_str()),
            ("@@BACKUP@@", backup.as_str()),
            ("@@VERIFY@@", verify),
        ],
    )
}

/// Однопроходная подстановка значений в шаблон apply-скрипта.
///
/// Значения не сканируются повторно: путь, содержащий `@@OWNER@@`, не может
/// изменить другую часть скрипта.
fn render_apply_script(template: &str, values: &[(&str, &str)]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(index) = rest.find("@@") {
        output.push_str(&rest[..index]);
        rest = &rest[index..];
        let mut replaced = false;
        for (key, value) in values {
            if let Some(after) = rest.strip_prefix(key) {
                output.push_str(value);
                rest = after;
                replaced = true;
                break;
            }
        }
        if !replaced {
            output.push_str("@@");
            rest = &rest[2..];
        }
    }
    output.push_str(rest);
    output
}

/// Понятная ошибка sudo без раскрытия пароля.
fn sudo_error(stderr: &str) -> MigrationError {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("incorrect password")
        || lower.contains("sorry, try again")
        || lower.contains("неверный пароль")
    {
        return MigrationError::PermissionDenied("неверный пароль sudo".to_string());
    }
    if lower.contains("terminal is required") || lower.contains("no tty present") {
        return MigrationError::SudoRequired(
            "sudo требует терминал (requiretty). Настройте беспарольный helper \
             или отключите requiretty для этой учётной записи."
                .to_string(),
        );
    }
    if lower.contains("not in the sudoers") {
        return MigrationError::SudoRequired(
            "учётная запись не имеет прав sudo на приёмнике".to_string(),
        );
    }
    if lower.contains("a password is required") || lower.contains("password is required") {
        return MigrationError::SudoRequired("требуется пароль sudo".to_string());
    }
    MigrationError::Ssh(format!("sudo: {}", stderr.trim()))
}

/// Проверить абсолютный удалённый путь.
fn validate_remote_path(path: &str) -> Result<()> {
    if path.is_empty()
        || !path.starts_with('/')
        || path == "/"
        || path.len() > 4096
        || path.contains(['\n', '\r', '\0'])
    {
        return Err(MigrationError::InvalidInput(format!(
            "некорректный абсолютный путь на приёмнике: {}",
            path
        )));
    }
    Ok(())
}

/// Проверить имя удалённого пользователя.
fn validate_remote_user(user: &str) -> Result<()> {
    let mut chars = user.chars();
    let valid_start = chars
        .next()
        .map(|value| value.is_ascii_alphanumeric() || value == '_')
        .unwrap_or(false);
    let valid_rest = chars
        .all(|value| value.is_ascii_alphanumeric() || value == '_' || value == '-' || value == '.');
    if !valid_start || !valid_rest || user.len() > 64 {
        return Err(MigrationError::InvalidInput(format!(
            "некорректное имя пользователя на приёмнике: {}",
            user
        )));
    }
    Ok(())
}

/// Преобразовать относительный путь в безопасный путь для удалённого shell.
fn remote_relative(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.contains(['\n', '\r', '\0']) {
                    return Err(MigrationError::InvalidInput(
                        "некорректный относительный путь".to_string(),
                    ));
                }
                parts.push(part.into_owned());
            }
            Component::CurDir => {}
            _ => {
                return Err(MigrationError::InvalidInput(format!(
                    "недопустимый компонент пути: {}",
                    path.display()
                )));
            }
        }
    }
    Ok(parts.join("/"))
}

/// Перевести килобайты из `df -Pk` в байты.
fn parse_kib(value: Option<&String>) -> Option<u64> {
    value
        .and_then(|value| value.parse::<u64>().ok())
        .map(|kib| kib.saturating_mul(1024))
}

/// Заключить строку в одинарные кавычки для shell.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
    fn test_default_remote_home_matches_login_and_target_user() {
        assert_eq!(default_remote_home("admin", None), "~");
        assert_eq!(default_remote_home("", None), "~");
        assert_eq!(default_remote_home("admin", Some("admin")), "~");
        assert_eq!(
            default_remote_home("admin", Some("timopheevsi")),
            "/home/timopheevsi"
        );
        assert_eq!(default_remote_home("admin", Some("root")), "/root");
        assert_eq!(default_remote_home("root", Some("root")), "/root");
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
    fn test_transfer_source_copies_directory_contents() {
        let directory = Path::new("/home/user/Desktop");

        let rsync_source = SshTransfer::transfer_source(directory, true, true);
        let rsync_text = rsync_source.to_string_lossy();
        assert!(
            rsync_text.ends_with('/') || rsync_text.ends_with('\\'),
            "rsync source must have a trailing separator: {rsync_text}"
        );

        let scp_source = SshTransfer::transfer_source(directory, true, false);
        assert_eq!(scp_source, directory.join("."));

        let file = Path::new("/home/user/Desktop/report.txt");
        assert_eq!(
            SshTransfer::transfer_source(file, false, true),
            file.to_path_buf()
        );
        assert_eq!(
            SshTransfer::transfer_source(file, false, false),
            file.to_path_buf()
        );
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

    #[test]
    fn test_secret_string_is_redacted_in_debug() {
        let secret = SecretString::new("sudo-password");
        assert_eq!(secret.expose(), "sudo-password");
        assert!(!format!("{secret:?}").contains("sudo-password"));
        assert_eq!(format!("{secret:?}"), "SecretString(***)");
        assert!(SecretString::new("").is_empty());
    }

    #[test]
    fn test_default_transfer_options_verify() {
        let options = SshTransferOptions::default();
        assert!(options.verify);
        assert!(options.compress);
        assert!(!options.dry_run);
        assert!(!options.delete_extra);
    }

    #[test]
    fn test_remote_path_validation() {
        assert!(validate_remote_path("/home/timopheevsi").is_ok());
        assert!(validate_remote_path("/home/user with space").is_ok());
        assert!(validate_remote_path("relative/path").is_err());
        assert!(validate_remote_path("/").is_err());
        assert!(validate_remote_path("/home/user\nrm -rf /").is_err());
        assert!(validate_remote_path("").is_err());
    }

    #[test]
    fn test_remote_user_validation() {
        assert!(validate_remote_user("timopheevsi").is_ok());
        assert!(validate_remote_user("_admin").is_ok());
        assert!(validate_remote_user("user.name").is_ok());
        assert!(validate_remote_user("").is_err());
        assert!(validate_remote_user("bad user").is_err());
        assert!(validate_remote_user("bad;user").is_err());
        assert!(validate_remote_user("-user").is_err());
    }

    #[test]
    fn test_remote_relative_normalizes_and_rejects_parent() {
        assert_eq!(
            remote_relative(Path::new("Desktop/file.txt")).expect("relative"),
            "Desktop/file.txt"
        );
        assert_eq!(
            remote_relative(Path::new("./Desktop")).expect("relative"),
            "Desktop"
        );
        assert!(remote_relative(Path::new("../secret")).is_err());
        assert!(remote_relative(Path::new("/absolute")).is_err());
    }

    #[test]
    fn test_apply_script_quotes_values_and_uses_backup() {
        let options = ApplyStagedOptions {
            target_home: "/home/o'brien".to_string(),
            target_user: "o'brien".to_string(),
            backup_dir: Some("/home/timopheevsi/.migration-master-backup/1".to_string()),
            verify: true,
        };
        let script = build_apply_script("/home/admin/.cache/migration-master/stage/1", &options);

        assert!(script.contains("STAGE='/home/admin/.cache/migration-master/stage/1'"));
        assert!(script.contains("DEST='/home/o'\\''brien'"));
        assert!(script.contains("OWNER='o'\\''brien'"));
        assert!(script.contains("--chown=$OWNER:$GROUP"));
        assert!(script.contains("--backup-dir=$BACKUP"));
        assert!(script.contains("--checksum"));
        assert!(script.contains("--exclude=/apply.sh"));
        assert!(script.contains("realpath -m -- \"$DEST\""));
        assert!(script.contains("getent passwd \"$OWNER\""));
        assert!(script.contains("небезопасный домашний каталог"));
        assert!(script.contains("/etc/*"));
        assert!(script.contains("/usr/*"));
        assert!(script.contains("/var/*"));
        assert!(script.contains("DEST_MODE=700"));
        assert!(script.contains("[ ! -L \"$DEST/.ssh\" ]"));
        assert!(script.contains("chown \"$OWNER:$GROUP\" \"$DEST\""));
        assert!(script.contains("rm -rf -- \"$STAGE\""));
        assert!(!script.contains("sudo-password"));
        assert!(!script.contains("\nPassword:"));
    }

    #[test]
    fn test_apply_script_placeholder_is_not_rescanned() {
        let options = ApplyStagedOptions {
            target_home: "/home/@@OWNER@@/data".to_string(),
            target_user: "target".to_string(),
            backup_dir: None,
            verify: false,
        };
        let script = build_apply_script("/stage/1", &options);
        assert!(script.contains("DEST='/home/@@OWNER@@/data'"));
        assert!(script.contains("OWNER='target'"));
        assert!(!script.contains("DEST='/home/'target'/data'"));
    }

    #[test]
    fn test_sudo_error_mapping() {
        assert!(matches!(
            sudo_error("sudo: incorrect password"),
            MigrationError::PermissionDenied(_)
        ));
        assert!(matches!(
            sudo_error("sudo: a terminal is required to read the password"),
            MigrationError::SudoRequired(_)
        ));
        assert!(matches!(
            sudo_error("sudo: user is not in the sudoers file"),
            MigrationError::SudoRequired(_)
        ));
    }
}
