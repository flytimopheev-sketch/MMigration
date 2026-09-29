//! Разрешение конфликтов файлов при восстановлении профиля.

use std::path::{Path, PathBuf};

use crate::error::{MigrationError, Result};
use crate::file_transfer::TransferItem;
use crate::security;

/// Стратегия разрешения конфликтов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStrategy {
    /// Пропустить файл
    Skip,
    /// Заменить файл на целевой системе
    Replace,
    /// Сохранить оба файла (целевой получает суффикс)
    KeepBoth,
    /// Переименовать старый файл и записать новый
    RenameOld,
    /// Заменять только если файл в архиве новее
    Newer,
    /// Спрашивать пользователя
    Ask,
}

impl ConflictStrategy {
    /// Ключ стратегии для CLI.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::Replace => "replace",
            Self::KeepBoth => "keep-both",
            Self::RenameOld => "rename",
            Self::Newer => "newer",
            Self::Ask => "ask",
        }
    }

    /// Разбор стратегии из строки.
    pub fn from_key(key: &str) -> std::result::Result<Self, String> {
        match key.trim().to_lowercase().replace('_', "-").as_str() {
            "skip" | "пропустить" => Ok(Self::Skip),
            "replace" | "заменить" => Ok(Self::Replace),
            "keep-both" | "both" | "сохранить-оба" => Ok(Self::KeepBoth),
            "rename" | "rename-old" | "переименовать" => Ok(Self::RenameOld),
            "newer" | "новее" => Ok(Self::Newer),
            "ask" | "спросить" => Ok(Self::Ask),
            other => Err(format!("неизвестная стратегия конфликтов: {}", other)),
        }
    }

    /// Описание стратегии для интерфейса.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Skip => "Оставить существующий файл без изменений",
            Self::Replace => "Заменить существующий файл данными из архива",
            Self::KeepBoth => "Сохранить оба файла (существующий получит суффикс)",
            Self::RenameOld => "Переименовать существующий файл в .old и записать новый",
            Self::Newer => "Заменять только если данные в архиве новее",
            Self::Ask => "Спрашивать решение для каждого файла",
        }
    }

    /// Все стратегии для интерфейса.
    pub fn all() -> Vec<Self> {
        vec![
            Self::Ask,
            Self::Skip,
            Self::Replace,
            Self::KeepBoth,
            Self::RenameOld,
            Self::Newer,
        ]
    }
}

/// Причина конфликта.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Файлы идентичны
    Identical,
    /// Данные в архиве новее
    SourceNewer,
    /// Файл на целевой системе новее
    TargetNewer,
    /// Файлы различаются
    Different,
}

impl ConflictKind {
    /// Понятное описание для интерфейса.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Identical => "файлы идентичны",
            Self::SourceNewer => "данные в архиве новее",
            Self::TargetNewer => "файл на компьютере новее",
            Self::Different => "файлы различаются",
        }
    }
}

/// Информация о конфликте.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConflictInfo {
    /// Путь на целевой системе
    pub target_path: PathBuf,
    /// Относительный путь в архиве
    pub relative_path: PathBuf,
    /// Тип конфликта
    pub kind: ConflictKind,
    /// Размер данных в архиве
    pub source_size: u64,
    /// Размер файла на диске
    pub target_size: u64,
    /// Решение, принятое пользователем
    pub decision: Option<ConflictStrategy>,
}

impl ConflictInfo {
    /// Текстовое описание конфликта.
    pub fn describe(&self) -> String {
        format!(
            "{} — {} (архив: {}, диск: {})",
            self.target_path.display(),
            self.kind.description(),
            human_bytes::human_bytes(self.source_size as f64),
            human_bytes::human_bytes(self.target_size as f64)
        )
    }
}

/// Итоговая статистика по конфликтам.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ConflictSummary {
    /// Всего конфликтов
    pub total: usize,
    /// Идентичные файлы
    pub identical: usize,
    /// Файлы, где данные архива новее
    pub source_newer: usize,
    /// Файлы, где данные на диске новее
    pub target_newer: usize,
    /// Различающиеся файлы
    pub different: usize,
}

