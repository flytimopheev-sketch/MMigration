//! Migration Master - Мастер миграции пользователя для РЕД ОС Linux
//!
//! Главный исполняемый файл CLI

use migration_master::cli::{Cli, Commands, parse_args};
use migration_master::profile_scanner::ProfileScanner;
use migration_master::error::Result;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = parse_args();

    // Настройка логирования
    init_logging(&cli.verbose);

    match run_command(cli) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Ошибка: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn init_logging(verbose: &str) {
    let env_filter = format!(
        "migration_master={},info,warn,error",
        verbose.to_lowercase()
    );
    
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .init();
}

fn run_command(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Scan { json, components, quick } => {
            cmd_scan(json, components, quick)
        }
        Commands::CreateArchive { output, password, interactive, components, compression, dry_run } => {
            cmd_create_archive(output, password, interactive, components, compression, dry_run)
        }
        Commands::InspectArchive { archive, json, verbose } => {
            cmd_inspect_archive(archive, json, verbose)
        }
        Commands::Restore { archive, password, components, target, dry_run, conflict_strategy, backup } => {
            cmd_restore(archive, password, components, target, dry_run, conflict_strategy, backup)
        }
        Commands::MigrateSsh { host, port, identity, components, dry_run, rate_limit, compress } => {
            cmd_migrate_ssh(host, port, identity, components, dry_run, rate_limit, compress)
        }
        Commands::Verify { path, expected, algorithm } => {
            cmd_verify(path, expected, algorithm)
        }
        Commands::ListPackages { json, category, user_only } => {
            cmd_list_packages(json, category, user_only)
        }
        Commands::ListPrinters { json, verbose } => {
            cmd_list_printers(json, verbose)
        }
        Commands::Report { id, format, output } => {
            cmd_report(id, format, output)
        }
        Commands::Config { action, key, value } => {
            cmd_config(action, key, value)
        }
        Commands::Gui { dark_theme } => {
            #[cfg(feature = "gui")]
            return cmd_gui(dark_theme);
            
            #[cfg(not(feature = "gui"))]
            {
                println!("GUI не скомпилирован. Используйте --features gui");
                Ok(())
            }
        }
    }
}

