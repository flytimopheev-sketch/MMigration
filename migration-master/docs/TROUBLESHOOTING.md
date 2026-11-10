# Устранение неполадок

## Общие проверки

```bash
migration-master --version
migration-master scan --quick          # состав профиля и наличие утилит
migration-master config list           # действующие настройки
```

Предупреждения «утилита не найден» на главном экране GUI прямо указывают,
чего не хватает.

## Установка и сборка

### `error: linker 'link.exe' not found` / `Failed to run custom build command`

Не установлены компилятор и системные библиотеки. Для РЕД ОС:

```bash
sudo dnf install rust cargo gcc make pkgconf-pkg-config \
                 gtk4-devel libadwaita-devel sqlite-devel
```

### `pkg-config` не находит gtk4 / libadwaita

```bash
pkg-config --modversion gtk4
pkg-config --modversion libadwaita-1
```

Если версий нет — установите `-devel`-пакеты. Собрать без GUI (только CLI):

```bash
cargo build --release            # без фичи gui
```

### `cargo test` падает на тестах ресурсов

```
polkit::tests::test_shipped_policy_matches_render
```

Файл `resources/com.redos.migration-master.policy` устарел относительно кода.
Перегенерируйте его:

```bash
cargo test --lib update_shipped_policy -- --ignored
```

### Сборка RPM падает в `%check`

Логи: `~/rpmbuild/BUILD/<каталог>/cargo-test.log`, либо повторите вручную:

```bash
cargo test --release --locked
```

### `no matching package for gtk4` в rpmbuild

Графическая часть собирается по умолчанию, поэтому нужны devel-пакеты GTK4 и
libadwaita: `sudo dnf install gtk4-devel libadwaita-devel`. Собрать пакет без
графической части можно флагом `--without gui`.

## Архивы

### `Повреждённый архив` / `checksum mismatch`

```bash
migration-master verify profile.rmm --ask-passphrase
```

Если `verify` падает — архив повреждён (обрыв при переносе, битый носитель).
Восстановление невозможно: пересоздайте архив на исходной машине.

### `Неверный пароль` / `Decryption failed`

Пароль не совпадает. Пароль нигде не сохраняется — восстановить его нельзя.
Проверьте раскладку и регистр; архив без пароля не содержит поля шифрования.

### `Недостаточно места`

```bash
df -h /home
migration-master create --output /path/with/space/profile.rmm --dry-run
```

### Восстановление «прошло успешно», но существующие файлы не заменились

В 0.2.0 `ask` действительно спрашивает в интерактивном терминале; если stdin
не TTY, команда завершается понятной ошибкой, а не молча пропускает файлы.
В GUI диалог для каждого файла не реализован, поэтому по умолчанию выбран
безопасный `rename`, а выбор `ask` блокируется с пояснением. Явные стратегии:

```bash
migration-master restore profile.rmm --target /home/user \
  --strategy rename --ask-passphrase
```

`rename` сохранит старый файл как `*.old`, `replace` заменит его, `skip`
оставит существующий. Для GUI используйте `rename`/`replace`/`skip`/`newer`;
интерактивный `ask` доступен только в CLI с терминалом.

## Path traversal / `запись за пределы целевого каталога`

Это защита, а не сбой: в архиве обнаружен путь с `..` либо абсолютный путь.
Не используйте такой архив. Команда отклонит опасные записи и перечислит их
в отчёте.

## SSH

### `Не удалось подключиться` / timeout

```bash
ssh -v user@192.168.1.20         # проверка вручную
ping 192.168.1.20
```

Причины: порт 22 закрыт файрволом, узлы в разных подсетях, не тот ключ.

### `Host key verification failed`

Отпечаток узла не совпал (возможна подмена сервера). Сравните отпечаток на
обеих сторонах:

```bash
ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub
```

**Не отключайте проверку host key** (`StrictHostKeyChecking=no`) — это
устраняет защиту, а не проблему. Добавьте корректный отпечаток в `known_hosts`.

### `Permission denied (publickey)`

```bash
migration-master migrate-ssh user@host --identity ~/.ssh/id_ed25519 --dry-run
```

Проверьте права: `~/.ssh` — `0700`, ключ — `0600`, владелец — текущий
пользователь.

### Приватные ключи не перенесены

Это ожидаемо: по умолчанию `include_private_ssh_keys = false`, приватные ключи
не копируются (§4). Публичные ключи, `config` и `known_hosts` переносятся
всегда. Чтобы включить приватные ключи или `authorized_keys`, используйте
явные флаги:

```bash
migration-master create --output profile.rmm --ask-passphrase \
  --include-private-ssh-keys --include-authorized-keys
```

В GUI это отдельные галочки на странице создания (выключены по умолчанию).
После восстановления `~/.ssh` получает права `0700`, приватные ключи и
`authorized_keys` — `0600`. Содержимое ключей не попадает в журналы.

## Firefox, Thunderbird, Git и shell

