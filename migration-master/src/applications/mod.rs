//! Приложения: сбор `.desktop`-запускаторов и импорт/экспорт списка.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::Result;

/// Описание приложения из `.desktop`-файла.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AppEntry {
    /// Идентификатор (имя файла без расширения)
    pub id: String,
    /// Отображаемое имя
    pub name: String,
    /// Команда запуска
    pub exec: String,
    /// Категории
    pub categories: String,
    /// Запускается в терминале
    pub terminal: bool,
    /// Путь к файлу относительно домашнего каталога
    pub relative_path: String,
}

/// Сканер пользовательских приложений.
pub struct ApplicationScanner {
    home: PathBuf,
}

impl ApplicationScanner {
    /// Создать сканер.
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    /// Каталоги с пользовательскими запускаторами.
    fn user_app_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.home.join(".local/share/applications"),
            self.home.join(".gnome/apps"),
        ]
    }

    /// Просканировать `.desktop`-файлы профиля.
    pub fn scan(&self) -> Result<Vec<AppEntry>> {
        let mut entries: BTreeMap<String, AppEntry> = BTreeMap::new();

        for dir in self.user_app_dirs() {
            let read_dir = match std::fs::read_dir(&dir) {
                Ok(read_dir) => read_dir,
                Err(_) => continue,
            };

            for file in read_dir.flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                    continue;
                }

                if let Ok(content) = std::fs::read_to_string(&path) {
                    let mut entry = parse_desktop(&content);
                    let name = file.file_name().to_string_lossy().to_string();
                    entry.id = name.trim_end_matches(".desktop").to_string();
                    entry.relative_path = format!(".local/share/applications/{}", name);
                    entries.insert(entry.id.clone(), entry);
                }
            }
        }

        Ok(entries.into_values().collect())
    }

    /// Экспорт списка приложений в многострочную строку (id<TAB>name).
    pub fn export_list(entries: &[AppEntry]) -> String {
        entries
            .iter()
            .map(|entry| format!("{}\t{}", entry.id, entry.name))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Импорт списка (возвращает id, которые нужно установить).
    pub fn import_list(content: &str) -> Vec<String> {
        content
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                line.split('\t').next().map(|id| id.to_string())
            })
            .collect()
    }
}

/// Разобрать содержимое `.desktop`-файла (секция `[Desktop Entry]`).
pub fn parse_desktop(content: &str) -> AppEntry {
    let mut entry = AppEntry::default();
    let mut in_entry_section = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry_section || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key.trim() {
            "Name" => entry.name = value.trim().to_string(),
            "Exec" => entry.exec = value.trim().to_string(),
            "Categories" => entry.categories = value.trim().to_string(),
            "Terminal" => entry.terminal = value.trim() == "true",
            _ => {}
        }
    }

    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const SAMPLE: &str = "\
[Desktop Entry]
Type=Application
Name=Текстовый редактор
Exec=editor %F
Categories=Utility;TextEditor;
Terminal=false
X-Custom=ignored

[Desktop Action new]
Name=New
";

    #[test]
    fn test_parse_desktop() {
        let entry = parse_desktop(SAMPLE);
        assert_eq!(entry.name, "Текстовый редактор");
        assert_eq!(entry.exec, "editor %F");
        assert_eq!(entry.categories, "Utility;TextEditor;");
        assert!(!entry.terminal);
    }

    #[test]
    fn test_scan_finds_user_entries() {
        let dir = tempdir().expect("tempdir");
        let apps = dir.path().join(".local/share/applications");
        std::fs::create_dir_all(&apps).expect("mkdir");
        std::fs::write(apps.join("myapp.desktop"), SAMPLE).expect("write");

        let entries = ApplicationScanner::new(dir.path()).scan().expect("scan");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "myapp");
        assert_eq!(entries[0].name, "Текстовый редактор");
        assert_eq!(
            entries[0].relative_path,
            ".local/share/applications/myapp.desktop"
        );
    }

    #[test]
    fn test_export_import_list() {
        let entries = vec![
            AppEntry {
                id: "a".into(),
                name: "App A".into(),
                ..Default::default()
            },
            AppEntry {
                id: "b".into(),
                name: "App B".into(),
                ..Default::default()
            },
        ];

        let exported = ApplicationScanner::export_list(&entries);
        assert_eq!(exported, "a\tApp A\nb\tApp B");

        let imported = ApplicationScanner::import_list(&exported);
        assert_eq!(imported, vec!["a".to_string(), "b".to_string()]);

        let with_comments = "# комментарий\n\nx\ny\tY";
        assert_eq!(
            ApplicationScanner::import_list(with_comments),
            vec!["x".to_string(), "y".to_string()]
        );
    }
}

