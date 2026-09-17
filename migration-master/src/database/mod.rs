//! Модуль базы данных для хранения истории миграций

use std::path::{Path, PathBuf};
use rusqlite::{Connection, params};
use chrono::{DateTime, Utc, Duration};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::error::{MigrationError, Result};

/// Статус операции миграции
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStatus {
    /// Ожидает выполнения
    Pending,
    /// В процессе
    InProgress,
    /// Успешно завершено
    Completed,
    /// Завершено с ошибками
    PartiallyCompleted,
    /// Отменено пользователем
    Cancelled,
    /// Ошибка выполнения
    Failed,
}

/// Тип операции
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationType {
    /// Создание архива
    CreateArchive,
    /// Восстановление из архива
    Restore,
    /// Прямая миграция по SSH
    SshMigration,
    /// Сканирование профиля
    ScanProfile,
    /// Проверка архива
    InspectArchive,
    /// Резервное копирование
    Backup,
}

/// Запись о миграции в БД
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRecord {
    /// Уникальный идентификатор
    pub id: String,
    /// Дата и время начала
    pub started_at: DateTime<Utc>,
    /// Дата и время окончания
    pub finished_at: Option<DateTime<Utc>>,
    /// Тип операции
    pub operation_type: OperationType,
    /// Статус
    pub status: MigrationStatus,
    /// Источник (путь или хост)
    pub source: String,
    /// Цель (путь)
    pub target: String,
    /// Пользователь
    pub username: String,
    /// Хост источника
    pub source_hostname: Option<String>,
    /// Хост цели
    pub target_hostname: Option<String>,
    /// Выбранные компоненты (JSON)
    pub components: String,
    /// Количество файлов
    pub file_count: u64,
    /// Общий размер в байтах
    pub total_size: u64,
    /// Количество перенесённых файлов
    pub transferred_files: u64,
    /// Размер перенесённых данных
    pub transferred_size: u64,
    /// Количество ошибок
    pub error_count: u32,
    /// Количество предупреждений
    pub warning_count: u32,
    /// Путь к отчёту
    pub report_path: Option<String>,
    /// Путь к резервной копии
    pub backup_path: Option<String>,
    /// Сообщение об ошибке
    pub error_message: Option<String>,
    /// Длительность в секундах
    pub duration_secs: Option<f64>,
}

impl MigrationRecord {
    pub fn new(operation_type: OperationType, source: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            started_at: Utc::now(),
            finished_at: None,
            operation_type,
            status: MigrationStatus::Pending,
            source: source.into(),
            target: target.into(),
            username: whoami::username(),
            source_hostname: None,
            target_hostname: None,
            components: "[]".to_string(),
            file_count: 0,
            total_size: 0,
            transferred_files: 0,
            transferred_size: 0,
            error_count: 0,
            warning_count: 0,
            report_path: None,
            backup_path: None,
            error_message: None,
            duration_secs: None,
        }
    }

    pub fn mark_started(&mut self) {
        self.status = MigrationStatus::InProgress;
        self.started_at = Utc::now();
    }

    pub fn mark_completed(&mut self) {
        self.finished_at = Some(Utc::now());
        self.status = MigrationStatus::Completed;
        self.duration_secs = self.finished_at.map(|f| f.signed_duration_since(self.started_at).num_milliseconds() as f64 / 1000.0);
    }

    pub fn mark_failed(&mut self, error: impl Into<String>) {
        self.finished_at = Some(Utc::now());
        self.status = MigrationStatus::Failed;
        self.error_message = Some(error.into());
        self.duration_secs = self.finished_at.map(|f| f.signed_duration_since(self.started_at).num_milliseconds() as f64 / 1000.0);
    }

    pub fn mark_cancelled(&mut self) {
        self.finished_at = Some(Utc::now());
        self.status = MigrationStatus::Cancelled;
        self.duration_secs = self.finished_at.map(|f| f.signed_duration_since(self.started_at).num_milliseconds() as f64 / 1000.0);
    }

    pub fn mark_partially_completed(&mut self) {
        self.finished_at = Some(Utc::now());
        self.status = MigrationStatus::PartiallyCompleted;
        self.duration_secs = self.finished_at.map(|f| f.signed_duration_since(self.started_at).num_milliseconds() as f64 / 1000.0);
    }
}

