//! Графический мастер Migration Master на GTK4 + libadwaita.
//!
//! Модуль собирается только на Linux с фичей `gui`. GUI не выполняет shell-команды
//! напрямую: все операции идут через те же сервисы, что и CLI
//! (`ArchiveManager`, `MigrationWizard`, SQLite-история).

#![cfg(all(feature = "gui", target_os = "linux"))]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use adw::prelude::*;
use gtk::{glib, Orientation};

use crate::archive::{ArchiveManager, CreateArchiveOptions, RestoreOptions};
use crate::config::{self, ComponentType, MigrationMode};
use crate::database::DatabaseManager;
use crate::error::{MigrationError, Result};
use crate::file_transfer::{ProgressObserver, TransferItem};
use crate::polkit::PolkitManager;
use crate::report::render_history_text;
use crate::ssh_transfer::SshTarget;
use crate::ui::main_screen::MainScreenInfo;
use crate::ui::util::{conflict_strategy, parse_ssh_target, STRATEGY_LABELS};
use crate::wizard::{MigrationWizard, WizardConfig};

/// Запустить графический интерфейс (блокирует поток до закрытия окна).
pub fn run() -> Result<()> {
    let app = adw::Application::builder()
        .application_id(crate::APP_ID)
        .build();

    app.connect_activate(build_window);
    let _ = app.run();
    Ok(())
}

/// Домашний каталог пользователя (для значений по умолчанию).
fn default_home() -> PathBuf {
    crate::platform::home_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn build_window(app: &adw::Application) {
    let nav = adw::NavigationView::new();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Мастер миграции пользователя")
        .default_width(1000)
        .default_height(720)
        .content(&nav)
        .build();

    let home = home_page(nav.clone(), window.clone());
    let _ = nav.push(&home);
    window.present();
}

/// Контейнер содержимого страницы с отступами.
fn content_box() -> gtk::Box {
    let content = gtk::Box::new(Orientation::Vertical, 18);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content
}

/// Обернуть содержимое в страницу навигации с заголовком и прокруткой.
fn page(title: &str, content: &gtk::Box) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(content)
        .build();
    toolbar.set_content(Some(&scroller));
    adw::NavigationPage::new(&toolbar, title)
}

/// Строка «название — значение» для групп настроек.
fn info_row(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    let label = gtk::Label::new(Some(value));
    label.add_css_class("dim-label");
    label.set_selectable(true);
    label.set_wrap(true);
    label.set_xalign(1.0);
    row.add_suffix(&label);
    row
}

/// Строка с произвольным элементом управления справа (Entry, SpinButton и т.п.).
fn entry_row(title: &str, widget: &impl IsA<gtk::Widget>) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    widget.set_valign(gtk::Align::Center);
    row.add_suffix(widget);
    row.set_activatable_widget(Some(widget));
    row
}

/// Кликабельная строка списка действий.
fn action_row(title: &str, subtitle: &str, icon: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .activatable(true)
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row
}

/// Диалог с сообщением (заменяет уведомления и используется для ошибок).
#[allow(deprecated)]
fn show_message(window: &adw::ApplicationWindow, heading: &str, body: &str) {
    let dialog = adw::MessageDialog::new(Some(window), Some(heading), Some(body));
    dialog.add_response("close", "Закрыть");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.present();
}

/// Предупреждения о недоступных инструментах (§1).
fn warning_box(info: &MainScreenInfo) -> gtk::Box {
    let box_ = gtk::Box::new(Orientation::Vertical, 6);
    let warnings = info.warnings();
    if warnings.is_empty() {
        let label = gtk::Label::new(Some("Все обязательные компоненты доступны."));
        label.add_css_class("dim-label");
        label.set_xalign(0.0);
        box_.append(&label);
        return box_;
    }

    box_.add_css_class("card");
    box_.set_margin_top(6);
    box_.set_margin_bottom(6);
    box_.set_margin_start(6);
    box_.set_margin_end(6);

    let title = gtk::Label::new(Some("Внимание"));
    title.add_css_class("heading");
    title.set_xalign(0.0);
    box_.append(&title);

    for warning in warnings {
        let text = format!("• {}", warning);
        let label = gtk::Label::new(Some(text.as_str()));
        label.set_xalign(0.0);
        label.set_wrap(true);
        box_.append(&label);
    }

    box_
}

