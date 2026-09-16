Name: migration-master
Version: 0.1.0
Release: 1%{?dist}
Summary: Мастер миграции пользователя для РЕД ОС Linux
License: GPL-3.0
URL: https://github.com/redos/migration-master
Source0: %{name}-%{version}.tar.gz

BuildRequires: rust >= 1.75
BuildRequires: cargo
BuildRequires: gtk4-devel
BuildRequires: libadwaita-devel
BuildRequires: sqlite-devel
BuildRequires: cups-devel
BuildRequires: openssl-devel

Requires: openssh-clients
Requires: cups
Requires: polkit

%description
Migration Master - это приложение для безопасного переноса пользовательского профиля
со старого компьютера или старой установки на новый компьютер. Поддерживает создание
зашифрованных архивов, прямую миграцию по SSH, перенос настроек приложений, SSH-ключей
и принтеров.

%package -n migration-master-gui
Summary: GUI интерфейс для Migration Master
Requires: %{name} = %{version}-%{release}
Requires: gtk4
Requires: libadwaita

%description -n migration-master-gui
Графический интерфейс пользователя для Migration Master, основанный на GTK4 и libadwaita.

%prep
%setup -q

%build
# Сборка CLI версии
cargo build --release --target x86_64-unknown-linux-gnu

# Сборка GUI версии (если доступны зависимости)
%if 0%{?with_gui}
cargo build --release --features gui --target x86_64-unknown-linux-gnu
%endif

%install
mkdir -p %{buildroot}/%{_bindir}
mkdir -p %{buildroot}/%{_datadir}/applications
mkdir -p %{buildroot}/%{_datadir}/icons/hicolor/scalable/apps
mkdir -p %{buildroot}/%{_datadir}/metainfo
mkdir -p %{buildroot}/%{_mandir}/man1

# Установка бинарных файлов
cp target/x86_64-unknown-linux-gnu/release/migration-master %{buildroot}/%{_bindir}/

# Установка desktop файла
install -m 644 resources/migration-master.desktop %{buildroot}/%{_datadir}/applications/

# Установка AppStream metadata
install -m 644 resources/com.redos.migration-master.metainfo.xml %{buildroot}/%{_datadir}/metainfo/ 2>/dev/null || :

# Установка man страницы
gzip -c docs/migration-master.1 > %{buildroot}/%{_mandir}/man1/migration-master.1.gz

%if 0%{?with_gui}
cp target/x86_64-unknown-linux-gnu/release/mm-backend %{buildroot}/%{_bindir}/
%endif

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
%{_bindir}/migration-master
%{_datadir}/applications/migration-master.desktop
%{_datadir}/metainfo/com.redos.migration-master.metainfo.xml
%{_mandir}/man1/migration-master.1.gz

%files -n migration-master-gui
%{_bindir}/mm-backend

%changelog
* Mon Jan 15 2024 Migration Master Team <team@redos.local> - 0.1.0-1
- Первая версия пакета
- Поддержка создания зашифрованных архивов
- Поддержка SSH миграции
- Перенос настроек приложений и SSH-ключей
- Интеграция с CUPS для переноса принтеров
