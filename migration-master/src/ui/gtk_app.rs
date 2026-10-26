//! Графический мастер Migration Master на GTK4 + libadwaita.
//!
//! Модуль собирается только на Linux с фичей `gui`. GUI не выполняет shell-команды
//! напрямую: все операции идут через те же сервисы, что и CLI
//! (`ArchiveManager`, `MigrationWizard`, SQLite-история).

#![cfg(all(feature = "gui", target_os = "linux"))]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// adw::prelude::* реэкспортирует и gtk::prelude::* (там, в частности, IsA),
// поэтому отдельного импорта GTK4-прелюдии не требуется.
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
    // GTK4 и libadwaitа инициализируются здесь же: без графической сессии
    // (не задан DISPLAY/WAYLAND_DISPLAY, запуск из-под sudo) adw::init()
    // возвращает ошибку. Раньше код молча выходил, и пользователь видел только
    // пустой терминал — теперь причина выводится явно.
    if let Err(error) = adw::init() {
        crate::log_error!("gui", "не удалось инициализировать GTK: {}", error);
        return Err(MigrationError::Unsupported(format!(
            "не удалось открыть графическую сессию: {}. Проверьте, что запущена \
             графическая подсистема (X11 или Wayland) и задан DISPLAY либо \
             WAYLAND_DISPLAY; из-под sudo графический мастер не запускается — \
             запускайте его от обычного пользователя",
            error
        )));
    }

    // Диагностика окружения — в stderr и в журнал: по ней видно, с какими
    // версиями GTK4/libadwaita запустился мастер.
    let environment = format!(
        "GUI: GTK {}.{}.{}, libadwaita {}.{}.{}, display={}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
        display_name()
    );
    eprintln!("{}", environment);
    crate::log_info!("gui", "{}", environment);

    let app = adw::Application::builder()
        .application_id(crate::APP_ID)
        .flags(adw::gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    // GApplication по умолчанию считает аргументы командной строки файлами
    // для открытия. Из-за этого `migration-master gui` приводил к
    // «GLib-GIO-CRITICAL: This application can not open files.» и выходу с
    // кодом 1 — окно не появлялось (и с ярлыка в меню тоже).
    // Командную строку разбирает clap, поэтому GApplication получает
    // пустую команду, а показать окно мы вызываем сами.
    app.connect_command_line(|app, _command_line| {
        app.activate();
        0
    });

    if selftest_requested() {
        return run_selftest(app);
    }

    app.connect_activate(build_window);
    // Код возврата g_application_run() не выбрасываем: если приложение не
    // зарегистрировалось в D-Bus или завершилось с ошибкой, команда
    // `migration-master gui` должна вернуть ненулевой код, а не сообщать
    // об успехе.
    let exit_code = app.run();
    if exit_code == glib::ExitCode::SUCCESS {
        crate::log_info!("gui", "GUI: сеанс завершён (версия {})", crate::VERSION);
        Ok(())
    } else {
        crate::log_error!("gui", "GUI: сеанс завершён с кодом {}", exit_code.value());
        Err(MigrationError::Unsupported(format!(
            "графический мастер завершился с кодом {}",
            exit_code.value()
        )))
    }
}

/// Режим самопроверки: создать окно, дождаться его показа и выйти с кодом 0.
///
/// Нужен для диагностики «GUI не запускается»: собирает то же окно, что и
/// обычный запуск, но не ждёт действий пользователя. Включается переменной
/// окружения `MIGRATION_MASTER_GUI_SELFTEST=1` (используется в CI под Xvfb).
fn run_selftest(app: adw::Application) -> Result<()> {
    let app_for_timeout = app.clone();
    // Если окно так и не показали (например, сломался компоновщик) — выходим
    // с ошибкой, а не молча.
    glib::timeout_add_local(Duration::from_secs(10), move || {
        if !SELFTEST_OK.with(|flag| flag.get()) {
            eprintln!("GUI: окно не было показано за 10 секунд — самопроверка не пройдена");
            crate::log_error!("gui", "самопроверка: окно не показано");
            app_for_timeout.quit();
        }
        glib::ControlFlow::Break
    });

    app.connect_activate(|app| {
        let window = create_main_window(app);
        let app_for_quit = app.clone();
        window.connect_map(move |_| {
            eprintln!("GUI: окно создано и показано — самопроверка пройдена");
            crate::log_info!("gui", "самопроверка: окно показано");
            SELFTEST_OK.with(|flag| flag.set(true));
            app_for_quit.quit();
        });
        window.present();
    });

    let _ = app.run();
    if SELFTEST_OK.with(|flag| flag.get()) {
        Ok(())
    } else {
        Err(MigrationError::Unsupported(
            "графический мастер не смог показать окно".to_string(),
        ))
    }
}

