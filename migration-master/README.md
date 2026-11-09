# Migration Master

**Мастер миграции пользователя для РЕД ОС Linux**

> Это документация уровня пакета (`migration-master/`), который собирается в RPM.
> Обзор проекта, скриншоты и быстрый старт — в README репозитория:
> <https://github.com/flytimopheev-sketch/MMigration>

Приложение предназначено для безопасного переноса пользовательского профиля со старого компьютера или старой установки на новый компьютер.

## Возможности

- **Создание зашифрованного архива** профиля пользователя
- **Прямая миграция по SSH** между компьютерами (`scp` или `rsync --partial`)
- **Восстановление из архива** с выбором компонентов, предпросмотром,
  отменой, автоматическим backup и отчётом
- **Перенос файлов и настроек** из `~/.config` и `~/.local/share` с учётом
  правил приложений: Firefox, Thunderbird, Git, shell-конфиги и другие пути
  добавляются в скан автоматически
- **Перенос SSH-ключей**: публичные ключи всегда, приватные ключи и
  `authorized_keys` — только при явном флаге/галочке, с проверкой прав `0700`
  и `0600`
- **Принтеры CUPS**: имя, модель, URI и принтер по умолчанию сохраняются в
  manifest; восстановление через `lpadmin`/`cupsenable`/`cupsaccept` и
  помощник PolicyKit
- **Пакеты**: список явно установленных пользователем пакетов сохраняется в
  manifest, при восстановлении показывается diff и установка идёт через
  помощник только после подтверждения
- **dconf и cron**: дамп dconf и crontab сохраняются и применяются от имени
  пользователя после восстановления
- **Проверка контрольных сумм** файлов
- **Журналирование операций** в SQLite и отчёты JSON/HTML/TXT

## Статус реализации

Готово и покрыто тестами:

- ядро: сканирование профиля, создание/просмотр/проверка/восстановление архива `.rmm`;
- шифрование age, SHA-256, защита от path traversal, атомарная запись файлов;
- режим `dry-run` без побочных эффектов (не создаёт ни файлов, ни каталогов,
  не делает резервных копий);
- частичный `config.toml` допустим: недостающие поля берутся из значений
  по умолчанию, битый TOML по-прежнему возвращает ошибку;
- SSH-политика ключей, стратегии конфликтов: интерактивный `ask` в TTY и
  явная ошибка без TTY, безопасный `rename` по умолчанию в GUI, отмена
  операций (Ctrl+C), SQLite-история, отчёты (JSON/HTML/TXT), резервные копии
  в GUI и CLI;
- единый сканер для `scan` и `create`, включая пути правил приложений и
  XDG-каталоги;
- пакеты, принтеры, dconf и cron: метаданные в manifest, diff, применение
  через пользователя или помощник PolicyKit;
- CLI: `scan`, `create`, `inspect`, `verify`, `restore`, `migrate-ssh`, `report`,
  `list-packages`, `list-printers`, `config`, `app-rules`;
- GUI на GTK4/libadwaita (`migration-master gui`, фича `gui`): главный экран,
  создание архива с предпросмотром, перенос по SSH, восстановление с
  предпросмотром и флагами пакетов/принтеров/dconf+cron, история и диагностика.

Осталось необязательное: полноценный диалог конфликтов в GUI, ACL/xattr/
SELinux/capabilities/hardlinks в архиве и расширенные e2e-тесты. Подробный
разбор — в [IMPROVEMENTS.md](docs/IMPROVEMENTS.md).

## Документация

| Документ | Содержание |
|---|---|
| [Установка](docs/INSTALL.md) | RPM и сборка из исходников, внешние утилиты |
| [Руководство пользователя](docs/USER_GUIDE_RU.md) | оба режима миграции, компоненты, конфликты |
| [Образ хоста без root](docs/HOST_IMAGE.md) | снимок профиля, дополнения, полный образ диска |
| [План улучшений](docs/IMPROVEMENTS.md) | статус P0/P1 в 0.2.0 и оставшиеся пункты |
| [Безопасность](docs/SECURITY.md) | модель угроз, шифрование, path traversal, polkit |
| [Резервное копирование](docs/BACKUP.md) | создание, проверка и откат копий |
| [Восстановление после ошибки](docs/RECOVERY.md) | откат и разбор сбоев |
| [Устранение неполадок](docs/TROUBLESHOOTING.md) | типовые ошибки и диагностика |
| [Сборка RPM](docs/BUILD_RPM.md) | rpmbuild, `%check`, проверка пакета |
| [Участие в разработке](docs/CONTRIBUTING.md) | структура проекта, чек-лист, тесты |
| [Архитектура](docs/ARCHITECTURE.md) | слои, потоки данных, формат архива |
| Man-страница | `man migration-master` |

Примеры: [`resources/examples/config.toml`](resources/examples/config.toml),
[`resources/examples/app-rules.yaml`](resources/examples/app-rules.yaml),
политика PolicyKit [`resources/com.redos.migration-master.policy`](resources/com.redos.migration-master.policy).


