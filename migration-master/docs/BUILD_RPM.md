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
tar czf migration-master-0.1.0.tar.gz --transform 's,^,migration-master-0.1.0/,' \
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

Результат: `~/rpmbuild/RPMS/x86_64/migration-master-0.1.0-1.<dist>.x86_64.rpm`.

### Что делает spec

- `%build` — `cargo build --release --locked` (CLI + helper);
  с флагом `--with gui` дополнительно собирается GUI (`--features gui`).
- `%check` — `cargo test --release --locked`: юнит-тесты архивов, шифрования,
  path traversal, конфликтов, отмены и драйверов ресурсов.
- `%install` — ставит бинарники, helper, desktop-файл, иконку, AppStream,
  политику PolicyKit, примеры конфигурации и man-страницу.

### Сборка с GUI

```bash
rpmbuild -ba ~/rpmbuild/SPECS/migration-master.spec --with gui
```

Без опции `--with gui` пакет содержит только CLI-часть: команда
`migration-master gui` сообщит, что графический интерфейс не включён, и
предложит консольный режим.

## Проверка пакета

```bash
rpm -Vp ~/rpmbuild/RPMS/x86_64/migration-master-*.rpm
rpm -qlp ~/rpmbuild/RPMS/x86_64/migration-master-*.rpm

# Просмотр зависимостей
rpm -qpR ~/rpmbuild/RPMS/x86_64/migration-master-*.rpm
```

## Установка и smoke-тест

```bash
sudo dnf install ./migration-master-0.1.0-1.x86_64.rpm

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