/// Запрошена ли самопроверка GUI.
fn selftest_requested() -> bool {
    std::env::var("MIGRATION_MASTER_GUI_SELFTEST")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

thread_local! {
    /// Результат самопроверки GUI: окно создано и показано.
    static SELFTEST_OK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Имя текущего дисплея (для диагностики).
fn display_name() -> String {
    std::env::var("WAYLAND_DISPLAY")
        .map(|name| format!("wayland:{}", name))
        .or_else(|_| std::env::var("DISPLAY").map(|name| format!("x11:{}", name)))
        .unwrap_or_else(|_| "нет".to_string())
}

/// Домашний каталог пользователя (для значений по умолчанию).
fn default_home() -> PathBuf {
    crate::platform::home_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Развернуть «~» в начале пути в домашний каталог.
fn expand_tilde(path: PathBuf) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => {
            let mut result = default_home();
            if !rest.as_os_str().is_empty() {
                result.push(rest);
            }
            result
        }
        Err(_) => path,
    }
}

/// Навигация по страницам мастера.
///
/// Собрана на `gtk::Stack`. Раньше здесь использовались `adw::NavigationView`,
/// `adw::NavigationPage` и `adw::ToolbarView`, но эти виджеты появились только
/// в libadwaita 1.4, а на РЕД ОС 8 доступна libadwaita 1.2: бинарник с такими
/// вызовами не стартует («undefined symbol: adw_navigation_view_new»).
/// Здесь используются только GTK4 и libadwaita 1.0.
#[derive(Clone)]
struct Navigator {
    stack: gtk::Stack,
    history: Rc<RefCell<Vec<gtk::Box>>>,
}

impl Navigator {
    fn new() -> Self {
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        Self {
            stack,
            history: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Показать стартовую страницу (кнопки «Назад» у неё нет).
    ///
    /// Корневая страница тоже попадает в историю: без этого кнопка «Назад»
    /// на первой подстранице ничего не делала — глубина истории была 1,
    /// а `pop()` требует минимум 2.
    fn set_root(&self, page: &gtk::Box) {
        self.stack.add_child(page);
        self.stack.set_visible_child(page);
        self.history.borrow_mut().push(page.clone());
    }

    /// Открыть страницу поверх текущей.
    fn push(&self, page: &gtk::Box) {
        self.stack.add_child(page);
        self.stack.set_visible_child(page);
        self.history.borrow_mut().push(page.clone());
    }

    /// Вернуться к предыдущей странице.
    fn pop(&self) {
        let previous = {
            let mut history = self.history.borrow_mut();
            if history.len() < 2 {
                return;
            }
            let _ = history.pop();
            history.last().cloned()
        };
        if let Some(previous) = previous {
            self.stack.set_visible_child(&previous);
        }
    }
}

impl Default for Navigator {
    fn default() -> Self {
        Self::new()
    }
}

/// Создать главное окно мастера (без показа).
fn create_main_window(app: &adw::Application) -> adw::ApplicationWindow {
    let nav = Navigator::new();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Мастер миграции пользователя")
        .default_width(1000)
        .default_height(720)
        .content(&nav.stack)
        .build();

    let home = home_page(&nav, &window);
    nav.set_root(&home);
    window
}

fn build_window(app: &adw::Application) {
    // Повторный запуск (второй клик по ярлыку в меню, повторная команда
    // `migration-master gui`) приходит уже в запущенный экземпляр: GApplication
    // с HANDLES_COMMAND_LINE вызывает activate(). Показываем существующее окно,
    // а не создаём второе.
    if let Some(window) = app
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<adw::ApplicationWindow>().ok())
    {
        window.present();
        return;
    }

    let window = create_main_window(app);
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

/// Обернуть содержимое в страницу: заголовок, кнопка «Назад» и прокрутка.
///
/// Заменяет `adw::ToolbarView` + `adw::NavigationPage` (libadwaita 1.4) —
/// на libadwaita 1.0/1.2 такой заголовок собирается из `adw::HeaderBar`
/// и вертикального `gtk::Box`.
fn page_frame(title: &str, content: &gtk::Box, nav: &Navigator, root: bool) -> gtk::Box {
    let header = adw::HeaderBar::new();
    if !root {
        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.set_tooltip_text(Some("Назад"));
        let nav = nav.clone();
        back.connect_clicked(move |_| nav.pop());
        header.pack_start(&back);
    }

    let title_label = gtk::Label::new(Some(title));
    title_label.add_css_class("title");
    header.set_title_widget(Some(&title_label));

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(content)
        .build();

    let page = gtk::Box::new(Orientation::Vertical, 0);
    page.append(&header);
    page.append(&scroller);
    page
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
///
/// `adw::MessageDialog` появился только в libadwaita 1.2, поэтому диалог
/// собран вручную из `adw::Window` (доступен с libadwaita 1.0).
fn show_message(window: &adw::ApplicationWindow, heading: &str, body: &str) {
    let dialog = adw::Window::builder()
        .transient_for(window)
        .modal(true)
        .default_width(460)
        .build();

    let content = gtk::Box::new(Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let title = gtk::Label::new(Some(heading));
    title.add_css_class("title-2");
    title.set_xalign(0.0);
    title.set_wrap(true);
    content.append(&title);

    let text = gtk::Label::new(Some(body));
    text.set_xalign(0.0);
    text.set_wrap(true);
    text.set_selectable(true);
    content.append(&text);

    let close = gtk::Button::with_label("Закрыть");
    close.add_css_class("suggested-action");
    close.set_halign(gtk::Align::End);
    let dialog_for_close = dialog.clone();
    close.connect_clicked(move |_| dialog_for_close.close());
    content.append(&close);

    dialog.set_content(Some(&content));
    dialog.present();
}

/// Поле ввода пароля.
///
/// `gtk::PasswordEntry` появился только в GTK 4.4, поэтому используется обычный
/// `gtk::Entry` с переключателем видимости (работает на GTK 4.0 и новее).
/// Возвращает само поле и контейнер с переключателем.
fn password_entry() -> (gtk::Entry, gtk::Box) {
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    entry.set_visibility(false);
    entry.set_input_purpose(gtk::InputPurpose::Password);

    let toggle = gtk::CheckButton::with_label("Показать");
    toggle.set_valign(gtk::Align::Center);
    let entry_for_toggle = entry.clone();
    toggle.connect_toggled(move |button| {
        entry_for_toggle.set_visibility(button.is_active());
    });

    let container = gtk::Box::new(Orientation::Horizontal, 6);
    container.append(&entry);
    container.append(&toggle);
    (entry, container)
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
fn home_page(nav: &Navigator, window: &adw::ApplicationWindow) -> gtk::Box {
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
            let page = create_page(&window, &nav);
            nav.push(&page);
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
            let page = ssh_page(&window, &nav);
            nav.push(&page);
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
            let page = restore_page(&window, &nav);
            nav.push(&page);
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
            let page = history_page(&nav);
            nav.push(&page);
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
            let page = settings_page(&nav);
            nav.push(&page);
        });
    }
    list.append(&settings);

    actions.add(&list);
    content.append(&actions);

    page_frame("Migration Master", &content, nav, true)
}

/// Страница создания архива профиля (режим «Локальный архив»).
fn create_page(window: &adw::ApplicationWindow, nav: &Navigator) -> gtk::Box {
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

    let (pass_entry, pass_widget) = password_entry();
    form.add(&entry_row(
        "Пароль шифрования (не сохраняется)",
        &pass_widget,
    ));

    let compression = gtk::SpinButton::with_range(0.0, 22.0, 1.0);
    compression.set_value(3.0);
    form.add(&entry_row("Уровень сжатия zstd", &compression));

    content.append(&form);

    let components_group = adw::PreferencesGroup::builder().title("Компоненты").build();
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
        window,
        &run_button,
        &progress,
        &status,
        &home_entry,
        &output_entry,
        &pass_entry,
        &compression,
        &dry_check,
        checks,
    );

    page_frame("Создать архив профиля", &content, nav, false)
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
    pass_entry: &gtk::Entry,
    compression: &gtk::SpinButton,
    dry_check: &gtk::CheckButton,
    checks: Vec<(ComponentType, gtk::CheckButton)>,
) {
    let window = (*window).clone();
    let run_button = (*run_button).clone();
    let progress = (*progress).clone();
    let status = (*status).clone();
    let home_entry = (*home_entry).clone();
    let output_entry = (*output_entry).clone();
    let pass_entry = (*pass_entry).clone();
    let compression = (*compression).clone();
    let dry_check = (*dry_check).clone();

    // connect_clicked() заимствует кнопку, поэтому замыкание должно захватывать
    // собственную копию — иначе "cannot move out of `run_button`".
    let button = run_button.clone();

    run_button.connect_clicked(move |_| {
        let source_home = expand_tilde(PathBuf::from(home_entry.text().to_string()));
        // Относительный путь к архиву отсчитываем от домашнего каталога:
        // процесс GUI может быть запущен из произвольного каталога (ярлык
        // в меню приложений), и относительный путь «против текущего
        // каталога» там неожиданен для пользователя.
        let output = {
            let entered = expand_tilde(PathBuf::from(output_entry.text().to_string()));
            if entered.is_absolute() {
                entered
            } else {
                default_home().join(entered)
            }
        };
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

        button.set_sensitive(false);
        status.set_text("Сканирование профиля…");
        let window_done = window.clone();
        let button_done = button.clone();
        spawn_task(
            &progress,
            &status,
            move |observer| {
                let items = scan_items(&source_home, &components, Some(output.as_path()))?;
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
                    if dry_run {
                        "проверен (dry-run)"
                    } else {
                        "создан"
                    },
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
fn restore_page(window: &adw::ApplicationWindow, nav: &Navigator) -> gtk::Box {
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

    let (pass_entry, pass_widget) = password_entry();
    form.add(&entry_row("Пароль шифрования (если есть)", &pass_widget));

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
        window,
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

    page_frame("Восстановить из архива", &content, nav, false)
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
    pass_entry: &gtk::Entry,
    strategy_drop: &gtk::DropDown,
    verify_check: &gtk::CheckButton,
    dry_check: &gtk::CheckButton,
) {
    let window = (*window).clone();
    let run_button = (*run_button).clone();
    let progress = (*progress).clone();
    let status = (*status).clone();
    let archive_entry = (*archive_entry).clone();
    let target_entry = (*target_entry).clone();
    let pass_entry = (*pass_entry).clone();
    let strategy_drop = (*strategy_drop).clone();
    let verify_check = (*verify_check).clone();
    let dry_check = (*dry_check).clone();

    let button = run_button.clone();

    run_button.connect_clicked(move |_| {
        let archive = expand_tilde(PathBuf::from(archive_entry.text().to_string()));
        let target_root = expand_tilde(PathBuf::from(target_entry.text().to_string()));
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

        button.set_sensitive(false);
        status.set_text("Восстановление…");
        let window_done = window.clone();
        let button_done = button.clone();
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
fn ssh_page(window: &adw::ApplicationWindow, nav: &Navigator) -> gtk::Box {
    let content = content_box();

    let form = adw::PreferencesGroup::builder()
        .title("Подключение к старому компьютеру")
        .build();

    let target_entry = gtk::Entry::builder()
        .placeholder_text("user@192.168.1.10:22")
        .hexpand(true)
        .build();
    form.add(&entry_row(
        "Адрес (пользователь@хост[:порт])",
        &target_entry,
    ));

    let remote_entry = gtk::Entry::builder()
        .placeholder_text("~/")
        .hexpand(true)
        .build();
    form.add(&entry_row(
        "Домашний каталог на удалённом хосте",
        &remote_entry,
    ));

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
        window,
        &run_button,
        &progress,
        &status,
        &target_entry,
        &remote_entry,
        &dry_check,
    );

    page_frame("Перенос по SSH", &content, nav, false)
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
    let window = (*window).clone();
    let run_button = (*run_button).clone();
    let progress = (*progress).clone();
    let status = (*status).clone();
    let target_entry = (*target_entry).clone();
    let remote_entry = (*remote_entry).clone();
    let dry_check = (*dry_check).clone();

    let button = run_button.clone();

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

        button.set_sensitive(false);
        status.set_text("Подключение и передача…");
        let window_done = window.clone();
        let button_done = button.clone();
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
fn history_page(nav: &Navigator) -> gtk::Box {
    let content = content_box();
    let text = gtk::Label::new(None);
    text.set_xalign(0.0);
    text.set_selectable(true);
    text.set_wrap(true);
    text.set_text(&load_history_text(40));
    content.append(&text);
    page_frame("История операций", &content, nav, false)
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
fn settings_page(nav: &Navigator) -> gtk::Box {
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

    let about = adw::PreferencesGroup::builder()
        .title("О программе")
        .build();
    about.add(&info_row("Приложение", crate::APP_NAME));
    about.add(&info_row("Версия", crate::VERSION));
    about.add(&info_row("Идентификатор", crate::APP_ID));
    about.add(&info_row(
        "Графическая среда",
        &format!(
            "GTK {}.{}.{}, libadwaita {}.{}.{}",
            gtk::major_version(),
            gtk::minor_version(),
            gtk::micro_version(),
            adw::major_version(),
            adw::minor_version(),
            adw::micro_version()
        ),
    ));
    content.append(&about);

    page_frame("Настройки и диагностика", &content, nav, false)
}

/// Собрать элементы профиля для упаковки (как в CLI-команде `create`).
///
/// `exclude` — файл, который не включается в архив (сам архив, если он лежит
/// внутри сканируемого каталога: иначе прошлый архив попадает в новый).
fn scan_items(
    home: &std::path::Path,
    components: &[ComponentType],
    exclude: Option<&std::path::Path>,
) -> Result<Vec<TransferItem>> {
    let config = WizardConfig {
        components: components.to_vec(),
        app_rules: crate::applications::load_effective_rules(),
        ..WizardConfig::new(MigrationMode::LocalArchive, home)
    };
    let mut wizard = MigrationWizard::new(config);
    let inventory = wizard.scan()?.clone();
    let mut items = inventory.items;
    if let Some(exclude) = exclude {
        items.retain(|item| item.source != exclude);
    }
    Ok(items)
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

    let bar = (*progress_bar).clone();
    let label = (*status_label).clone();
    let mut on_done = Some(on_done);
    let _ = glib::timeout_add_local(Duration::from_millis(150), move || {
        let snapshot = match state.lock() {
            Ok(snapshot) => snapshot.clone(),
            Err(poisoned) => {
                // Фоновый поток запаниковал, удерживая блокировку. Раньше опрос
                // просто прекращался: кнопка оставалась неактивной, а в статусе
                // навсегда оставалось «Подготовка…». Теперь показываем ошибку
                // и возвращаем кнопку в рабочее состояние.
                let mut guard = poisoned.into_inner();
                guard.finished = true;
                guard.success = false;
                guard.message = "Ошибка".to_string();
                guard.summary =
                    "внутренняя ошибка: фоновая операция завершилась аварийно".to_string();
                guard.clone()
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_uses_home_directory() {
        let home = default_home();
        assert_eq!(
            expand_tilde(PathBuf::from("~/Documents")),
            home.join("Documents")
        );
        assert_eq!(expand_tilde(PathBuf::from("~")), home);
    }

    #[test]
    fn expand_tilde_keeps_absolute_paths() {
        let path = PathBuf::from("/var/tmp/profile.rmm");
        assert_eq!(expand_tilde(path.clone()), path);
    }
}
