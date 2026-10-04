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
                 gtk4-devel libadwaita-devel sqlite-devel cups-devel
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

В `%build` включён `--with gui`, но не установлены зависимости. Либо
поставьте `gtk4-devel libadwaita-devel`, либо соберите без опции `--with gui`.

## Архивы

### `Повреждённый архив` / `checksum mismatch`

```bash
migration-master verify profile.rmm --passphrase '...'
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
всегда. Перенос приватных ключей возможен только при явном включении флага
`include_private_ssh_keys` в конфигурации мастера (`WizardConfig`) — в
текущей версии CLI и GUI отдельного переключателя нет, намеренно: это решение
должно приниматься осознанно, а не одним кликом. Приватные ключи никогда не
пишутся в журналы и отчёты, а после восстановления получают права `0600`.

## Принтеры

```bash
lpstat -e          # список принтеров
lpstat -p          # состояние
```

Если список пуст — CUPS не запущен: `sudo systemctl start cups`.
Перенос принтеров требует полного URI (`ipp://`, `lpd://`, `smb://`); локальные
`file://` на новой машине недоступны и отмечаются как конфликты.

## Пакеты

```bash
migration-master list-packages --json
```

Пакеты **не копируются** как файлы: сохраняется список, а на целевой машине
ставятся из репозиториев (через `migration-master-helper install-packages`).
Недоступный пакет попадает в отчёт с именем, версией, причиной и альтернативой.

## Политики PolicyKit

### `pkexec не найден` / `агент авторизации не запущен`

```bash
sudo dnf install polkit
# агент должен быть запущен (polkit-gnome / polkit-kde / lxqt-policykit)
```

Без этого привилегированные операции (установка пакетов, копирование в
`/etc/cups`) недоступны — обычные операции миграции продолжают работать.

### Запрос пароля появляется слишком часто

Каждое действие PolicyKit авторизуется отдельно — так задумано (белый
список, минимальные права). См. `resources/com.redos.migration-master.policy`.

## GUI

### `графический интерфейс недоступен`

Соберите с фичей `gui` на Linux:

```bash
cargo build --features gui --release
```

Команда `migration-master gui` без фичи возвращает подсказку, а не падает.

### Окно не открывается / пустой экран

```bash
GDK_BACKEND=wayland migration-master gui   # либо x11
journalctl --user -n 50
```

Проверьте наличие `libadwaita` и драйверов OpenGL.

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
