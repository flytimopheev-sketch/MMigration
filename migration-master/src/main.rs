//! Точка входа `migration-master`.

use clap::Parser;
use migration_master::cli::Cli;
use migration_master::{config, logging, platform};

fn main() {
    let cli = Cli::parse();

    // Логирование в файл (не фатально, если каталог недоступен)
    let log_path = config::get_data_dir().join("logs").join("migration-master.log");
    let _ = logging::init_logger(&log_path, 10);
    let _ = platform::hostname();

    match cli.run() {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("Ошибка: {}", error.user_message());
            std::process::exit(1);
        }
    }
}
