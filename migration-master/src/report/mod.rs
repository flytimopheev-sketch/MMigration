//! Отчёты о миграции: JSON, HTML, текстовый файл и PDF.

use std::path::{Path, PathBuf};

use crate::error::{MigrationError, Result};

/// Формат экспорта отчёта (требование §17).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportFormat {
    /// Структурированный JSON
    #[default]
    Json,
    /// Страница HTML
    Html,
    /// Простой текстовый файл
    #[serde(rename = "txt")]
    Text,
    /// PDF (через системный конвертер, если доступен)
    Pdf,
}

impl ReportFormat {
    /// Ключ формата для CLI.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html => "html",
            Self::Text => "txt",
            Self::Pdf => "pdf",
        }
    }

    /// Разбор формата из строки.
    pub fn from_key(key: &str) -> std::result::Result<Self, String> {
        match key.trim().to_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "html" | "htm" => Ok(Self::Html),
            "txt" | "text" => Ok(Self::Text),
            "pdf" => Ok(Self::Pdf),
            other => Err(format!(
                "неизвестный формат отчёта '{}'; доступны: json, html, txt, pdf",
                other
            )),
        }
    }

    /// Все форматы для интерфейса.
    pub fn all() -> Vec<Self> {
        vec![Self::Json, Self::Html, Self::Text, Self::Pdf]
    }
}

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
    /// Перенесённые файлы (относительные пути)
    #[serde(default)]
    pub transferred_files: Vec<String>,
    /// Пропущенные файлы с причинами (`путь: причина`)
    #[serde(default)]
    pub skipped_files: Vec<String>,
    /// Разрешённые конфликты (описания)
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// Восстановленные пакеты
    #[serde(default)]
    pub installed_packages: Vec<String>,
    /// Восстановленные принтеры
    #[serde(default)]
    pub restored_printers: Vec<String>,
    /// Рекомендации пользователю
    #[serde(default)]
    pub recommendations: Vec<String>,
    /// Путь созданной резервной копии
    #[serde(default)]
    pub backup_path: Option<String>,
    /// Проверка совместимости (краткое описание)
    #[serde(default)]
    pub compatibility: Option<String>,
    /// Отменена ли операция пользователем
    #[serde(default)]
    pub cancelled: bool,
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
            transferred_files: Vec::new(),
            skipped_files: Vec::new(),
            conflicts: Vec::new(),
            installed_packages: Vec::new(),
            restored_printers: Vec::new(),
            recommendations: Vec::new(),
            backup_path: None,
            compatibility: None,
            cancelled: false,
        }
    }

    /// Отчёт успешен (нет ошибок).
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }

    /// Итоговый статус для интерфейса и отчётов.
    pub fn status(&self) -> &'static str {
        if self.cancelled {
            "отменено"
        } else if self.is_success() {
            "успешно"
        } else {
            "с ошибками"
        }
    }

    /// Добавить перенесённый файл (для детальных отчётов).
    pub fn record_transferred(&mut self, relative: impl Into<String>) {
        self.transferred_files.push(relative.into());
    }

    /// Добавить пропущенный файл с указанием причины.
    pub fn record_skipped(&mut self, path: impl Into<String>, reason: impl Into<String>) {
        self.skipped_files
            .push(format!("{}: {}", path.into(), reason.into()));
    }

    /// Добавить рекомендацию (без дублей).
    pub fn add_recommendation(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.recommendations.contains(&text) {
            self.recommendations.push(text);
        }
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
        let extra = self.extra_html();
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
{extra}<h2>Предупреждения</h2><ul>{warnings}</ul>\n\
<h2>Ошибки</h2><ul>{errors}</ul>\n\
</body></html>\n",
            id = escape_html(&self.id),
            created_at = escape_html(&self.created_at),
            source = escape_html(&self.source),
            target = escape_html(&self.target),
            status = self.status(),
            files = self.stats.files_copied,
            bytes = self.stats.bytes_copied,
            conflicts = self.stats.conflicts_resolved,
            skipped = self.stats.files_skipped,
            packages = self.stats.packages,
            printers = self.stats.printers,
            duration = self.stats.duration_ms,
            warnings = if warnings.is_empty() {
                "<li>нет</li>"
            } else {
                &warnings
            },
            errors = if errors.is_empty() {
                "<li>нет</li>"
            } else {
                &errors
            },
            components = if components.is_empty() {
                "<li>нет</li>"
            } else {
                &components
            },
            extra = extra,
        )
    }

    /// Дополнительные разделы HTML-отчёта (детали переноса, §17).
    fn extra_html(&self) -> String {
        let mut sections: Vec<(String, Vec<String>)> = Vec::new();

        if !self.transferred_files.is_empty() {
            sections.push((
                format!("Перенесено файлов ({})", self.transferred_files.len()),
                self.transferred_files.clone(),
            ));
        }
        if !self.skipped_files.is_empty() {
            sections.push((
                format!("Пропущено ({})", self.skipped_files.len()),
                self.skipped_files.clone(),
            ));
        }
        if !self.conflicts.is_empty() {
            sections.push((
                format!("Конфликты ({})", self.conflicts.len()),
                self.conflicts.clone(),
            ));
        }
        if !self.installed_packages.is_empty() {
            sections.push((
                format!("Пакеты ({})", self.installed_packages.len()),
                self.installed_packages.clone(),
            ));
        }
        if !self.restored_printers.is_empty() {
            sections.push((
                format!("Принтеры ({})", self.restored_printers.len()),
                self.restored_printers.clone(),
            ));
        }
        if !self.recommendations.is_empty() {
            sections.push(("Рекомендации".to_string(), self.recommendations.clone()));
        }

        let mut details = String::new();
        if let Some(backup) = &self.backup_path {
            details.push_str(&format!(
                "<p>Резервная копия: {}</p>\n",
                escape_html(backup)
            ));
        }
        if let Some(compatibility) = &self.compatibility {
            details.push_str(&format!(
                "<p>Совместимость: {}</p>\n",
                escape_html(compatibility)
            ));
        }

        let sections_html = sections
            .into_iter()
            .map(|(title, items)| {
                let list = items
                    .iter()
                    .map(|item| format!("<li>{}</li>", escape_html(item)))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("<h2>{}</h2><ul>{}</ul>\n", escape_html(&title), list)
            })
            .collect::<String>();

        format!("{}{}", details, sections_html)
    }

    /// Сохранить HTML-отчёт, вернуть путь.
    pub fn save_html(&self, directory: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("report-{}.html", self.id));
        std::fs::write(&path, self.to_html())?;
        Ok(path)
    }

    /// Текстовое представление отчёта (§17).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("ОТЧЕТ О МИГРАЦИИ\n================\n");
        out.push_str(&format!("Операция: {}\n", self.id));
        out.push_str(&format!("Дата: {}\n", self.created_at));
        out.push_str(&format!("Источник: {}\n", self.source));
        out.push_str(&format!("Назначение: {}\n", self.target));
        out.push_str(&format!("Статус: {}\n\n", self.status()));

        out.push_str("СТАТИСТИКА\n");
        out.push_str(&format!(
            "  Файлов перенесено: {}\n",
            self.stats.files_copied
        ));
        out.push_str(&format!(
            "  Байт перенесено: {}\n",
            human_bytes::human_bytes(self.stats.bytes_copied as f64)
        ));
        out.push_str(&format!("  Пропущено: {}\n", self.stats.files_skipped));
        out.push_str(&format!(
            "  Конфликтов разрешено: {}\n",
            self.stats.conflicts_resolved
        ));
        out.push_str(&format!("  Пакетов: {}\n", self.stats.packages));
        out.push_str(&format!("  Принтеров: {}\n", self.stats.printers));
        out.push_str(&format!(
            "  Длительность, мс: {}\n\n",
            self.stats.duration_ms
        ));

        out.push_str("КОМПОНЕНТЫ\n");
        if self.components.is_empty() {
            out.push_str("  нет\n");
        }
        for component in &self.components {
            out.push_str(&format!("  - {}\n", component.description()));
        }
        out.push('\n');

        if let Some(backup) = &self.backup_path {
            out.push_str(&format!("Резервная копия: {}\n\n", backup));
        }
        if let Some(compatibility) = &self.compatibility {
            out.push_str(&format!("Совместимость: {}\n\n", compatibility));
        }

        write_section(&mut out, "ПЕРЕНЕСЁННЫЕ ФАЙЛЫ", &self.transferred_files);
        write_section(&mut out, "ПРОПУЩЕНО", &self.skipped_files);
        write_section(&mut out, "КОНФЛИКТЫ", &self.conflicts);
        write_section(&mut out, "ПАКЕТЫ", &self.installed_packages);
        write_section(&mut out, "ПРИНТЕРЫ", &self.restored_printers);
        write_section(&mut out, "ПРЕДУПРЕЖДЕНИЯ", &self.warnings);
        write_section(&mut out, "ОШИБКИ", &self.errors);
        write_section(&mut out, "РЕКОМЕНДАЦИИ", &self.recommendations);
        out
    }

    /// Сохранить текстовый отчёт, вернуть путь.
    pub fn save_text(&self, directory: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("report-{}.txt", self.id));
        std::fs::write(&path, self.to_text())?;
        Ok(path)
    }

    /// Сохранить PDF, конвертируя HTML внешним инструментом
    /// (`libreoffice --headless`, `soffice --headless` или `pandoc`).
    ///
    /// Генерировать PDF «напрямую» не стал: встроенный генератор не смог бы
    /// корректно отрисовать кириллицу без встраивания TTF-шрифта. Если
    /// конвертер недоступен — возвращается `Unsupported` с подсказкой, а
    /// HTML-отчёт сохраняется в любом случае.
    pub fn save_pdf(&self, directory: &Path) -> Result<PathBuf> {
        let html = self.save_html(directory)?;
        let path = directory.join(format!("report-{}.pdf", self.id));

        let converter = ["libreoffice", "soffice", "pandoc"]
            .into_iter()
            .find(|tool| command_available(tool))
            .ok_or_else(|| {
                MigrationError::Unsupported(format!(
                    "для PDF нужен libreoffice, soffice или pandoc; HTML-отчёт сохранён: {}",
                    html.display()
                ))
            })?;

        let status = if converter == "pandoc" {
            std::process::Command::new(converter)
                .arg("--from=html")
                .arg(html.as_os_str())
                .arg("--output")
                .arg(&path)
                .status()
        } else {
            std::process::Command::new(converter)
                .args([
                    "--headless",
                    "--norestore",
                    "--convert-to",
                    "pdf",
                    "--outdir",
                ])
                .arg(directory)
                .arg(&html)
                .status()
        }
        .map_err(|error| MigrationError::Unsupported(format!("{}: {}", converter, error)))?;

        if !status.success() || !path.exists() {
            return Err(MigrationError::Unsupported(format!(
                "{} не создал PDF; HTML-отчёт сохранён: {}",
                converter,
                html.display()
            )));
        }

        Ok(path)
    }

    /// Сохранить отчёт в указанном формате.
    pub fn save(&self, directory: &Path, format: ReportFormat) -> Result<PathBuf> {
        match format {
            ReportFormat::Json => self.save_json(directory),
            ReportFormat::Html => self.save_html(directory),
            ReportFormat::Text => self.save_text(directory),
            ReportFormat::Pdf => self.save_pdf(directory),
        }
    }

    /// Сохранить отчёт во всех доступных форматах (PDF — если есть конвертер).
    pub fn save_all(&self, directory: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for format in [ReportFormat::Json, ReportFormat::Html, ReportFormat::Text] {
            if let Ok(path) = self.save(directory, format) {
                paths.push(path);
            }
        }
        if let Ok(path) = self.save_pdf(directory) {
            paths.push(path);
        }
        paths
    }
}

