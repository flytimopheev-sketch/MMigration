# Участие в разработке

Спасибо, что помогаете развивать Migration Master. Проект небольшой, правил
не много — и все они проверяются автоматически.

## Быстрый старт

```bash
sudo dnf install rust cargo gcc make pkgconf-pkg-config \
                 gtk4-devel libadwaita-devel sqlite-devel cups-devel openssh-clients

cd migration-master
cargo build --features gui     # CLI + GUI
cargo test --features gui
```

Графическая часть собирается только на Linux с GTK4 и libadwaita; на других
платформах доступен только CLI (`cargo build` без фичи `gui`).

## Проверка перед отправкой

```bash
cd migration-master
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

`--all-features` обязателен: без него GUI-код (`src/ui/gtk_app.rs`) вообще
не компилируется и не проверяется линтером.

Те же проверки выполняет CI (`.github/workflows/rust.yml`) на каждом пуше в
`main` и в каждом pull request.

## Структура репозитория

```
.
├── README.md               # описание проекта (этот уровень)
├── CHANGELOG.md            # история изменений
├── LICENSE                 # GPL-3.0
├── .github/workflows/      # CI: тесты и сборка релизного RPM
└── migration-master/       # сам проект (crate + spec-файл RPM)
    ├── Cargo.toml
    ├── src/                # исходный код (см. docs/ARCHITECTURE.md)
    ├── docs/               # документация
    ├── resources/          # иконка, desktop-файл, AppStream, политика polkit
    └── rpm/                # spec-файл пакета
```

Подробное описание слоёв, структуры модулей и правил разработки —
в [docs/CONTRIBUTING.md](migration-master/docs/CONTRIBUTING.md) и
[docs/ARCHITECTURE.md](migration-master/docs/ARCHITECTURE.md).

## Правила изменений

- Не добавляйте прямой вызов shell: внешние команды — только через
  `std::process::Command` с отдельными аргументами.
- Новые пути проходят `security::validate_path` / `safe_join` (path traversal).
- Секреты не попадают в структуры `serde`, журналы и отчёты.
- Долгие операции проверяют токен отмены (`cancel`).
- На каждую ветку поведения — тест, включая ветку «ошибка» и «dry-run».
- GUI использует только GTK4 4.0 и libadwaita 1.0: более новые API
  (`v1_2`/`v1_4`-фичи крейта `adw`, `gtk::PasswordEntry`) приводят к
  `undefined symbol` на РЕД ОС 8.

## Как отправить изменения

1. Создайте ветку от `main`: `git checkout -b feature/short-description`.
2. Внесите изменения, поправьте замечания `cargo fmt` и `cargo clippy`.
3. Обновите `CHANGELOG.md` (раздел «Незавершённое») и документацию, если
   изменилось поведение.
4. Откройте pull request — CI проверит форматирование, линтер и тесты.

## Релиз

Релизные RPM собираются автоматически: тег `vX.Y.Z` запускает workflow
`.github/workflows/release-rpm.yml`, который собирает пакет и прикрепляет его
к GitHub Releases. Подробности — в
[docs/BUILD_RPM.md](migration-master/docs/BUILD_RPM.md).
