//! Графический интерфейс (GTK4 + libadwaita) и данные главного экрана.
//!
//! GTK-код собирается только для Linux с включённой фичей `gui`. На остальных
//! платформах и без фичи модуль предоставляет данные главного экрана
//! (`main_screen`) и возвращает понятную ошибку при попытке запуска GUI.

pub mod main_screen;
pub mod util;

#[cfg(all(feature = "gui", target_os = "linux"))]
mod gtk_app;

use crate::error::Result;

/// Доступен ли графический интерфейс в этой сборке.
pub fn gui_available() -> bool {
    cfg!(all(feature = "gui", target_os = "linux"))
}

/// Запустить графический мастер переноса.
///
/// На РЕД ОС (Linux) проект собирается с фичей `gui` (`cargo build --features gui`);
/// в этом случае открывается окно мастера. Иначе возвращается понятная ошибка
/// с подсказкой использовать консольный режим.
pub fn run_gui() -> Result<()> {
    #[cfg(all(feature = "gui", target_os = "linux"))]
    {
        gtk_app::run()
    }

    #[cfg(not(all(feature = "gui", target_os = "linux")))]
    {
        Err(crate::error::MigrationError::Unsupported(
            "графический интерфейс недоступен: соберите проект на Linux с фичей `gui` \
             (cargo build --features gui) или используйте консольный режим"
                .to_string(),
        ))
    }
}