/// Резольвер конфликтов: применяет выбранную стратегию к набору файлов.
pub struct ConflictResolver {
    strategy: ConflictStrategy,
    apply_to_all: Option<ConflictStrategy>,
    decisions: Vec<(PathBuf, ConflictStrategy)>,
    conflicts: Vec<ConflictInfo>,
    interactive: Option<Box<dyn FnMut(&ConflictInfo) -> ConflictStrategy + Send>>,
}

impl ConflictResolver {
    /// Создать резольвер с указанной стратегией.
    pub fn new(strategy: ConflictStrategy) -> Self {
        Self {
            strategy,
            apply_to_all: None,
            decisions: Vec::new(),
            conflicts: Vec::new(),
            interactive: None,
        }
    }

    /// Интерактивный обработчик (консольный или GTK-интерфейс).
    pub fn with_interactive<F>(mut self, handler: F) -> Self
    where
        F: FnMut(&ConflictInfo) -> ConflictStrategy + Send + 'static,
    {
        self.interactive = Some(Box::new(handler));
        self
    }

    /// Текущая стратегия.
    pub fn strategy(&self) -> ConflictStrategy {
        self.strategy
    }

    /// Применить одну стратегию ко всем последующим конфликтам.
    pub fn apply_to_all(&mut self, strategy: ConflictStrategy) {
        self.apply_to_all = Some(strategy);
    }

    /// Определить тип конфликта по размеру и времени изменения.
    pub fn detect(item: &TransferItem, target_root: &Path) -> Result<Option<ConflictInfo>> {
        let target_path = security::safe_join(target_root, &item.relative)?;
        if !target_path.exists() {
            return Ok(None);
        }

        let metadata = std::fs::metadata(&target_path)?;
        let target_size = metadata.len();
        let target_modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        let source_modified = std::fs::metadata(&item.source)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        let source_newer = match (source_modified, target_modified) {
            (Some(source), Some(target)) => source > target,
            _ => true,
        };

        let kind = if item.size == target_size {
            ConflictKind::Identical
        } else if source_newer {
            ConflictKind::SourceNewer
        } else {
            ConflictKind::TargetNewer
        };

        Ok(Some(ConflictInfo {
            target_path,
            relative_path: item.relative.clone(),
            kind,
            source_size: item.size,
            target_size,
            decision: None,
        }))
    }

    /// Принять решение по конфликту с учётом стратегии и ответов пользователя.
    pub fn resolve(&mut self, mut conflict: ConflictInfo) -> Result<ConflictStrategy> {
        if let Some((_, decision)) = self
            .decisions
            .iter()
            .find(|(path, _)| path == &conflict.relative_path)
        {
            let decision = *decision;
            conflict.decision = Some(decision);
            self.conflicts.push(conflict);
            return Ok(decision);
        }

        if let Some(strategy) = self.apply_to_all {
            conflict.decision = Some(strategy);
            self.conflicts.push(conflict);
            return Ok(strategy);
        }

        let decision = match self.strategy {
            ConflictStrategy::Ask => match self.interactive.as_mut() {
                Some(handler) => handler(&conflict),
                // Без обработчика безопасное поведение — пропуск файла
                None => ConflictStrategy::Skip,
            },
            ConflictStrategy::Newer => match conflict.kind {
                ConflictKind::SourceNewer => ConflictStrategy::Replace,
                _ => ConflictStrategy::Skip,
            },
            ConflictStrategy::Replace => match conflict.kind {
                ConflictKind::Identical => ConflictStrategy::Skip,
                _ => ConflictStrategy::Replace,
            },
            other => other,
        };

        conflict.decision = Some(decision);
        self.conflicts.push(conflict);
        Ok(decision)
    }

    /// Запомнить решение пользователя для конкретного пути.
    pub fn remember(&mut self, relative_path: impl Into<PathBuf>, strategy: ConflictStrategy) {
        self.decisions.push((relative_path.into(), strategy));
    }

