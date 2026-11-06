# Руководство пользователя (RU)

Migration Master переносит **пользовательский профиль** со старого компьютера
на новый: файлы, настройки приложений и SSH-ключи. Это не образ диска:
системные пакеты, `/etc`, другие пользователи и загрузчик не переносятся.
Полный образ хоста и сценарий «снимок профиля без root» описаны в
[HOST_IMAGE.md](HOST_IMAGE.md).

Программа работает от обычного пользователя и не требует root для создания
архива и восстановления в свой домашний каталог.

## Что реально переносится в 0.1.5

| Компонент | Состояние |
|---|---|
| `desktop`, `documents`, `downloads`, `pictures`, `videos`, `music`, `templates` | полностью |
| `app_configs` (`~/.config`), `app_data` (`~/.local/share`) | полностью, кроме кэшей и исключений |
| `local_bin`, `local_apps`, `themes`, `icons`, `fonts` | полностью |
| `ssh_keys` | публичные ключи, `config`, `known_hosts`; приватные и `authorized_keys` — нет |
| `user_services` | файлы `~/.config/systemd/user` (без включения юнитов) |
| `printers` | только пользовательский `~/.cups`; системные принтеры не восстанавливаются |
| `packages` | только команда `list-packages`; в архив список не попадает |
| `system_settings`, `cron_jobs`, `custom_dirs` | заготовки, автоматически не работают |
| Firefox, Thunderbird, Git, `.bashrc`, `.zshrc` | **не попадают автоматически** (см. `HOST_IMAGE.md`) |

Подробный разбор ограничений и планов — в [IMPROVEMENTS.md](IMPROVEMENTS.md).

## Почему можно работать без root

Все основные компоненты лежат в домашнем каталоге пользователя, поэтому
обычных прав UNIX достаточно:

- чтение `~/.config`, `~/.local`, `~/Documents` и `~/.ssh` разрешено владельцу;
- архив создаётся туда, куда у пользователя есть права записи;
- восстановление пишет только внутрь `--target` (обычно `$HOME`);
- владелец файлов из архива не применяется — это требует root и бессмысленно
  на другом компьютере. Права файлов (`0600` и т.п.) сохраняются.

Для установки пакетов, настройки CUPS и записи в `/etc` нужен root. В проекте
есть отдельный `migration-master-helper` с белым списком и PolicyKit, но в
0.1.5 GUI/CLI его не вызывают: эти шаги выполняются вручную.

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

# Создание зашифрованного архива
migration-master create --output profile.rmm --passphrase '...'
```

`--components` принимает `default`, `all` или список ключей:

```bash
migration-master create --output profile.rmm \
  --components documents,app_configs,ssh_keys \
  --passphrase '...'
```

`--passphrase` **требует значения**. Без него архив будет без шифрования.
Пароль в командной строке виден в `ps` на время работы и остаётся в истории;
для секретов используйте GUI или временный пароль.

Проверка:

```bash
migration-master verify profile.rmm --passphrase '...'
migration-master inspect profile.rmm --passphrase '...'
```

`inspect` показывает реальный состав архива; `scan` для этого не подходит,
потому что использует другой сканер и не видит часть путей.

### Новый компьютер

```bash
# Сначала всегда dry-run
migration-master restore profile.rmm --target /home/user --dry-run \
  --strategy rename --passphrase '...'

# Реальное восстановление
migration-master restore profile.rmm --target /home/user \
  --strategy rename --passphrase '...'
```

Рекомендуется `rename` (старый файл получает суффикс `.old`) или `keep-both`.
Стратегия `replace` заменяет файлы, `skip` оставляет старые.

> CLI `restore` **не создаёт автоматическую резервную копию**. Она есть только
> в GUI-мастере. Перед восстановлением в существующий `$HOME` сохраните его
> отдельно или используйте `rename`.

Восстановить только часть:

```bash
migration-master restore profile.rmm --target /home/user \
  --components documents,app_configs,ssh_keys \
  --strategy rename --passphrase '...'
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
| `printers` | `~/.cups` |
| `packages`, `system_settings`, `cron_jobs`, `custom_dirs` | пока не реализованы |

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

