# Установка Migration Master

Руководство по установке на РЕД ОС Linux (x86_64).

## Способ 1. RPM-пакет (рекомендуется)

```bash
sudo dnf install ./migration-master-0.1.3-1.x86_64.rpm
```

Пакет устанавливает:

| Путь | Назначение |
|---|---|
| `/usr/bin/migration-master` | CLI и графический мастер |
| `/usr/bin/migration-master-helper` | Привилегированный помощник (белый список операций) |
| `/usr/share/applications/migration-master.desktop` | Запуск из меню приложений |
| `/usr/share/icons/hicolor/scalable/apps/migration-master.svg` | Иконка |
| `/usr/share/metainfo/com.redos.migration-master.metainfo.xml` | Метаданные AppStream |
| `/usr/share/polkit-1/actions/com.redos.migration-master.policy` | Политики PolicyKit |
| `/usr/share/migration-master/examples/` | Примеры конфигурации и правил |
| `/usr/share/man/man1/migration-master.1.gz` | Man-страница |

Проверка:

```bash
migration-master --version
migration-master --help
man migration-master
```

### Графический мастер в пакете

Пакет из репозитория и релизный RPM содержат **и CLI, и графический мастер**
(GTK4 + libadwaita). Графическая часть использует только GTK4 ≥ 4.0 и
libadwaita ≥ 1.0, поэтому запускается на РЕД ОС 8 (там libadwaita 1.2).

Библиотеки GTK4/libadwaita должны быть установлены в системе:

```bash
rpm -q gtk4 libadwaita
sudo dnf install gtk4 libadwaita      # если их нет
```

Если пакет собран без GUI (`rpmbuild --without gui`), команда `migration-master gui`
ответит «графический интерфейс недоступен» — это ожидаемо для CLI-only варианта
(например, статической musl-сборки).

Проверить, что графическая часть в пакете есть:

```bash
rpm -qR migration-master | grep -E 'gtk|adwaita'
```

## Способ 2. Из исходников

### Зависимости

```bash
sudo dnf install rust cargo gcc make pkgconf-pkg-config \
                 gtk4-devel libadwaita-devel sqlite-devel cups-devel \
                 openssh-clients
```

Для GUI нужны GTK4 ≥ 4.0 и libadwaita ≥ 1.0 (devel-пакеты: `gtk4-devel`,
`libadwaita-devel`).

### Сборка и установка

```bash
cd migration-master

# Только CLI (без GTK — можно собрать где угодно)
cargo build --release
cargo test --release

# CLI + GUI
cargo build --release --features gui

# Установка в ~/.cargo/bin
cargo install --path . --release
```

Графический мастер запускается командой:

```bash
migration-master gui
```

Без фичи `gui` (или не на Linux) команда вернёт понятную ошибку и подсказку
использовать консольный режим. Если графическая сессия недоступна (нет
`DISPLAY`/`WAYLAND_DISPLAY`, запуск из-под `sudo`), команда выведет причину
и подсказку, как её устранить.

## Требуемые внешние инструменты

| Инструмент | Обязателен | Назначение |
|---|---|---|
| `ssh` (OpenSSH) | да | прямая миграция по SSH |
| `tar`, `zstd` | да | упаковка и сжатие архивов |
| `rsync` | нет | ускоренная передача (иначе tar через SSH) |
| `age` / `gpg` | нет | внешние утилиты (шифрование age встроено в бинарник) |
| `lpstat`, CUPS | нет | перенос принтеров |
| `pkexec` (polkit) | нет | привилегированные операции |

Проверить наличие можно командой `migration-master` → «Настройки и диагностика»
в GUI или `migration-master scan`.

## Обновление

```bash
sudo dnf upgrade ./migration-master-*.rpm
```

История миграций (`~/.local/share/migration-master/migrations.db`) и настройки
(`~/.config/migration-master/`) при обновлении не удаляются.

## Удаление

```bash
sudo dnf remove migration-master
# Полная очистка данных (история, отчёты, резервные копии):
rm -rf ~/.local/share/migration-master ~/.config/migration-master
```

## См. также

- [Сборка RPM](BUILD_RPM.md)
- [Руководство пользователя](USER_GUIDE_RU.md)
- [Устранение неполадок](TROUBLESHOOTING.md)
