//! GTK4-интерфейс (мастер переноса). Требует фичу `gui`.

use crate::error::{MigrationError, Result};
use crate::wizard::{MigrationWizard, WizardStage};

/// Доступен ли графический интерфейс в этой сборке.
pub fn gui_available() -> bool {
    cfg!(feature = "gui")
}

/// Запустить графический мастер переноса.
pub fn run_gui(_wizard: &MigrationWizard) -> Result<WizardStage> {
    #[cfg(feature = "gui")]
    {
        // TODO(gtk): подключить страницы мастера к wizard API.
        Err(MigrationError::Unsupported(
            "GTK4-мастер пока не подключён; используйте консольный режим".to_string(),
        ))
    }

    #[cfg(not(feature = "gui"))]
    {
        Err(MigrationError::Unsupported(
            "графический интерфейс не включён (соберите с фичей `gui`)".to_string(),
        ))
    }
}
