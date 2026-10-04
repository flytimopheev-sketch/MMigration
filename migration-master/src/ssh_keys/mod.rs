//! SSH-ключи и настройки клиента: сканирование, сбор, восстановление.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::platform;

/// Тип файла в каталоге `.ssh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshKeyKind {
    /// Приватный ключ
    Private,
    /// Публичный ключ
    Public,
    /// Конфигурация клиента (config)
    Config,
    /// Известные хосты (known_hosts)
    KnownHosts,
    /// Прочий файл (например, сертификаты)
    Other,
}

/// Файл SSH в профиле.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SshKeyInfo {
    /// Относительный путь от домашнего каталога (`/.ssh/id_ed25519`)
    pub relative_path: String,
    /// Тип файла
    pub kind: SshKeyKind,
    /// Размер в байтах
    pub size: u64,
    /// Комментарий/подпись из публичного ключа
    pub comment: Option<String>,
    /// Публичный ключ соответствует приватному (для пар)
    pub paired: bool,
}

/// Сканер SSH-ключей профиля.
pub struct SshKeysScanner {
    home: PathBuf,
}

impl SshKeysScanner {
    /// Создать сканер для домашнего каталога.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    /// Каталог `.ssh`.
    pub fn ssh_dir(&self) -> PathBuf {
        self.home.join(".ssh")
    }

    /// Просканировать каталог `.ssh`.
    pub fn scan(&self) -> Result<Vec<SshKeyInfo>> {
        let dir = self.ssh_dir();
        let mut items = Vec::new();

        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => return Ok(items),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let name = entry.file_name().to_string_lossy().to_string();
            let kind = classify(&name, &path);
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let comment = extract_comment(&path, kind);

            items.push(SshKeyInfo {
                relative_path: format!(".ssh/{}", name),
                kind,
                size,
                comment,
                paired: false,
            });
        }

        mark_pairs(&mut items);
        items.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        Ok(items)
    }

    /// Собрать файлы SSH в каталог назначения (структура `.ssh/...`).
    pub fn collect(&self, destination: &Path) -> Result<usize> {
        let items = self.scan()?;
        let dir = self.ssh_dir();
        let mut copied = 0;

        for item in &items {
            let source = self.home.join(&item.relative_path);
            let target = destination.join(&item.relative_path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source, &target)?;
            copied += 1;
        }

        let _ = dir;
        Ok(copied)
    }

    /// Исправить права доступа: `.ssh` — 0700, приватные ключи — 0600.
    pub fn fix_permissions(root: &Path) -> Result<()> {
        let ssh_dir = root.join(".ssh");
        platform::set_mode(&ssh_dir, 0o700)?;

        if let Ok(entries) = std::fs::read_dir(&ssh_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let is_private = classify(&name, &path) == SshKeyKind::Private;
                let mode = if is_private { 0o600 } else { 0o644 };
                platform::set_mode(&path, mode)?;
            }
        }

        Ok(())
    }
}

/// Определить тип файла по имени и содержимому.
fn classify(name: &str, path: &Path) -> SshKeyKind {
    if name == "config" {
        return SshKeyKind::Config;
    }
    if name == "known_hosts" || name == "known_hosts.old" {
        return SshKeyKind::KnownHosts;
    }
    if name.ends_with(".pub") {
        return SshKeyKind::Public;
    }

    let is_private = std::fs::read_to_string(path)
        .map(|content| content.contains("PRIVATE KEY"))
        .unwrap_or(false);

    if is_private || name.starts_with("id_") || name.ends_with("_rsa") || name.ends_with("_ed25519")
    {
        SshKeyKind::Private
    } else {
        SshKeyKind::Other
    }
}

/// Извлечь комментарий из публичного ключа.
fn extract_comment(path: &Path, kind: SshKeyKind) -> Option<String> {
    if kind != SshKeyKind::Public {
        return None;
    }

    let content = std::fs::read_to_string(path).ok()?;
    let mut parts = content.split_whitespace();
    let _algorithm = parts.next()?;
    let _key = parts.next()?;
    let comment = parts.collect::<Vec<_>>().join(" ");
    if comment.is_empty() {
        None
    } else {
        Some(comment)
    }
}

/// Пометить пары «приватный + публичный».
fn mark_pairs(items: &mut [SshKeyInfo]) {
    for index in 0..items.len() {
        if items[index].kind == SshKeyKind::Private {
            let base = items[index]
                .relative_path
                .trim_end_matches(".pub")
                .to_string();
            let has_public = items.iter().any(|other| {
                other.kind == SshKeyKind::Public && other.relative_path == format!("{}.pub", base)
            });
            items[index].paired = has_public;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_ssh_dir(dir: &Path) -> PathBuf {
        let ssh = dir.join(".ssh");
        std::fs::create_dir_all(&ssh).expect("mkdir");
        std::fs::write(
            ssh.join("id_ed25519"),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC\n-----END OPENSSH PRIVATE KEY-----\n",
        )
        .expect("write");
        std::fs::write(
            ssh.join("id_ed25519.pub"),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 user@host\n",
        )
        .expect("write");
        std::fs::write(ssh.join("config"), "Host *\n  ForwardAgent no\n").expect("write");
        std::fs::write(ssh.join("known_hosts"), "host.example ssh-ed25519 AAAA\n").expect("write");
        ssh
    }

    #[test]
    fn test_scan_classifies_files() {
        let dir = tempdir().expect("tempdir");
        make_ssh_dir(dir.path());

        let items = SshKeysScanner::new(dir.path()).scan().expect("scan");
        assert_eq!(items.len(), 4);

        let private = items
            .iter()
            .find(|item| item.relative_path == ".ssh/id_ed25519")
            .expect("private");
        assert_eq!(private.kind, SshKeyKind::Private);
        assert!(private.paired, "приватный ключ должен иметь пару");

        let public = items
            .iter()
            .find(|item| item.relative_path == ".ssh/id_ed25519.pub")
            .expect("public");
        assert_eq!(public.kind, SshKeyKind::Public);
        assert_eq!(public.comment.as_deref(), Some("user@host"));

        assert_eq!(
            items
                .iter()
                .find(|item| item.relative_path == ".ssh/config")
                .expect("config")
                .kind,
            SshKeyKind::Config
        );
        assert_eq!(
            items
                .iter()
                .find(|item| item.relative_path == ".ssh/known_hosts")
                .expect("known_hosts")
                .kind,
            SshKeyKind::KnownHosts
        );
    }

    #[test]
    fn test_scan_missing_dir_is_empty() {
        let dir = tempdir().expect("tempdir");
        let items = SshKeysScanner::new(dir.path()).scan().expect("scan");
        assert!(items.is_empty());
    }

    #[test]
    fn test_collect_copies_files() {
        let dir = tempdir().expect("tempdir");
        make_ssh_dir(dir.path());
        let dest = dir.path().join("collected");

        let copied = SshKeysScanner::new(dir.path())
            .collect(&dest)
            .expect("collect");
        assert_eq!(copied, 4);
        assert!(dest.join(".ssh/id_ed25519").exists());
        assert!(dest.join(".ssh/config").exists());
    }

    #[test]
    fn test_fix_permissions_runs() {
        let dir = tempdir().expect("tempdir");
        make_ssh_dir(dir.path());
        SshKeysScanner::fix_permissions(dir.path()).expect("fix");
    }
}
