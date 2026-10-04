//! Данные главного экрана (§1): пользователь, система, место и наличие утилит.

use std::path::{Path, PathBuf};

use crate::platform;

/// Инструменты, без которых базовая миграция невозможна.
pub const REQUIRED_TOOLS: &[&str] = &["ssh", "tar", "zstd"];
/// Необязательные инструменты, расширяющие возможности (§1, §9).
pub const OPTIONAL_TOOLS: &[&str] = &["rsync", "age", "gpg", "lpstat", "pkexec"];

/// Состояние внешней утилиты.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStatus {
    /// Имя команды в PATH
    pub name: String,
    /// Доступна ли команда
    pub available: bool,
    /// Обязательна ли для базовой работы
    pub required: bool,
}

/// Сводка для главного экрана (§1).
#[derive(Debug, Clone, Default)]
pub struct MainScreenInfo {
    /// Текущий пользователь
    pub username: String,
    /// Имя компьютера
    pub hostname: String,
    /// Читаемое имя ОС
    pub os_pretty_name: String,
    /// Версия ОС
    pub os_version: String,
    /// Архитектура
    pub architecture: String,
    /// Домашний каталог
    pub home_path: PathBuf,
    /// Размер домашнего каталога (байт)
    pub home_size_bytes: u64,
    /// Количество файлов в домашнем каталоге
    pub home_files: u64,
    /// Свободное место на диске с домашним каталогом (байт)
    pub free_bytes: u64,
    /// Состояние внешних утилит
    pub tools: Vec<ToolStatus>,
}

impl MainScreenInfo {
    /// Собрать сведения о текущей системе.
    pub fn collect() -> Self {
        let home_path = platform::home_dir().unwrap_or_default();
        let (home_files, home_size_bytes) = home_usage(&home_path);
        let release = platform::os_release();

        let mut tools = Vec::new();
        for name in REQUIRED_TOOLS {
            tools.push(ToolStatus {
                name: (*name).to_string(),
                available: platform::command_exists(name),
                required: true,
            });
        }
        for name in OPTIONAL_TOOLS {
            tools.push(ToolStatus {
                name: (*name).to_string(),
                available: platform::command_exists(name),
                required: false,
            });
        }

        Self {
            username: platform::username(),
            hostname: platform::hostname(),
            os_pretty_name: release.pretty_name,
            os_version: release.version,
            architecture: platform::architecture(),
            free_bytes: platform::free_space(&home_path),
            home_path,
            home_size_bytes,
            home_files,
            tools,
        }
    }

    /// Недоступные утилиты.
    pub fn missing_tools(&self) -> Vec<&ToolStatus> {
        self.tools.iter().filter(|tool| !tool.available).collect()
    }

    /// Все ли обязательные утилиты на месте.
    pub fn required_tools_ok(&self) -> bool {
        self.tools
            .iter()
            .filter(|tool| tool.required)
            .all(|tool| tool.available)
    }

    /// Предупреждения о недоступных компонентах (§1).
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        for tool in &self.tools {
            if !tool.available {
                warnings.push(format!(
                    "{} не найден{}",
                    tool.name,
                    if tool.required {
                        " (обязателен для миграции)"
                    } else {
                        ""
                    }
                ));
            }
        }
        warnings
    }
}

/// Количество файлов и суммарный размер домашнего каталога.
fn home_usage(home: &Path) -> (u64, u64) {
    if !home.is_dir() {
        return (0, 0);
    }

    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in walkdir::WalkDir::new(home)
        .follow_links(false)
        .into_iter()
        .flatten()
    {
        if entry.file_type().is_file() {
            files += 1;
            bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        }
    }

    (files, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_has_user_and_host() {
        let info = MainScreenInfo::collect();
        assert!(!info.username.is_empty());
        assert!(!info.hostname.is_empty());
        assert!(!info.architecture.is_empty());
    }

    #[test]
    fn test_required_tools_are_listed() {
        let info = MainScreenInfo::collect();
        for name in REQUIRED_TOOLS {
            assert!(
                info.tools
                    .iter()
                    .any(|tool| tool.name == *name && tool.required),
                "обязательная утилита {} отсутствует в списке",
                name
            );
        }
    }

    #[test]
    fn test_warnings_match_missing_tools() {
        let info = MainScreenInfo::collect();
        assert_eq!(info.warnings().len(), info.missing_tools().len());
    }
}
