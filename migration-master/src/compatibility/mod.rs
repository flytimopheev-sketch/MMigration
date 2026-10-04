//! Проверка совместимости источника и целевой системы.

use crate::error::Result;
use crate::platform::{self, OsRelease};

/// Результат проверки совместимости.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompatibilityReport {
    /// ОС источника
    pub source_os: OsRelease,
    /// ОС цели
    pub target_os: OsRelease,
    /// Архитектура источника
    pub source_arch: String,
    /// Архитектура цели
    pub target_arch: String,
    /// Архитектуры совместимы
    pub arch_compatible: bool,
    /// Взаимодействие ОС
    pub os_compatible: bool,
    /// Предупреждения
    pub warnings: Vec<String>,
    /// Несовместимости (блокирующие проблемы)
    pub blockers: Vec<String>,
    /// Итоговая оценка риска
    pub risk: crate::config::RiskLevel,
}

impl Default for CompatibilityReport {
    fn default() -> Self {
        Self {
            source_os: OsRelease::default(),
            target_os: OsRelease::default(),
            source_arch: String::new(),
            target_arch: String::new(),
            arch_compatible: true,
            os_compatible: true,
            warnings: Vec::new(),
            blockers: Vec::new(),
            risk: crate::config::RiskLevel::Low,
        }
    }
}

impl CompatibilityReport {
    /// Можно ли продолжать миграцию.
    pub fn can_proceed(&self) -> bool {
        self.blockers.is_empty()
    }

    /// Краткое описание для отчёта.
    pub fn summary(&self) -> String {
        format!(
            "{} {} -> {} {}; предупреждений: {}, блокеров: {}",
            self.source_os.pretty_name,
            self.source_arch,
            self.target_os.pretty_name,
            self.target_arch,
            self.warnings.len(),
            self.blockers.len()
        )
    }
}

/// Метаданные источника, сохранённые в архиве.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SourceMetadata {
    /// ОС источника
    pub os_info: Option<crate::archive::OsInfo>,
    /// Пользователь источника
    pub user: Option<String>,
}

/// Проверить совместимость метаданных источника с текущей системой.
pub fn check(source: &SourceMetadata) -> Result<CompatibilityReport> {
    check_against(source, &platform::os_release(), &platform::architecture())
}

/// Проверить совместимость с заданной целевой системой (для тестов и планирования).
pub fn check_against(
    source: &SourceMetadata,
    target_os: &platform::OsRelease,
    target_arch: &str,
) -> Result<CompatibilityReport> {
    let mut report = CompatibilityReport {
        target_os: target_os.clone(),
        target_arch: target_arch.to_string(),
        ..CompatibilityReport::default()
    };
    report.source_arch = report.target_arch.clone();

    if let Some(source_info) = &source.os_info {
        report.source_os = platform::OsRelease {
            id: source_info.name.to_lowercase().replace(' ', ""),
            pretty_name: source_info.name.clone(),
            version: source_info.version.clone(),
        };
        report.source_arch = source_info.architecture.clone();

        report.arch_compatible = arch_compatible(&report.source_arch, &report.target_arch);
        if !report.arch_compatible {
            report.blockers.push(format!(
                "несовместимые архитектуры: {} -> {}",
                report.source_arch, report.target_arch
            ));
        }

        report.os_compatible = report.source_os.id == report.target_os.id
            || compatible_family(&report.source_os, &report.target_os);

        if !report.os_compatible {
            report.warnings.push(format!(
                "разные дистрибутивы: {} -> {}",
                report.source_os.pretty_name, report.target_os.pretty_name
            ));
        }

        if version_number(&report.source_os.version) > version_number(&report.target_os.version) {
            report.warnings.push(
                "целевая система старше источника: возможны проблемы с настройками".to_string(),
            );
        }
    } else {
        report
            .warnings
            .push("метаданные источника недоступны — проверка выполнена частично".to_string());
    }

    report.risk = if !report.blockers.is_empty() {
        crate::config::RiskLevel::Critical
    } else if report.warnings.len() > 2 {
        crate::config::RiskLevel::High
    } else if !report.warnings.is_empty() {
        crate::config::RiskLevel::Medium
    } else {
        crate::config::RiskLevel::Low
    };

    Ok(report)
}

fn arch_compatible(source: &str, target: &str) -> bool {
    if source == target {
        return true;
    }
    matches!(
        (source, target),
        ("x86_64", "amd64") | ("amd64", "x86_64") | ("aarch64", "arm64") | ("arm64", "aarch64")
    )
}

fn compatible_family(
    source: &crate::platform::OsRelease,
    target: &crate::platform::OsRelease,
) -> bool {
    let family = |release: &crate::platform::OsRelease| -> &'static str {
        let id = release.id.to_lowercase().replace([' ', '-'], "");
        if ["redos", "rhel", "centos", "fedora", "ol"]
            .iter()
            .any(|prefix| id.starts_with(prefix))
        {
            "rhel"
        } else if id.starts_with("debian") || id.starts_with("ubuntu") {
            "debian"
        } else {
            "other"
        }
    };
    family(source) == family(target)
}

fn version_number(version: &str) -> u64 {
    version
        .split('.')
        .next()
        .and_then(|part| part.parse::<u64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_info(name: &str, version: &str, arch: &str) -> crate::archive::OsInfo {
        crate::archive::OsInfo {
            name: name.to_string(),
            version: version.to_string(),
            architecture: arch.to_string(),
        }
    }

    fn target_release() -> crate::platform::OsRelease {
        crate::platform::OsRelease {
            id: "redos".to_string(),
            pretty_name: "RED OS 9.3".to_string(),
            version: "9.3".to_string(),
        }
    }

    #[test]
    fn test_same_os_is_low_risk() {
        let source = SourceMetadata {
            os_info: Some(os_info("redos", "9.3", "x86_64")),
            user: Some("ivan".into()),
        };
        let report = check_against(&source, &target_release(), "x86_64").expect("check");
        assert!(report.can_proceed());
        assert!(report.arch_compatible);
        assert_eq!(report.risk, crate::config::RiskLevel::Low);
        assert!(report.summary().contains("x86_64"));
    }

    #[test]
    fn test_arch_mismatch_blocks() {
        let source = SourceMetadata {
            os_info: Some(os_info("redos", "9.3", "aarch64")),
            user: None,
        };
        let mut report = check_against(&source, &target_release(), "x86_64").expect("check");
        assert!(!report.can_proceed());
        assert!(!report.arch_compatible);
        assert_eq!(report.risk, crate::config::RiskLevel::Critical);

        // Ошибки понятны пользователю
        assert!(report.blockers[0].contains("aarch64"));
        report.blockers.clear();
        assert!(report.can_proceed());
    }

    #[test]
    fn test_missing_metadata_warns() {
        let report =
            check_against(&SourceMetadata::default(), &target_release(), "x86_64").expect("check");
        assert!(report.can_proceed());
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn test_older_target_warns() {
        let source = SourceMetadata {
            os_info: Some(os_info("redos", "10.0", "x86_64")),
            user: None,
        };
        let report = check_against(&source, &target_release(), "x86_64").expect("check");
        assert!(report.can_proceed());
        assert!(report.warnings.iter().any(|w| w.contains("старше")));
    }
}