## Требования

- РЕД ОС Linux (x86_64)
- Rust 1.75+
- GTK4 ≥ 4.0 и libadwaita ≥ 1.0 (для GUI; на РЕД ОС 8 — `gtk4` и `libadwaita`)
- OpenSSH клиент (`ssh`, `scp`)
- CUPS (`lpstat`) — необязательно, только для списка принтеров
- `rsync` — необязательно, ускоряет SSH-передачу
- `tar`, `zstd`, `gpg` — необязательно, для ручных дополнительных архивов

Сами `.rmm` создаются Rust-библиотеками; внешние `tar`/`zstd` для работы
программы не требуются.

## Сборка

### Установка зависимостей

```bash
# Для РЕД ОС
sudo dnf install rust cargo gtk4-devel libadwaita-devel sqlite-devel

# Дополнительные зависимости
sudo dnf install openssh-clients
```

### Сборка проекта

```bash
cd migration-master

# Отладочная сборка
cargo build

# Релизная сборка
cargo build --release

# Сборка с GUI
cargo build --features gui --release
```

## Загрузка и сборка RPM

Готовые пакеты публикуются в GitHub Releases:
<https://github.com/flytimopheev-sketch/MMigration/releases>

```bash
# Скачать последний релиз
gh release download v0.2.0 -p '*.rpm'

# Установить на РЕД ОС
sudo dnf install ./migration-master-0.2.0-1.x86_64.rpm
```

Пакет собирается в CI на Linux (без Docker): workflow
`.github/workflows/release-rpm.yml` запускается по тегу `v*` или вручную
(Actions → Release RPM → Run workflow). Он выполняет `rpmbuild -bb` с
`%check` (внутри — `cargo test`) и прикрепляет RPM к релизу.

По умолчанию пакет собирается **с GUI** (GTK4 + libadwaita) через
cargo-zigbuild с потолком glibc 2.28: бинарники не зависят от версии glibc
хоста CI и устанавливаются на РЕД ОС (раньше установка падала с
`libc.so.6(GLIBC_2.xx)(64bit) is needed`) — см. [Сборка RPM](docs/BUILD_RPM.md).
Статический musl-вариант (только CLI, без GUI) собирается ручным запуском
workflow с параметром `variant=musl`.

> Проверка кода — workflow `.github/workflows/rust.yml`: `cargo fmt --check`,
> `clippy -D warnings --all-features` (включая GUI-код), сборка и тесты CLI
> и GUI.

## Установка

### Из исходников

```bash
cargo install --path .
```

### Из RPM пакета

```bash
sudo dnf install ./migration-master-*.rpm
```

## Использование

### Командная строка

```bash
# Справка по всем командам
migration-master --help

# Сканирование профиля
migration-master scan
migration-master scan --json
migration-master scan --quick
migration-master scan --home /home/user --components all

# Создание архива профиля
migration-master create --output profile.rmm
migration-master create --output profile.rmm --ask-passphrase
migration-master create --output profile.rmm --passphrase-file ~/.rmm-pass
migration-master create --output profile.rmm --components documents,ssh_keys

# Приватные ключи и authorized_keys — только при явном согласии
migration-master create --output profile.rmm \
  --include-private-ssh-keys --include-authorized-keys

# Просмотр содержимого без восстановления
migration-master inspect profile.rmm

# Проверка целостности и хешей SHA-256
migration-master verify profile.rmm

# Восстановление из архива (backup создаётся автоматически)
migration-master restore profile.rmm --target ./restore-test --dry-run
migration-master restore profile.rmm --target ./restore-test --components documents,ssh_keys
migration-master restore profile.rmm --target ./restore-test --strategy rename
migration-master restore profile.rmm --target /home/user \
  --install-packages --configure-printers --apply-system-settings
migration-master restore profile.rmm --target /home/user --no-backup

# Список пакетов и принтеров
migration-master list-packages
migration-master list-printers

# Настройки
migration-master config path
migration-master config list
migration-master config reset

# Графический мастер (требует сборки с --features gui на Linux)
migration-master gui
```

> Для зашифрованных архивов используйте `--ask-passphrase` (интерактивный
> ввод), `--passphrase-file <файл>` или `--passphrase '<пароль>'` командам
> `inspect`, `verify`, `restore`. Пароль в аргументах виден в `ps` и истории
> оболочки, поэтому первые два способа предпочтительнее; нигде не сохраняется.

### Графический интерфейс

GUI-мастер (GTK4/libadwaita) собирается на Linux с фичей `gui`:

```bash
cargo build --features gui --release
./target/release/migration-master gui
```

Требования к графической подсистеме: **GTK4 ≥ 4.0** и **libadwaita ≥ 1.0**
(мастер намеренно не использует API libadwaita 1.4 — `NavigationView`,
`ToolbarView`, — и API GTK 4.4 — `PasswordEntry`, иначе бинарник не запустился
бы на РЕД ОС 8 с libadwaita 1.2: «undefined symbol: adw_navigation_view_new»).

