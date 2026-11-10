# Образ хоста без root: что реально может Migration Master

Это практическое руководство для случая «хочу снять образ текущего хоста и
развернуть его на другом компьютере». Оно объясняет, какие данные можно
скопировать **от обычного пользователя**, а для каких нужен root и другие
инструменты.

> Коротко: Migration Master переносит **пользовательский профиль** (домашний
> каталог и настройки приложений), а не образ диска. Полный образ ОС, пакетов,
> `/etc`, других пользователей и загрузчика без root сделать нельзя.

## 1. Два разных «образа»

| Что нужно | Инструмент | Нужен root |
|---|---|---|
| Снимок пользовательского окружения: файлы, `.config`, `.local`, SSH-ключи, шрифты, темы | Migration Master + вручную несколько каталогов | **Нет** |
| Полный образ диска/разделов: ОС, пакеты, `/etc`, все пользователи, загрузчик | `dd`, `partclone`, Clonezilla/Rescuezilla, `btrfs send`, LVM-снимок | **Да** |

Если цель — «золотой образ» одинаковых рабочих станций, полный образ диска
обычно не лучший выбор: железо и UUID разделов отличаются. Надёжнее
kickstart/Ansible + список пакетов + профиль пользователя из этого руководства.

## 2. Почему программа работает без root

Migration Master не вызывает `sudo` для обычных операций. Все компоненты,
которые она переносит, лежат в домашнем каталоге пользователя:

- чтение и запись `~/.config`, `~/.local`, `~/Documents`, `~/.ssh` разрешены
  владельцу обычными правами UNIX;
- архив создаётся в каталог, куда у пользователя есть права записи;
- при восстановлении файлы пишутся только внутрь `--target` (обычно `$HOME`);
- владелец (`uid`/`gid`) из архива **не применяется** — это невозможно без
  root и неправильно на новом компьютере. Владельцем становится тот
  пользователь, который выполняет восстановление; права файлов (`0600` и т.п.)
  сохраняются.

Системные операции (установка RPM, настройка CUPS, запись в `/etc`) требуют
root. В 0.2.0 их выполняет отдельный `migration-master-helper` с белым списком
команд и политикой PolicyKit через `pkexec` — только при явном флаге
`--install-packages`/`--configure-printers` или соответствующей галочке GUI.
Полный образ системы по-прежнему требует root и вручную (см. раздел 6).

## 3. Что реально попадает в архив

### Переносится полностью

| Компонент | Пути |
|---|---|
| `desktop`, `documents`, `downloads`, `pictures`, `videos`, `music`, `templates` | стандартные каталоги (`Desktop`, `Documents`, … и русские варианты) |
| `app_configs` | `~/.config` (кроме исключений) |
| `app_data` | `~/.local/share` (кроме исключений) |
| `local_bin`, `local_apps` | `~/.local/bin`, `~/.local/share/applications` |
| `themes`, `icons`, `fonts` | `~/.themes`, `~/.icons`, `~/.fonts`, `~/.local/share/fonts` |
| `ssh_keys` | `~/.ssh`: публичные ключи, `config`, `known_hosts` |
| `user_services` | `~/.config/systemd/user` (файлы юнитов) |

### Переносится частично или не переносится автоматически

| Компонент | Фактическое состояние в 0.2.0 |
|---|---|
| `printers` | имя, модель, URI и принтер по умолчанию сохраняются в manifest; восстановление через helper по флагу `--configure-printers` (в GUI — галочка) |
| `packages` | список явно установленных пакетов сохраняется в manifest; diff показывается при восстановлении, установка — через helper по флагу `--install-packages` |
| `system_settings` | dconf снимается в manifest и применяется от имени пользователя; системные `/etc`-файлы вслепую не копируются |
| `cron_jobs` | `crontab -l` сохраняется в manifest и применяется от имени пользователя при `--apply-system-settings` |
| `custom_dirs` | зарезервирован, в список компонентов не входит; используйте правила приложений |
| приватные SSH-ключи | исключаются по умолчанию; включаются флагом `--include-private-ssh-keys` или галочкой GUI |
| `authorized_keys` | исключается по умолчанию; включается флагом `--include-authorized-keys` или галочкой GUI |
| Firefox, Thunderbird, Git, `.bashrc`, `.zshrc` | попадают автоматически: правила приложений добавляют свои пути в скан |
| Chromium/Chrome, VS Code, systemd, файловые менеджеры | попадают, потому что их пути уже внутри `~/.config` или `~/.local/share` |