/// Главный экран (§1): сведения о системе и точки входа в сценарии.
fn home_page(nav: adw::NavigationView, window: adw::ApplicationWindow) -> adw::NavigationPage {
    let info = MainScreenInfo::collect();
    let content = content_box();

    content.append(&warning_box(&info));

    let system = adw::PreferencesGroup::builder().title("Система").build();
    system.add(&info_row("Пользователь", &info.username));
    system.add(&info_row("Компьютер", &info.hostname));
    system.add(&info_row(
        "Операционная система",
        &format!("{} {}", info.os_pretty_name, info.os_version),
    ));
    system.add(&info_row("Архитектура", &info.architecture));
    system.add(&info_row(
        "Домашний каталог",
        &info.home_path.display().to_string(),
    ));
    system.add(&info_row(
        "Размер профиля",
        &format!(
            "{} ({} файлов)",
            human_bytes::human_bytes(info.home_size_bytes as f64),
            info.home_files
        ),
    ));
    system.add(&info_row(
        "Свободно на диске",
        &human_bytes::human_bytes(info.free_bytes as f64),
    ));
    content.append(&system);

    let actions = adw::PreferencesGroup::builder().title("Действия").build();
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);

    let create = action_row(
        "Создать архив профиля",
        "Зашифрованный архив .rmm для переноса на другой компьютер",
        "document-save-symbolic",
    );
    {
        let nav = nav.clone();
        let window = window.clone();
        create.connect_activated(move |_| {
            let page = create_page(window.clone());
            let _ = nav.push(&page);
        });
    }
    list.append(&create);

    let ssh = action_row(
        "Перенести с другого компьютера",
        "Прямая миграция по SSH",
        "network-server-symbolic",
    );
    {
        let nav = nav.clone();
        let window = window.clone();
        ssh.connect_activated(move |_| {
            let page = ssh_page(window.clone());
            let _ = nav.push(&page);
        });
    }
    list.append(&ssh);

    let restore = action_row(
        "Восстановить из архива",
        "Распаковать профиль из файла .rmm",
        "document-open-symbolic",
    );
    {
        let nav = nav.clone();
        let window = window.clone();
        restore.connect_activated(move |_| {
            let page = restore_page(window.clone());
            let _ = nav.push(&page);
        });
    }
    list.append(&restore);

    let history = action_row(
        "История операций",
        "Последние миграции из локальной базы SQLite",
        "document-open-recent-symbolic",
    );
    {
        let nav = nav.clone();
        history.connect_activated(move |_| {
            let page = history_page();
            let _ = nav.push(&page);
        });
    }
    list.append(&history);

    let settings = action_row(
        "Настройки и диагностика",
        "Пути, доступные инструменты, готовность PolicyKit",
        "preferences-system-symbolic",
    );
    {
        let nav = nav.clone();
        settings.connect_activated(move |_| {
            let page = settings_page();
            let _ = nav.push(&page);
        });
    }
    list.append(&settings);

    actions.add(&list);
    content.append(&actions);

    page("Migration Master", &content)
}