/// Раздел текстового отчёта.
fn write_section(output: &mut String, title: &str, items: &[String]) {
    output.push_str(title);
    output.push('\n');
    if items.is_empty() {
        output.push_str("  нет\n");
    }
    for item in items {
        output.push_str(&format!("  - {}\n", item));
    }
    output.push('\n');
}

/// Доступна ли команда в PATH (без запуска полезной работы).
fn command_available(command: &str) -> bool {
    which::which(command).is_ok()
}

/// Экранирование HTML-спецсимволов.
pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Текстовый отчёт по истории миграций (§17).
pub fn render_history_text(records: &[crate::database::MigrationRecord]) -> String {
    let mut out = String::new();
    out.push_str("ОТЧЁТ ОБ ИСТОРИИ МИГРАЦИЙ\n=========================\n");
    out.push_str(&format!("Операций: {}\n\n", records.len()));

    for record in records {
        let finished = record
            .finished_at
            .map(|date| date.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "—".to_string());
        out.push_str(&format!(
            "{} | {:?} | {:?} | файлов: {} | байт: {} | {} -> {}\n",
            finished,
            record.operation_type,
            record.status,
            record.transferred_files,
            record.transferred_size,
            record.source,
            record.target
        ));
    }
    out
}

/// HTML-отчёт по истории миграций (§17).
pub fn render_history_html(records: &[crate::database::MigrationRecord]) -> String {
    let mut rows = String::new();
    for record in records {
        let finished = record
            .finished_at
            .map(|date| date.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "—".to_string());
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{:?}</td><td>{:?}</td><td>{}</td><td>{}</td><td>{} → {}</td></tr>\n",
            escape_html(&finished),
            record.operation_type,
            record.status,
            record.transferred_files,
            record.transferred_size,
            escape_html(&record.source),
            escape_html(&record.target)
        ));
    }

    format!(
        "<!DOCTYPE html>\n<html lang=\"ru\"><head><meta charset=\"utf-8\">\
         <title>История миграций</title></head><body>\
         <h1>История миграций</h1><p>Операций: {}</p>\
         <table border=\"1\" cellpadding=\"6\" cellspacing=\"0\">\
         <thead><tr><th>Дата</th><th>Тип</th><th>Статус</th><th>Файлов</th><th>Байт</th>\
         <th>Источник → Цель</th></tr></thead>\
         <tbody>{rows}</tbody></table></body></html>\n",
        records.len()
    )
}