/// Менеджер базы данных
pub struct DatabaseManager {
    db_path: PathBuf,
    conn: Connection,
}

impl DatabaseManager {
    /// Создание нового менеджера БД
    pub fn new(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref().to_path_buf();
        
        // Создаём директорию если нужно
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)?;
        
        let mut manager = Self { db_path, conn };
        manager.init_schema()?;
        
        Ok(manager)
    }

    /// Инициализация схемы БД
    fn init_schema(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS migrations (
                id TEXT PRIMARY KEY,
                started_at TEXT NOT NULL,
                finished_at TEXT,
                operation_type TEXT NOT NULL,
                status TEXT NOT NULL,
                source TEXT NOT NULL,
                target TEXT NOT NULL,
                username TEXT NOT NULL,
                source_hostname TEXT,
                target_hostname TEXT,
                components TEXT NOT NULL,
                file_count INTEGER NOT NULL DEFAULT 0,
                total_size INTEGER NOT NULL DEFAULT 0,
                transferred_files INTEGER NOT NULL DEFAULT 0,
                transferred_size INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                warning_count INTEGER NOT NULL DEFAULT 0,
                report_path TEXT,
                backup_path TEXT,
                error_message TEXT,
                duration_secs REAL
            );

            CREATE TABLE IF NOT EXISTS migration_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                migration_id TEXT NOT NULL,
                item_type TEXT NOT NULL,
                source_path TEXT NOT NULL,
                target_path TEXT NOT NULL,
                size INTEGER NOT NULL,
                status TEXT NOT NULL,
                error_message TEXT,
                FOREIGN KEY (migration_id) REFERENCES migrations(id)
            );

            CREATE TABLE IF NOT EXISTS hosts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                hostname TEXT NOT NULL UNIQUE,
                address TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 22,
                username TEXT NOT NULL,
                last_connected TEXT,
                fingerprint TEXT,
                trusted BOOLEAN NOT NULL DEFAULT FALSE
            );

            CREATE TABLE IF NOT EXISTS archives (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                path TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                source_hostname TEXT NOT NULL,
                username TEXT NOT NULL,
                uid INTEGER NOT NULL,
                gid INTEGER NOT NULL,
                format_version INTEGER NOT NULL,
                app_version TEXT NOT NULL,
                os_info TEXT NOT NULL,
                encrypted BOOLEAN NOT NULL DEFAULT FALSE,
                signed BOOLEAN NOT NULL DEFAULT FALSE,
                checksum TEXT NOT NULL,
                total_size INTEGER NOT NULL,
                file_count INTEGER NOT NULL,
                components TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS application_rules (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                app_name TEXT NOT NULL UNIQUE,
                config_paths TEXT NOT NULL,
                data_paths TEXT,
                transfer_mode TEXT NOT NULL,
                requires_restart BOOLEAN NOT NULL DEFAULT FALSE,
                min_version TEXT,
                max_version TEXT,
                warnings TEXT,
                post_migration_hook TEXT
            );

            CREATE TABLE IF NOT EXISTS operation_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                migration_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                level TEXT NOT NULL,
                category TEXT NOT NULL,
                message TEXT NOT NULL,
                data TEXT,
                FOREIGN KEY (migration_id) REFERENCES migrations(id)
            );

            CREATE TABLE IF NOT EXISTS backups (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                migration_id TEXT NOT NULL,
                path TEXT NOT NULL,
                created_at TEXT NOT NULL,
                size INTEGER NOT NULL,
                files_count INTEGER NOT NULL,
                restored BOOLEAN NOT NULL DEFAULT FALSE,
                FOREIGN KEY (migration_id) REFERENCES migrations(id)
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_migrations_status ON migrations(status);
            CREATE INDEX IF NOT EXISTS idx_migrations_started_at ON migrations(started_at);
            CREATE INDEX IF NOT EXISTS idx_migrations_operation_type ON migrations(operation_type);
            CREATE INDEX IF NOT EXISTS idx_migration_items_migration_id ON migration_items(migration_id);
            CREATE INDEX IF NOT EXISTS idx_operation_logs_migration_id ON operation_logs(migration_id);
            "
        )?;

        Ok(())
    }

    /// Сохранение записи о миграции
    pub fn save_migration(&mut self, record: &MigrationRecord) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO migrations (
                id, started_at, finished_at, operation_type, status,
                source, target, username, source_hostname, target_hostname,
                components, file_count, total_size, transferred_files,
                transferred_size, error_count, warning_count, report_path,
                backup_path, error_message, duration_secs
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                record.id,
                record.started_at.to_rfc3339(),
                record.finished_at.map(|t| t.to_rfc3339()),
                serde_json::to_string(&record.operation_type)?,
                serde_json::to_string(&record.status)?,
                record.source,
                record.target,
                record.username,
                record.source_hostname,
                record.target_hostname,
                record.components,
                record.file_count,
                record.total_size,
                record.transferred_files,
                record.transferred_size,
                record.error_count,
                record.warning_count,
                record.report_path,
                record.backup_path,
                record.error_message,
                record.duration_secs,
            ],
        )?;

        Ok(())
    }

    /// Получение записи по ID
    pub fn get_migration(&self, id: &str) -> Result<Option<MigrationRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM migrations WHERE id = ?1"
        )?;

        let row = stmt.query_row(params![id], |row| {
            Ok(MigrationRecord {
                id: row.get("id")?,
                started_at: DateTime::parse_from_rfc3339(&row.get::<_, String>("started_at")?)
                    .map(|d| d.with_timezone(&Utc))
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?,
                finished_at: row.get::<_, Option<String>>("finished_at")?
                    .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                    .map(|d| d.with_timezone(&Utc)),
                operation_type: serde_json::from_str(&row.get::<_, String>("operation_type")?)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?,
                status: serde_json::from_str(&row.get::<_, String>("status")?)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?,
                source: row.get("source")?,
                target: row.get("target")?,
                username: row.get("username")?,
                source_hostname: row.get("source_hostname")?,
                target_hostname: row.get("target_hostname")?,
                components: row.get("components")?,
                file_count: row.get("file_count")?,
                total_size: row.get("total_size")?,
                transferred_files: row.get("transferred_files")?,
                transferred_size: row.get("transferred_size")?,
                error_count: row.get("error_count")?,
                warning_count: row.get("warning_count")?,
                report_path: row.get("report_path")?,
                backup_path: row.get("backup_path")?,
                error_message: row.get("error_message")?,
                duration_secs: row.get("duration_secs")?,
            })
        })?;

        Ok(row)
    }

    /// Получение истории миграций
    pub fn get_migrations(&self, limit: usize, offset: usize) -> Result<Vec<MigrationRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM migrations ORDER BY started_at DESC LIMIT ?1 OFFSET ?2"
        )?;

        let rows = stmt.query_map(params![limit, offset], |row| {
            let started_at_str: String = row.get("started_at")?;
            let started_at = DateTime::parse_from_rfc3339(&started_at_str)
                .map(|d| d.with_timezone(&Utc))
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))))?;

            let finished_at = row.get::<_, Option<String>>("finished_at")?
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&Utc));

            let operation_type_str: String = row.get("operation_type")?;
            let operation_type = serde_json::from_str(&operation_type_str)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))))?;

            let status_str: String = row.get("status")?;
            let status = serde_json::from_str(&status_str)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))))?;

            Ok(MigrationRecord {
                id: row.get("id")?,
                started_at,
                finished_at,
                operation_type,
                status,
                source: row.get("source")?,
                target: row.get("target")?,
                username: row.get("username")?,
                source_hostname: row.get("source_hostname")?,
                target_hostname: row.get("target_hostname")?,
                components: row.get("components")?,
                file_count: row.get("file_count")?,
                total_size: row.get("total_size")?,
                transferred_files: row.get("transferred_files")?,
                transferred_size: row.get("transferred_size")?,
                error_count: row.get("error_count")?,
                warning_count: row.get("warning_count")?,
                report_path: row.get("report_path")?,
                backup_path: row.get("backup_path")?,
                error_message: row.get("error_message")?,
                duration_secs: row.get("duration_secs")?,
            })
        })?;

        let migrations: Vec<_> = rows.filter_map(|r| r.ok()).collect();
        Ok(migrations)
    }

    /// Получение миграций за период
    pub fn get_migrations_by_date_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<MigrationRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM migrations 
             WHERE started_at >= ?1 AND started_at <= ?2 
             ORDER BY started_at DESC"
        )?;

        let rows = stmt.query_map(params![start.to_rfc3339(), end.to_rfc3339()], |row| {
            let started_at_str: String = row.get("started_at")?;
            let started_at = DateTime::parse_from_rfc3339(&started_at_str)
                .map(|d| d.with_timezone(&Utc))
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))))?;

            let finished_at = row.get::<_, Option<String>>("finished_at")?
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&Utc));

            let operation_type_str: String = row.get("operation_type")?;
            let operation_type = serde_json::from_str(&operation_type_str)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))))?;

            let status_str: String = row.get("status")?;
            let status = serde_json::from_str(&status_str)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))))?;

            Ok(MigrationRecord {
                id: row.get("id")?,
                started_at,
                finished_at,
                operation_type,
                status,
                source: row.get("source")?,
                target: row.get("target")?,
                username: row.get("username")?,
                source_hostname: row.get("source_hostname")?,
                target_hostname: row.get("target_hostname")?,
                components: row.get("components")?,
                file_count: row.get("file_count")?,
                total_size: row.get("total_size")?,
                transferred_files: row.get("transferred_files")?,
                transferred_size: row.get("transferred_size")?,
                error_count: row.get("error_count")?,
                warning_count: row.get("warning_count")?,
                report_path: row.get("report_path")?,
                backup_path: row.get("backup_path")?,
                error_message: row.get("error_message")?,
                duration_secs: row.get("duration_secs")?,
            })
        })?;

        let migrations: Vec<_> = rows.filter_map(|r| r.ok()).collect();
        Ok(migrations)
    }

    /// Удаление старых записей
    pub fn cleanup_old_records(&mut self, days: u32) -> Result<usize> {
        let cutoff = Utc::now() - Duration::days(days as i64);
        
        let tx = self.conn.transaction()?;
        
        // Удаляем связанные записи сначала
        tx.execute(
            "DELETE FROM migration_items WHERE migration_id IN (
                SELECT id FROM migrations WHERE started_at < ?1
            )",
            params![cutoff.to_rfc3339()],
        )?;

        tx.execute(
            "DELETE FROM operation_logs WHERE migration_id IN (
                SELECT id FROM migrations WHERE started_at < ?1
            )",
            params![cutoff.to_rfc3339()],
        )?;

        tx.execute(
            "DELETE FROM backups WHERE migration_id IN (
                SELECT id FROM migrations WHERE started_at < ?1
            )",
            params![cutoff.to_rfc3339()],
        )?;

        let deleted = tx.execute(
            "DELETE FROM migrations WHERE started_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;

        tx.commit()?;
        
        Ok(deleted)
    }

    /// Сохранение хоста
    pub fn save_host(
        &mut self,
        hostname: &str,
        address: &str,
        port: u16,
        username: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO hosts (hostname, address, port, username, last_connected, trusted)
             VALUES (?1, ?2, ?3, ?4, NULL, FALSE)",
            params![hostname, address, port, username],
        )?;

        Ok(())
    }

    /// Обновление времени последнего подключения к хосту
    pub fn update_host_connection(&mut self, hostname: &str, fingerprint: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE hosts SET last_connected = ?1, fingerprint = ?2, trusted = TRUE
             WHERE hostname = ?3",
            params![Utc::now().to_rfc3339(), fingerprint, hostname],
        )?;

        Ok(())
    }

    /// Получение сохранённых хостов
    pub fn get_hosts(&self) -> Result<Vec<(String, String, u16, String, bool)>> {
        let mut stmt = self.conn.prepare(
            "SELECT hostname, address, port, username, trusted FROM hosts ORDER BY last_connected DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>("hostname")?,
                row.get::<_, String>("address")?,
                row.get::<_, u16>("port")?,
                row.get::<_, String>("username")?,
                row.get::<_, bool>("trusted")?,
            ))
        })?;

        let hosts: Vec<_> = rows.filter_map(|r| r.ok()).collect();
        Ok(hosts)
    }

    /// Сохранение настройки
    pub fn save_setting(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value, updated_at)
             VALUES (?1, ?2, ?3)",
            params![key, value, Utc::now().to_rfc3339()],
        )?;

        Ok(())
    }

    /// Получение настройки
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT value FROM settings WHERE key = ?1"
        )?;

        let result = stmt.query_row(params![key], |row| {
            row.get::<_, String>("value")
        });

        match result {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(MigrationError::Database(e.into())),
        }
    }
}