/// Страница создания архива профиля (режим «Локальный архив»).
fn create_page(window: adw::ApplicationWindow) -> adw::NavigationPage {
    let content = content_box();

    let form = adw::PreferencesGroup::builder()
        .title("Параметры архива")
        .build();

    let home_entry = gtk::Entry::builder()
        .text(default_home().display().to_string())
        .hexpand(true)
        .build();
    form.add(&entry_row("Домашний каталог", &home_entry));

    let output_entry = gtk::Entry::builder()
        .text(default_home().join("profile.rmm").display().to_string())
        .hexpand(true)
        .build();
    form.add(&entry_row("Файл архива (.rmm)", &output_entry));

    let pass_entry = gtk::PasswordEntry::builder().show_peek_icon(true).build();
    form.add(&entry_row("Пароль шифрования (не сохраняется)", &pass_entry));

    let compression = gtk::SpinButton::with_range(0.0, 22.0, 1.0);
    compression.set_value(3.0);
    form.add(&entry_row("Уровень сжатия zstd", &compression));

    content.append(&form);

    let components_group = adw::PreferencesGroup::builder()
        .title("Компоненты")
        .build();
    let defaults = ComponentType::default_components();
    let mut checks: Vec<(ComponentType, gtk::CheckButton)> = Vec::new();
    for component in ComponentType::all_components() {
        let row = adw::ActionRow::builder()
            .title(component.description())
            .build();
        let check = gtk::CheckButton::new();
        check.set_active(defaults.contains(&component));
        check.set_valign(gtk::Align::Center);
        row.add_suffix(&check);
        row.set_activatable_widget(Some(&check));
        components_group.add(&row);
        checks.push((component, check));
    }
    content.append(&components_group);

    let options_group = adw::PreferencesGroup::builder()
        .title("Параметры запуска")
        .build();
    let dry_row = adw::ActionRow::builder()
        .title("Пробный запуск (dry-run)")
        .subtitle("Просканировать профиль без создания файла")
        .build();
    let dry_check = gtk::CheckButton::new();
    dry_check.set_valign(gtk::Align::Center);
    dry_row.add_suffix(&dry_check);
    dry_row.set_activatable_widget(Some(&dry_check));
    options_group.add(&dry_row);
    content.append(&options_group);

    let run_button = gtk::Button::with_label("Создать архив");
    run_button.add_css_class("suggested-action");
    content.append(&run_button);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    content.append(&progress);

    let status = gtk::Label::new(Some("Готово к запуску"));
    status.set_xalign(0.0);
    status.set_wrap(true);
    content.append(&status);

    bind_create_action(
        &window, &run_button, &progress, &status, &home_entry, &output_entry, &pass_entry,
        &compression, &dry_check, checks,
    );

    page("Создать архив профиля", &content)
}

/// Подключить обработчик кнопки «Создать архив».
#[allow(clippy::too_many_arguments)]
fn bind_create_action(
    window: &adw::ApplicationWindow,
    run_button: &gtk::Button,
    progress: &gtk::ProgressBar,
    status: &gtk::Label,
    home_entry: &gtk::Entry,
    output_entry: &gtk::Entry,
    pass_entry: &gtk::PasswordEntry,
    compression: &gtk::SpinButton,
    dry_check: &gtk::CheckButton,
    checks: Vec<(ComponentType, gtk::CheckButton)>,
) {
    let window = window.clone();
    let run_button = run_button.clone();
    let progress = progress.clone();
    let status = status.clone();
    let home_entry = home_entry.clone();
    let output_entry = output_entry.clone();
    let pass_entry = pass_entry.clone();
    let compression = compression.clone();
    let dry_check = dry_check.clone();

    run_button.connect_clicked(move |_| {
        let source_home = PathBuf::from(home_entry.text().to_string());
        let output = PathBuf::from(output_entry.text().to_string());
        let passphrase = {
            let text = pass_entry.text().to_string();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        };
        let compression_level = compression.value().round() as i32;
        let dry_run = dry_check.is_active();
        let components: Vec<ComponentType> = checks
            .iter()
            .filter(|(_, check)| check.is_active())
            .map(|(component, _)| *component)
            .collect();

        if !source_home.is_dir() {
            show_message(
                &window,
                "Проверьте параметры",
                "Укажите существующий домашний каталог.",
            );
            return;
        }
        if output.as_os_str().is_empty() {
            show_message(
                &window,
                "Проверьте параметры",
                "Укажите путь к файлу архива (.rmm).",
            );
            return;
        }
        if components.is_empty() {
            show_message(
                &window,
                "Проверьте параметры",
                "Выберите хотя бы один компонент для переноса.",
            );
            return;
        }

        run_button.set_sensitive(false);
        status.set_text("Сканирование профиля…");
        let window_done = window.clone();
        let button_done = run_button.clone();
        spawn_task(
            &progress,
            &status,
            move |observer| {
                let items = scan_items(&source_home, &components)?;
                let options = CreateArchiveOptions {
                    output,
                    source_home,
                    items,
                    components,
                    passphrase,
                    compression_level,
                    dry_run,
                    cancel: Some(crate::cancel::global()),
                };
                let created = ArchiveManager::create(&options, observer)?;
                Ok(format!(
                    "Архив {}: {} файлов, {}",
                    if dry_run { "проверен (dry-run)" } else { "создан" },
                    created.manifest.total_files,
                    human_bytes::human_bytes(created.manifest.total_size as f64)
                ))
            },
            move |success, summary| {
                button_done.set_sensitive(true);
                let heading = if success {
                    "Готово"
                } else {
                    "Операция не завершена"
                };
                show_message(&window_done, heading, summary);
            },
        );
    });
}

