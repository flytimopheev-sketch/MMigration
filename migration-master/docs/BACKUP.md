# Руководство по созданию резервной копии

Резервное копирование защищает данные от потери при перезаписи во время
миграции. Резервные копии создаются **автоматически** перед изменением файлов
(при `auto_backup = true`, значение по умолчанию) и хранятся в:

```
~/.local/share/migration-master/backups/
```

## Как это работает

1. Перед восстановлением/заменой целевой файл копируется в каталог бэкапов.
2. Создаётся архив `<id>.backup.tar.zst` (`tar` + `zstd`) со встроенным
   `manifest.json` (версия формата, id, дата, базовый каталог, список
   относительных путей, версия приложения).
3. Рядом записывается sidecar-файл `<id>.json` с описанием копии: размер,
   количество файлов, дата, базовый каталог, сохранённые пути.
4. Путь к копии записывается в отчёт операции (`backup_path`) и в таблицу
   `backups` SQLite-истории.
5. Идентификатор вида `20261004-153012-<uuid>`, поэтому повторное копирование
   не затирает существующие копии — каждая версионируется.

## Настройка

```bash
migration-master config list      # показать auto_backup
```

Включается/выключается полем `auto_backup` в
`~/.config/migration-master/config.toml`. Отключение допустимо только если
целевой каталог не содержит ценного (например, чистая VM).

## Ручное создание копии перед миграцией

Перед крупной операцией сделайте полную копию профиля вручную:

```bash
# 1. Чистый архив профиля (эталон состояния ДО миграции)
migration-master create --output ~/backup-before-migration.rmm --passphrase '...'

# 2. Список пакетов (для восстановления окружения)
migration-master list-packages --json > ~/packages-before.json

# 3. Проверка, что архив читается
migration-master verify ~/backup-before-migration.rmm --passphrase '...'
```

Храните `~/backup-before-migration.rmm` на отдельном носителе до полной
проверки нового окружения.

## Проверка копии

```bash
ls -lh ~/.local/share/migration-master/backups
# 20261004-153012-<uuid>.backup.tar.zst   — сама копия
# 20261004-153012-<uuid>.json             — описание (sidecar)
```

Посмотреть содержимое без распаковки:

```bash
zstd -dc ~/.local/share/migration-master/backups/<id>.backup.tar.zst | tar -tvf -
zstd -dc ~/.local/share/migration-master/backups/<id>.backup.tar.zst | tar -xOf - manifest.json
```

## Ручное восстановление копии

Архив не шифруется (в отличие от `.rmm`), поэтому восстановление — обычным
распаковщиком:

```bash
# Полная распаковка в текущий каталог (проверьте список перед запуском!)
zstd -dc ~/.local/share/migration-master/backups/<id>.backup.tar.zst | tar -xvf -

# Только один файл обратно в профиль
zstd -dc ~/.local/share/migration-master/backups/<id>.backup.tar.zst | \
  tar -xvf - --strip-components=0 -C /home/user <относительный/путь>
```

Либо восстановите из основного архива безопасной стратегией (старые файлы
сохраняются как `*.old`):

```bash
migration-master restore ~/backup-before-migration.rmm \
    --target /home/user --strategy rename --passphrase '...'
```

## Ротация

Копии накапливаются. Рекомендуемый порядок очистки (старше 5 последних,
удаляем и sidecar-JSON):

```bash
cd ~/.local/share/migration-master/backups
ls -1dt *.backup.tar.zst | tail -n +6 | while read f; do
  rm -f "$f" "${f%.backup.tar.zst}.json"
done
```

Храните минимум 3–5 последних копий на случай повторного отката.

## Что НЕ попадает в резервную копию

- кэш, `Trash`, временные файлы (исключены по умолчанию — `DEFAULT_EXCLUSIONS`);
- приватные SSH-ключи, если не включён перенос по явному согласию (§4);
- пароли и passphrase — они не сохраняются нигде.

## Связанные документы

- [Руководство пользователя](USER_GUIDE_RU.md)
- [Восстановление после ошибки](RECOVERY.md)
