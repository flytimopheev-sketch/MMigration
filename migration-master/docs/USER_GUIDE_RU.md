# Руководство пользователя (RU)

Migration Master переносит пользовательский профиль со старого компьютера на
новый: файлы, настройки приложений, SSH-ключи, список пакетов и принтеров.

Два режима:

1. **Локальный архив** — создаёте зашифрованный `.rmm` на старом компьютере,
   переносите его на новый (флешкой) и восстанавливаете.
2. **Прямая миграция по SSH** — данные передаются по сети напрямую.

## Перед началом

1. Убедитесь, что на обоих компьютерах установлены `ssh`, `tar`, `zstd`
   (обязательные) и, при необходимости, `rsync`, CUPS.
2. Проверьте свободное место: размер профиля × 2 (архив + копия).
3. Отключите защищённые приложения (браузеры, почтовые клиенты) — их файлы
   должны быть закрыты.

## Главный экран

GUI показывает: пользователя, имя компьютера, версию ОС, свободное место,
размер домашнего каталога, наличие утилит и предупреждения об отсутствующих
компонентах.

## Режим 1. Локальный архив

### Старый компьютер

```bash
# Создание (GUI: «Создать архив профиля»)
migration-master create --output profile.rmm --passphrase '...'
```

Без `--passphrase` архив создаётся без шифрования (не рекомендуется для
переноса через флешку). Оценка без записи:

```bash
migration-master create --output profile.rmm --dry-run
```

Проверка перед переносом:

```bash
migration-master verify profile.rmm --passphrase '...'
migration-master inspect profile.rmm --passphrase '...'
```

### Новый компьютер

```bash
# Обязательно сначала dry-run
migration-master restore profile.rmm --target /home/user --dry-run --passphrase '...'

# Реальное восстановление
migration-master restore profile.rmm --target /home/user \
    --strategy replace --passphrase '...'
```

`--components documents,ssh_keys` — восстановить только выбранные компоненты.

## Режим 2. Прямая миграция по SSH

```bash
# Проверка соединения и плана (без передачи)
migration-master migrate-ssh user@192.168.1.20 --dry-run

# Реальная передача
migration-master migrate-ssh user@192.168.1.20 --components default
```

Перед подключением проверьте **отпечаток узла (host key)** — проверка никогда
не отключается автоматически. Приватный ключ: `--identity ~/.ssh/id_ed25519`;
каталог на приёмнике: `--remote-home /home/user`.

## Компоненты

| Ключ | Что переносится |
|---|---|
| `desktop`, `documents`, `downloads`, `pictures`, `videos`, `music`, `templates` | стандартные каталоги |
| `app_configs`, `app_data` | `~/.config`, `~/.local/share` |
| `local_bin`, `local_apps` | `~/.local/bin`, `.local/share/applications` |
| `themes`, `icons`, `fonts` | оформление |
| `ssh_keys` | `~/.ssh` (приватные — только по согласию) |
| `printers`, `packages`, `system_settings` | принтеры, RPM, системные настройки |
| `cron_jobs`, `user_services` | cron и user-systemd |

Список: `migration-master scan --components all`.

**Не переносится автоматически:** кэш, `Trash`, временные файлы, сокеты,
lock-файлы, данные других пользователей (см. `DEFAULT_EXCLUSIONS`).

## Настройки приложений (§5)

Встроенные правила: Firefox, Chromium/Chrome, Thunderbird, LibreOffice,
VS Code, Git, терминал, SSH, systemd (пользователь), файловые менеджеры.

```bash
migration-master app-rules list --json      # правила и объёмы
migration-master app-rules export           # выгрузить в YAML
migration-master app-rules import my.yaml   # загрузить свои
```

Пример правил: `resources/examples/app-rules.yaml` (в RPM —
`/usr/share/migration-master/examples/`). Выключите правило
(`enabled: false`), чтобы его пути исключались из сканирования.

Post-migration hooks выполняются **только** с явным подтверждением:

```bash
migration-master app-rules run-hooks --confirm
```

## Конфликты файлов

`--strategy`:

| Значение | Поведение |
|---|---|
| `ask` (по умолчанию) | спросить для каждого файла |
| `skip` | оставить существующий |
| `replace` | заменить |
| `keep-both` | сохранить оба (суффикс у нового) |
| `rename` (`rename-old`) | переименовать старый в `.old` |
| `newer` | заменить только если в архиве новее |

Перед массовой заменой создаётся резервная копия.

## История и отчёты

```bash
migration-master report --format html --limit 50
migration-master config path      # ~/.config/migration-master/config.toml
migration-master config list
```

Отчёт создаётся после каждой операции в форматах `json`, `html`, `txt` (и `pdf`
при наличии `libreoffice`/`pandoc`).

## Графический мастер

```bash
cargo build --release --features gui
migration-master gui
```

Шаги мастера: приветствие → подключение/архив → компоненты → исключения →
параметры → предпросмотр → совместимость → резервная копия → выполнение →
конфликты → отчёт → завершение. Кнопки «Назад», «Далее», «Отмена» доступны на
каждом шаге; состояние сохраняется между запусками.

## Пример полного цикла

```bash
# 1. Старый ПК
migration-master scan --json > scan.json
migration-master create --output profile.rmm --passphrase '...'

# 2. Перенести profile.rmm на новый ПК

# 3. Новый ПК
migration-master verify profile.rmm --passphrase '...'
migration-master restore profile.rmm --target /home/user --dry-run --passphrase '...'
migration-master restore profile.rmm --target /home/user --strategy replace --passphrase '...'
migration-master report --format txt
```

## Перенос приватных SSH-ключей

По умолчанию приватные ключи **не переносятся** (§4): публичные ключи,
`~/.ssh/config` и `known_hosts` переносятся всегда, а приватные — только при
явном включении `include_private_ssh_keys` в конфигурации мастера. Это
сделано намеренно: решение не должно приниматься одним кликом. Содержимое
приватных ключей не попадает в журналы и отчёты, права после восстановления —
`0600` (`~/.ssh` — `0700`). Подробнее — [Безопасность](SECURITY.md).

## См. также

- [Установка](INSTALL.md)
- [Безопасность](SECURITY.md)
- [Устранение неполадок](TROUBLESHOOTING.md)