/// Страница восстановления профиля из архива (§10).
fn restore_page(window: adw::ApplicationWindow) -> adw::NavigationPage {
    let content = content_box();

    let form = adw::PreferencesGroup::builder()
        .title("Источник и назначение")
        .build();

    let archive_entry = gtk::Entry::builder().hexpand(true).build();
    form.add(&entry_row("Файл архива (.rmm)", &archive_entry));

    let target_entry = gtk::Entry::builder()
        .text(default_home().display().to_string())
        .hexpand(true)
        .build();
    form.add(&entry_row("Каталог восстановления", &target_entry));

    let pass_entry = gtk::PasswordEntry::builder().show_peek_icon(true).build();
    form.add(&entry_row("Пароль шифрования (если есть)", &pass_entry));

    let strategy_drop = gtk::DropDown::from_strings(&STRATEGY_LABELS);
    strategy_drop.set_selected(0);
    form.add(&entry_row("Стратегия конфликтов", &strategy_drop));

    content.append(&form);

    let options_group = adw::PreferencesGroup::builder().title("Параметры").build();

    let verify_row = adw::ActionRow::builder()
        .title("Проверять хеши после восстановления")
        .build();
    let verify_check = gtk::CheckButton::new();
    verify_check.set_active(true);
    verify_check.set_valign(gtk::Align::Center);
    verify_row.add_suffix(&verify_check);
    verify_row.set_activatable_widget(Some(&verify_check));
    options_group.add(&verify_row);

    let dry_row = adw::ActionRow::builder()
        .title("Пробный запуск (dry-run)")
        .subtitle("Показать план без записи файлов")
        .build();
    let dry_check = gtk::CheckButton::new();
    dry_check.set_valign(gtk::Align::Center);
    dry_row.add_suffix(&dry_check);
    dry_row.set_activatable_widget(Some(&dry_check));
    options_group.add(&dry_row);

    content.append(&options_group);

    let run_button = gtk::Button::with_label("Восстановить");
    run_button.add_css_class("suggested-action");
    content.append(&run_button);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    content.append(&progress);

    let status = gtk::Label::new(Some("Готово к запуску"));
    status.set_xalign(0.0);
    status.set_wrap(true);
    content.append(&status);

    bind_restore_action(
        &window,
        &run_button,
        &progress,
        &status,
        &archive_entry,
        &target_entry,
        &pass_entry,
        &strategy_drop,
        &verify_check,
        &dry_check,
    );

    page("Восстановить из архива", &content)
}

