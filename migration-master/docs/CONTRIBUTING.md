# Участие в разработке

Migration Master — Rust-проект для РЕД ОС Linux. CLI и GUI используют одну
библиотеку: весь функционал доступен из `migration-master` и как подкоманды
CLI, и из окна мастера.

## Требования

- Rust 1.75+ (edition 2021)
- Для GUI: `gtk4-devel`, `libadwaita-devel` (Linux)
- Для сборки RPM: `rpm-build`, `rpmdevtools`

## Быстрый старт

```bash
cargo build                 # отладочная сборка (CLI)
cargo check --all-targets   # быстрая проверка
cargo test                  # юнит-тесты
cargo clippy --all-targets  # линтер
```

С GUI (только Linux):

```bash
cargo build --features gui
cargo test --features gui
```

## Структура проекта

Слои (см. `docs/ARCHITECTURE.md`):

| Уровень | Модули | Назначение |
|---|---|---|
| 1. UI | `cli`, `ui` | команды и GTK4-мастер |
| 2. Сервисы | `wizard`, `archive`, `ssh_transfer`, `file_transfer`, `report` | сценарии миграции |
| 3. Домен | `config`, `profile_scanner`, `applications`, `ssh_keys`, `printers`, `packages`, `system_settings`, `compatibility`, `conflict_resolver`, `backup` | модели и правила |
| 4. Инфраструктура | `database`, `logging`, `platform`, `security`, `polkit`, `cancel` | хранилища и системный доступ |
| 5. Привилегированный backend | `src/bin/migration-master-helper.rs` | root-операции по белому списку |

Правило: **не добавляйте прямой вызов shell.** Внешние команды — только через
`std::process::Command` с отдельными аргументами.

## Проверка перед отправкой

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Чек-лист по коду:

- нет `unwrap()`/`expect()` в рабочих путях (допустимо в тестах и infallible-контекстах);
- нет конкатенации строк в командах (защита от command injection);
- новые пути проходят `security::validate_path`/`safe_join` (path traversal);
- секреты не попадают в `serde`-структуры (`skip_serializing`), логи и отчёты;
- долгие операции проверяют токен отмены (`cancel`);
- есть тесты на ветку «ошибка» и на «dry-run».

## Добавление компонента переноса

1. Вариант компонента — `ComponentType` (`src/config/mod.rs`):
   `default_paths`, `key`, `from_key`, `description`, `risk_level`.
2. Добавьте ключ в `parse_list`/`from_key` и в таблицу ключей README.
3. Тесты: пути внутри `HOME`, ключ↔вариант round-trip.

## Добавление правила приложения (§5)

```yaml
- app_name: "Моё приложение"
  config_paths: ["~/.config/myapp"]
  data_paths: ["~/.local/share/myapp"]
  transfer_mode: "copy"
  requires_restart: true
  warnings: ["не переносить кэш"]
  enabled: true
```

Файл: `~/.config/migration-master/app-rules.yaml` либо импорт
`migration-master app-rules import`. Пример: `resources/examples/app-rules.yaml`.
Тесты должны проверить загрузку примера (см. `test_example_rules_file_is_valid`).

## Изменение политик PolicyKit

`ALLOWED_ACTIONS` в `src/polkit/mod.rs` — единственный источник истины. После
правки перегенерируйте файл политики:

```bash
cargo test --lib update_shipped_policy -- --ignored
cargo test --lib polkit
```

Тест `test_shipped_policy_matches_render` не даст закоммитить расхождение.

## Коммиты и тесты

- Один коммит — одна логическая измена; сообщение в форме
  `component: что сделано` (например, `archive: проверка хешей при dry-run`).
- Новое поведение покрывается тестом; регрессия — тестом на воспроизведение.
- Документация обновляется вместе с кодом (README, `docs/`).

## Структура pull request

1. Что исправляет/добавляет и почему.
2. Как проверялось: `cargo test`, `cargo clippy`, при необходимости — сборка RPM.
3. Ограничения и известные риски.

## Сборка и проверка пакета

См. [BUILD_RPM.md](BUILD_RPM.md): `rpmbuild -ba` + `%check` запускает
`cargo test` внутри сборки.

## Лицензия

GPL-3.0. Отправляя изменения, вы соглашаетесь на распределение под этой лицензией.
