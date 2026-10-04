//! Приложения: `.desktop`-запускатели, правила миграции (§5) и импорт/экспорт.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use yaml_rust2::{Yaml, YamlLoader};

use crate::error::{MigrationError, Result};

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

/// Правило миграции настроек приложения (§5).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppRule {
    /// Название приложения
    pub app_name: String,
    /// Каталоги/файлы конфигурации (относительно HOME, допускается префикс `~/`)
    #[serde(default)]
    pub config_paths: Vec<String>,
    /// Каталоги/файлы данных приложения
    #[serde(default)]
    pub data_paths: Vec<String>,
    /// Способ переноса: `copy` | `merge` | `skip`
    #[serde(default = "default_transfer_mode")]
    pub transfer_mode: String,
    /// Требуется перезапуск приложения после миграции
    #[serde(default)]
    pub requires_restart: bool,
    /// Минимальная совместимая версия приложения
    #[serde(default)]
    pub min_version: Option<String>,
    /// Предупреждения для пользователя
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Команда post-migration hook (выполняется только с подтверждением)
    #[serde(default)]
    pub post_migration_hook: Option<String>,
    /// Правило включено
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_transfer_mode() -> String {
    "copy".to_string()
}

fn default_enabled() -> bool {
    true
}

impl AppRule {
    /// Правило с обязательными полями.
    pub fn new(app_name: impl Into<String>, config_paths: &[&str], data_paths: &[&str]) -> Self {
        Self {
            app_name: app_name.into(),
            config_paths: config_paths.iter().map(|s| s.to_string()).collect(),
            data_paths: data_paths.iter().map(|s| s.to_string()).collect(),
            transfer_mode: default_transfer_mode(),
            requires_restart: true,
            min_version: None,
            warnings: Vec::new(),
            post_migration_hook: None,
            enabled: true,
        }
    }

    /// Добавить предупреждение (для вызова из конструктора встроенных правил).
    fn with_warnings(mut self, warnings: &[&str]) -> Self {
        self.warnings = warnings.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Все пути правила относительно HOME (без префикса `~/`).
    pub fn relative_paths(&self) -> Vec<String> {
        self.config_paths
            .iter()
            .chain(self.data_paths.iter())
            .map(|path| {
                path.strip_prefix("~/")
                    .or_else(|| path.strip_prefix('/'))
                    .unwrap_or(path)
                    .to_string()
            })
            .collect()
    }

    /// Суммарный размер данных правила в профиле пользователя (байты).
    pub fn size_in(&self, home: &Path) -> u64 {
        let mut total = 0u64;
        for relative in self.relative_paths() {
            let path = home.join(&relative);
            if path.is_file() {
                total += std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            } else if path.is_dir() {
                for entry in walkdir::WalkDir::new(&path).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        total += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                    }
                }
            }
        }
        total
    }

    /// Относится ли относительный путь (от HOME) к этому правилу.
    ///
    /// Считается совпадение самого пути (`~/.gitconfig`) и вложенных элементов
    /// (`~/.config/chromium/Default/...`).
    pub fn matches_relative(&self, relative: &str) -> bool {
        let relative = relative.replace('\\', "/");
        self.relative_paths().iter().any(|path| {
            let path = path.trim_end_matches('/');
            !path.is_empty() && (relative == path || relative.starts_with(&format!("{}/", path)))
        })
    }
}

/// Встроенные правила миграции для типовых приложений РЕД ОС (§5).
pub fn builtin_rules() -> Vec<AppRule> {
    vec![
        AppRule::new(
            "Firefox",
            &[".mozilla/firefox"],
            &[".cache/mozilla/firefox"],
        )
        .with_warnings(&[
            "кэш браузера не переносится автоматически",
            "профиль может содержать сохранённые пароли",
        ]),
        AppRule::new(
            "Chromium/Chrome",
            &[".config/chromium", ".config/google-chrome"],
            &[],
        )
        .with_warnings(&["секреты браузера переносятся только с отдельным подтверждением"]),
        AppRule::new("Thunderbird", &[".thunderbird"], &[]),
        AppRule::new("LibreOffice", &[".config/libreoffice"], &[]),
        AppRule::new(
            "VS Code / Code-OSS",
            &[".config/Code", ".config/Code - OSS", ".vscode"],
            &[],
        ),
        AppRule::new("Git", &[".gitconfig", ".config/git"], &[]),
        AppRule::new(
            "Терминал",
            &[
                ".bashrc",
                ".bash_profile",
                ".zshrc",
                ".config/fish",
                ".config/alacritty",
                ".config/kitty",
            ],
            &[],
        ),
        AppRule::new("SSH", &[".ssh/config", ".ssh/known_hosts"], &[])
            .with_warnings(&["приватные ключи переносятся только по явному согласию"]),
        AppRule::new("systemd (пользователь)", &[".config/systemd/user"], &[]),
        AppRule::new(
            "Файловые менеджеры",
            &[
                ".config/nautilus",
                ".local/share/nautilus",
                ".config/dolphinrc",
                ".config/Thunar",
                ".config/nemo",
            ],
            &[],
        ),
    ]
}