/// Сканирование профиля
fn cmd_scan(json: bool, components: Option<String>, quick: bool) -> Result<()> {
    let scanner = ProfileScanner::new()?;
    
    if quick {
        let (files, size) = scanner.quick_estimate()?;
        if json {
            println!(r#"{{"files": {}, "size": {} }}"#, files, size);
        } else {
            println!("Файлов: {}", files);
            println!("Размер: {}", human_bytes::human_bytes(size as f64));
        }
    } else {
        let result = scanner.scan()?;
        
        if json {
            println!("{}", serde_json::to_string_pretty(&result)?);
        } else {
            println!("=== Результат сканирования ===");
            println!("Пользователь: {}", result.username);
            println!("Домашняя директория: {}", result.home_dir.display());
            println!("Всего файлов: {}", result.total_files);
            println!("Общий размер: {}", result.total_size_human());
            
            if !result.errors.is_empty() {
                println!("\nОшибки:");
                for error in &result.errors {
                    println!("  - {}", error);
                }
            }
        }
    }
    
    Ok(())
}

/// Создание архива
fn cmd_create_archive(
    output: Option<std::path::PathBuf>,
    password: Option<String>,
    interactive: bool,
    components: Option<String>,
    compression: i32,
    dry_run: bool,
) -> Result<()> {
    println!("Создание архива...");
    
    if dry_run {
        println!("[DRY-RUN] Архив не будет создан");
        println!("Путь: {:?}", output.unwrap_or_else(|| std::path::PathBuf::from("profile.rmm")));
        println!("Сжатие: {}", compression);
        return Ok(());
    }
    
    // TODO: Реализация создания архива
    println!("Функция в разработке...");
    Ok(())
}

/// Проверка архива
fn cmd_inspect_archive(
    archive: std::path::PathBuf,
    json: bool,
    verbose: bool,
) -> Result<()> {
    if !archive.exists() {
        return Err(migration_master::error::MigrationError::FileNotFound(
            format!("Архив не найден: {}", archive.display())
        ));
    }
    
    println!("Проверка архива: {}", archive.display());
    // TODO: Реализация проверки архива
    Ok(())
}

/// Восстановление из архива
fn cmd_restore(
    archive: std::path::PathBuf,
    password: Option<String>,
    components: Option<String>,
    target: Option<std::path::PathBuf>,
    dry_run: bool,
    conflict_strategy: String,
    backup: bool,
) -> Result<()> {
    if dry_run {
        println!("[DRY-RUN] Восстановление не будет выполнено");
        println!("Архив: {}", archive.display());
        println!("Стратегия конфликтов: {}", conflict_strategy);
        return Ok(());
    }
    
    println!("Восстановление из архива...");
    // TODO: Реализация восстановления
    Ok(())
}

/// SSH миграция
fn cmd_migrate_ssh(
    host: String,
    port: u16,
    identity: Option<std::path::PathBuf>,
    components: Option<String>,
    dry_run: bool,
    rate_limit: Option<u64>,
    compress: bool,
) -> Result<()> {
    if dry_run {
        println!("[DRY-RUN] SSH миграция не будет выполнена");
        println!("Хост: {}:{}", host, port);
        return Ok(());
    }
    
    println!("Подключение к {}:{}...", host, port);
    // TODO: Реализация SSH миграции
    Ok(())
}

/// Проверка хеша
fn cmd_verify(
    path: std::path::PathBuf,
    expected: Option<String>,
    algorithm: String,
) -> Result<()> {
    use migration_master::security::compute_sha256;
    
    if !path.exists() {
        return Err(migration_master::error::MigrationError::FileNotFound(
            format!("Файл не найден: {}", path.display())
        ));
    }
    
    let result = compute_sha256(&path)?;
    
    println!("Файл: {}", path.display());
    println!("SHA-256: {}", result.sha256);
    println!("Размер: {} байт", result.size);
    
    if let Some(expected_hash) = expected {
        if result.sha256 == expected_hash {
            println!("✓ Хеш совпадает");
        } else {
            println!("✗ Хеш не совпадает!");
            println!("Ожидался: {}", expected_hash);
            return Err(migration_master::error::MigrationError::ChecksumMismatch);
        }
    }
    
    Ok(())
}

/// Список пакетов
fn cmd_list_packages(json: bool, category: Option<String>, user_only: bool) -> Result<()> {
    println!("Список пакетов...");
    // TODO: Реализация получения списка пакетов RPM
    Ok(())
}

/// Список принтеров
fn cmd_list_printers(json: bool, verbose: bool) -> Result<()> {
    println!("Список принтеров...");
    // TODO: Реализация получения списка принтеров CUPS
    Ok(())
}

/// Отчёт
fn cmd_report(
    id: Option<String>,
    format: String,
    output: Option<std::path::PathBuf>,
) -> Result<()> {
    println!("Генерация отчёта...");
    // TODO: Реализация генерации отчёта
    Ok(())
}

/// Конфигурация
fn cmd_config(action: String, key: Option<String>, value: Option<String>) -> Result<()> {
    use migration_master::config::{load_config, save_config, AppConfig};
    
    match action.as_str() {
        "list" => {
            let config = load_config().unwrap_or_default();
            println!("{:#?}", config);
        }
        "get" => {
            if let Some(k) = key {
                println!("Ключ: {}", k);
            }
        }
        "set" => {
            if let (Some(k), Some(v)) = (key, value) {
                println!("Установка {} = {}", k, v);
            }
        }
        "reset" => {
            let config = AppConfig::default();
            save_config(&config).map_err(|e| MigrationError::Generic(e.to_string()))?;
            println!("Конфигурация сброшена");
        }
        _ => {
            println!("Неизвестное действие: {}", action);
        }
    }
    
    Ok(())
}

/// GUI интерфейс
#[cfg(feature = "gui")]
fn cmd_gui(dark_theme: bool) -> Result<()> {
    println!("Запуск GUI...");
    // TODO: Реализация GUI
    Ok(())
}