Мастер открывает главный экран со сведениями о системе и точками входа:
создание архива профиля, прямой перенос по SSH, восстановление из архива,
история операций и диагностика. Долгие операции выполняются в фоновом потоке
с индикатором прогресса, результат показывается диалогом.

Если графическая сессия недоступна (нет `DISPLAY`/`WAYLAND_DISPLAY`, запуск
из-под `sudo`), команда `gui` выводит причину и подсказку, а не завершается
молча.

На других платформах (и без фичи `gui`) команда `gui` возвращает понятную
ошибку с подсказкой использовать консольный режим.

## Структура проекта

```
migration-master/
├── src/
│   ├── main.rs           # CLI точка входа
│   ├── lib.rs            # Библиотека
│   ├── cancel.rs         # Отмена длительных операций (Ctrl+C)
│   ├── cli/              # CLI интерфейс
│   ├── config/           # Конфигурация
│   ├── database/         # SQLite база данных
│   ├── logging/          # Журналирование
│   ├── security/         # Шифрование и хеширование
│   ├── profile_scanner/  # Сканирование профиля
│   ├── archive/          # Работа с архивами
│   ├── ssh_transfer/     # SSH передача
│   ├── file_transfer/    # Передача файлов
│   ├── conflict_resolver/# Разрешение конфликтов
│   ├── ssh_keys/         # SSH ключи
│   ├── applications/     # Настройки приложений
│   ├── printers/         # Принтеры CUPS
│   ├── packages/         # RPM пакеты
│   ├── system_settings/  # Системные настройки
│   ├── compatibility/    # Проверка совместимости
│   ├── backup/           # Резервное копирование
│   ├── polkit/           # Polkit интеграция
│   ├── report/           # Отчёты
│   ├── wizard/           # Мастер миграции
│   └── ui/               # GUI интерфейс (GTK4)
├── tests/                # Тесты
├── resources/            # Ресурсы (иконки, UI файлы)
├── docs/                 # Документация
├── rpm/                  # RPM spec файл
└── Cargo.toml
```

## Формат архива

Архивы имеют расширение `.rmm`. Это zstd-сжатый tar, при наличии пароля
целиком зашифрованный age:

- `manifest.json` — первая запись tar: метаданные, список файлов и их SHA-256;
- файлы профиля с относительными путями;
- отдельного `data.zst` и `checksum.sha256` внутри нет: сжатие и шифрование
  применяются ко всему tar-потоку, хеши лежат в манифесте.

### Структура manifest.json

```json
{
  "format_version": 1,
  "created_at": "2026-11-06T10:30:00Z",
  "source_hostname": "old-pc",
  "source_user": "user",
  "source_uid": 1000,
  "source_gid": 1000,
  "app_version": "0.2.0",
  "os_info": {
    "name": "РЕД ОС",
    "version": "8.0",
    "architecture": "x86_64"
  },
  "components": ["documents", "ssh_keys", "app_configs"],
  "files": [
    {
      "relative_path": "Documents/report.txt",
      "size": 1234,
      "hash": "sha256:...",
      "mode": 420,
      "uid": 1000,
      "gid": 1000,
      "is_symlink": false,
      "component": "documents"
    }
  ],
  "total_size": 1234,
  "total_files": 1,
  "encrypted": true,
  "compression": "zstd",
  "metadata": {
    "packages": [
      { "name": "firefox", "version": "128.0-1" }
    ],
    "printers": [
      {
        "name": "Office",
        "model": "everywhere",
        "device_uri": "ipp://printer.local/ipp/print",
        "shared": false,
        "default": true
      }
    ],
    "dconf": "[org/gnome/desktop/interface]\ncolor-scheme='prefer-dark'\n",
    "crontab": "0 9 * * 1-5 /home/user/bin/backup.sh\n",
    "warnings": []
  }
}
```

## Безопасность

- **Шифрование**: age с passphrase
- **Хеширование**: SHA-256 для проверки целостности
- **SSH**: host key проверяется вручную (используется `accept-new` — TOFU),
  поддерживаются ключи Ed25519
- **Path traversal защита**: проверка путей при восстановлении
- **Polkit**: привилегированный помощник `migration-master-helper` с белым
  списком команд; GUI вызывает его через `pkexec` только для явно
  подтверждённых установки пакетов и настройки принтеров. Системные файлы
  (`/etc`) вслепую не копируются

Подробнее — [SECURITY.md](docs/SECURITY.md) и
[IMPROVEMENTS.md](docs/IMPROVEMENTS.md).

## Лицензия

GPL-3.0

## Контакты

- Repository: https://github.com/flytimopheev-sketch/MMigration
- Issue Tracker: https://github.com/flytimopheev-sketch/MMigration/issues
- Releases: https://github.com/flytimopheev-sketch/MMigration/releases
- Team: team@redos.local
