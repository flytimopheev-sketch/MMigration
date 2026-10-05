# Migration Master

**Мастер миграции пользователя для РЕД ОС Linux**

Приложение предназначено для безопасного переноса пользовательского профиля со старого компьютера или старой установки на новый компьютер.

## Возможности

- **Создание зашифрованного архива** профиля пользователя
- **Прямая миграция по SSH** между компьютерами
- **Восстановление из архива** с выбором компонентов
- **Перенос настроек приложений** (Firefox, LibreOffice, VS Code и др.)
- **Перенос SSH-ключей** с соблюдением безопасности
- **Перенос принтеров** CUPS
- **Экспорт списка пакетов** RPM
- **Проверка контрольных сумм** файлов
- **Журналирование операций** в SQLite

## Статус реализации

Готово и покрыто тестами:

- ядро: сканирование профиля, создание/просмотр/проверка/восстановление архива `.rmm`;
- шифрование age, SHA-256, защита от path traversal, атомарная запись файлов;
- режим `dry-run` без побочных эффектов (не создаёт ни файлов, ни каталогов,
  не делает резервных копий);
- частичный `config.toml` допустим: недостающие поля берутся из значений
  по умолчанию, битый TOML по-прежнему возвращает ошибку;
- SSH-политика ключей, разрешение конфликтов, отмена операций (Ctrl+C), SQLite-история,
  отчёты (JSON/HTML/TXT/PDF), резервные копии;
- CLI: `scan`, `create`, `inspect`, `verify`, `restore`, `migrate-ssh`, `report`,
  `list-packages`, `list-printers`, `config`, `app-rules`;
- GUI-мастер GTK4/libadwaita (`migration-master gui`, фича `gui`): главный экран,
  создание архива, перенос по SSH, восстановление, история и диагностика.

Проверяется на целевой РЕД ОС и дорабатывается:

- детализация страниц мастера (пошаговый выбор исключений и параметров, интерактивное
  разрешение конфликтов, предварительный просмотр плана);
- привилегированные операции через polkit-помощник, вызываемые из GUI;
- сборка RPM (сборка выполняется на РЕД ОС, `rpmbuild -ba`).

## Документация

| Документ | Содержание |
|---|---|
| [Установка](docs/INSTALL.md) | RPM и сборка из исходников, внешние утилиты |
| [Руководство пользователя](docs/USER_GUIDE_RU.md) | оба режима миграции, компоненты, конфликты |
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
- GTK4 + libadwaita (для GUI)
- OpenSSH клиент
- CUPS (для работы с принтерами)

## Сборка

### Установка зависимостей

```bash
# Для РЕД ОС
sudo dnf install rust cargo gtk4-devel libadwaita-devel sqlite-devel

# Дополнительные зависимости
sudo dnf install openssh-clients cups-devel
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
gh release download v0.1.1 -p '*.rpm'

# Установить на РЕД ОС
sudo dnf install ./migration-master-0.1.1-1.x86_64.rpm
```

Пакет собирается в CI на Linux (без Docker): workflow
`.github/workflows/release-rpm.yml` запускается по тегу `v*` или вручную
(Actions → Release RPM → Run workflow). Он выполняет `rpmbuild -bb` с
`%check` (внутри — `cargo test`) и прикрепляет RPM к релизу.

По умолчанию пакет собирается **статически (musl)**: бинарники не зависят
от версии glibc хоста CI, поэтому устанавливаются на любую РЕД ОС
(иначе установка падала с `libc.so.6(GLIBC_2.xx)(64bit) is needed`) —
см. [Сборка RPM](docs/BUILD_RPM.md). Вариант с GUI (для Ubuntu) собирается
только при ручном запуске с флагом `with_gui`.

Проверка кода — workflow `.github/workflows/rust.yml`: `cargo fmt --check`,
`clippy -D warnings`, сборка и тесты CLI, а также сборка GUI с `--features gui`.

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
migration-master create --output profile.rmm --passphrase
migration-master create --output profile.rmm --components documents,ssh_keys

# Просмотр содержимого без восстановления
migration-master inspect profile.rmm

# Проверка целостности и хешей SHA-256
migration-master verify profile.rmm

# Восстановление из архива
migration-master restore profile.rmm --target ./restore-test --dry-run
migration-master restore profile.rmm --target ./restore-test --components documents,ssh_keys
migration-master restore profile.rmm --target ./restore-test --strategy replace

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

> Для зашифрованных архивов передавайте `--passphrase '<пароль>'` командам
> `inspect`, `verify`, `restore`. Пароль нигде не сохраняется.

### Графический интерфейс

GUI-мастер (GTK4/libadwaita) собирается на Linux с фичей `gui`:

```bash
cargo build --features gui --release
./target/release/migration-master gui
```

Мастер открывает главный экран со сведениями о системе и точками входа:
создание архива профиля, прямой перенос по SSH, восстановление из архива,
история операций и диагностика. Долгие операции выполняются в фоновом потоке
с индикатором прогресса, результат показывается диалогом.

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

Архивы имеют расширение `.rmm` и содержат:

- `manifest.json` - метаданные архива
- `data.zst` - сжатые данные
- `checksum.sha256` - контрольная сумма

### Структура manifest.json

```json
{
  "version": 1,
  "created_at": "2024-01-15T10:30:00Z",
  "source_hostname": "old-pc",
  "username": "user",
  "uid": 1000,
  "gid": 1000,
  "os_info": "RED OS 7.3",
  "app_version": "0.1.0",
  "encrypted": true,
  "components": ["documents", "ssh_keys", "app_configs"],
  "files_count": 1234,
  "total_size": 5678901234
}
```

## Безопасность

- **Шифрование**: age с passphrase
- **Хеширование**: SHA-256 для проверки целостности
- **SSH**: проверка host key, поддержка ключей Ed25519
- **Path traversal защита**: проверка путей при восстановлении
- **Polkit**: привилегированные операции через polkit

## Лицензия

GPL-3.0

## Контакты

- Repository: https://github.com/redos/migration-master
- Issue Tracker: https://github.com/redos/migration-master/issues