Исключаются по умолчанию: кэши, `Trash`, временные файлы, `.git`,
`node_modules`, `venv`, `__pycache__`, `*.pyc`, `*.swp` и подобное. Полный
список — `DEFAULT_EXCLUSIONS` в `src/config/mod.rs`.

> Начиная с 0.2.0 `migration-master scan` и `create` используют один сканер,
> поэтому состав и размеры совпадают. Для проверки уже готового архива
> используйте `migration-master inspect` — он показывает файлы и метаданные.

## 4. Снимок пользовательского окружения (без root)

### Шаг 0. Подготовка

Закройте браузеры, почтовые клиенты, IDE и мессенджеры: их файлы должны быть
закрыты. Проверьте место и создайте каталог для снимка:

```bash
df -h "$HOME"
du -sh "$HOME/.config" "$HOME/.local/share" "$HOME/.mozilla" 2>/dev/null
mkdir -p "$HOME/host-image"
umask 077
```

Для оценки размера без записи:

```bash
migration-master scan --components all
```

### Шаг 1. Инвентаризация

```bash
migration-master scan --components all > "$HOME/host-image/scan.txt" 2>&1
migration-master list-packages --json > "$HOME/host-image/packages.json"
lpstat -p -d > "$HOME/host-image/printers.txt" 2>&1 || true
lpstat -v >> "$HOME/host-image/printers.txt" 2>&1 || true
dconf dump / > "$HOME/host-image/dconf.ini" 2>/dev/null || true
flatpak list --app --columns=application,origin > "$HOME/host-image/flatpaks.txt" 2>/dev/null || true
rpm -qa --qf '%{NAME}\t%{VERSION}-%{RELEASE}\n' | sort > "$HOME/host-image/rpm-all.tsv"
```

Это не сам образ, а «опись»: по ней удобно понять, что ещё нужно доустановить
и настроить на новом хосте.

### Шаг 2. Основной архив

Список компонентов ниже — практический максимум для профиля. `app_data` может
быть очень большим (Steam, контейнеры, локальные базы) — проверьте размер
заранее и при необходимости уберите его.

```bash
COMPONENTS="desktop,documents,downloads,pictures,videos,music,templates,app_configs,app_data,local_bin,local_apps,themes,icons,fonts,ssh_keys,user_services,printers"

migration-master create \
  --output "$HOME/host-image/profile-$(date +%Y%m%d).rmm" \
  --components "$COMPONENTS" \
  --compression 6 \
  --ask-passphrase
```

Важно:

- `--ask-passphrase` запрашивает пароль интерактивно и не оставляет его в
  `argv`/истории оболочки. Альтернатива — `--passphrase-file` с файлом,
  доступным только владельцу (`chmod 600`).
- Без пароля архив создаётся **без шифрования** — не рекомендуется для
  флешки и сети.
- Метаданные пакетов, принтеров, dconf и cron попадают в manifest при выборе
  компонентов `packages`, `printers`, `system_settings`, `cron_jobs`; в
  `COMPONENTS` выше они не включены, добавьте их при необходимости.

### Шаг 3. Дополнительные каталоги, которые не входят в компоненты

Firefox/Thunderbird, Git, shell-конфиги и GPG-ключи лежат вне стандартных
компонентов. Соберите их отдельным архивом и зашифруйте его. Для сжатия нужен
`zstd` (`sudo dnf install zstd`), для шифрования — `gnupg2`:

```bash
EXTRA=()
for p in .mozilla .thunderbird .gitconfig .bashrc .bash_profile .zshrc .gnupg .pki .var/app; do
  [ -e "$HOME/$p" ] && EXTRA+=("$p")
done

if [ ${#EXTRA[@]} -gt 0 ]; then
  tar -I 'zstd -T0 -19' -cf "$HOME/host-image/extra-settings.tar.zst" \
    --exclude='.mozilla/firefox/*/cache*' \
    --exclude='.mozilla/firefox/*/Cache*' \
    -C "$HOME" "${EXTRA[@]}"

  gpg -c --cipher-algo AES256 \
    -o "$HOME/host-image/extra-settings.tar.zst.gpg" \
    "$HOME/host-image/extra-settings.tar.zst"
  rm -f "$HOME/host-image/extra-settings.tar.zst"
fi
```

Если `gpg` не установлен: `sudo dnf install gnupg2`. Вместо `gpg -c` можно
использовать `age -p` (пакет `age`), если он есть в репозитории.

> Приватные SSH-ключи и `authorized_keys` этим архивом не затрагиваются.
> Если их нужно перенести, сделайте это осознанно вручную (см. раздел 7).

### Шаг 4. Проверка и копирование

```bash
cd "$HOME/host-image"
ARCHIVE=$(ls -1t ./*.rmm | head -n1)
[ -n "$ARCHIVE" ] || { echo "Архив .rmm не найден"; exit 1; }

migration-master verify "$ARCHIVE" --ask-passphrase
migration-master inspect "$ARCHIVE" --ask-passphrase > inspect.txt

sha256sum ./*.rmm > SHA256SUMS
[ -f extra-settings.tar.zst.gpg ] && sha256sum extra-settings.tar.zst.gpg >> SHA256SUMS
```

Скопируйте весь каталог `host-image` на внешний носитель, лучше в двух
экземплярах:

```bash
rsync -a --info=progress2 "$HOME/host-image/" /media/"$USER"/EXT/
```

Проверьте копию на носителе: `cd /media/$USER/EXT && sha256sum -c SHA256SUMS`.

## 5. Развёртывание на новом хосте

### Шаг 1. Подготовка нового хоста

1. Создайте пользователя (имя может отличаться от старого — пути в архиве
   относительны домашнего каталога).
2. Войдите в графическую сессию один раз, чтобы создался `$HOME` и профиль.
3. Закройте все приложения. Ещё лучше — выполнять восстановление по SSH с
   другого компьютера, пока графическая сессия на новом хосте не запущена.
4. Скопируйте каталог `host-image` на новый хост.

### Шаг 2. Проверка и пробный запуск

```bash
cd ~/host-image
sha256sum -c SHA256SUMS

ARCHIVE=$(ls -1t ./*.rmm | head -n1)
[ -n "$ARCHIVE" ] || { echo "Архив .rmm не найден"; exit 1; }

migration-master verify "$ARCHIVE" --ask-passphrase

migration-master restore "$ARCHIVE" \
  --target "$HOME" \
  --strategy rename \
  --dry-run \
  --ask-passphrase
```

`--strategy rename` в пробном запуске покажет, сколько файлов будет
переименовано. Для реального восстановления используйте именно `rename` или
`keep-both`, пока не убедитесь, что архив правильный:

| Стратегия | Что делает |
|---|---|
| `skip` | оставляет существующие файлы |
| `rename` (`rename-old`) | переименовывает существующий файл в `*.old`, затем пишет новый |
| `keep-both` | сохраняет оба файла, новый получает суффикс |
| `replace` | заменяет существующие файлы |
| `newer` | заменяет, только если файл в архиве новее |
| `ask` | спрашивает для каждого файла; только CLI с интерактивным TTY |

> В GUI по умолчанию выбран безопасный `rename`; `ask` заблокирован, потому
> что диалог для каждого файла ещё не реализован. В CLI `ask` без TTY
> завершается ошибкой, а не пропускает файлы молча.

### Шаг 3. Восстановление

```bash
migration-master restore "$ARCHIVE" \
  --target "$HOME" \
  --strategy rename \
  --ask-passphrase
```

CLI создаёт резервную копию перезаписываемых файлов автоматически;
`--no-backup` отключает её. Если нужно восстановить только часть:

```bash
migration-master restore "$ARCHIVE" --target "$HOME" \
  --components documents,app_configs,ssh_keys \
  --strategy rename --ask-passphrase
```

Дополнительный архив (потребуется пароль, заданный при `gpg -c`):

```bash
gpg -d extra-settings.tar.zst.gpg | tar -C "$HOME" --zstd -xvf -
```

### Шаг 4. Пакеты

Если при создании был выбран компонент `packages`, список уже есть в manifest.
Установить его можно при восстановлении флагом `--install-packages` — helper
покажет diff и запросит подтверждение через PolicyKit. Ручной путь ниже нужен,
если флаг не использовался или в архиве есть только `packages.json`/`rpm-all.tsv`.
Сравните списки и установите только недостающее:

```bash
# Пакеты, которые пользователь ставил сам (dnf 4)
dnf repoquery --userinstalled --qf '%{name}\n' 2>/dev/null | sort -u > user-packages.txt
# Если команда недоступна:
# dnf history userinstalled | sed '1d' | awk '{print $1}' | sort -u > user-packages.txt

rpm -qa --qf '%{NAME}\n' | sort -u > installed.txt
comm -13 installed.txt user-packages.txt > missing.txt

# Просмотрите missing.txt: ядра и драйверы туда попадать не должны
if [ -s missing.txt ]; then
  sudo dnf install -y $(cat missing.txt)
fi
```

Пакеты **не копируются** как файлы: ставятся из репозиториев. Для Flatpak:

```bash
flatpak list --app --columns=application
flatpak install flathub <application-id>
```

### Шаг 5. Принтеры

```bash
cat printers.txt          # что было на источнике
lpstat -p -d              # что есть на новом хосте
```

Если при создании был выбран компонент `printers`, восстановление можно
выполнить флагом `--configure-printers`: helper добавит принтеры через
`lpadmin` и включит их. Ручной путь — веб-интерфейс CUPS
(`http://localhost:631`) или команда (нужен root или членство в группе
`lpadmin`):

```bash
sudo lpadmin -p NAME -v 'ipp://server/printer' -E -m everywhere
```

URI берите из `lpstat -v` на исходном хосте.

### Шаг 6. Системные настройки

- Если выбран `system_settings`/`cron_jobs`, dconf и crontab применяются при
  восстановлении с `--apply-system-settings` (в GUI — галочка) от имени
  пользователя. Вручную: `dconf load / < dconf.ini` (после входа в сессию;
  не запускайте при работающем приложении настроек).
- Юниты systemd пользователя: `systemctl --user daemon-reload`, затем
  `systemctl --user enable --now <unit>`.
- Шрифты: `fc-cache -f`.
- Меню приложений: `update-desktop-database "$HOME/.local/share/applications"`
  (если утилита установлена).
- Системные файлы (`/etc`, NetworkManager, firewall, timezone, hostname,
  репозитории, sudoers) в архив **не входят** и требуют root/ручной настройки.

### Шаг 7. Завершение

1. Выйдите из сессии и войдите снова (или перезагрузитесь), чтобы приложения
   перечитали настройки.
2. Проверьте SSH: `stat -c '%a %n' ~/.ssh ~/.ssh/id_*` — каталог `0700`,
   приватные ключи `0600`.
3. Проверьте браузер, почту, Git, терминал.
4. Удалите временные копии архива с флешки, если они больше не нужны.

## 6. Полный образ хоста (нужен root)

Эти команды **не выполняет** Migration Master. Они приведены для полноты.

### Вариант A. Посекторная копия диска

Загрузитесь с live-USB, чтобы разделы не были смонтированы:

```bash
lsblk -f
sudo dd if=/dev/nvme0n1 of=/mnt/external/host.img bs=4M status=progress conv=fsync
# со сжатием:
sudo dd if=/dev/nvme0n1 bs=4M status=progress | zstd -T0 > /mnt/external/host.img.zst
```

Восстановление (`of=` указывает на диск, все данные на нём будут уничтожены):

