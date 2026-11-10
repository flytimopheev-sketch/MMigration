# Руководство пользователя (RU)

Migration Master переносит **пользовательский профиль** со старого компьютера
на новый: файлы, настройки приложений и SSH-ключи. Это не образ диска:
системные пакеты, `/etc`, другие пользователи и загрузчик не переносятся.
Полный образ хоста и сценарий «снимок профиля без root» описаны в
[HOST_IMAGE.md](HOST_IMAGE.md).

Программа работает от обычного пользователя и не требует root для создания
архива и восстановления в свой домашний каталог.

## Что реально переносится в 0.2.0

| Компонент | Состояние |
|---|---|
| `desktop`, `documents`, `downloads`, `pictures`, `videos`, `music`, `templates` | полностью; пути читаются из XDG `user-dirs.dirs` |
| `app_configs` (`~/.config`), `app_data` (`~/.local/share`) | полностью, кроме кэшей и исключений; сюда же добавляются пути правил приложений |
| `local_bin`, `local_apps`, `themes`, `icons`, `fonts` | полностью |
| `ssh_keys` | публичные ключи, `config`, `known_hosts`; приватные ключи и `authorized_keys` — только с явным флагом |
| `user_services` | файлы `~/.config/systemd/user` (без включения юнитов) |
| `printers` | имя, модель, URI и принтер по умолчанию в manifest; восстановление через helper по явному флагу |
| `packages` | список явно установленных пакетов в manifest; diff и установка через helper по явному флагу |
| `system_settings`, `cron_jobs` | dconf и crontab в manifest; применяются от имени пользователя после восстановления |
| Firefox, Thunderbird, Git, `.bashrc`, `.zshrc` | попадают автоматически через правила приложений |
| `custom_dirs` | зарезервирован и не отображается в списке компонентов; используйте правила приложений |

Подробный разбор ограничений и планов — в [IMPROVEMENTS.md](IMPROVEMENTS.md).

## Почему можно работать без root

Все основные компоненты лежат в домашнем каталоге пользователя, поэтому
обычных прав UNIX достаточно:

- чтение `~/.config`, `~/.local`, `~/Documents` и `~/.ssh` разрешено владельцу;
- архив создаётся туда, куда у пользователя есть права записи;
- восстановление пишет только внутрь `--target` (обычно `$HOME`);
- владелец файлов из архива не применяется — это требует root и бессмысленно
  на другом компьютере. Права файлов (`0600` и т.п.) сохраняются.

Для установки пакетов, настройки CUPS и записи в `/etc` нужен root. Эти шаги
выполняет отдельный `migration-master-helper` с белым списком через `pkexec`
(PolicyKit) — только когда пользователь явно выбрал установку пакетов или
настройку принтеров. GUI при этом остаётся запущенным от обычного пользователя.
dconf и crontab применяются без root, от имени пользователя.

## Перед началом

1. На обоих компьютерах нужен `ssh`/`scp` (пакет `openssh-clients`). Для
   SSH-режима желателен `rsync`; для ручных дополнительных архивов —
   `tar`, `zstd`, `gpg`.
2. Внешние `tar` и `zstd` **не обязательны** для самой программы: архивы
   `.rmm` создаются Rust-библиотеками.
3. Проверьте свободное место: размер профиля × 2.
4. Закройте браузеры, почту и IDE — их файлы должны быть закрыты.
5. На новом компьютере создайте пользователя и войдите один раз, чтобы
   появился `$HOME`.

## Режим 1. Локальный архив

### Старый компьютер

```bash
# Оценка без записи
migration-master create --output profile.rmm --dry-run

# Создание зашифрованного архива (пароль вводится интерактивно)
migration-master create --output profile.rmm --ask-passphrase
# или из файла
migration-master create --output profile.rmm --passphrase-file ~/.rmm-pass
```

`--components` принимает `default`, `all` или список ключей:

```bash
migration-master create --output profile.rmm \
  --components documents,app_configs,ssh_keys \
  --ask-passphrase
```

Приватные ключи и `authorized_keys` включаются только явно:

```bash
migration-master create --output profile.rmm --ask-passphrase \
  --include-private-ssh-keys --include-authorized-keys
```

`--passphrase '<пароль>'` тоже работает, но пароль виден в `ps` и остаётся в
истории оболочки. Предпочитайте `--ask-passphrase` или `--passphrase-file`.

Проверка:

