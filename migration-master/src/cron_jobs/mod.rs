//! Пользовательские cron-задачи: выгрузка и восстановление `crontab`.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::error::Result;

/// Работа с crontab текущего пользователя.
pub struct CronJobs;

impl CronJobs {
    /// Прочитать crontab текущего пользователя.
    ///
    /// Отсутствие crontab (код 1 у `crontab -l`) не считается ошибкой:
    /// возвращается пустая строка. Если утилита не установлена — `Ok(None)`.
    pub fn collect() -> Result<Option<String>> {
        let output = match Command::new("crontab")
            .arg("-l")
            .stdin(Stdio::null())
            .output()
        {
            Ok(output) => output,
            Err(_) => return Ok(None),
        };

        if output.status.success() {
            Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
        } else {
            // `crontab -l` пишет "no crontab for ..." и возвращает 1.
            Ok(Some(String::new()))
        }
    }

    /// Заменить crontab текущего пользователя.
    pub fn restore(dump: &str) -> Result<()> {
        if dump.trim().is_empty() {
            // Пустой crontab лучше не устанавливать автоматически: это удалит
            // текущие задачи. Пропускаем, чтобы не уничтожить данные.
            return Ok(());
        }

        let mut child = Command::new("crontab")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| crate::error::MigrationError::missing_tool("crontab"))?;

        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(dump.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(crate::error::MigrationError::Command {
                command: "crontab".to_string(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_crontab_is_not_restored() {
        // Функция должна завершиться успешно, не вызывая crontab.
        assert!(CronJobs::restore("").is_ok());
        assert!(CronJobs::restore("   \n").is_ok());
    }
}