/// Подключить обработчик кнопки «Восстановить».
#[allow(clippy::too_many_arguments)]
fn bind_restore_action(
    window: &adw::ApplicationWindow,
    run_button: &gtk::Button,
    progress: &gtk::ProgressBar,
    status: &gtk::Label,
    archive_entry: &gtk::Entry,
    target_entry: &gtk::Entry,
    pass_entry: &gtk::PasswordEntry,
    strategy_drop: &gtk::DropDown,
    verify_check: &gtk::CheckButton,
    dry_check: &gtk::CheckButton,
) {
    let window = window.clone();
    let run_button = run_button.clone();
    let progress = progress.clone();
    let status = status.clone();
    let archive_entry = archive_entry.clone();
    let target_entry = target_entry.clone();
    let pass_entry = pass_entry.clone();
    let strategy_drop = strategy_drop.clone();
    let verify_check = verify_check.clone();
    let dry_check = dry_check.clone();

    run_button.connect_clicked(move |_| {
        let archive = PathBuf::from(archive_entry.text().to_string());
        let target_root = PathBuf::from(target_entry.text().to_string());
        let passphrase = {
            let text = pass_entry.text().to_string();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        };
        let strategy = conflict_strategy(strategy_drop.selected());
        let verify_hash = verify_check.is_active();
        let dry_run = dry_check.is_active();

        if !archive.is_file() {
            show_message(
                &window,
                "Проверьте параметры",
                "Укажите существующий файл архива (.rmm).",
            );
            return;
        }
        if target_root.as_os_str().is_empty() {
            show_message(
                &window,
                "Проверьте параметры",
                "Укажите каталог восстановления.",
            );
            return;
        }

        run_button.set_sensitive(false);
        status.set_text("Восстановление…");
        let window_done = window.clone();
        let button_done = run_button.clone();
        spawn_task(
            &progress,
            &status,
            move |observer| {
                let options = RestoreOptions {
                    archive,
                    passphrase,
                    target_root,
                    components: None,
                    strategy,
                    verify_hash,
                    dry_run,
                    cancel: Some(crate::cancel::global()),
                };
                let result = ArchiveManager::restore(&options, observer)?;
                if !result.is_success() {
                    return Err(MigrationError::Conflict(format!(
                        "восстановление завершено с ошибками: {}",
                        result.errors.len()
                    )));
                }
                Ok(format!(
                    "Восстановлено файлов: {} ({}); пропущено: {}; конфликтов: {}",
                    result.restored_files,
                    human_bytes::human_bytes(result.restored_bytes as f64),
                    result.skipped_files,
                    result.conflicts.len()
                ))
            },
            move |success, summary| {
                button_done.set_sensitive(true);
                let heading = if success {
                    "Готово"
                } else {
                    "Операция не завершена"
                };
                show_message(&window_done, heading, summary);
            },
        );
    });
}

/// Страница прямой миграции по SSH (§9).
fn ssh_page(window: adw::ApplicationWindow) -> adw::NavigationPage {
    let content = content_box();

    let form = adw::PreferencesGroup::builder()
        .title("Подключение к старому компьютеру")
        .build();

    let target_entry = gtk::Entry::builder()
        .placeholder_text("user@192.168.1.10:22")
        .hexpand(true)
        .build();
    form.add(&entry_row("Адрес (пользователь@хост[:порт])", &target_entry));

    let remote_entry = gtk::Entry::builder()
        .placeholder_text("~/")
        .hexpand(true)
        .build();
    form.add(&entry_row("Домашний каталог на удалённом хосте", &remote_entry));

    content.append(&form);

    let note = gtk::Label::new(Some(
        "Перед подключением проверьте отпечаток узла (host key). \
         Проверка host key не отключается автоматически.",
    ));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.add_css_class("dim-label");
    content.append(&note);

    let options_group = adw::PreferencesGroup::builder().title("Параметры").build();
    let dry_row = adw::ActionRow::builder()
        .title("Пробный запуск (dry-run)")
        .subtitle("Проверить соединение и план без передачи")
        .build();
    let dry_check = gtk::CheckButton::new();
    dry_check.set_valign(gtk::Align::Center);
    dry_row.add_suffix(&dry_check);
    dry_row.set_activatable_widget(Some(&dry_check));
    options_group.add(&dry_row);
    content.append(&options_group);

    let run_button = gtk::Button::with_label("Начать перенос");
    run_button.add_css_class("suggested-action");
    content.append(&run_button);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    content.append(&progress);

    let status = gtk::Label::new(Some("Готово к запуску"));
    status.set_xalign(0.0);
    status.set_wrap(true);
    content.append(&status);

    bind_ssh_action(
        &window,
        &run_button,
        &progress,
        &status,
        &target_entry,
        &remote_entry,
        &dry_check,
    );

    page("Перенос по SSH", &content)
}