/// Найти встроенное правило по имени приложения (без учёта регистра).
pub fn find_builtin(app_name: &str) -> Option<AppRule> {
    let needle = app_name.to_lowercase();
    builtin_rules()
        .into_iter()
        .find(|rule| rule.app_name.to_lowercase().contains(&needle))
}

/// Путь к пользовательскому файлу правил миграции приложений (§5).
pub fn default_rules_path() -> PathBuf {
    crate::config::get_config_dir().join("app-rules.yaml")
}

/// Загрузить действующие правила: пользовательский YAML-файл или встроенные (§5).
///
/// Пустой или повреждённый пользовательский файл не ломает работу — берутся
/// встроенные правила.
pub fn load_effective_rules() -> Vec<AppRule> {
    let path = default_rules_path();
    if path.is_file() {
        if let Ok(rules) = load_rules_file(&path) {
            if !rules.is_empty() {
                return rules;
            }
        }
    }
    builtin_rules()
}

/// Сохранить правила в пользовательский YAML-файл (§5). Возвращает путь файла.
pub fn save_rules(rules: &[AppRule]) -> Result<PathBuf> {
    let path = default_rules_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, export_rules_yaml(rules))?;
    Ok(path)
}

/// Прочитать строку из YAML-значения.
fn yaml_str(node: &Yaml) -> Option<String> {
    match node {
        Yaml::String(value) => Some(value.clone()),
        Yaml::Integer(value) => Some(value.to_string()),
        Yaml::Real(value) => Some(value.clone()),
        _ => None,
    }
}

/// Прочитать булево из YAML-значения.
fn yaml_bool(node: &Yaml) -> Option<bool> {
    match node {
        Yaml::Boolean(value) => Some(*value),
        _ => None,
    }
}

/// Прочитать список строк (или одиночную строку) из YAML-значения.
fn yaml_str_list(node: &Yaml) -> Vec<String> {
    match node {
        Yaml::Array(items) => items.iter().filter_map(yaml_str).collect(),
        Yaml::String(value) => vec![value.clone()],
        _ => Vec::new(),
    }
}

/// Собрать правило из YAML-отображения.
fn rule_from_yaml(node: &Yaml) -> Result<AppRule> {
    let hash = match node {
        Yaml::Hash(hash) => hash,
        _ => {
            return Err(MigrationError::Yaml(
                "правило должно быть отображением (mapping)".to_string(),
            ))
        }
    };
    let get = |key: &str| hash.get(&Yaml::String(key.to_string()));

    let app_name = get("app_name").and_then(yaml_str).unwrap_or_default();
    if app_name.trim().is_empty() {
        return Err(MigrationError::Yaml(
            "у правила не задано поле app_name".to_string(),
        ));
    }

    Ok(AppRule {
        app_name,
        config_paths: get("config_paths").map(yaml_str_list).unwrap_or_default(),
        data_paths: get("data_paths").map(yaml_str_list).unwrap_or_default(),
        transfer_mode: get("transfer_mode")
            .and_then(yaml_str)
            .unwrap_or_else(default_transfer_mode),
        requires_restart: get("requires_restart").and_then(yaml_bool).unwrap_or(false),
        min_version: get("min_version").and_then(yaml_str),
        warnings: get("warnings").map(yaml_str_list).unwrap_or_default(),
        post_migration_hook: get("post_migration_hook").and_then(yaml_str),
        enabled: get("enabled").and_then(yaml_bool).unwrap_or(true),
    })
}

