# Архитектура Migration Master

## Общая структура

Приложение разделено на следующие модули:

### 1. Core (Ядро)
- `config` - Конфигурация приложения и настройки
- `database` - SQLite база данных для хранения истории
- `logging` - Система журналирования операций
- `security` - Шифрование (age), хеширование (SHA-256), безопасность

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
- `wizard` - Пошаговый мастер миграции
- `ui` - Графический интерфейс GTK4/libadwaita
- `report` - Генерация отчётов

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
- Привилегированные операции выполняются через polkit
- Минимальные необходимые права
- Журналирование всех привилегированных действий

## Формат архива

```
profile.rmm/
├── manifest.json       # Метаданные
├── checksum.sha256     # Контрольная сумма манифеста
├── data.zst.age        # Зашифрованные сжатые данные
└── signatures/         # Подписи (опционально)
    └── manifest.sig
```

### manifest.json структура

```json
{
  "version": 1,
  "format": "rmm",
  "created_at": "2024-01-15T10:30:00Z",
  "source": {
    "hostname": "old-pc",
    "username": "user",
    "uid": 1000,
    "gid": 1000,
    "os": "RED OS 7.3",
    "app_version": "0.1.0"
  },
  "components": ["documents", "ssh_keys", "app_configs"],
  "files": [...],
  "total_size": 5678901234,
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
