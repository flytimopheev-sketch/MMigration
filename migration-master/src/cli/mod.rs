//! Migration Master - Мастер миграции пользователя для РЕД ОС Linux
//!
//! CLI интерфейс

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "migration-master")]
#[command(author = "Migration Master Team")]
#[command(version = "0.1.0")]
#[command(about = "Мастер миграции пользователя для РЕД ОС Linux", long_about = None)]
pub struct Cli {
    /// Уровень логирования (debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    pub verbose: String,

    /// Путь к файлу конфигурации
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Сканирование текущего профиля пользователя
    Scan {
        /// Вывод в формате JSON
        #[arg(long)]
        json: bool,

        /// Компоненты для сканирования (через запятую)
        #[arg(short, long)]
        components: Option<String>,

        /// Быстрая оценка без детального сканирования
        #[arg(long)]
        quick: bool,
    },

    /// Создание зашифрованного архива профиля
    CreateArchive {
        /// Путь к выходному файлу
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Пароль для шифрования
        #[arg(short, long)]
        password: Option<String>,

        /// Интерактивный ввод пароля
        #[arg(long)]
        interactive: bool,

        /// Компоненты для включения (через запятую)
        #[arg(short, long)]
        components: Option<String>,

        /// Уровень сжатия (0-22)
        #[arg(short, long, default_value = "3")]
        compression: i32,

        /// Dry-run (без создания файла)
        #[arg(long)]
        dry_run: bool,
    },

    /// Проверка содержимого архива
    InspectArchive {
        /// Путь к архиву
        archive: PathBuf,

        /// Вывод в формате JSON
        #[arg(long)]
        json: bool,

        /// Показать подробную информацию
        #[arg(short, long)]
        verbose: bool,
    },

    /// Восстановление из архива
    Restore {
        /// Путь к архиву
        archive: PathBuf,

        /// Пароль для расшифрования
        #[arg(short, long)]
        password: Option<String>,

        /// Компоненты для восстановления (через запятую)
        #[arg(long)]
        components: Option<String>,

        /// Целевая директория (по умолчанию домашняя)
        #[arg(short, long)]
        target: Option<PathBuf>,

        /// Dry-run (без внесения изменений)
        #[arg(long)]
        dry_run: bool,

        /// Стратегия обработки конфликтов (skip, replace, rename, ask)
        #[arg(long, default_value = "ask")]
        conflict_strategy: String,

        /// Создать резервную копию перед восстановлением
        #[arg(long)]
        backup: bool,
    },

    /// Прямая миграция по SSH
    MigrateSsh {
        /// Хост источника (user@hostname или IP)
        host: String,

        /// Порт SSH
        #[arg(short, long, default_value = "22")]
        port: u16,

        /// Путь к SSH ключу
        #[arg(short, long)]
        identity: Option<PathBuf>,

        /// Компоненты для переноса (через запятую)
        #[arg(short, long)]
        components: Option<String>,

        /// Dry-run (без передачи данных)
        #[arg(long)]
        dry_run: bool,

        /// Ограничение скорости (байт/сек)
        #[arg(long)]
        rate_limit: Option<u64>,

        /// Использовать сжатие
        #[arg(long)]
        compress: bool,
    },

    /// Проверка контрольных сумм
    Verify {
        /// Путь к файлу или архиву
        path: PathBuf,

        /// Ожидаемый хеш
        #[arg(short, long)]
        expected: Option<String>,

        /// Алгоритм хеширования (sha256)
        #[arg(long, default_value = "sha256")]
        algorithm: String,
    },

    /// Список установленных пакетов
    ListPackages {
        /// Вывод в формате JSON
        #[arg(long)]
        json: bool,

        /// Фильтр по категории
        #[arg(short, long)]
        category: Option<String>,

        /// Только пользовательские пакеты
        #[arg(long)]
        user_only: bool,
    },

    /// Список принтеров
    ListPrinters {
        /// Вывод в формате JSON
        #[arg(long)]
        json: bool,

        /// Подробная информация
        #[arg(short, long)]
        verbose: bool,
    },

    /// Генерация отчёта о последней операции
    Report {
        /// ID миграции
        #[arg(short, long)]
        id: Option<String>,

        /// Формат вывода (text, json, html)
        #[arg(long, default_value = "text")]
        format: String,

        /// Путь к файлу отчёта
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Управление настройками
    Config {
        /// Действие (get, set, list, reset)
        action: String,

        /// Ключ настройки
        key: Option<String>,

        /// Значение настройки
        value: Option<String>,
    },

    /// Запуск GUI интерфейса
    Gui {
        /// Тёмная тема
        #[arg(long)]
        dark_theme: bool,
    },
}

/// Парсинг аргументов командной строки
pub fn parse_args() -> Cli {
    Cli::parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_scan() {
        let cli = Cli::parse_from(["migration-master", "scan", "--json"]);
        match cli.command {
            Commands::Scan { json, .. } => assert!(json),
            _ => panic!("Неверная команда"),
        }
    }

    #[test]
    fn test_cli_create_archive() {
        let cli = Cli::parse_from([
            "migration-master",
            "create-archive",
            "-o",
            "backup.rmm",
            "--compression",
            "5",
        ]);
        match cli.command {
            Commands::CreateArchive { output, compression, .. } => {
                assert_eq!(output, Some(PathBuf::from("backup.rmm")));
                assert_eq!(compression, 5);
            }
            _ => panic!("Неверная команда"),
        }
    }

    #[test]
    fn test_cli_restore() {
        let cli = Cli::parse_from([
            "migration-master",
            "restore",
            "backup.rmm",
            "--dry-run",
        ]);
        match cli.command {
            Commands::Restore { archive, dry_run, .. } => {
                assert_eq!(archive, PathBuf::from("backup.rmm"));
                assert!(dry_run);
            }
            _ => panic!("Неверная команда"),
        }
    }
}
