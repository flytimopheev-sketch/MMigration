//! Отмена длительных операций (копирование, архивирование, SSH-передача).
//!
//! `CancelToken` — разделяемый флаг отмены. UI (кнопка «Отмена») или обработчик
//! Ctrl+C вызывают [`CancelToken::cancel`], а рабочие циклы периодически
//! проверяют токен через [`CancelToken::check`] и прерываются с ошибкой
//! [`MigrationError::Cancelled`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use crate::error::{MigrationError, Result};

/// Разделяемый токен отмены длительной операции.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// Создать новый неотменённый токен.
    pub fn new() -> Self {
        Self::default()
    }

    /// Отменить операцию.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Сброшен ли флаг отмены.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Вернуть `Err(Cancelled)`, если операция отменена.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(MigrationError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Лёгкая копия для UI (`Send + Sync + 'static`).
    pub fn handle(&self) -> CancelHandle {
        CancelHandle {
            cancelled: self.cancelled.clone(),
        }
    }
}

/// Ручка отмены для интерфейса.
#[derive(Debug, Clone)]
pub struct CancelHandle {
    cancelled: Arc<AtomicBool>,
}

impl CancelHandle {
    /// Отменить операцию, за которой закреплена ручка.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Состояние флага.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

static GLOBAL_TOKEN: OnceLock<CancelToken> = OnceLock::new();

/// Глобальный токен процесса (отменяется обработчиком Ctrl+C).
pub fn global() -> CancelToken {
    GLOBAL_TOKEN.get_or_init(CancelToken::new).clone()
}

/// Установить обработчик Ctrl+C (SIGINT/SIGTERM), отменяющий глобальный токен.
///
/// Вызывается один раз из точки входа. Обработчик не прерывает процесс
/// мгновенно: рабочие циклы сами проверяют [`global`] и корректно завершают
/// начатые операции (удаляют `.part` файлы, закрывают архивы).
pub fn install_ctrl_c_handler() {
    imp::install(global().handle());
}

#[cfg(unix)]
mod imp {
    use super::CancelHandle;
    use std::sync::Once;

    static INSTALL: Once = Once::new();

    unsafe extern "C" fn on_signal(_signum: i32) {
        // Обработчик асинхронно-безопасный: только атомарная запись флага.
        super::global().cancel();
    }

    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;

    #[link(name = "c")]
    extern "C" {
        fn signal(signum: i32, handler: Option<unsafe extern "C" fn(i32)>) -> usize;
    }

    /// Регистрирует обработчики SIGINT/SIGTERM (без внешних зависимостей).
    pub fn install(_handle: CancelHandle) {
        INSTALL.call_once(|| unsafe {
            signal(SIGINT, Some(on_signal));
            signal(SIGTERM, Some(on_signal));
        });
    }
}

#[cfg(windows)]
mod imp {
    use super::CancelHandle;
    use std::sync::Once;

    static INSTALL: Once = Once::new();

    unsafe extern "system" fn on_ctrl_event(_ctrl_type: u32) -> i32 {
        super::global().cancel();
        // Не завершаем процесс — даём циклам корректно отмениться.
        0
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    /// Регистрирует обработчик Console Ctrl+C (без внешних зависимостей).
    pub fn install(_handle: CancelHandle) {
        INSTALL.call_once(|| unsafe {
            SetConsoleCtrlHandler(Some(on_ctrl_event), 1);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_starts_not_cancelled() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        assert!(token.check().is_ok());
    }

    #[test]
    fn test_handle_cancels_token() {
        let token = CancelToken::new();
        let handle = token.handle();
        assert!(!handle.is_cancelled());

        handle.cancel();
        assert!(handle.is_cancelled());
        assert!(token.is_cancelled());

        let error = token.check().expect_err("cancelled");
        assert!(error.is_cancelled());
    }

    #[test]
    fn test_clone_shares_state() {
        let token = CancelToken::new();
        let clone = token.clone();
        clone.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_ctrl_c_handler_installs_and_is_idempotent() {
        install_ctrl_c_handler();
        install_ctrl_c_handler();
        assert!(!global().is_cancelled());
    }
}
