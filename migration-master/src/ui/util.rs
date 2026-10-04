//! Платформонезависимые помощники интерфейса (тестируются без GTK).
//!
//! Эти функции используются GTK-мастером (`ui::gtk_app`), но не зависят от
//! библиотек GUI, поэтому их можно проверять на любой платформе.

use crate::conflict_resolver::ConflictStrategy;

/// Подписи стратегий разрешения конфликтов (§11).
pub const STRATEGY_LABELS: [&str; 6] = [
    "Спрашивать для каждого файла",
    "Пропустить существующие",
    "Заменить существующие",
    "Сохранить оба файла",
    "Переименовать старый в .old",
    "Только если в архиве новее",
];

/// Ключи стратегий (см. `ConflictStrategy::from_key`).
pub const STRATEGY_KEYS: [&str; 6] = ["ask", "skip", "replace", "keep-both", "rename", "newer"];

/// Выбранная стратегия конфликтов по индексу выпадающего списка.
pub fn conflict_strategy(index: u32) -> ConflictStrategy {
    STRATEGY_KEYS
        .get(index as usize)
        .and_then(|key| ConflictStrategy::from_key(key).ok())
        .unwrap_or(ConflictStrategy::Ask)
}

/// Разобрать строку подключения `[user@]host[:port]` (§9).
///
/// Возвращает `(пользователь, хост, порт)`. Пользователь может быть пустым,
/// порт по умолчанию — `22`.
pub fn parse_ssh_target(spec: &str) -> Option<(String, String, u16)> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let (user, rest) = match spec.split_once('@') {
        Some((user, rest)) => (user.to_string(), rest.to_string()),
        None => (String::new(), spec.to_string()),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, value)) => match value.parse::<u16>() {
            Ok(port) => (host.to_string(), port),
            Err(_) => (rest.clone(), 22),
        },
        None => (rest, 22),
    };
    let host = host.trim().to_string();
    if host.is_empty() {
        return None;
    }
    Some((user, host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssh_target_full() {
        let parsed = parse_ssh_target("user@192.168.1.10:2222").expect("разбор");
        assert_eq!(parsed.0, "user");
        assert_eq!(parsed.1, "192.168.1.10");
        assert_eq!(parsed.2, 2222);
    }

    #[test]
    fn test_parse_ssh_target_defaults() {
        assert_eq!(
            parse_ssh_target("host.local").expect("разбор"),
            (String::new(), "host.local".to_string(), 22)
        );
        assert_eq!(
            parse_ssh_target("user@host").expect("разбор"),
            ("user".to_string(), "host".to_string(), 22)
        );
    }

    #[test]
    fn test_parse_ssh_target_invalid() {
        assert!(parse_ssh_target("").is_none());
        assert!(parse_ssh_target("   ").is_none());
        assert!(parse_ssh_target("user@").is_none());
        // Нечисловой порт трактуется как часть хоста с портом по умолчанию.
        assert_eq!(
            parse_ssh_target("host:notaport").expect("разбор"),
            (String::new(), "host:notaport".to_string(), 22)
        );
    }

    #[test]
    fn test_conflict_strategy_mapping() {
        assert_eq!(conflict_strategy(0), ConflictStrategy::Ask);
        assert_eq!(conflict_strategy(1), ConflictStrategy::Skip);
        assert_eq!(conflict_strategy(2), ConflictStrategy::Replace);
        assert_eq!(conflict_strategy(3), ConflictStrategy::KeepBoth);
        assert_eq!(conflict_strategy(4), ConflictStrategy::RenameOld);
        assert_eq!(conflict_strategy(5), ConflictStrategy::Newer);
        // Выход за границы — безопасное значение по умолчанию.
        assert_eq!(conflict_strategy(99), ConflictStrategy::Ask);
    }

    #[test]
    fn test_strategy_tables_are_consistent() {
        assert_eq!(STRATEGY_LABELS.len(), STRATEGY_KEYS.len());
        for key in STRATEGY_KEYS {
            assert!(
                ConflictStrategy::from_key(key).is_ok(),
                "неизвестный ключ стратегии: {}",
                key
            );
        }
    }
}