/// Загрузить правила из YAML-текста (список правил или одно правило).
pub fn load_rules_yaml(text: &str) -> Result<Vec<AppRule>> {
    let documents =
        YamlLoader::load_from_str(text).map_err(|error| MigrationError::Yaml(error.to_string()))?;

    let mut rules = Vec::new();
    for document in &documents {
        match document {
            Yaml::Array(items) => {
                for item in items {
                    rules.push(rule_from_yaml(item)?);
                }
            }
            Yaml::Hash(_) => rules.push(rule_from_yaml(document)?),
            Yaml::BadValue | Yaml::Null => {}
            _ => {
                return Err(MigrationError::Yaml(
                    "ожидался список правил или одно правило".to_string(),
                ))
            }
        }
    }
    Ok(rules)
}

/// Загрузить правила из YAML-файла.
pub fn load_rules_file(path: &Path) -> Result<Vec<AppRule>> {
    let text = std::fs::read_to_string(path).map_err(|_| MigrationError::not_found(path))?;
    load_rules_yaml(&text)
}

/// Экспортировать правила в YAML (для резервной копии и ручного редактирования).
pub fn export_rules_yaml(rules: &[AppRule]) -> String {
    let mut out = String::new();
    for rule in rules {
        out.push_str(&format!("- app_name: \"{}\"\n", rule.app_name));
        push_yaml_list(&mut out, "  config_paths", &rule.config_paths);
        push_yaml_list(&mut out, "  data_paths", &rule.data_paths);
        out.push_str(&format!("  transfer_mode: \"{}\"\n", rule.transfer_mode));
        out.push_str(&format!("  requires_restart: {}\n", rule.requires_restart));
        if let Some(version) = &rule.min_version {
            out.push_str(&format!("  min_version: \"{}\"\n", version));
        }
        push_yaml_list(&mut out, "  warnings", &rule.warnings);
        if let Some(hook) = &rule.post_migration_hook {
            out.push_str(&format!("  post_migration_hook: \"{}\"\n", hook));
        }
        out.push_str(&format!("  enabled: {}\n", rule.enabled));
    }
    out
}

fn push_yaml_list(out: &mut String, key: &str, items: &[String]) {
    out.push_str(key);
    out.push_str(":\n");
    for item in items {
        out.push_str(&format!("    - \"{}\"\n", item));
    }
}

/// Разобрать команду hook на программу и аргументы (без shell-интерпретации).
pub fn parse_hook_command(command: &str) -> Option<(String, Vec<String>)> {
    let mut parts = command.split_whitespace();
    let program = parts.next()?.to_string();
    Some((program, parts.map(str::to_string).collect()))
}

