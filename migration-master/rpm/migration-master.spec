Name:           migration-master
Version:        0.1.2
Release:        1%{?dist}
Summary:        Мастер миграции пользователя для РЕД ОС Linux
License:        GPL-3.0
URL:            https://github.com/flytimopheev-sketch/MMigration
Source0:        %{name}-%{version}.tar.gz

# Опция сборки: rpmbuild --with musl — статическая линковка musl.
# Такие бинарники не зависят от glibc целевой системы и ставятся на любую
# РЕД ОС (иначе при установке rpm ругается на libc.so.6(GLIBC_x.yy), если
# пакет собран на системе с более новым glibc, например на Debian CI).
%bcond_with musl
%bcond_with gui

# CI-вариант GUI: rpmbuild --with gui --define "zigbuild 1" — линковка через
# cargo-zigbuild (zig cc) против glibc версии не новее %{glibc_floor}:
# пакет с GUI устанавливается на РЕД ОС, где glibc старше, чем на хосте CI
# (ubuntu-latest даёт GLIBC_2.39). Потолок переопределяется опцией
# --define "glibc_floor X.Y".
%{!?glibc_floor: %global glibc_floor 2.28}

%if %{with musl}
%global bin_dir target/x86_64-unknown-linux-musl/release
%else
%global bin_dir target/release
%endif

BuildRequires:  rust >= 1.75
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libadwaita-1)
BuildRequires:  pkgconfig(sqlite3)
BuildRequires:  pkgconfig(cups)

# Привилегированные операции выполняются через pkexec + белый список действий.
Requires:       polkit
Requires:       openssh-clients
Requires:       cups

%description
Migration Master - это приложение для безопасного переноса пользовательского профиля
со старого компьютера или старой установки на новый компьютер. Поддерживает создание
зашифрованных архивов, прямую миграцию по SSH, перенос настроек приложений, SSH-ключей
и принтеров. Данные не покидают локальную сеть.

%prep
%setup -q

%build
# CLI и helper собираются всегда; GUI включается флагом сборки `--with gui`.
%if %{with musl}
# Статическая musl-линковка: бинарники не зависят от glibc/линковщика целевой
# системы — `rpm -qpR` не покажет ни одной версии GLIBC, установка идёт на
# любую РЕД ОС. Для этого нужны musl-gcc и rust-тarget:
#   rustup target add x86_64-unknown-linux-musl
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
cargo build --release --locked --target x86_64-unknown-linux-musl
%if %{with gui}
%{error: --with gui несовместим с --with musl: GTK4/libadwaita нельзя линковать статически; GUI собирайте на самой РЕД ОС}
%endif
%else
%if %{with gui}
%if 0%{?zigbuild}
# GUI через cargo-zigbuild: zig cc линкует бинарники против glibc
# %{glibc_floor} (см. шапку spec) — пакет с GUI устанавливается на РЕД ОС
# независимо от версии glibc на хосте сборки.
# rustc резолвит -l-имена через свой линкер zig и не заходит в /usr/lib* —
# каталоги dev-симлинков GTK/GLib/Cairo указываем явно флагом -L.
export RUSTFLAGS="${RUSTFLAGS:-} -L native=/usr/lib/x86_64-linux-gnu -L native=/usr/lib"
cargo zigbuild --release --locked --features gui --target x86_64-unknown-linux-gnu.%{glibc_floor}
# cargo-zigbuild складывает бинарники в target/<triple>/release —
# нормализуем в target/release, чтобы %install не зависел от layout.
mkdir -p target/release
SRC=$(dirname "$(find target -type f -path '*/release/migration-master' | head -n 1)")
if [ "$SRC" != "target/release" ]; then
    cp -f "$SRC/migration-master" "$SRC/migration-master-helper" target/release/
fi
%else
cargo build --release --locked
cargo build --release --locked --features gui
%endif
%else
cargo build --release --locked
%endif
%endif

%check
# Юнит-тесты (архивы, шифрование, path traversal, конфликты, отмена, драйверы ресурсов).
%if %{with musl}
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
cargo test --release --locked --target x86_64-unknown-linux-musl
%else
cargo test --release --locked
%endif

