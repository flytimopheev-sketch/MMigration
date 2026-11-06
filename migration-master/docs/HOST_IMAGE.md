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
root. В проекте есть отдельный `migration-master-helper` с белым списком
команд и политикой PolicyKit, но в версии 0.1.5 он **не вызывается** из GUI и
CLI: такие действия выполняются вручную (см. раздел 6).

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

| Компонент | Фактическое состояние в 0.1.5 |
|---|---|
| `printers` | копируется только пользовательский `~/.cups` (если есть). Системные принтеры в `/etc/cups` недоступны без root; список можно получить через `lpstat`, но автоматического восстановления принтеров нет |
| `packages` | список пакетов **не попадает в архив**. Есть только команда `migration-master list-packages`; установка — вручную |
| `system_settings` | модуль собран, но в поток создания/восстановления не подключён; `dconf`-дамп автоматически не снимается |
| `cron_jobs` | файл-заготовка `~/.config/migration-master/crontab.txt` нигде не создаётся |
| `custom_dirs` | путь не сканируется |
| приватные SSH-ключи | по умолчанию исключаются; в 0.1.5 нет переключателя в CLI/GUI, только внутренний флаг `WizardConfig` |
| `authorized_keys` | исключается по умолчанию; переключателя в CLI/GUI нет |
| Firefox, Thunderbird, Git, `.bashrc`, `.zshrc` | **не попадают**: правила приложений задают эти пути, но сканер их не добавляет (они лежат вне `.config`/`.local/share`) |
| Chromium/Chrome, VS Code, systemd, файловые менеджеры | попадают, потому что их пути уже внутри `~/.config` или `~/.local/share` |

Исключаются по умолчанию: кэши, `Trash`, временные файлы, `.git`,
`node_modules`, `venv`, `__pycache__`, `*.pyc`, `*.swp` и подобное. Полный
список — `DEFAULT_EXCLUSIONS` в `src/config/mod.rs`.

> `migration-master scan` использует другой сканер, чем `create`: он показывает
> только домашние каталоги и не учитывает `.cups`, systemd-юниты и
> файлы-заготовки. Для проверки реального состава архива используйте
> `migration-master inspect`.

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

read -rsp 'Пароль архива: ' RMM_PASS
printf '\n'

migration-master create \
  --output "$HOME/host-image/profile-$(date +%Y%m%d).rmm" \
  --components "$COMPONENTS" \
  --compression 6 \
  --passphrase "$RMM_PASS"

unset RMM_PASS
```

Важно:

- `--passphrase` **требует значения**. Пример `create --passphrase` без пароля
  из старых README не работает.
- Пароль передаётся аргументом процесса и на время работы виден в `ps`/`top`
  самому пользователю и root. Для максимальной гигиены создавайте архив через
  GUI (поле пароля) или используйте временный пароль и меняйте его позже.
- Без `--passphrase` архив создаётся **без шифрования** — не рекомендуется для
  флешки и сети.

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

read -rsp 'Пароль архива: ' RMM_PASS
printf '\n'
migration-master verify "$ARCHIVE" --passphrase "$RMM_PASS"
migration-master inspect "$ARCHIVE" --passphrase "$RMM_PASS" > inspect.txt
unset RMM_PASS

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

read -rsp 'Пароль архива: ' RMM_PASS
printf '\n'

migration-master verify "$ARCHIVE" --passphrase "$RMM_PASS"

migration-master restore "$ARCHIVE" \
  --target "$HOME" \
  --strategy rename \
  --dry-run \
  --passphrase "$RMM_PASS"
```

`--strategy rename` в пробном запуске покажет, сколько файлов будет
переименовано. Выполняйте блоки 2 и 3 в одном терминале, чтобы переменные
`ARCHIVE` и `RMM_PASS` сохранились. Для реального восстановления используйте
именно `rename` или `keep-both`, пока не убедитесь, что архив правильный:

| Стратегия | Что делает |
|---|---|
| `skip` | оставляет существующие файлы |
| `rename` (`rename-old`) | переименовывает существующий файл в `*.old`, затем пишет новый |
| `keep-both` | сохраняет оба файла, новый получает суффикс |
| `replace` | заменяет существующие файлы |
| `newer` | заменяет, только если файл в архиве новее |
| `ask` | **в 0.1.5 фактически работает как `skip`**: интерактивного диалога нет |