```bash
sudo dd if=/mnt/external/host.img of=/dev/nvme0n1 bs=4M status=progress conv=fsync
```

### Вариант B. Файловая копия с сохранением метаданных

```bash
sudo rsync -aHAX --numeric-ids --info=progress2 \
  --exclude={/proc,/sys,/dev,/run,/tmp,/mnt,/media,/lost+found} \
  / /mnt/external/root/
```

`-H` — жёсткие ссылки, `-A` — ACL, `-X` — xattr. Загрузчик, `/etc/fstab`,
UUID и initramfs после такого переноса нужно настраивать отдельно.

### Вариант C. Специализированные инструменты

- Clonezilla / Rescuezilla — образы разделов с проверкой и восстановлением.
- `partclone.ext4 -c -s /dev/sda2 -o root.pcl` — копия только занятых блоков.
- `btrfs send`/`btrfs receive` или LVM-снимок — для btrfs/LVM.
- Ansible/kickstart + список пакетов + профиль — для парка одинаковых машин.

Без root невозможно прочитать `/etc/shadow`, чужие домашние каталоги, raw-диск
и изменить загрузчик. Поэтому «образ хоста без root» = образ **профиля**, а не
всей системы.

## 7. Приватные SSH-ключи и секреты

Приватные ключи и `authorized_keys` включаются только явно:

```bash
migration-master create --output profile.rmm --ask-passphrase \
  --include-private-ssh-keys --include-authorized-keys
```

В GUI это отдельные галочки, выключенные по умолчанию. После восстановления
`~/.ssh` получает права `0700`, приватные ключи и `authorized_keys` — `0600`.
Не копируйте `.ssh` на чужой компьютер и не храните приватные ключи в
незашифрованном архиве.

Помните, что `.config`, `.local/share` и дополнительные архивы могут содержать
токены, cookies, пароли браузеров, keyring и GPG-ключи. Используйте стойкий
пароль, шифруйте дополнительные архивы и храните носитель отдельно.

## 8. Ограничения, о которых стоит знать

- Нет инкрементальных архивов: каждый снимок полный.
- Не сохраняются владелец, ACL, xattr, SELinux-контексты, capabilities,
  жёсткие ссылки и пустые каталоги; права файлов сохраняются, каталогов — нет.
- Firefox/Thunderbird/Git/shell-конфиги входят в скан через правила
  приложений; каталоги вне правил (например, GPG) добавляйте отдельным архивом.
- Пакеты и принтеры восстанавливаются только с явным флагом/галочкой через
  helper; dconf и cron — с `--apply-system-settings`.
- `ask` работает только в CLI с TTY; в GUI доступны `rename`/`replace`/`skip`/
  `newer`.
- CLI `restore` создаёт backup автоматически; отключается `--no-backup`.
- Пароль, переданный через `--passphrase`, виден в списке процессов; в GUI,
  `--ask-passphrase` и `--passphrase-file` — нет.
- Архив не заменяет резервное копирование: перед восстановлением сохраните
  текущий `$HOME` отдельно.

## 9. Чек-лист

- [ ] Приложения закрыты, место проверено.
- [ ] Основной `.rmm` создан с паролем и проверен `verify`.
- [ ] При необходимости включены `--include-private-ssh-keys`/
      `--include-authorized-keys`.
- [ ] Дополнительные каталоги вне правил приложений (например, GPG) зашифрованы.
- [ ] `SHA256SUMS` создан и проверен на носителе.
- [ ] Пакеты, принтеры, Flatpak и dconf выгружены или включены в manifest.
- [ ] На новом хосте выполнен `restore --dry-run`.
- [ ] Реальное восстановление сделано со стратегией `rename`.
- [ ] При необходимости использованы `--install-packages`,
      `--configure-printers`, `--apply-system-settings`.
- [ ] Выполнен повторный вход, проверены SSH-права и приложения.
- [ ] Временные копии удалены.

## См. также

- [Руководство пользователя](USER_GUIDE_RU.md)
- [Безопасность](SECURITY.md)
- [Резервное копирование](BACKUP.md)
- [План улучшений](IMPROVEMENTS.md)