/// Подключить обработчик кнопки «Начать перенос» (SSH).
fn bind_ssh_action(
    window: &adw::ApplicationWindow,
    run_button: &gtk::Button,
    progress: &gtk::ProgressBar,
    status: &gtk::Label,
    target_entry: &gtk::Entry,
    remote_entry: &gtk::Entry,
    dry_check: &gtk::CheckButton,
) {
    let window = window.clone();
    let run_button = run_button.clone();
    let progress = progress.clone();
    let status = status.clone();
    let target_entry = target_entry.clone();
    let remote_entry = remote_entry.clone();
    let dry_check = dry_check.clone();

    run_button.connect_clicked(move |_| {
        let Some((user, host, port)) = parse_ssh_target(&target_entry.text()) else {
            show_message(
                &window,
                "Проверьте параметры",
                "Укажите адрес вида пользователь@хост[:порт].",
            );
            return;
        };

        let remote = remote_entry.text().to_string();
        let remote_home = if remote.trim().is_empty() {
            if user.is_empty() {
                "~".to_string()
            } else {
                format!("/home/{}", user)
            }
        } else {
            remote.trim().to_string()
        };

        let ssh = SshTarget {
            host,
            user,
            port,
            identity_file: None,
            known_hosts_file: None,
        };

        let mut wizard_config = WizardConfig::new(MigrationMode::SshDirect, default_home());
        wizard_config.target_home = PathBuf::from(remote_home);
        wizard_config.components = ComponentType::default_components();
        wizard_config.app_rules = crate::applications::load_effective_rules();
        wizard_config.ssh = Some(ssh);
        wizard_config.dry_run = dry_check.is_active();
        wizard_config.cancel = Some(crate::cancel::global());
        wizard_config.database_path = config::load_config().ok().map(|cfg| cfg.database_path);

        run_button.set_sensitive(false);
        status.set_text("Подключение и передача…");
        let window_done = window.clone();
        let button_done = run_button.clone();
        spawn_task(
            &progress,
            &status,
            move |observer| {
                let mut wizard = MigrationWizard::new(wizard_config);
                let report = wizard.run(observer)?;
                Ok(format!(
                    "Передача: файлов {}, {}; статус: {}",
                    report.stats.files_copied,
                    human_bytes::human_bytes(report.stats.bytes_copied as f64),
                    report.status()
                ))
            },
            move |success, summary| {
                button_done.set_sensitive(true);
                let heading = if success {
                    "Готово"
                } else {
                    "Операция не завершена"
                };
                show_message(&window_done, heading, summary);
            },
        );
    });
}

/// Страница истории операций из локальной базы SQLite (§16).
fn history_page() -> adw::NavigationPage {
    let content = content_box();
    let text = gtk::Label::new(None);
    text.set_xalign(0.0);
    text.set_selectable(true);
    text.set_wrap(true);
    text.set_text(&load_history_text(40));
    content.append(&text);
    page("История операций", &content)
}

/// Прочитать историю и отрендерить её в текст (ошибки — тоже текстом).
fn load_history_text(limit: usize) -> String {
    match load_history(limit) {
        Ok(body) => body,
        Err(error) => format!("Не удалось прочитать историю: {}", error.user_message()),
    }
}

fn load_history(limit: usize) -> Result<String> {
    let current = config::load_config()?;
    let db_path = current.database_path.clone();
    if !db_path.exists() {
        return Ok(format!(
            "История недоступна: база данных не найдена ({})",
            db_path.display()
        ));
    }
    let database = DatabaseManager::new(db_path)?;
    let records = database.get_migrations(limit, 0)?;
    if records.is_empty() {
        return Ok("История операций пуста.".to_string());
    }
    Ok(render_history_text(&records))
}

/// Страница настроек и диагностики (§1, §15).
fn settings_page() -> adw::NavigationPage {
    let content = content_box();
    let info = MainScreenInfo::collect();

    let tools = adw::PreferencesGroup::builder()
        .title("Внешние инструменты")
        .build();
    for tool in &info.tools {
        let state = if tool.available {
            "доступен"
        } else if tool.required {
            "НЕ НАЙДЕН (обязателен)"
        } else {
            "не найден"
        };
        tools.add(&info_row(&tool.name, state));
    }
    content.append(&tools);

    let polkit = adw::PreferencesGroup::builder()
        .title("Привилегированные операции")
        .build();
    match PolkitManager::check_environment() {
        Ok(notes) if notes.is_empty() => {
            polkit.add(&info_row(
                "PolicyKit",
                "готов: pkexec и агент авторизации доступны",
            ));
        }
        Ok(notes) => {
            for note in notes {
                polkit.add(&info_row("PolicyKit", &note));
            }
        }
        Err(error) => {
            polkit.add(&info_row(
                "PolicyKit",
                &format!("проверка недоступна: {}", error.user_message()),
            ));
        }
    }
    content.append(&polkit);

    let paths = adw::PreferencesGroup::builder().title("Пути").build();
    paths.add(&info_row(
        "Настройки",
        &config::get_config_dir().display().to_string(),
    ));
    paths.add(&info_row(
        "Данные",
        &config::get_data_dir().display().to_string(),
    ));
    match config::load_config() {
        Ok(cfg) => paths.add(&info_row(
            "База истории",
            &cfg.database_path.display().to_string(),
        )),
        Err(error) => paths.add(&info_row(
            "База истории",
            &format!("недоступна: {}", error.user_message()),
        )),
    }
    content.append(&paths);

    let about = adw::PreferencesGroup::builder().title("О программе").build();
    about.add(&info_row("Приложение", crate::APP_NAME));
    about.add(&info_row("Версия", crate::VERSION));
    about.add(&info_row("Идентификатор", crate::APP_ID));
    content.append(&about);

    page("Настройки и диагностика", &content)
}