    /// Все обнаруженные конфликты.
    pub fn conflicts(&self) -> &[ConflictInfo] {
        &self.conflicts
    }

    /// Статистика по конфликтам.
    pub fn summary(&self) -> ConflictSummary {
        let mut summary = ConflictSummary {
            total: self.conflicts.len(),
            ..ConflictSummary::default()
        };

        for conflict in &self.conflicts {
            match conflict.kind {
                ConflictKind::Identical => summary.identical += 1,
                ConflictKind::SourceNewer => summary.source_newer += 1,
                ConflictKind::TargetNewer => summary.target_newer += 1,
                ConflictKind::Different => summary.different += 1,
            }
        }

        summary
    }
}

/// Имя файла для сохранения обеих версий (`file.conflict-20260131.txt`).
pub fn keep_both_name(path: &Path, stamp: &str) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    path.with_file_name(format!("{}.conflict-{}{}", stem, stamp, extension))
}

/// Имя файла для сохранения старой версии (`file.old`).
pub fn renamed_old_name(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let extension = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    path.with_file_name(format!("{}.old{}", stem, extension))
}

/// Требует ли стратегия действий с уже существующим файлом.
pub fn strategy_replaces_existing(strategy: ConflictStrategy) -> bool {
    matches!(
        strategy,
        ConflictStrategy::Replace | ConflictStrategy::KeepBoth | ConflictStrategy::RenameOld
    )
}

