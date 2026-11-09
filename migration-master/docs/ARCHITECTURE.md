# Архитектура Migration Master

## Общая структура

Приложение разделено на следующие модули:

### 1. Core (Ядро)
- `config` - Конфигурация приложения и настройки
- `database` - SQLite база данных для хранения истории
- `logging` - Система журналирования операций
- `security` - Шифрование (age), хеширование (SHA-256), безопасность
- `cancel` - Отмена длительных операций (Ctrl+C, кнопка «Отмена»)

### 2. Profile Analysis (Анализ профиля)
- `profile_scanner` - Сканирование домашнего каталога пользователя
- `compatibility` - Проверка совместимости систем

### 3. Data Transfer (Передача данных)
- `archive` - Создание и чтение архивов (.rmm формат)
- `ssh_transfer` - Прямая передача по SSH
- `file_transfer` - Копирование файлов с прогрессом

### 4. Migration Components (Компоненты миграции)
- `ssh_keys` - Перенос SSH ключей с проверкой безопасности
- `applications` - Настройки приложений (Firefox, LibreOffice, VS Code)
- `printers` - Принтеры CUPS
- `packages` - Список RPM пакетов
- `system_settings` - Системные настройки

### 5. Safety & Recovery (Безопасность и восстановление)
- `backup` - Резервное копирование перед изменениями
- `conflict_resolver` - Разрешение конфликтов файлов
- `polkit` - Привилегированные операции через polkit

### 6. User Interface (Интерфейс)
- `cli` - Интерфейс командной строки
- `wizard` - Оркестратор миграции (не путать с пошаговым GUI)
- `ui` - Графический интерфейс GTK4/libadwaita
- `report` - Генерация отчётов

> **Статус 0.2.0.** `applications`, `printers`, `packages`, `system_settings`
> и `polkit` подключены к основному потоку. Правила приложений добавляют свои
> пути в скан, метаданные пакетов/принтеров/dconf/cron сохраняются в manifest,
> а установка пакетов и настройка принтеров выполняются помощником
> `migration-master-helper` через `pkexec` только после явного подтверждения.
> dconf и crontab применяются от имени пользователя. Остались необязательные
> улучшения (ACL/xattr/SELinux, диалог конфликтов в GUI) — см.
> [IMPROVEMENTS.md](IMPROVEMENTS.md).

## Поток данных

```
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│   CLI/UI    │────▶│   Wizard     │────▶│   Profile   │
│             │     │              │     │   Scanner   │
└─────────────┘     └──────────────┘     └─────────────┘
                                              │
                    ┌─────────────────────────┼─────────────────────────┐
                    │                         │                         │
           ┌────────▼────────┐      ┌────────▼────────┐      ┌────────▼────────┐
           │    Archive      │      │    SSH          │      │    Restore      │
           │    Creation     │      │    Transfer     │      │    Engine       │
           └────────┬────────┘      └────────┬────────┘      └────────┬────────┘
                    │                        │                        │
                    │                ┌───────▼────────┐               │
                    │                │   Security     │               │
                    │                │   (Encryption) │               │
                    │                └────────────────┘               │
                    └────────────────────────┬───────────────────────┘
                                             │
                                    ┌────────▼────────┐
                                    │    Database     │
                                    │    (SQLite)     │
                                    └─────────────────┘
```

## Безопасность

### Шифрование
- Используется age для шифрования архивов
- Пароль не сохраняется в памяти после использования
- Поддержка passphrase с оценкой сложности

### SSH
- Проверка host key fingerprint
- Поддержка ключей Ed25519
- Таймауты и повторные подключения
- Никаких приватных ключей без явного подтверждения

### Path Traversal Защита
- Валидация всех путей относительно базовой директории
- Запрет символов `..` в путях архива
- Проверка canonical paths

### Polkit
- Привилегированные операции выполняет отдельный helper с белым списком:
  установка пользовательских пакетов и настройка принтеров CUPS
- GUI не запускается от root: он вызывает `pkexec` и helper только после
  явной галочки/флага, остальные операции выполняются от пользователя
- Helper проверяет аргументы (имена пакетов, имя/URI/модель принтера) и не
  принимает произвольные shell-команды; вызовы журналируются

## Формат архива

```
profile.rmm
└── zstd-tar (если задан пароль — целиком зашифрован age)
    ├── manifest.json       # Первая запись: метаданные и хеши файлов
    └── файлы профиля       # Относительные пути внутри $HOME
```

Отдельных `data.zst`, `checksum.sha256` и каталога `signatures/` в формате
нет. `manifest.json` содержит `manifest_hash`; `verify` сверяет его и SHA-256
каждого файла. Необязательное поле `metadata` хранит пакеты, принтеры, дамп
dconf и crontab; старые архивы без него читаются благодаря `serde(default)`.

### manifest.json структура

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
    "packages": [{ "name": "firefox", "version": "128.0-1" }],
    "printers": [{
      "name": "Office",
      "model": "everywhere",
      "device_uri": "ipp://printer.local/ipp/print",
      "shared": false,
      "default": true
    }],
    "dconf": "[org/gnome/desktop/interface]\ncolor-scheme='prefer-dark'\n",
    "crontab": "0 9 * * 1-5 /home/user/bin/backup.sh\n",
    "warnings": []
  }
}
```

## Обработка ошибок

Все ошибки обрабатываются через тип `MigrationError`:

```rust
pub enum MigrationError {
    Io(Error),
    Json(Error),
    Database(Error),
    Ssh(Error),
    Encryption(String),
    FileNotFound(String),
    PermissionDenied(String),
    InsufficientSpace { required: u64, available: u64 },
    Cancelled,
    Conflict(String),
    ChecksumMismatch,
    PathTraversal(String),
    // ...
}
```

## Тестирование

### Unit тесты
- Тесты отдельных функций
- Mock внешних зависимостей

### Интеграционные тесты
- Тесты создания/восстановления архива
- Тесты SSH подключения (с тестовым сервером)
- Тесты conflict resolution

### Тесты безопасности
- Path traversal атаки
- Проверка прав SSH файлов
- Шифрование/расшифрование

## Расширяемость

### Правила приложений
Добавление поддержки нового приложения через YAML:

```yaml
app_name: "MyApp"
config_paths:
  - "~/.config/myapp"
data_paths:
  - "~/.local/share/myapp"
transfer_mode: "copy"
requires_restart: true
min_version: "1.0"
warnings:
  - "Не переносить кэш"
post_migration_hook: "myapp-migrate-hook"
```

### Плагины
Возможность добавления собственных hooks через систему плагинов.
