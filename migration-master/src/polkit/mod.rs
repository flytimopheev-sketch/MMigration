//! Политики привилегий (PolicyKit): белый список действий и проверка окружения.

use std::process::{Command, Stdio};

use crate::error::Result;

/// Действия, которые разрешены привилегированному помощнику.
pub const ALLOWED_ACTIONS: &[&str] = &[
    "com.redos.migration-master.helper.install-packages",
    "com.redos.migration-master.helper.configure-printers",
    "com.redos.migration-master.helper.apply-system-settings",
];

/// Менеджер привилегированных операций.
pub struct PolkitManager;

impl PolkitManager {
    /// Действие разрешено белым списком.
    pub fn is_action_allowed(action: &str) -> bool {
        ALLOWED_ACTIONS.contains(&action) && action.starts_with("com.redos.migration-master.")
    }

    /// Утилита `pkexec` доступна.
    pub fn pkexec_available() -> bool {
        Command::new("pkexec")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Политика агента авторизации запущена (по имени процесса).
    pub fn agent_running() -> bool {
        Command::new("pgrep")
            .args([
                "-f",
                "polkit-gnome-authentication-agent|polkit-kde-authentication-agent|lxqt-policykit",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// XML файла политики для установки в `/usr/share/polkit-1/actions`.
    pub fn render_policy() -> String {
        let actions = ALLOWED_ACTIONS
            .iter()
            .map(|action| {
                format!(
                    "      <action id=\"{action}\">\n        \
                     <description>Migration Master: {action}</description>\n        \
                     <message>Требуются права администратора</message>\n        \
                     <defaults><allow_active>auth_admin</allow_active>\
                     <allow_inactive>auth_admin</allow_inactive></defaults>\n      </action>",
                    action = action
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<policyconfig xmlns=\"http://www.freedesktop.org/Standards/PolicyKit/1\">\n\
  <vendor>Migration Master</vendor>\n\
  <vendor_url>https://github.com/flytimopheev-sketch/MMigration</vendor_url>\n\
{actions}\n\
</policyconfig>\n"
        )
    }

    /// Команда установки файла политики (выполняется через pkexec).
    pub fn install_policy_command(source: &Path) -> Vec<String> {
        vec![
            "pkexec".to_string(),
            "install".to_string(),
            "-m".to_string(),
            "644".to_string(),
            source.to_string_lossy().to_string(),
            "/usr/share/polkit-1/actions/com.redos.migration-master.policy".to_string(),
        ]
    }

    /// Проверить, что окружение готово к привилегированным операциям.
    pub fn check_environment() -> Result<Vec<String>> {
        let mut notes = Vec::new();

        if !Self::pkexec_available() {
            notes.push("pkexec не найден — привилегированные операции недоступны".to_string());
        }
        if !Self::agent_running() {
            notes.push("агент авторизации PolicyKit не запущен".to_string());
        }

        Ok(notes)
    }
}

use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_whitelist() {
        assert!(PolkitManager::is_action_allowed(
            "com.redos.migration-master.helper.install-packages"
        ));
        assert!(!PolkitManager::is_action_allowed(
            "com.redos.migration-master.helper.fmt-disk"
        ));
        assert!(!PolkitManager::is_action_allowed("org.freedesktop.policykit.exec"));

        let unique: std::collections::HashSet<_> = ALLOWED_ACTIONS.iter().collect();
        assert_eq!(unique.len(), ALLOWED_ACTIONS.len());
    }

    #[test]
    fn test_render_policy_contains_actions() {
        let xml = PolkitManager::render_policy();
        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("<policyconfig"));
        for action in ALLOWED_ACTIONS {
            assert!(xml.contains(&format!("id=\"{}\"", action)));
        }
    }

    #[test]
    fn test_install_policy_command() {
        let command = PolkitManager::install_policy_command(Path::new("/tmp/x.policy"));
        assert_eq!(command[0], "pkexec");
        assert!(command
            .iter()
            .any(|arg| arg.ends_with("com.redos.migration-master.policy")));
    }
}

