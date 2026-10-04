Name:           migration-master
Version:        0.1.0
Release:        1%{?dist}
Summary:        Мастер миграции пользователя для РЕД ОС Linux
License:        GPL-3.0
URL:            https://github.com/flytimopheev-sketch/MMigration
Source0:        %{name}-%{version}.tar.gz

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
cargo build --release --locked

%if 0%{?with_gui}
cargo build --release --locked --features gui
%endif

%check
# Юнит-тесты (архивы, шифрование, path traversal, конфликты, отмена, драйверы ресурсов).
cargo test --release --locked

%install
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/scalable/apps
mkdir -p %{buildroot}%{_datadir}/metainfo
mkdir -p %{buildroot}%{_datadir}/polkit-1/actions
mkdir -p %{buildroot}%{_datadir}/%{name}/examples
mkdir -p %{buildroot}%{_mandir}/man1

# Основной бинарник и привилегированный помощник (белый список операций).
install -m 755 target/release/migration-master %{buildroot}%{_bindir}/migration-master
install -m 755 target/release/migration-master-helper %{buildroot}%{_bindir}/migration-master-helper

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
