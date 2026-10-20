Name:           migration-master
Version:        0.1.3
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
# Графический мастер (GTK4 + libadwaita) ВКЛЮЧЁН ПО УМОЛЧАНИЮ.
# Раньше bcond gui был объявлен как bcond_with, и обычная сборка `rpmbuild -bb`
# давала пакет только с CLI: команда `migration-master gui` отвечала
# «графический интерфейс недоступен». Теперь GUI собирается всегда, а CLI-only
# вариант получается явным флагом `--without gui`.
%bcond_without gui

# musl-линковка несовместима с GUI: GTK4/libadwaita нельзя линковать статически.
%if %{with musl}
%global build_gui 0
%else
%global build_gui %{with_gui}
%endif

# CI-вариант GUI: rpmbuild --with gui --define "zigbuild 1" — линковка через
# cargo-zigbuild (zig cc) против glibc версии не новее %{glibc_floor}: пакет
# с GUI устанавливается на РЕД ОС, где glibc старше, чем на хосте CI
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
BuildRequires:  pkgconfig(sqlite3)
BuildRequires:  pkgconfig(cups)
# GTK4 и libadwaita нужны только для сборки GUI.
%if %{build_gui}
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libadwaita-1)
%endif

# Привилегированные операции выполняются через pkexec + белый список действий.
Requires:       polkit
Requires:       openssh-clients
Requires:       cups
%if %{build_gui}
# Графический мастер использует только GTK4 и libadwaita 1.0, но RPM
# автоматически добавляет зависимость только по soname (libadwaita-1.so.0),
# поэтому версия библиотеки в требованиях не видна. Имена пакетов добавлены
# как Recommends: dnf поставит gtk4 и libadwaita из репозитория РЕД ОС, если
# они там есть, и не сломает установку, если их нет.
Recommends:     gtk4
Recommends:     libadwaita
%endif

%description
Migration Master - это приложение для безопасного переноса пользовательского профиля
со старого компьютера или старой установки на новый компьютер. Поддерживает создание
зашифрованных архивов, прямую миграцию по SSH, перенос настроек приложений, SSH-ключей
и принтеров. Данные не покидают локальную сеть.

%prep
%setup -q

%build
# CLI и helper собираются всегда; графический мастер — по умолчанию.
# Вариант без GUI собирается флагом `--without gui`.
%if %{with musl}
# Статическая musl-линковка: бинарники не зависят от glibc/линковщика целевой
# системы — `rpm -qpR` не покажет ни одной версии GLIBC, установка идёт на
# любую РЕД ОС. Для этого нужны musl-gcc и rust-таргет:
#   rustup target add x86_64-unknown-linux-musl
%if %{with_gui}
echo "ВНИМАНИЕ: вариант --with musl собирает только CLI (GTK4 нельзя линковать статически); GUI отключён."
%endif
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
cargo build --release --locked --target x86_64-unknown-linux-musl
%else
%if %{build_gui}
%if 0%{?zigbuild}
# GUI через cargo-zigbuild: zig cc линкует бинарники против glibc
# %{glibc_floor} (см. шапку spec) — пакет с GUI устанавливается на РЕД ОС
# независимо от версии glibc на хосте сборки.
# rustc резолвит -l-имена через свой линкер zig и не заходит в /usr/lib* —
# каталоги dev-симлинков GTK/GLib/Cairo указываем явно флагом -L.
# Хостовые .so (libvulkan от ubuntu) требуют glibc новее потолка: не проверяем
# их собственные ссылки — на целевой РЕД ОС подставятся свои библиотеки.
export RUSTFLAGS="${RUSTFLAGS:-} -L native=/usr/lib/x86_64-linux-gnu -L native=/usr/lib -C link-arg=-Wl,--allow-shlib-undefined"
cargo zigbuild --release --locked --features gui --target x86_64-unknown-linux-gnu.%{glibc_floor}
# cargo-zigbuild складывает бинарники в target/<triple>/release —
# нормализуем в target/release, чтобы %install не зависел от layout.
mkdir -p target/release
SRC=$(dirname "$(find target -type f -path '*/release/migration-master' | head -n 1)")
if [ "$SRC" != "target/release" ]; then
    cp -f "$SRC/migration-master" "$SRC/migration-master-helper" target/release/
fi
%else
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
%if %{build_gui}
# Тесты — с той же фичей gui, что и сборка: иначе cargo пересоберёт бинарник
# без графической части и в пакет попадёт CLI-only вариант.
cargo test --release --locked --features gui
%else
cargo test --release --locked
%endif
%endif
%if %{build_gui}
# Пакет без GUI — это ровно тот баг, из-за которого `migration-master gui`
# отвечал «графический интерфейс недоступен». Проверяем, что бинарник
# действительно линкует GTK4.
if ! grep -q "libgtk-4" %{bin_dir}/migration-master; then
    echo "ОШИБКА: бинарник собран без GTK4 — графический мастер в пакете не работает" >&2
    exit 1
fi
echo "OK: графический мастер слинкован с GTK4."
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
* Tue Oct 20 2026 Migration Master Team <team@redos.local> - 0.1.3-1
- Графический мастер включён по умолчанию: обычная сборка `rpmbuild -bb`
  больше не даёт пакет только с CLI (было: «графический интерфейс недоступен»)
- GUI переведён на GTK4 + libadwaita 1.0: убраны NavigationView/NavigationPage/
  ToolbarView (libadwaita 1.4) и MessageDialog (1.2), PasswordEntry (GTK 4.4) —
  мастер запускается на РЕД ОС 8 с libadwaita 1.2
- При запуске GUI выводится понятная причина сбоя (нет графической сессии,
  DISPLAY/WAYLAND_DISPLAY), а не пустой терминал
- В секции check пакета проверяется, что бинарник линкует GTK4
- Пакеты gtk4 и libadwaita добавлены в Recommends (RPM тянет их из репозитория)

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