> Это важно: в CLI и GUI по умолчанию выбрана стратегия `ask`, но обработчик
> конфликтов не подключён, поэтому существующие файлы молча пропускаются.
> Для восстановления выбирайте `rename` (безопасно) или `replace` (осознанно).

### Шаг 3. Восстановление

```bash
migration-master restore "$ARCHIVE" \
  --target "$HOME" \
  --strategy rename \
  --passphrase "$RMM_PASS"

unset RMM_PASS
```

Если нужно восстановить только часть:

```bash
migration-master restore "$ARCHIVE" --target "$HOME" \
  --components documents,app_configs,ssh_keys \
  --strategy rename --passphrase "$RMM_PASS"
```

Дополнительный архив (потребуется пароль, заданный при `gpg -c`):

```bash
gpg -d extra-settings.tar.zst.gpg | tar -C "$HOME" --zstd -xvf -
```

### Шаг 4. Пакеты

Список с исходного хоста — `packages.json`/`rpm-all.tsv`. На новом хосте
сравните списки и установите только недостающее:

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

Автоматического восстановления нет. Добавьте принтеры через веб-интерфейс
CUPS (`http://localhost:631`) или командой (нужен root или членство в группе
`lpadmin`):

```bash
sudo lpadmin -p NAME -v 'ipp://server/printer' -E -m everywhere
```

URI берите из `lpstat -v` на исходном хосте.

### Шаг 6. Системные настройки

- Пользовательские настройки GNOME/GTK обычно уже в `.config`/`dconf`.
  Если нужно применить дамп отдельно: `dconf load / < dconf.ini` (после входа
  в сессию; не запускайте при работающем приложении настроек).
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

В 0.1.5 приватные ключи и `authorized_keys` нельзя включить через CLI/GUI.
Если перенос необходим, скопируйте их вручную и сразу выставьте права:

```bash
install -d -m 700 "$HOME/.ssh"
cp -a /media/OLD/ssh/id_ed25519 "$HOME/.ssh/"
chmod 600 "$HOME/.ssh/id_ed25519"
chmod 644 "$HOME/.ssh/id_ed25519.pub"
```

Аналогично для `authorized_keys` (`0600`). Не копируйте `.ssh` на чужой
компьютер и не храните приватные ключи в незашифрованном архиве.

Помните, что `.config`, `.local/share` и дополнительные архивы могут содержать
токены, cookies, пароли браузеров, keyring и GPG-ключи. Используйте стойкий
пароль, шифруйте дополнительные архивы и храните носитель отдельно.

## 8. Ограничения, о которых стоит знать

- Нет инкрементальных архивов: каждый снимок полный.
- Не сохраняются владелец, ACL, xattr, SELinux-контексты, capabilities,
  жёсткие ссылки и пустые каталоги; права файлов сохраняются, каталогов — нет.
- Firefox/Thunderbird/Git/shell-конфиги нужно добавлять вручную (см. шаг 3).
- Пакеты, принтеры и системные настройки автоматически не восстанавливаются.
- `ask` в текущей версии не спрашивает, а пропускает конфликты.
- CLI `restore` не делает автоматическую резервную копию (в отличие от GUI).
- Пароль в CLI виден в списке процессов; в GUI — нет.
- Архив не заменяет резервное копирование: перед восстановлением сохраните
  текущий `$HOME` отдельно.

## 9. Чек-лист

- [ ] Приложения закрыты, место проверено.
- [ ] Основной `.rmm` создан с паролем и проверен `verify`.
- [ ] Дополнительные каталоги (Firefox, Thunderbird, Git, GPG) зашифрованы.
- [ ] `SHA256SUMS` создан и проверен на носителе.
- [ ] Пакеты, принтеры, Flatpak и dconf выгружены.
- [ ] На новом хосте выполнен `restore --dry-run`.
- [ ] Реальное восстановление сделано со стратегией `rename`.
- [ ] Пакеты/принтеры/системные настройки настроены вручную.
- [ ] Выполнен повторный вход, проверены SSH-права и приложения.
- [ ] Временные копии удалены.

## См. также

- [Руководство пользователя](USER_GUIDE_RU.md)
- [Безопасность](SECURITY.md)
- [Резервное копирование](BACKUP.md)
- [План улучшений](IMPROVEMENTS.md)
