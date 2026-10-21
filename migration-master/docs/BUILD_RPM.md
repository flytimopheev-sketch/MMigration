# Сборка RPM

Инструкция по сборке пакета Migration Master для РЕД ОС Linux.

## Подготовка

```bash
sudo dnf install rpm-build rpmdevtools rust cargo gcc make pkgconf-pkg-config \
                 gtk4-devel libadwaita-devel sqlite-devel cups-devel
rpmdev-setuptree
```

## Исходники

```bash
cd ~/rpmbuild/SOURCES
tar czf migration-master-0.1.4.tar.gz --transform 's,^,migration-master-0.1.4/,' \
    -C /путь/к/репозиторию migration-master
```

Альтернатива — `spectool` при наличии тегов в git:

```bash
spectool -g -C ~/rpmbuild/SOURCES migration-master.spec
```

## Сборка

```bash
cp rpm/migration-master.spec ~/rpmbuild/SPECS/
rpmbuild -ba ~/rpmbuild/SPECS/migration-master.spec
```

Результат: `~/rpmbuild/RPMS/x86_64/migration-master-0.1.4-1.<dist>.x86_64.rpm`.

**Графический мастер собирается по умолчанию.** Пакет без GUI получается
явным флагом `--without gui`:

```bash
rpmbuild -ba ~/rpmbuild/SPECS/migration-master.spec --without gui
```

> Раньше GUI был опциональным (`--with gui`), и обычная сборка `rpmbuild -ba`
> давала пакет только с CLI: команда `migration-master gui` отвечала
> «графический интерфейс недоступен». Теперь обратная ситуация: по умолчанию
> GUI включён, а `--without gui` даёт CLI-only пакет.

### Сборка для любой РЕД ОС (статическая musl, без glibc)

Если пакет собран не на самой РЕД ОС (например, на CI под Debian/Ubuntu),
бинарники требуют новый glibc, и на целевой РЕД ОС установка падает:

```text
error: Failed dependencies:
    libc.so.6(GLIBC_2.34)(64bit) is needed by migration-master-...
```

Проверка:

```bash
# какие версии GLIBC требует пакет (у собранного на Debian CI будет 2.39)
rpm -qpR ~/rpmbuild/RPMS/x86_64/migration-master-*.rpm | grep GLIBC
# версия glibc в целевой системе
ldd --version | head -1
# хост/платформа сборки — не должна быть чуждой ОС
rpm -qp --qf '%{VENDOR}\n%{BUILDHOST}\n' migration-master-*.rpm
```

Решение — статическая линковка musl (в бинарниках **нет ни одной
динамической зависимости, включая libc**, `rpm -qpR` не показывает GLIBC,
пакет ставится на любую РЕД ОС):

```bash
# зависимости сборки (пакет, предоставляющий musl-gcc):
sudo dnf install musl-gcc          # на Debian/Ubuntu CI: sudo apt install musl-tools
rustup target add x86_64-unknown-linux-musl

rpmbuild -ba ~/rpmbuild/SPECS/migration-master.spec --with musl --without gui
```

Ограничения опции `--with musl`:

- только CLI и `migration-master-helper`: GTK4/libadwaita нельзя линковать
  статически, поэтому GUI в musl-сборке отключается автоматически;
- `%check` тоже выполняется под musl-тегом — нужен установленный target.

Альтернатива без musl — собирать RPM прямо на РЕД ОС (или в chroot/mock с
репозиториями РЕД ОС): тогда glibc-зависимости совпадут с целевой системой
из коробки.

> ⚠️ Не «встраивайте» системный glibc в пакет (каталог с libc + свой
> ld-linux + patchelf): это ломает системные правила обновления, конфликтует
> с пакетом `glibc` и не поддерживается ни RPM, ни dnf. Статическая musl-сборка
> даёт тот же эффект «всё внутри пакета» штатными средствами.

### Что делает spec

- `%build` — `cargo build --release --locked --features gui` (CLI + helper +
  графический мастер); без GUI (`--without gui`) собирается только CLI;
- `%check` — `cargo test --release --locked`: юнит-тесты архивов, шифрования,
  path traversal, конфликтов, отмены и драйверов ресурсов. Для GUI-сборки
  дополнительно проверяется, что бинарник линкует GTK4 (иначе в пакет попал бы
  CLI-only вариант);
- `%install` — ставит бинарники, helper, desktop-файл, иконку, AppStream,
  политику PolicyKit, примеры конфигурации и man-страницу.

### Сборка с GUI

GUI включён по умолчанию, отдельный флаг не нужен:

```bash
rpmbuild -ba ~/rpmbuild/SPECS/migration-master.spec
```

При сборке **не на самой РЕД ОС** (например, на CI под Ubuntu) добавляйте
`--define "zigbuild 1"`: линковку выполнит zig cc против glibc версии не новее
`%{glibc_floor}` (по умолчанию 2.28 — версия glibc целевой РЕД ОС), иначе
пакет потребует `libc.so.6(GLIBC_2.xx)` новее целевой системы и не установится:

```bash
rpmbuild -ba ~/rpmbuild/SPECS/migration-master.spec --define "zigbuild 1"
```

Так линкуются и CLI, и helper; в CI это делает workflow
`.github/workflows/release-rpm.yml` (нужны zig и `cargo install cargo-zigbuild`).

Графическая часть использует только GTK4 ≥ 4.0 и libadwaita ≥ 1.0 — это
проверяется в CI по списку импортируемых символов (`objdump -T`), чтобы
бинарник не тянул API libadwaita 1.4 (`adw_navigation_view_new` и подобные):
на РЕД ОС 8 libadwaita 1.2, и такой вызов даёт «undefined symbol», то есть GUI
не запускается.

## Проверка пакета

```bash
rpm -Vp ~/rpmbuild/RPMS/x86_64/migration-master-*.rpm
rpm -qlp ~/rpmbuild/RPMS/x86_64/migration-master-*.rpm

# Просмотр зависимостей
rpm -qpR ~/rpmbuild/RPMS/x86_64/migration-master-*.rpm
```

## Установка и smoke-тест

```bash
sudo dnf install ./migration-master-0.1.4-1.x86_64.rpm

migration-master --version
migration-master scan --quick
migration-master config list

# Файлы интеграции на месте
ls /usr/share/polkit-1/actions/com.redos.migration-master.policy
ls /usr/share/applications/migration-master.desktop
```

## Типовые ошибки

| Причина | Решение |
|---|---|
| `error: Failed buildRequires: pkgconfig(gtk4)` | `sudo dnf install gtk4-devel` |
| Установка падает: `libc.so.6(GLIBC_2.xx)(64bit) is needed` / `lib...so not found` | пакет собран на чужой ОС с более новым glibc — пересоберите `rpmbuild ... --with musl` (см. раздел «Сборка для любой РЕД ОС») или на самой РЕД ОС |
| `%check` падает | `cargo test --release` в каталоге исходников — логи ошибок |
| `No such file or directory: target/release/migration-master` | в `%build` не запустилась сборка, проверьте версию rust ≥ 1.75 |
| Устаревшая политика PolicyKit | тест `polkit::tests::test_shipped_policy_matches_render` подскажет: перегенерируйте `resources/com.redos.migration-master.policy` командой `cargo test --lib update_shipped_policy -- --ignored` |

## Чистая сборка (mock)

Для сборки в изолированном окружении без доступа в сеть прокладывайте
зависимости в `vendor/`:

```bash
cargo vendor vendor
# добавьте .cargo/config.toml с source replacement для vendor/
```

## См. также

- [Установка](INSTALL.md)
- [Безопасность](SECURITY.md)
