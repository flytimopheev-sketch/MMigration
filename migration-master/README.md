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

## Установка

### Из исходников

```bash
cargo install --path .
```

### Из RPM пакета

```bash
# После сборки RPM (см. rpm/README.md)
sudo dnf install ./migration-master-*.rpm
```

## Использование

### Командная строка

```bash
# Сканирование профиля
migration-master scan

# Сканирование с выводом JSON
migration-master scan --json

# Быстрая оценка размера
migration-master scan --quick

# Создание архива
migration-master create-archive -o backup.rmm

# Создание зашифрованного архива
migration-master create-archive -o backup.rmm -p

# Проверка архива
migration-master inspect-archive backup.rmm

# Восстановление из архива
migration-master restore backup.rmm

# Восстановление с dry-run
migration-master restore backup.rmm --dry-run

# SSH миграция
migration-master migrate-ssh user@192.168.1.100

# SSH миграция с dry-run
migration-master migrate-ssh user@192.168.1.100 --dry-run

# Проверка хеша файла
migration-master verify file.txt --expected <hash>

# Список пакетов
migration-master list-packages

# Список принтеров
migration-master list-printers

# Генерация отчёта
migration-master report

# Конфигурация
migration-master config list
migration-master config reset
```

### GUI интерфейс

```bash
# Запуск GUI
migration-master gui

# Запуск с тёмной темой
migration-master gui --dark-theme
```

## Структура проекта

```
migration-master/
├── src/
│   ├── main.rs           # CLI точка входа
│   ├── lib.rs            # Библиотека
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