Начиная с 0.2.0 пути правил приложений добавляются в скан: при выборе
`app_configs`/`app_data` в архив попадают `.mozilla/firefox`, `.thunderbird`,
`.gitconfig`, `.bashrc`, `.zshrc` и другие включённые правила. Если нужный
каталог не попал, проверьте правило в `migration-master app-rules list` или
`resources/examples/app-rules.yaml`; дополнительные каталоги можно перенести
отдельным архивом (см.
[HOST_IMAGE.md](HOST_IMAGE.md#шаг-3-дополнительные-каталоги-которые-не-входят-в-компоненты)).

## Принтеры

```bash
lpstat -p -d       # список и принтер по умолчанию
lpstat -v          # device URI
```

Если список пуст — CUPS не запущен: `sudo systemctl start cups`.
В 0.2.0 имя, модель, URI и принтер по умолчанию сохраняются в manifest.
Восстановление выполняется через `migration-master-helper configure-printers`
по PolicyKit только при явном флаге `--configure-printers` (в GUI — галочка).
Если `pkexec`/агент polkit недоступен, принтеры можно добавить вручную через
`http://localhost:631` или `lpadmin`; URI берите из `lpstat -v` на исходной
машине.

## Пакеты

```bash
migration-master list-packages --json
```

Пакеты **не копируются** как файлы. В 0.2.0 список явно установленных
пользователем пакетов сохраняется в manifest, при восстановлении показывается
diff с целевой системой. Установка выполняется через
`migration-master-helper install-packages` по PolicyKit только при явном
флаге `--install-packages` (в GUI — галочка) и только после подтверждения.
Без флага установите недостающее вручную (`sudo dnf install ...`). Подробный
сценарий — в [HOST_IMAGE.md](HOST_IMAGE.md#шаг-4-пакеты).

## Политики PolicyKit

### `pkexec не найден` / `агент авторизации не запущен`

```bash
sudo dnf install polkit
# агент должен быть запущен (polkit-gnome / polkit-kde / lxqt-policykit)
```

Это не блокирует обычную миграцию: файлы, dconf и cron обрабатываются от
пользователя. Без `pkexec`/агента polkit не будут работать только
`--install-packages` и `--configure-printers` (и соответствующие галочки GUI);
их можно выполнить вручную после восстановления.

### Запрос пароля появляется слишком часто

PolicyKit может запрашивать пароль администратора для каждой привилегированной
операции. Это ожидаемо и не кэшируется приложением. Политика и helper входят в
пакет: `resources/com.redos.migration-master.policy`.

## GUI

### `GLib-GIO-CRITICAL: This application can not open files.`

Сообщение означает, что GApplication воспринял аргументы командной строки как
файлы для открытия и завершил приложение с кодом 1 — окно не появляется.
В версиях до 0.1.4 это происходило при любом запуске `migration-master gui`.

Обновитесь до 0.1.4 или новее. Проверить, что графическая часть работает:

```bash
MIGRATION_MASTER_GUI_SELFTEST=1 migration-master gui
# ожидается: «GUI: окно создано и показано — самопроверка пройдена»
```

### Самопроверка графической части

```bash
MIGRATION_MASTER_GUI_SELFTEST=1 migration-master gui
```

Мастер создаёт окно, дожидается его показа и выходит с кодом 0. Если окно не
показалось — с сообщением об ошибке и ненулевым кодом. В stderr выводится
окружение:

```
GUI: GTK 4.8.3, libadwaita 1.2.2, display=x11::0
```

### `графический интерфейс недоступен`

Сообщение означает, что бинарник собран **без** графической части: команда
`migration-master gui` в такой сборке всегда отвечает этой подсказкой.

```bash
# что установлено
rpm -q migration-master
# есть ли GTK4/libadwaita в зависимостях пакета
rpm -qR migration-master | grep -E 'gtk|adwaita'
```

Если в списке зависимостей нет `libgtk-4.so.1` — установлен CLI-only пакет
(например, статическая musl-сборка из старых релизов или сборка
`rpmbuild --without gui`). Поставьте обычный релизный RPM с GUI:

```bash
sudo dnf install ./migration-master-0.2.0-1.x86_64.rpm
```

Сборка из исходников с графической частью:

```bash
cargo build --release --features gui
```

### `symbol lookup error: ... undefined symbol: adw_navigation_view_new`

Бинарник собран против libadwaita 1.4 (`NavigationView`, `ToolbarView`), а в
системе libadwaita 1.2 (РЕД ОС 8) — таких символов нет, и процесс падает при
запуске. Мастер использует только GTK4 ≥ 4.0 и libadwaita ≥ 1.0; версии
0.1.3 и новее такой ошибки не дают. Проверить версию:

```bash
rpm -q libadwaita
pkg-config --modversion libadwaita-1
```

### `не удалось открыть графическую сессию`

Приложение не смогло подключиться к X11/Wayland:

```bash
echo "$DISPLAY $WAYLAND_DISPLAY"     # должно быть непустым
loginctl show-session "$XDG_SESSION_ID" 2>/dev/null | head -3
GDK_BACKEND=wayland migration-master gui   # либо x11
```

Из-под `sudo` графический мастер не запускается — запускайте от обычного
пользователя. Консольный режим (`migration-master scan/create/restore`) работает
всегда.

### Окно не открывается / пустой экран

```bash
GDK_BACKEND=wayland migration-master gui   # либо x11
journalctl --user -n 50
tail -50 ~/.local/share/migration-master/logs/migration-master.log
```

В журнале записываются версии GTK4 и libadwaita, с которыми запустился мастер,
и причина сбоя инициализации.

### `error while loading shared libraries: libgtk-4.so.1`

В системе нет GTK4:

```bash
sudo dnf install gtk4 libadwaita
```

## Журналы

```bash
tail -100 ~/.local/share/migration-master/logs/migration-master.log
migration-master report --format json --output /tmp
```

Лог не содержит паролей и содержимого приватных ключей.

## Если ничего не помогло

Соберите для отчёта:

```bash
migration-master --version
cargo --version
cat /etc/os-release
migration-master scan --json > /tmp/scan.json
tail -200 ~/.local/share/migration-master/logs/migration-master.log
```

Не прикладывайте пароли и приватные ключи. См. [Восстановление после ошибки](RECOVERY.md).

## См. также

- [Руководство пользователя](USER_GUIDE_RU.md)
- [Установка](INSTALL.md)
- [Безопасность](SECURITY.md)