/// Выполнить post-migration hook.
///
/// Требует явного подтверждения пользователя и запускается без shell-интерполяции
/// (аргументы передаются отдельными параметрами — защита от command injection).
pub fn run_hook(rule: &AppRule, confirmed: bool) -> Result<String> {
    if !confirmed {
        return Err(MigrationError::PrivilegedRequired(format!(
            "hook приложения '{}' выполняется только после подтверждения",
            rule.app_name
        )));
    }

    let command = rule.post_migration_hook.as_deref().ok_or_else(|| {
        MigrationError::InvalidInput(format!(
            "у приложения '{}' не задан post-migration hook",
            rule.app_name
        ))
    })?;

    let (program, args) = parse_hook_command(command)
        .ok_or_else(|| MigrationError::InvalidInput("пустая команда hook".to_string()))?;

    let output = std::process::Command::new(&program)
        .args(&args)
        .output()
        .map_err(|_| MigrationError::missing_tool(&program))?;

    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    if !output.status.success() {
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    Ok(text)
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

    #[test]
    fn test_builtin_rules_cover_required_apps() {
        let rules = builtin_rules();
        assert!(
            rules.len() >= 10,
            "ожидалось >= 10 правил, есть {}",
            rules.len()
        );
        for name in ["Firefox", "Thunderbird", "LibreOffice", "Git", "SSH"] {
            assert!(
                rules.iter().any(|rule| rule.app_name.contains(name)),
                "нет встроенного правила для {name}"
            );
        }
        assert!(find_builtin("firefox").is_some());
        assert!(find_builtin("Такого приложения нет").is_none());
    }

    #[test]
    fn test_rules_yaml_round_trip() {
        let rules = builtin_rules();
        let yaml = export_rules_yaml(&rules);
        let parsed = load_rules_yaml(&yaml).expect("load");
        assert_eq!(parsed.len(), rules.len());
        assert_eq!(parsed[0].app_name, rules[0].app_name);
        assert_eq!(parsed[0].config_paths, rules[0].config_paths);
        assert_eq!(parsed[0].enabled, rules[0].enabled);
    }

    /// Пример правил из `resources/examples` должен загружаться так же, как
    /// пользовательский файл, и содержать корректные пути (§5).
    #[test]
    fn test_example_rules_file_is_valid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/examples/app-rules.yaml");
        let rules = load_rules_file(&path)
            .unwrap_or_else(|error| panic!("пример правил не загрузился: {}", error));

        assert!(!rules.is_empty(), "в примере нет правил");
        for rule in &rules {
            assert!(!rule.app_name.trim().is_empty(), "правило без имени");
            assert!(
                rule.config_paths
                    .iter()
                    .chain(rule.data_paths.iter())
                    .any(|p| { p.starts_with("~/") || p.starts_with('/') }),
                "правило '{}' не содержит путей относительно HOME",
                rule.app_name
            );
        }

        // Отключённое правило в примере присутствует и остаётся выключенным.
        let disabled = rules.iter().find(|rule| !rule.enabled);
        assert!(
            disabled.is_some(),
            "в примере нет выключенного правила — его нельзя отключить вручную"
        );
        // Hook в примере — только для правила с явным подтверждением.
        assert!(rules.iter().any(|rule| rule.post_migration_hook.is_some()));
    }

    #[test]
    fn test_load_custom_rule_from_yaml() {
        let yaml = "app_name: \"MyApp\"\n\
                    config_paths:\n  - \"~/.config/myapp\"\n\
                    data_paths:\n  - \"~/.local/share/myapp\"\n\
                    transfer_mode: \"merge\"\n\
                    requires_restart: true\n\
                    min_version: \"1.0\"\n\
                    warnings:\n  - \"не переносить кэш\"\n\
                    post_migration_hook: \"myapp-migrate-hook\"\n";

        let rules = load_rules_yaml(yaml).expect("load");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].app_name, "MyApp");
        assert_eq!(rules[0].transfer_mode, "merge");
        assert!(rules[0].requires_restart);
        assert_eq!(
            rules[0].relative_paths(),
            vec![
                ".config/myapp".to_string(),
                ".local/share/myapp".to_string()
            ]
        );
        assert_eq!(
            rules[0].post_migration_hook.as_deref(),
            Some("myapp-migrate-hook")
        );
    }

    #[test]
    fn test_rule_size_in_home() {
        let dir = tempdir().expect("tempdir");
        let config = dir.path().join(".config/myapp");
        std::fs::create_dir_all(&config).expect("mkdir");
        std::fs::write(config.join("settings.json"), b"0123456789").expect("write");

        let rule = AppRule::new("MyApp", &["~/.config/myapp"], &[]);
        assert_eq!(rule.size_in(dir.path()), 10);
    }

    #[test]
    fn test_hook_requires_confirmation_and_parsing() {
        let mut rule = AppRule::new("MyApp", &[], &[]);
        rule.post_migration_hook = Some("myapp-hook --flag value".to_string());

        // Без подтверждения hook не выполняется.
        assert!(run_hook(&rule, false).is_err());

        assert_eq!(
            parse_hook_command("myapp-hook --flag value"),
            Some((
                "myapp-hook".to_string(),
                vec!["--flag".to_string(), "value".to_string()]
            ))
        );
        assert_eq!(parse_hook_command("   "), None);
    }

    #[test]
    fn test_rule_matches_relative() {
        let rule = AppRule::new(
            "Chromium/Chrome",
            &[".config/chromium"],
            &[".cache/chromium"],
        );
        assert!(rule.matches_relative(".config/chromium"));
        assert!(rule.matches_relative(".config/chromium/Default/Preferences"));
        assert!(!rule.matches_relative(".config/chromiumx/Preferences"));
        assert!(!rule.matches_relative(".config/firefox"));
    }
}