```bash
migration-master verify profile.rmm --ask-passphrase
migration-master inspect profile.rmm --ask-passphrase
```

`inspect` показывает реальный состав архива и метаданные (пакеты, принтеры,
dconf, cron); `scan` использует тот же сканер, что и `create`, поэтому их
состав совпадает.

### Новый компьютер

```bash
# Сначала всегда dry-run
migration-master restore profile.rmm --target /home/user --dry-run \
  --strategy rename --ask-passphrase

# Реальное восстановление; backup создаётся автоматически
migration-master restore profile.rmm --target /home/user \
  --strategy rename --ask-passphrase
```

Рекомендуется `rename` (старый файл получает суффикс `.old`) или `keep-both`.
Стратегия `replace` заменяет файлы, `skip` оставляет старые. `ask` спрашивает
в терминале; без TTY команда завершится ошибкой. В GUI по умолчанию выбран
`rename`, а `ask` недоступен.

CLI `restore` создаёт backup по умолчанию; отключается только явным
`--no-backup`.

Восстановить только часть:

```bash
migration-master restore profile.rmm --target /home/user \
  --components documents,app_configs,ssh_keys \
  --strategy rename --ask-passphrase
```

Установка пакетов, настройка принтеров и применение dconf/cron — только по
явным флагам:

```bash
migration-master restore profile.rmm --target /home/user \
  --install-packages --configure-printers --apply-system-settings
```

## Режим 2. Прямая миграция по SSH

```bash
# Проверка соединения и плана
migration-master migrate-ssh user@192.168.1.20 --dry-run

# Передача
migration-master migrate-ssh user@192.168.1.20 --components default
```

Полезные параметры:

- `--identity ~/.ssh/id_ed25519` — приватный ключ;
- `--remote-home /home/user` — домашний каталог на приёмнике;
- `--port 2222` — нестандартный порт.

Передача идёт через `scp`, а если установлен `rsync` — через него. Архив
`.rmm` в этом режиме не создаётся, данные шифруются транспортом SSH.

`StrictHostKeyChecking=accept-new` автоматически принимает **новый** host key
при первом подключении. Это TOFU, а не проверка: сверьте отпечаток с
администратором. `BatchMode=yes` означает, что пароль SSH не запрашивается —
нужен ключ или ssh-agent.

## Компоненты

| Ключ | Что переносится |
|---|---|
| `desktop`, `documents`, `downloads`, `pictures`, `videos`, `music`, `templates` | стандартные каталоги |
| `app_configs`, `app_data` | `~/.config`, `~/.local/share` |
| `local_bin`, `local_apps` | `~/.local/bin`, `~/.local/share/applications` |
| `themes`, `icons`, `fonts` | `~/.themes`, `~/.icons`, `~/.fonts`/`~/.local/share/fonts` |
| `ssh_keys` | `~/.ssh` без приватных ключей и `authorized_keys` |
| `user_services` | `~/.config/systemd/user` |
| `printers` | имя, модель, URI и принтер по умолчанию (восстановление через helper) |
| `packages` | список явно установленных пакетов (установка через helper) |
| `system_settings` | dconf и системные пути профиля (dconf применяется от пользователя) |
| `cron_jobs` | `crontab -l` (применяется от пользователя) |
| `custom_dirs` | зарезервирован; в список компонентов не входит |

Список: `migration-master scan --components all`.

По умолчанию исключаются кэши, `Trash`, временные файлы, `.git`,
`node_modules`, `venv`, `__pycache__` и подобное. Полный список —
`DEFAULT_EXCLUSIONS` в `src/config/mod.rs`.

## Настройки приложений и правила (§5)

Встроенные правила описывают Firefox, Chromium/Chrome, Thunderbird,
LibreOffice, VS Code, Git, терминал, SSH, systemd и файловые менеджеры:

```bash
migration-master app-rules list --json
migration-master app-rules export
migration-master app-rules import my.yaml
```

Правила не только задают размеры, подсказки и исключения, но и **добавляют
свои пути в скан**: при выборе `app_configs`/`app_data` в архив попадают
`.mozilla/firefox`, `.thunderbird`, `.gitconfig`, `.bashrc`, `.zshrc` и другие
включённые пути. Chromium/Chrome и VS Code также попадают в архив.

Выключенное правило (`enabled: false`) исключает свои пути из сканирования.

Post-migration hooks выполняются только с явным подтверждением:

```bash
migration-master app-rules run-hooks --confirm
```

## Конфликты файлов

| Значение | Поведение |
|---|---|
| `ask` | спросить для каждого файла (только CLI с интерактивным TTY) |
| `skip` | оставить существующий |
| `replace` | заменить |
| `keep-both` | сохранить оба (суффикс у нового) |
| `rename` (`rename-old`) | переименовать старый в `.old` |
| `newer` | заменить только если в архиве новее |

CLI по умолчанию использует `ask`: в терминале он спрашивает, без TTY
завершается понятной ошибкой. GUI по умолчанию выбирает безопасный `rename`,
а `ask` блокирует с пояснением. Резервная копия перед заменой создаётся и в
GUI, и в CLI (в CLI отключается `--no-backup`).

## Приватные SSH-ключи

По умолчанию приватные ключи и `authorized_keys` не переносятся. Включить их
можно только явно — флагами CLI:

```bash
migration-master create --output profile.rmm --ask-passphrase \
  --include-private-ssh-keys --include-authorized-keys
```

или галочками на странице создания архива в GUI. После восстановления
`~/.ssh` получает права `0700`, приватные ключи и `authorized_keys` — `0600`.
Не храните приватные ключи в незашифрованном архиве. Подробнее —
[Безопасность](SECURITY.md).

## Графический мастер

```bash
migration-master gui
```

Главный экран показывает пользователя, ОС, размер профиля, свободное место и
доступность утилит. Действия:

- **Создать архив профиля** — путь архива, пароль с подтверждением и
  индикатором стойкости, генератор пароля, сжатие, компоненты, dry-run,
  приватные SSH-ключи и `authorized_keys`, предпросмотр состава;
- **Передача по SSH** — адрес, пользователь, порт, удалённый `$HOME`,
  компоненты, приватный ключ, предпросмотр;
- **Восстановить из архива** — архив, каталог восстановления, пароль,
  стратегия конфликтов, проверка хешей, dry-run, предпросмотр, галочки
  установки пакетов/настройки принтеров/dconf+cron;
- **История операций** и **Диагностика**.

Долгие операции можно отменить кнопкой; итоговый экран показывает числа
восстановленных/пропущенных файлов, ошибки, предупреждения, рекомендации и
путь к отчёту. По умолчанию выбрана безопасная стратегия `rename`; `ask`
блокируется, потому что диалог для каждого файла в GUI не реализован.
Автоматическая резервная копия создаётся.

## После восстановления

1. Выйдите из сессии и войдите снова (или перезагрузитесь).
2. Шрифты: `fc-cache -f`.
3. Юниты systemd: `systemctl --user daemon-reload`, затем включите нужные
   `systemctl --user enable --now <unit>`.
4. Меню: `update-desktop-database ~/.local/share/applications` (если есть).
5. SSH: `stat -c '%a %n' ~/.ssh ~/.ssh/id_*` — каталог `0700`, ключи `0600`.
6. Проверьте браузер, почту, Git и терминал — их настройки входят в
   `app_configs`/`app_data` через правила приложений.
7. Если не использовались флаги `--install-packages` и
   `--configure-printers`, установите пакеты и настройте принтеры вручную
   (см. `HOST_IMAGE.md`); иначе проверьте результат в отчёте.

## История и отчёты

```bash
migration-master report --format html --limit 50
migration-master config path
migration-master config list
```

История хранится в SQLite; отчёты — `json`, `html`, `txt` (PDF для отчёта
операции создаётся при наличии `libreoffice`/`pandoc`). Пароли и содержимое
приватных ключей в журналы не попадают.

## Пример полного цикла

```bash
# 1. Старый ПК
migration-master scan --json > scan.json
migration-master create --output profile.rmm --ask-passphrase
migration-master verify profile.rmm --ask-passphrase

# 2. Перенести profile.rmm на новый ПК

# 3. Новый ПК
migration-master restore profile.rmm --target /home/user --dry-run \
  --strategy rename --ask-passphrase
migration-master restore profile.rmm --target /home/user \
  --strategy rename --ask-passphrase
migration-master report --format txt
```

## См. также

- [Образ хоста без root](HOST_IMAGE.md)
- [План улучшений](IMPROVEMENTS.md)
- [Установка](INSTALL.md)
- [Безопасность](SECURITY.md)
- [Устранение неполадок](TROUBLESHOOTING.md)
- [Резервное копирование](BACKUP.md)