Важно: в 0.1.5 правила используются для размеров, подсказок и исключений, но
**не добавляют свои пути в скан**. Поэтому Firefox и Thunderbird нужно
переносить отдельно (см. `HOST_IMAGE.md`). Chromium/Chrome и VS Code попадают
в архив потому, что лежат внутри `~/.config`.

Выключенное правило (`enabled: false`) исключает свои пути из сканирования.

Post-migration hooks выполняются только с явным подтверждением:

```bash
migration-master app-rules run-hooks --confirm
```

## Конфликты файлов

| Значение | Поведение |
|---|---|
| `ask` | в 0.1.5 **фактически пропускает** файл: интерактивного диалога нет |
| `skip` | оставить существующий |
| `replace` | заменить |
| `keep-both` | сохранить оба (суффикс у нового) |
| `rename` (`rename-old`) | переименовать старый в `.old` |
| `newer` | заменить только если в архиве новее |

GUI по умолчанию выбирает `ask` (индекс 0), CLI тоже. Это известная проблема:
выбирайте `rename` или `replace` осознанно. Резервная копия перед заменой
создаётся **только в GUI-мастере**, не в CLI.

## Приватные SSH-ключи

По умолчанию приватные ключи и `authorized_keys` не переносятся. В 0.1.5
переключателя в CLI/GUI нет: внутренние флаги `WizardConfig` недоступны
пользователю. Если ключи нужны, скопируйте их вручную и выставьте права:

```bash
install -d -m 700 "$HOME/.ssh"
cp -a /media/OLD/ssh/id_ed25519 "$HOME/.ssh/"
chmod 600 "$HOME/.ssh/id_ed25519"
chmod 644 "$HOME/.ssh/id_ed25519.pub"
```

Не храните приватные ключи в незашифрованном архиве. Подробнее —
[Безопасность](SECURITY.md).

## Графический мастер

```bash
migration-master gui
```

Главный экран показывает пользователя, ОС, размер профиля, свободное место и
доступность утилит. Действия:

- **Создать архив профиля** — путь архива, пароль, сжатие, компоненты,
  dry-run;
- **Передача по SSH** — адрес, пользователь, порт, удалённый `$HOME`,
  компоненты;
- **Восстановить из архива** — архив, каталог восстановления, пароль,
  стратегия конфликтов, проверка хешей, dry-run;
- **История операций** и **Диагностика**.

Это не пошаговый мастер с экраном предпросмотра: форма запускает операцию
сразу. По умолчанию выбрана стратегия `ask`, которая в 0.1.5 молча
пропускает конфликты, поэтому перед восстановлением переключите её на
`rename`. Автоматическая резервная копия в GUI создаётся.

## После восстановления

1. Выйдите из сессии и войдите снова (или перезагрузитесь).
2. Шрифты: `fc-cache -f`.
3. Юниты systemd: `systemctl --user daemon-reload`, затем включите нужные
   `systemctl --user enable --now <unit>`.
4. Меню: `update-desktop-database ~/.local/share/applications` (если есть).
5. SSH: `stat -c '%a %n' ~/.ssh ~/.ssh/id_*` — каталог `0700`, ключи `0600`.
6. Проверьте браузер, почту, Git и терминал. Firefox/Thunderbird, Git и
   shell-конфиги нужно было добавить отдельным архивом.
7. Пакеты и принтеры установите/настройте вручную (см. `HOST_IMAGE.md`).

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
migration-master create --output profile.rmm --passphrase '...'
migration-master verify profile.rmm --passphrase '...'

# 2. Перенести profile.rmm на новый ПК

# 3. Новый ПК
migration-master restore profile.rmm --target /home/user --dry-run \
  --strategy rename --passphrase '...'
migration-master restore profile.rmm --target /home/user \
  --strategy rename --passphrase '...'
migration-master report --format txt
```

## См. также

- [Образ хоста без root](HOST_IMAGE.md)
- [План улучшений](IMPROVEMENTS.md)
- [Установка](INSTALL.md)
- [Безопасность](SECURITY.md)
- [Устранение неполадок](TROUBLESHOOTING.md)
- [Резервное копирование](BACKUP.md)
