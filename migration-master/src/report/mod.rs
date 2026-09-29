//! Отчёты о миграции: JSON и HTML.

use std::path::{Path, PathBuf};

use crate::error::Result;

/// Статистика миграции в отчёте.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ReportStats {
    /// Скопировано файлов
    pub files_copied: u64,
    /// Скопировано байт
    pub bytes_copied: u64,
    /// Разрешено конфликтов
    pub conflicts_resolved: u64,
    /// Пропущено файлов
    pub files_skipped: u64,
    /// Перенесено пакетов
    pub packages: u64,
    /// Перенесено принтеров
    pub printers: u64,
    /// Длительность в миллисекундах
    pub duration_ms: u128,
}

/// Итоговый отчёт о миграции.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MigrationReport {
    /// Идентификатор операции
    pub id: String,
    /// Режим миграции
    pub mode: crate::config::MigrationMode,
    /// Источник (хост или путь)
    pub source: String,
    /// Назначение (хост или путь)
    pub target: String,
    /// Задействованные компоненты
    pub components: Vec<crate::config::ComponentType>,
    /// Статистика
    pub stats: ReportStats,
    /// Предупреждения
    pub warnings: Vec<String>,
    /// Ошибки
    pub errors: Vec<String>,
    /// Дата создания (RFC 3339)
    pub created_at: String,
}

impl MigrationReport {
    /// Создать пустой отчёт.
    pub fn new(
        id: impl Into<String>,
        mode: crate::config::MigrationMode,
        source: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            mode,
            source: source.into(),
            target: target.into(),
            components: Vec::new(),
            stats: ReportStats::default(),
            warnings: Vec::new(),
            errors: Vec::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Отчёт успешен (нет ошибок).
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }

    /// Сериализация в JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Сохранить JSON-отчёт в каталоге, вернуть путь.
    pub fn save_json(&self, directory: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("report-{}.json", self.id));
        std::fs::write(&path, self.to_json()?)?;
        Ok(path)
    }

    /// HTML-представление отчёта.
    pub fn to_html(&self) -> String {
        let warnings = self
            .warnings
            .iter()
            .map(|item| format!("<li class=\"warning\">{}</li>", escape_html(item)))
            .collect::<Vec<_>>()
            .join("\n");
        let errors = self
            .errors
            .iter()
            .map(|item| format!("<li class=\"error\">{}</li>", escape_html(item)))
            .collect::<Vec<_>>()
            .join("\n");
        let components = self
            .components
            .iter()
            .map(|component| format!("<li>{}</li>", escape_html(component.description())))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "<!DOCTYPE html>\n<html lang=\"ru\"><head><meta charset=\"utf-8\">\
<title>Отчёт {id}</title>\n\
<style>body{{font-family:sans-serif;max-width:60rem;margin:2rem auto;padding:0 1rem}}\
.warning{{color:#8a6d00}}.error{{color:#b00020}}table{{border-collapse:collapse}}\
td,th{{border:1px solid #ccc;padding:.4rem .8rem;text-align:left}}</style></head>\n\
<body><h1>Отчёт о миграции</h1>\n\
<p>Операция: <b>{id}</b> от {created_at}<br>\n\
Источник: {source} → Назначение: {target}<br>\n\
Статус: {status}</p>\n\
<h2>Статистика</h2>\n\
<table><tr><th>Файлов</th><td>{files}</td></tr>\n\
<tr><th>Байт</th><td>{bytes}</td></tr>\n\
<tr><th>Конфликтов разрешено</th><td>{conflicts}</td></tr>\n\
<tr><th>Пропущено</th><td>{skipped}</td></tr>\n\
<tr><th>Пакетов</th><td>{packages}</td></tr>\n\
<tr><th>Принтеров</th><td>{printers}</td></tr>\n\
<tr><th>Длительность, мс</th><td>{duration}</td></tr></table>\n\
<h2>Компоненты</h2><ul>{components}</ul>\n\
<h2>Предупреждения</h2><ul>{warnings}</ul>\n\
<h2>Ошибки</h2><ul>{errors}</ul>\n\
</body></html>\n",
            id = escape_html(&self.id),
            created_at = escape_html(&self.created_at),
            source = escape_html(&self.source),
            target = escape_html(&self.target),
            status = if self.is_success() { "успешно" } else { "с ошибками" },
            files = self.stats.files_copied,
            bytes = self.stats.bytes_copied,
            conflicts = self.stats.conflicts_resolved,
            skipped = self.stats.files_skipped,
            packages = self.stats.packages,
            printers = self.stats.printers,
            duration = self.stats.duration_ms,
            warnings = if warnings.is_empty() { "<li>нет</li>" } else { &warnings },
            errors = if errors.is_empty() { "<li>нет</li>" } else { &errors },
            components = if components.is_empty() { "<li>нет</li>" } else { &components },
        )
    }

    /// Сохранить HTML-отчёт, вернуть путь.
    pub fn save_html(&self, directory: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("report-{}.html", self.id));
        std::fs::write(&path, self.to_html())?;
        Ok(path)
    }
}

/// Экранирование HTML-спецсимволов.
pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_escape_html() {
        assert_eq!(
            escape_html("<b>\"x\" & 'y'</b>"),
            "&lt;b&gt;&quot;x&quot; &amp; 'y'&lt;/b&gt;"
        );
    }

    #[test]
    fn test_report_json_and_html() {
        let mut report = MigrationReport::new(
            "test-id",
            crate::config::MigrationMode::LocalArchive,
            "old-pc",
            "new-pc",
        );
        report.components = vec![crate::config::ComponentType::Documents];
        report.stats.files_copied = 10;
        report.stats.bytes_copied = 1024;
        report.warnings.push("файл пропущен <по правам>".to_string());
        report.errors.push("net".to_string());

        assert!(!report.is_success());

        let json = report.to_json().expect("json");
        assert!(json.contains("test-id"));
        assert!(json.contains("\"files_copied\": 10"));

        let html = report.to_html();
        assert!(html.contains("test-id"));
        assert!(html.contains("файл пропущен &lt;по правам&gt;"));
        assert!(html.contains("с ошибками"));
        assert!(html.contains("Документы"));
    }

    #[test]
    fn test_save_reports() {
        let dir = tempdir().expect("tempdir");
        let report = MigrationReport::new(
            "save-test",
            crate::config::MigrationMode::Restore,
            "a",
            "b",
        );
        assert!(report.is_success());

        let json_path = report.save_json(dir.path()).expect("json");
        assert!(json_path.exists());
        let html_path = report.save_html(dir.path()).expect("html");
        assert!(html_path.exists());
        assert!(std::fs::read_to_string(html_path)
            .expect("read")
            .contains("save-test"));
    }
}