%install
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/scalable/apps
mkdir -p %{buildroot}%{_datadir}/metainfo
mkdir -p %{buildroot}%{_datadir}/polkit-1/actions
mkdir -p %{buildroot}%{_datadir}/%{name}/examples
mkdir -p %{buildroot}%{_mandir}/man1

# Основной бинарник и привилегированный помощник (белый список операций).
# %{bin_dir} = target/release либо target/x86_64-unknown-linux-musl/release (опция --with musl).
install -m 755 %{bin_dir}/migration-master %{buildroot}%{_bindir}/migration-master
install -m 755 %{bin_dir}/migration-master-helper %{buildroot}%{_bindir}/migration-master-helper

# Интеграция с рабочим столом, AppStream и PolicyKit.
install -m 644 resources/migration-master.desktop %{buildroot}%{_datadir}/applications/
install -m 644 resources/migration-master.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/
install -m 644 resources/com.redos.migration-master.metainfo.xml %{buildroot}%{_datadir}/metainfo/
install -m 644 resources/com.redos.migration-master.policy %{buildroot}%{_datadir}/polkit-1/actions/

# Примеры конфигурации и правил миграции приложений (§5).
install -m 644 resources/examples/config.toml %{buildroot}%{_datadir}/%{name}/examples/
install -m 644 resources/examples/app-rules.yaml %{buildroot}%{_datadir}/%{name}/examples/

# Man-страница.
gzip -c docs/migration-master.1 > %{buildroot}%{_mandir}/man1/migration-master.1.gz

%post
update-desktop-database %{_datadir}/applications &>/dev/null || :
touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :

%postun
update-desktop-database %{_datadir}/applications &>/dev/null || :
if [ $1 -eq 0 ]; then
    touch --no-create %{_datadir}/icons/hicolor &>/dev/null || :
    gtk-update-icon-cache %{_datadir}/icons/hicolor &>/dev/null || :
fi

%files
%doc README.md
%{_bindir}/migration-master
%{_bindir}/migration-master-helper
%{_datadir}/applications/migration-master.desktop
%{_datadir}/icons/hicolor/scalable/apps/migration-master.svg
%{_datadir}/metainfo/com.redos.migration-master.metainfo.xml
%{_datadir}/polkit-1/actions/com.redos.migration-master.policy
%{_datadir}/%{name}/examples/config.toml
%{_datadir}/%{name}/examples/app-rules.yaml
%{_mandir}/man1/migration-master.1.gz

%changelog
* Fri Oct 09 2026 Migration Master Team <team@redos.local> - 0.1.2-1
- Release RPM по умолчанию собирается с GUI (фича gui, GTK4 + libadwaita)
- CI: линковка GUI через cargo-zigbuild с потолком glibc 2.28 — пакет
  устанавливается на РЕД ОС независимо от версии glibc хоста CI
- CI: smoke-тест GUI-бинарника (команда gui запускает GTK-код)

* Mon Oct 05 2026 Migration Master Team <team@redos.local> - 0.1.1-1
- Опция сборки --with musl: статическая линковка CLI и helper,
  бинарники не зависят от glibc целевой системы
- Пакет устанавливается на любую РЕД ОС независимо от версии glibc
  (раньше установка падала из-за libc.so.6(GLIBC_2.xx) из чужого окружения CI)
- CI: Release RPM по умолчанию собирает статический musl-пакет

* Sun Oct 04 2026 Migration Master Team <team@redos.local> - 0.1.0-1
- Установка migration-master-helper, политики PolicyKit, иконки и примеров
- Подключены юнит-тесты в %check
- Исправлена установка несуществующего бинарника mm-backend

* Mon Jan 15 2024 Migration Master Team <team@redos.local> - 0.1.0-1
- Первая версия пакета
- Поддержка создания зашифрованных архивов
- Поддержка SSH миграции
- Перенос настроек приложений и SSH-ключей
- Интеграция с CUPS для переноса принтеров