/// Собрать элементы профиля для упаковки (как в CLI-команде `create`).
fn scan_items(home: &std::path::Path, components: &[ComponentType]) -> Result<Vec<TransferItem>> {
    let config = WizardConfig {
        components: components.to_vec(),
        app_rules: crate::applications::load_effective_rules(),
        ..WizardConfig::new(MigrationMode::LocalArchive, home)
    };
    let mut wizard = MigrationWizard::new(config);
    let inventory = wizard.scan()?.clone();
    Ok(inventory.items)
}

/// Состояние фоновой операции, публикуемое для GTK-потока.
#[derive(Clone, Default)]
struct ProgressState {
    fraction: f64,
    message: String,
    finished: bool,
    success: bool,
    summary: String,
}

/// Наблюдатель прогресса, обновляющий общее состояние.
struct GuiProgress {
    state: Arc<Mutex<ProgressState>>,
}

impl ProgressObserver for GuiProgress {
    fn on_progress(&self, done_bytes: u64, total_bytes: u64, current: &std::path::Path) {
        if let Ok(mut state) = self.state.lock() {
            state.fraction = if total_bytes == 0 {
                0.0
            } else {
                (done_bytes as f64 / total_bytes as f64).clamp(0.0, 1.0)
            };
            state.message = current.display().to_string();
        }
    }

    fn on_message(&self, message: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.message = message.to_string();
        }
    }
}

/// Выполнить длительную операцию в фоновом потоке, обновляя прогресс в интерфейсе.
fn spawn_task<F, D>(progress_bar: &gtk::ProgressBar, status_label: &gtk::Label, run: F, on_done: D)
where
    F: FnOnce(&dyn ProgressObserver) -> Result<String> + Send + 'static,
    D: FnOnce(bool, &str) + 'static,
{
    let state = Arc::new(Mutex::new(ProgressState {
        message: "Подготовка…".to_string(),
        ..ProgressState::default()
    }));
    let state_background = Arc::clone(&state);

    std::thread::spawn(move || {
        let observer = GuiProgress {
            state: Arc::clone(&state_background),
        };
        let outcome = run(&observer);
        if let Ok(mut snapshot) = state_background.lock() {
            snapshot.finished = true;
            match outcome {
                Ok(summary) => {
                    snapshot.success = true;
                    snapshot.fraction = 1.0;
                    snapshot.message = "Готово".to_string();
                    snapshot.summary = summary;
                }
                Err(error) => {
                    snapshot.success = false;
                    snapshot.message = "Ошибка".to_string();
                    snapshot.summary = error.user_message();
                }
            }
        }
    });

    let bar = progress_bar.clone();
    let label = status_label.clone();
    let mut on_done = Some(on_done);
    let _ = glib::timeout_add_local(Duration::from_millis(150), move || {
        let snapshot = match state.lock() {
            Ok(snapshot) => snapshot.clone(),
            Err(_) => return glib::ControlFlow::Break,
        };
        bar.set_fraction(snapshot.fraction);
        let percent_text = format!("{}%", (snapshot.fraction * 100.0).round() as i64);
        bar.set_text(Some(percent_text.as_str()));
        label.set_text(&snapshot.message);
        if snapshot.finished {
            if let Some(callback) = on_done.take() {
                callback(snapshot.success, &snapshot.summary);
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}