/// JSON-отчёт по истории миграций (§17).
pub fn render_history_json(records: &[crate::database::MigrationRecord]) -> Result<String> {
    Ok(serde_json::to_string_pretty(records)?)
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
        report
            .warnings
            .push("файл пропущен <по правам>".to_string());
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
    fn test_report_format_keys() {
        for format in ReportFormat::all() {
            assert_eq!(ReportFormat::from_key(format.key()).expect("key"), format);
        }
        assert_eq!(
            ReportFormat::from_key("TEXT").expect("alias"),
            ReportFormat::Text
        );
        assert!(ReportFormat::from_key("docx").is_err());
    }

    #[test]
    fn test_report_text_sections() {
        let mut report = MigrationReport::new(
            "text-test",
            crate::config::MigrationMode::Restore,
            "old-pc",
            "new-pc",
        );
        report.components = vec![crate::config::ComponentType::Documents];
        report.stats.files_copied = 3;
        report.stats.bytes_copied = 2048;
        report.record_transferred("Documents/otchet.odt");
        report.record_skipped("Documents/staroe.odt", "уже существует");
        report
            .conflicts
            .push("Documents/a.odt — заменён".to_string());
        report.installed_packages.push("libreoffice".to_string());
        report.restored_printers.push("HP-LaserJet".to_string());
        report.add_recommendation("Проверьте принтер после входа".to_string());
        report.add_recommendation("Проверьте принтер после входа".to_string());
        report.backup_path = Some("/backup/pre.tar".to_string());
        report.compatibility = Some("RED OS 8 -> RED OS 9".to_string());

        let text = report.to_text();
        assert!(text.contains("ОТЧЕТ О МИГРАЦИИ"));
        assert!(text.contains("text-test"));
        assert!(text.contains("ПЕРЕНЕСЁННЫЕ ФАЙЛЫ"));
        assert!(text.contains("Documents/otchet.odt"));
        assert!(text.contains("Documents/staroe.odt: уже существует"));
        assert!(text.contains("HP-LaserJet"));
        assert!(text.contains("Резервная копия: /backup/pre.tar"));
        assert!(text.contains("Совместимость: RED OS 8 -> RED OS 9"));
        // Рекомендации без дублей
        assert_eq!(report.recommendations.len(), 1);
        assert_eq!(report.status(), "успешно");

        let html = report.to_html();
        assert!(html.contains("Перенесено файлов (1)"));
        assert!(html.contains("Рекомендации"));
    }

    #[test]
    fn test_cancelled_status_and_saved_formats() {
        let dir = tempdir().expect("tempdir");
        let mut report = MigrationReport::new(
            "cancel-test",
            crate::config::MigrationMode::LocalArchive,
            "a",
            "b",
        );
        report.cancelled = true;
        assert_eq!(report.status(), "отменено");
        assert!(report.to_text().contains("Статус: отменено"));

        let path = report
            .save(dir.path(), ReportFormat::Text)
            .expect("save text");
        assert!(path.exists());

        let paths = report.save_all(dir.path());
        assert!(paths.len() >= 3, "ожидались json/html/txt: {:?}", paths);
    }

    #[test]
    fn test_report_json_round_trip_with_new_fields() {
        let mut report = MigrationReport::new(
            "serde-test",
            crate::config::MigrationMode::Restore,
            "a",
            "b",
        );
        report.record_transferred("Documents/x.txt");
        let json = report.to_json().expect("json");
        let back: MigrationReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.transferred_files, vec!["Documents/x.txt".to_string()]);

        // Старый отчёт без новых полей читается (serde default):
        // удаляем поле из JSON-объекта, не полагаясь на форматирование вывода.
        let mut value: serde_json::Value = serde_json::from_str(&json).expect("json value");
        value
            .as_object_mut()
            .expect("object")
            .remove("transferred_files");
        let legacy: MigrationReport = serde_json::from_value(value).expect("legacy deserialize");
        assert!(legacy.transferred_files.is_empty());
        assert!(!legacy.cancelled);
    }

    #[test]
    fn test_save_reports() {
        let dir = tempdir().expect("tempdir");
        let report =
            MigrationReport::new("save-test", crate::config::MigrationMode::Restore, "a", "b");
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
