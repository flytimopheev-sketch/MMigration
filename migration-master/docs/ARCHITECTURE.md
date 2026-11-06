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

> **Статус 0.1.5.** `applications`, `printers`, `packages`,
> `system_settings` и `polkit` реализованы частично: модули и структуры есть,
> но не все вызовы подключены к основному потоку. Правила приложений сейчас
> влияют на размеры/исключения, но не добавляют свои пути в скан. Пакеты,
> принтеры, системные настройки и cron не архивируются автоматически.
> Подробный список — в [IMPROVEMENTS.md](IMPROVEMENTS.md).

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
- Для привилегированных операций предусмотрен helper с белым списком
- В 0.1.5 helper не вызывается из GUI/CLI; интеграция запланирована
- Минимальные необходимые права и журналирование — в будущих вызовах

## Формат архива

```
profile.rmm
└── zstd-tar (если задан пароль — целиком зашифрован age)
    ├── manifest.json       # Первая запись: метаданные и хеши файлов
    └── файлы профиля       # Относительные пути внутри $HOME
```

Отдельных `data.zst`, `checksum.sha256` и каталога `signatures/` в формате
0.1.5 нет. `manifest.json` содержит `manifest_hash`; `verify` сверяет его и
SHA-256 каждого файла.

### manifest.json структура

```json
{
  "format_version": 1,
  "created_at": "2026-11-06T10:30:00Z",
  "source_hostname": "old-pc",
  "source_user": "user",
  "source_uid": 1000,
  "source_gid": 1000,
  "app_version": "0.1.5",
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
  "compression": "zstd"
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