/// Убедиться, что путь конфликта не выходит за пределы каталога восстановления.
pub fn ensure_conflict_path_inside(root: &Path, path: &Path) -> Result<()> {
    if security::is_within(root, path) {
        Ok(())
    } else {
        Err(MigrationError::traversal(path))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn item(source: &Path, relative: &str, size: u64) -> TransferItem {
        TransferItem::file(source, PathBuf::from(relative), size)
    }

    fn conflict(relative: &str, kind: ConflictKind, source_size: u64, target_size: u64) -> ConflictInfo {
        ConflictInfo {
            target_path: PathBuf::from("/tmp").join(relative),
            relative_path: PathBuf::from(relative),
            kind,
            source_size,
            target_size,
            decision: None,
        }
    }

    #[test]
    fn test_strategy_keys_round_trip() {
        for strategy in ConflictStrategy::all() {
            assert_eq!(
                ConflictStrategy::from_key(strategy.key()).expect("parse"),
                strategy
            );
            assert!(!strategy.description().is_empty());
        }
        assert!(ConflictStrategy::from_key("неведомая").is_err());
    }

    #[test]
    fn test_keep_both_and_renamed_old_names() {
        let path = PathBuf::from("/home/user/file.txt");
        assert_eq!(
            keep_both_name(&path, "20260131"),
            PathBuf::from("/home/user/file.conflict-20260131.txt")
        );
        assert_eq!(
            renamed_old_name(&path),
            PathBuf::from("/home/user/file.old.txt")
        );
        assert_eq!(
            renamed_old_name(Path::new("/home/user/README")),
            PathBuf::from("/home/user/README.old")
        );
    }

    #[test]
    fn test_detect_no_conflict_when_target_missing() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        std::fs::write(&source, b"data").expect("write");

        let target_root = dir.path().join("target");
        std::fs::create_dir_all(&target_root).expect("mkdir");

        let conflict =
            ConflictResolver::detect(&item(&source, "file.txt", 4), &target_root).expect("detect");
        assert!(conflict.is_none());
    }

    #[test]
    fn test_detect_identical_and_different() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        let target_root = dir.path().join("target");
        std::fs::create_dir_all(&target_root).expect("mkdir");
        std::fs::write(&source, b"same").expect("write");
        std::fs::write(target_root.join("same.txt"), b"same").expect("write");
        std::fs::write(target_root.join("other.txt"), b"12345678").expect("write");

        let identical =
            ConflictResolver::detect(&item(&source, "same.txt", 4), &target_root).expect("detect");
        assert_eq!(identical.expect("conflict").kind, ConflictKind::Identical);

        let different =
            ConflictResolver::detect(&item(&source, "other.txt", 4), &target_root).expect("detect");
        assert_ne!(different.expect("conflict").kind, ConflictKind::Identical);
    }

    #[test]
    fn test_detect_rejects_path_traversal() {
        let dir = tempdir().expect("tempdir");
        let source = dir.path().join("source.txt");
        std::fs::write(&source, b"data").expect("write");

        let result = ConflictResolver::detect(&item(&source, "../escape.txt", 4), dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_replace_skips_identical() {
        let mut resolver = ConflictResolver::new(ConflictStrategy::Replace);
        let decision = resolver
            .resolve(conflict("file.txt", ConflictKind::Identical, 4, 4))
            .expect("resolve");
        assert_eq!(decision, ConflictStrategy::Skip);

        let decision = resolver
            .resolve(conflict("other.txt", ConflictKind::Different, 4, 8))
            .expect("resolve");
        assert_eq!(decision, ConflictStrategy::Replace);
        assert_eq!(resolver.summary().total, 2);
    }

    #[test]
    fn test_resolve_newer_strategy() {
        let mut resolver = ConflictResolver::new(ConflictStrategy::Newer);
        assert_eq!(
            resolver
                .resolve(conflict("file.txt", ConflictKind::SourceNewer, 10, 4))
                .expect("resolve"),
            ConflictStrategy::Replace
        );
        assert_eq!(
            resolver
                .resolve(conflict("other.txt", ConflictKind::TargetNewer, 4, 10))
                .expect("resolve"),
            ConflictStrategy::Skip
        );
        assert_eq!(resolver.summary().source_newer, 1);
        assert_eq!(resolver.summary().target_newer, 1);
    }

    #[test]
    fn test_resolve_ask_uses_interactive_handler_and_apply_to_all() {
        let mut resolver = ConflictResolver::new(ConflictStrategy::Ask)
            .with_interactive(|_conflict| ConflictStrategy::KeepBoth);

        assert_eq!(
            resolver
                .resolve(conflict("file.txt", ConflictKind::Different, 1, 2))
                .expect("resolve"),
            ConflictStrategy::KeepBoth
        );

        resolver.apply_to_all(ConflictStrategy::Skip);
        assert_eq!(
            resolver
                .resolve(conflict("second.txt", ConflictKind::Different, 1, 2))
                .expect("resolve"),
            ConflictStrategy::Skip
        );
    }

    #[test]
    fn test_resolve_ask_without_handler_skips() {
        let mut resolver = ConflictResolver::new(ConflictStrategy::Ask);
        assert_eq!(
            resolver
                .resolve(conflict("file.txt", ConflictKind::Different, 1, 2))
                .expect("resolve"),
            ConflictStrategy::Skip
        );
    }

    #[test]
    fn test_remembered_decision_wins() {
        let mut resolver = ConflictResolver::new(ConflictStrategy::Ask);
        resolver.remember("file.txt", ConflictStrategy::RenameOld);

        assert_eq!(
            resolver
                .resolve(conflict("file.txt", ConflictKind::Different, 1, 2))
                .expect("resolve"),
            ConflictStrategy::RenameOld
        );
    }

    #[test]
    fn test_helpers_and_descriptions() {
        assert!(strategy_replaces_existing(ConflictStrategy::Replace));
        assert!(!strategy_replaces_existing(ConflictStrategy::Skip));

        let root = PathBuf::from("/home/user");
        assert!(ensure_conflict_path_inside(&root, Path::new("/home/user/file.txt")).is_ok());
        assert!(ensure_conflict_path_inside(&root, Path::new("/etc/passwd")).is_err());

        let info = conflict("file.txt", ConflictKind::TargetNewer, 100, 200);
        assert!(info.describe().contains("file.txt"));
        assert!(info.describe().contains("новее"));
        assert_eq!(ConflictKind::Different.description(), "файлы различаются");
    }
}

