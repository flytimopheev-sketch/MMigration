//! Мастер миграции: 11 этапов, сохраняемое состояние, отмена, отчёты, история.
//!
//! Мастер связывает сервисы уровня приложения: [`crate::profile_scanner`],
//! [`crate::archive`], [`crate::file_transfer`], [`crate::backup`],
//! [`crate::database`] и [`crate::report`].
//!
//! Особенности:
//! * [`CancelToken`] передаётся во все длительные операции (архивирование,
//!   восстановление, копирование), а [`crate::cancel::install_ctrl_c_handler`]
//!   отменяет глобальный токен процесса по Ctrl+C;
//! * состояние мастера сохраняется в JSON, парольная фраза при этом
//!   **не** сериализуется;
//! * приватные SSH-ключи и `authorized_keys` не попадают в перенос без
//!   явного согласия;
//! * операция попадает в историю SQLite и сохраняется в отчёте (§17).

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::applications::AppRule;
use crate::archive::{ArchiveManager, CreateArchiveOptions, RestoreOptions};
use crate::backup::BackupManager;
use crate::cancel::{CancelHandle, CancelToken};
use crate::compatibility::{self, CompatibilityReport, SourceMetadata};
use crate::config::{self, ComponentType, MigrationMode};
use crate::conflict_resolver::ConflictStrategy;
use crate::database::{DatabaseManager, MigrationRecord, OperationType};
use crate::error::{MigrationError, Result};
use crate::file_transfer::{ProgressObserver, TransferItem};
use crate::packages::PackageManager;
use crate::platform;
use crate::printers::PrinterManager;
use crate::report::{MigrationReport, ReportFormat};
use crate::ssh_keys::SshKeysScanner;
use crate::ssh_transfer::{SshTarget, SshTransfer, SshTransferOptions};

/// Версия формата сохраняемого состояния мастера.
pub const STATE_VERSION: u32 = 1;

/// Этап мастера (§2 — 11 шагов assistants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WizardStep {
    /// Приветствие и выбор сценария
    Welcome,
    /// Параметры подключения (SSH) или путь к архиву
    Connection,
    /// Выбор компонентов
    Components,
    /// Исключения
    Exclusions,
    /// Дополнительные параметры
    Options,
    /// Предварительный просмотр
    Preview,
    /// Проверка совместимости
    Compatibility,
    /// Резервная копия
    Backup,
    /// Выполнение и прогресс
    Progress,
    /// Разрешение конфликтов
    Conflicts,
    /// Итоговый отчёт
    Report,
    /// Завершение
    Finish,
}

impl WizardStep {
    /// Этапы в порядке прохождения.
    pub fn all() -> [Self; 12] {
        [
            Self::Welcome,
            Self::Connection,
            Self::Components,
            Self::Exclusions,
            Self::Options,
            Self::Preview,
            Self::Compatibility,
            Self::Backup,
            Self::Progress,
            Self::Conflicts,
            Self::Report,
            Self::Finish,
        ]
    }

    /// Номер этапа (0 — первый).
    pub fn index(self) -> usize {
        Self::all()
            .into_iter()
            .position(|step| step == self)
            .unwrap_or(0)
    }

    /// Название этапа для интерфейса.
    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Добро пожаловать",
            Self::Connection => "Подключение",
            Self::Components => "Компоненты",
            Self::Exclusions => "Исключения",
            Self::Options => "Параметры",
            Self::Preview => "Предварительный просмотр",
            Self::Compatibility => "Совместимость",
            Self::Backup => "Резервная копия",
            Self::Progress => "Выполнение",
            Self::Conflicts => "Конфликты",
            Self::Report => "Отчёт",
            Self::Finish => "Завершение",
        }
    }

    /// Следующий этап (или `None` на последнем).
    pub fn next(self) -> Option<Self> {
        Self::all().get(self.index() + 1).copied()
    }

    /// Предыдущий этап (или `None` на первом).
    pub fn previous(self) -> Option<Self> {
        self.index()
            .checked_sub(1)
            .and_then(|index| Self::all().get(index).copied())
    }
}

/// Сцена мастера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WizardStage {
    /// Создан
    Created,
    /// Профиль просканирован
    Scanned,
    /// Выполняется
    Running,
    /// Завершено успешно
    Completed,
    /// Завершено с ошибкой
    Failed,
    /// Прервано пользователем
    Cancelled,
}

/// Встроенные правила приложений по умолчанию (§5).
fn default_app_rules() -> Vec<AppRule> {
    crate::applications::builtin_rules()
}

/// Конфигурация мастера (сериализуется в состояние, кроме секрета и токена).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardConfig {
    /// Режим миграции
    pub mode: MigrationMode,
    /// Домашний каталог-источник
    pub source_home: PathBuf,
    /// Домашний каталог-приёмник (для SSH и восстановления)
    pub target_home: PathBuf,
    /// Компоненты
    pub components: Vec<ComponentType>,
    /// Путь к архиву (LocalArchive — куда писать, Restore — откуда читать)
    pub archive_path: Option<PathBuf>,
    /// Пароль шифрования архива (в состояние не сохраняется)
    #[serde(default, skip_serializing)]
    pub passphrase: Option<String>,
    /// Подключение SSH (для SshDirect)
    pub ssh: Option<SshTarget>,
    /// Стратегия конфликтов
    pub conflict_strategy: ConflictStrategy,
    /// Режим без реальных изменений
    pub dry_run: bool,
    /// Проверять хеши после восстановления
    pub verify: bool,
    /// Уровень сжатия архива
    pub compression_level: i32,
    /// Пользовательские шаблоны исключений (дополняют `DEFAULT_EXCLUSIONS`)
    pub exclusions: Vec<String>,
    /// Отключить стандартные исключения (§3 — кэш, Temp, Trash)
    pub skip_defaults_exclusions: bool,
    /// Переносить приватные SSH-ключи (§4 — только по явному согласию)
    pub include_private_ssh_keys: bool,
    /// Переносить `authorized_keys` (§4)
    pub include_authorized_keys: bool,
    /// Правила миграции настроек приложений (§5). По умолчанию — встроенные.
    #[serde(default = "default_app_rules")]
    pub app_rules: Vec<AppRule>,
    /// Ограничение скорости передачи, байт/с (0 — без ограничения)
    pub rate_limit: u64,
    /// Создавать резервную копию перед перезаписью (§9)
    pub auto_backup: bool,
    /// Каталог резервных копий (по умолчанию — каталог данных приложения)
    pub backup_dir: Option<PathBuf>,
    /// Каталог отчётов (по умолчанию — каталог данных приложения)
    pub report_dir: Option<PathBuf>,
    /// Форматы отчёта (§17)
    pub report_formats: Vec<ReportFormat>,
    /// Каталог/файл БД истории (None — история не ведётся)
    pub database_path: Option<PathBuf>,
    /// Отмена длительных операций (не сериализуется)
    #[serde(skip, default)]
    pub cancel: Option<CancelToken>,
}

impl WizardConfig {
    /// Конфигурация по умолчанию для указанного режима.
    pub fn new(mode: MigrationMode, source_home: impl Into<PathBuf>) -> Self {
        Self {
            mode,
            source_home: source_home.into(),
            target_home: PathBuf::new(),
            components: ComponentType::default_components(),
            archive_path: None,
            passphrase: None,
            ssh: None,
            conflict_strategy: ConflictStrategy::Ask,
            dry_run: false,
            verify: true,
            compression_level: 3,
            exclusions: Vec::new(),
            skip_defaults_exclusions: false,
            include_private_ssh_keys: false,
            include_authorized_keys: false,
            app_rules: crate::applications::builtin_rules(),
            rate_limit: 0,
            auto_backup: true,
            backup_dir: None,
            report_dir: None,
            report_formats: vec![ReportFormat::Json],
            database_path: None,
            cancel: None,
        }
    }

    /// Эффективные шаблоны исключений (§3).
    pub fn effective_exclusions(&self) -> Result<Vec<glob::Pattern>> {
        let mut patterns: Vec<String> = Vec::new();
        if !self.skip_defaults_exclusions {
            patterns.extend(
                config::DEFAULT_EXCLUSIONS
                    .iter()
                    .map(|item| item.to_string()),
            );
        }
        patterns.extend(
            self.exclusions
                .iter()
                .filter(|item| !item.trim().is_empty())
                .cloned(),
        );

        patterns
            .iter()
            .map(|pattern| {
                glob::Pattern::new(pattern).map_err(|error| {
                    MigrationError::InvalidInput(format!(
                        "некорректный шаблон исключения '{}': {}",
                        pattern, error
                    ))
                })
            })
            .collect()
    }

    /// Токен отмены (из конфигурации или глобальный токен процесса).
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone().unwrap_or_else(crate::cancel::global)
    }
}

/// Сохраняемое состояние мастера (без секретов).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardState {
    /// Версия формата состояния
    pub version: u32,
    /// Текущий этап
    pub step: WizardStep,
    /// Текущая сцена
    pub stage: WizardStage,
    /// Конфигурация
    pub config: WizardConfig,
    /// Итоговый статус последней операции
    pub status: Option<String>,
    /// Идентификатор последнего отчёта
    pub report_id: Option<String>,
    /// Сохранённые отчёты
    pub report_paths: Vec<PathBuf>,
    /// Дата сохранения (RFC 3339)
    pub saved_at: String,
}

impl WizardState {
    /// Разбор состояния из файла (с проверкой версии).
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let state: Self = serde_json::from_str(&content)?;
        if state.version != STATE_VERSION {
            return Err(MigrationError::InvalidInput(format!(
                "несовместимая версия состояния мастера: найдена {}, ожидалась {}",
                state.version, STATE_VERSION
            )));
        }
        Ok(state)
    }

    /// Записать состояние в файл.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Удалить сохранённое состояние (если есть).
    pub fn clear(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// Инвентарь файлов профиля (результат сканирования мастера).
#[derive(Debug, Clone, Default)]
pub struct ScanInventory {
    /// Элементы для переноса
    pub items: Vec<TransferItem>,
    /// Всего файлов
    pub total_files: u64,
    /// Всего байт
    pub total_size: u64,
    /// Ошибки сканирования (недоступные пути)
    pub errors: Vec<String>,
    /// Файлы, отфильтрованные исключениями (§3)
    pub excluded: Vec<String>,
    /// Приватные SSH-ключи, не включённые в перенос (§4)
    pub private_keys: Vec<String>,
    /// Разбивка по компонентам: компонент, файлов, байт
    pub by_component: Vec<(ComponentType, u64, u64)>,
    /// Разбивка по приложениям (§5)
    pub applications: Vec<AppBreakdown>,
    /// Предупреждения включённых правил приложений (§5)
    pub app_warnings: Vec<String>,
}

/// Разбивка переноса по приложениям (§5).
#[derive(Debug, Clone, Default)]
pub struct AppBreakdown {
    /// Название приложения
    pub name: String,
    /// Количество файлов в переносе
    pub files: u64,
    /// Суммарный размер файлов (байт)
    pub bytes: u64,
    /// Включено ли правило
    pub enabled: bool,
    /// Требуется ли перезапуск приложения после миграции
    pub requires_restart: bool,
}

/// План миграции для предварительного просмотра (§3).
#[derive(Debug, Clone, Default)]
pub struct PreviewPlan {
    /// Режим операции
    pub mode: MigrationMode,
    /// Файлов будет перенесено
    pub total_files: u64,
    /// Байт будет перенесено
    pub total_size: u64,
    /// Свободно на целевом диске
    pub free_bytes: u64,
    /// Хватит ли места (с запасом)
    pub enough_space: bool,
    /// Разбивка по компонентам
    pub by_component: Vec<(ComponentType, u64, u64)>,
    /// Приватные SSH-ключи в переносе (если разрешено)
    pub private_keys: Vec<String>,
    /// Файлы, которые уже есть в целевом HOME
    pub conflicts: Vec<String>,
    /// Предупреждения для этапа просмотра
    pub warnings: Vec<String>,
}

/// Мастер миграции пользователя.
pub struct MigrationWizard {
    config: WizardConfig,
    step: WizardStep,
    stage: WizardStage,
    cancel: CancelToken,
    inventory: Option<ScanInventory>,
    report: Option<MigrationReport>,
    report_paths: Vec<PathBuf>,
    compatibility: Option<CompatibilityReport>,
}

impl MigrationWizard {
    /// Создать мастер по конфигурации.
    pub fn new(config: WizardConfig) -> Self {
        let cancel = config.cancel_token();
        Self {
            config,
            step: WizardStep::Welcome,
            stage: WizardStage::Created,
            cancel,
            inventory: None,
            report: None,
            report_paths: Vec::new(),
            compatibility: None,
        }
    }

    /// Создать мастер из сохранённого состояния.
    pub fn restore_state(state: WizardState) -> Self {
        let mut wizard = Self::new(state.config);
        wizard.step = state.step;
        wizard.stage = state.stage;
        wizard
    }

    /// Конфигурация.
    pub fn config(&self) -> &WizardConfig {
        &self.config
    }

    /// Изменить конфигурацию (результат сканирования сбрасывается).
    pub fn config_mut(&mut self) -> &mut WizardConfig {
        self.inventory = None;
        self.stage = WizardStage::Created;
        &mut self.config
    }

    /// Текущая сцена.
    pub fn stage(&self) -> WizardStage {
        self.stage
    }

    /// Текущий этап.
    pub fn step(&self) -> WizardStep {
        self.step
    }

    /// Перейти к этапу.
    pub fn set_step(&mut self, step: WizardStep) {
        self.step = step;
    }

    /// Этап «Далее» (на последнем этапе остаётся на месте).
    pub fn next_step(&mut self) -> WizardStep {
        if let Some(next) = self.step.next() {
            self.step = next;
        }
        self.step
    }

    /// Этап «Назад» (на первом этапе остаётся на месте).
    pub fn previous_step(&mut self) -> WizardStep {
        if let Some(previous) = self.step.previous() {
            self.step = previous;
        }
        self.step
    }

    /// Токен отмены операций мастера.
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Ручка отмены для интерфейса (кнопка «Отмена»).
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.handle()
    }

    /// Отменить текущую/следующую операцию (шаг «Отмена»).
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Результат сканирования (после `scan`/`run`).
    pub fn inventory(&self) -> Option<&ScanInventory> {
        self.inventory.as_ref()
    }

    /// Последний отчёт (после `run`).
    pub fn report(&self) -> Option<&MigrationReport> {
        self.report.as_ref()
    }

    /// Пути сохранённых отчётов.
    pub fn report_paths(&self) -> &[PathBuf] {
        &self.report_paths
    }

    /// Результат проверки совместимости (после `check_compatibility`/`run`).
    pub fn compatibility(&self) -> Option<&CompatibilityReport> {
        self.compatibility.as_ref()
    }

    /// Снимок состояния для сохранения.
    pub fn state(&self) -> WizardState {
        WizardState {
            version: STATE_VERSION,
            step: self.step,
            stage: self.stage,
            config: self.config.clone(),
            status: self
                .report
                .as_ref()
                .map(|report| report.status().to_string()),
            report_id: self.report.as_ref().map(|report| report.id.clone()),
            report_paths: self.report_paths.clone(),
            saved_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Сохранить состояние мастера в указанный файл.
    pub fn save_state(&self, path: &Path) -> Result<()> {
        self.state().save(path)
    }

    /// Разом: файл состояния по умолчанию (в каталоге конфигурации).
    pub fn default_state_path() -> PathBuf {
        config::get_config_dir().join("wizard-state.json")
    }

    /// Сохранить состояние в каталог конфигурации.
    pub fn save_default_state(&self) -> Result<PathBuf> {
        let path = Self::default_state_path();
        self.save_state(&path)?;
        Ok(path)
    }

    /// Продолжить работу из состояния в каталоге конфигурации.
    pub fn load_default_state() -> Result<Self> {
        Ok(Self::restore_state(WizardState::load(
            &Self::default_state_path(),
        )?))
    }

    /// Каталог отчётов (из конфигурации или каталог данных приложения).
    pub fn report_directory(&self) -> PathBuf {
        self.config
            .report_dir
            .clone()
            .unwrap_or_else(|| config::get_data_dir().join("reports"))
    }

    /// Каталог резервных копий (из конфигурации или каталог данных приложения).
    pub fn backup_directory(&self) -> PathBuf {
        self.config
            .backup_dir
            .clone()
            .unwrap_or_else(BackupManager::default_root)
    }

    /// Просканировать компоненты профиля (§3) с учётом исключений и SSH-политики.
    pub fn scan(&mut self) -> Result<&ScanInventory> {
        self.cancel.check()?;
        let mut exclusions = self.config.effective_exclusions()?;
        // §5: выключенные правила приложений не переносятся — их пути исключаются.
        for rule in self.config.app_rules.iter().filter(|rule| !rule.enabled) {
            for path in rule.relative_paths() {
                let path = path.trim_matches('/');
                if path.is_empty() {
                    continue;
                }
                if let Ok(pattern) = glob::Pattern::new(&format!("**/{}", path)) {
                    exclusions.push(pattern);
                }
                if let Ok(pattern) = glob::Pattern::new(&format!("**/{}/**", path)) {
                    exclusions.push(pattern);
                }
            }
        }
        let mut inventory = ScanInventory::default();

        for component in self.config.components.clone() {
            let start = inventory.items.len();
            for root in component.default_paths(&self.config.source_home) {
                if !root.exists() {
                    continue;
                }
                if let Err(error) =
                    collect_items(&root, &self.config.source_home, &exclusions, &mut inventory)
                {
                    inventory
                        .errors
                        .push(format!("{}: {}", root.display(), error));
                }
            }

            let files = (inventory.items.len() - start) as u64;
            let bytes = inventory.items[start..].iter().map(|item| item.size).sum();
            if files > 0 {
                inventory.by_component.push((component, files, bytes));
            }
        }

        let (items, private_keys, skipped) = filter_ssh_items(
            inventory.items,
            self.config.include_private_ssh_keys,
            self.config.include_authorized_keys,
        );
        inventory.items = items;
        inventory.private_keys = private_keys;
        inventory.excluded.extend(skipped);

        inventory.items.sort_by(|a, b| a.source.cmp(&b.source));
        inventory.items.dedup_by(|a, b| a.source == b.source);
        inventory.total_files = inventory.items.len() as u64;
        inventory.total_size = inventory.items.iter().map(|item| item.size).sum();

        // §5: разбивка по приложениям и предупреждения их правил.
        let (applications, app_warnings) = app_breakdown(&inventory.items, &self.config.app_rules);
        inventory.applications = applications;
        inventory.app_warnings = app_warnings;

        self.stage = WizardStage::Scanned;
        self.inventory = Some(inventory);
        Ok(self.inventory.as_ref().expect("inventory set"))
    }

    /// Обязательный путь к архиву.
    fn require_archive(&self) -> Result<&Path> {
        self.config
            .archive_path
            .as_deref()
            .ok_or_else(|| MigrationError::InvalidInput("не задан путь к архиву".to_string()))
    }

    /// Каталог, в который пишутся данные (для проверки места и резервной копии).
    fn probe_target(&self) -> PathBuf {
        match self.config.mode {
            MigrationMode::LocalArchive => self
                .config
                .archive_path
                .as_ref()
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| self.config.source_home.clone()),
            MigrationMode::Restore | MigrationMode::SshDirect => {
                if self.config.target_home.as_os_str().is_empty() {
                    self.config.source_home.clone()
                } else {
                    self.config.target_home.clone()
                }
            }
        }
    }

    /// Метаданные источника из манифеста архива (§8).
    pub fn source_metadata(&self) -> Result<SourceMetadata> {
        let archive = self.require_archive()?;
        let manifest = ArchiveManager::read_manifest(archive, self.config.passphrase.as_deref())?;
        Ok(SourceMetadata {
            os_info: Some(manifest.os_info),
            user: Some(manifest.source_user),
        })
    }

    /// Проверить совместимость источника с текущей системой (§8).
    pub fn check_compatibility(&mut self) -> Result<Option<CompatibilityReport>> {
        let source = match self.config.mode {
            MigrationMode::Restore => self.source_metadata()?,
            _ => SourceMetadata::default(),
        };

        let report = compatibility::check(&source)?;
        self.compatibility = Some(report.clone());
        Ok(Some(report))
    }

    /// Использовать сохранённую проверку совместимости или выполнить её.
    fn ensure_compatibility(&mut self) -> Option<CompatibilityReport> {
        if self.compatibility.is_some() {
            return self.compatibility.clone();
        }
        if self.config.mode != MigrationMode::Restore {
            return None;
        }
        self.check_compatibility().ok().flatten()
    }

    /// Файлы архива, которые уже есть в целевом HOME (§3).
    pub fn target_conflicts(&self) -> Result<Vec<String>> {
        if self.config.mode != MigrationMode::Restore {
            return Ok(Vec::new());
        }

        let archive = self.require_archive()?;
        let manifest = ArchiveManager::read_manifest(archive, self.config.passphrase.as_deref())?;
        Ok(manifest
            .files
            .iter()
            .map(|file| file.relative_path.clone())
            .filter(|relative| self.config.target_home.join(relative).exists())
            .collect())
    }

    /// План операции для предварительного просмотра (§3).
    pub fn preview(&mut self) -> Result<PreviewPlan> {
        self.cancel.check()?;
        let inventory = self.scan()?.clone();
        let target = self.probe_target();
        let free_bytes = platform::free_space(&target);
        let required = inventory.total_size.saturating_mul(11) / 10;
        let enough_space = free_bytes == 0 || free_bytes >= required;

        let mut warnings = inventory.errors.clone();
        if !enough_space {
            warnings.push(format!(
                "может не хватить места: нужно около {} байт, доступно {}",
                required, free_bytes
            ));
        }
        if !inventory.excluded.is_empty() {
            warnings.push(format!(
                "исключено по правилам: {} файлов",
                inventory.excluded.len()
            ));
        }
        if self.config.include_private_ssh_keys && !inventory.private_keys.is_empty() {
            warnings.push(format!(
                "разрешён перенос приватных SSH-ключей: {}",
                inventory.private_keys.join(", ")
            ));
        }
        // §5: предупреждения правил приложений.
        warnings.extend(inventory.app_warnings.clone());

        let conflicts = self.target_conflicts().unwrap_or_default();
        if !conflicts.is_empty() {
            warnings.push(format!("конфликтующих файлов в цели: {}", conflicts.len()));
        }

        self.step = WizardStep::Preview;
        Ok(PreviewPlan {
            mode: self.config.mode,
            total_files: inventory.total_files,
            total_size: inventory.total_size,
            free_bytes,
            enough_space,
            by_component: inventory.by_component.clone(),
            private_keys: inventory.private_keys.clone(),
            conflicts,
            warnings,
        })
    }

    /// Создать резервную копию файлов цели, которые могут быть перезаписаны (§9).
    pub fn create_backup(&mut self, observer: &dyn ProgressObserver) -> Result<Option<PathBuf>> {
        if !matches!(
            self.config.mode,
            MigrationMode::Restore | MigrationMode::SshDirect
        ) {
            return Ok(None);
        }
        if self.config.dry_run || !self.config.auto_backup {
            observer.on_message("резервное копирование пропущено");
            return Ok(None);
        }

        let target = self.probe_target();
        let mut existing: Vec<PathBuf> = Vec::new();
        for component in self.config.components.clone() {
            for root in component.default_paths(&target) {
                if root.is_dir() {
                    existing.push(root);
                }
            }
        }

        if existing.is_empty() {
            observer
                .on_message("резервная копия не нужна: в цели нет файлов выбранных компонентов");
            return Ok(None);
        }

        self.cancel.check()?;
        let manager = BackupManager::new(self.backup_directory());
        let entry = manager.create_backup(
            &target,
            &existing,
            &format!(
                "мастер миграции: перед {} ({})",
                mode_label(self.config.mode),
                target.display()
            ),
        )?;
        observer.on_message(&format!("резервная копия: {}", entry.path.display()));
        Ok(Some(entry.path))
    }

    /// Выполнить post-migration hooks включённых правил (§5).
    ///
    /// Требует явного подтверждения (`confirmed`), каждый hook запускается без
    /// shell-интерполяции и фиксируется в журнале операций.
    pub fn run_app_hooks(&self, confirmed: bool) -> Result<Vec<(String, String)>> {
        if !confirmed {
            return Err(MigrationError::PrivilegedRequired(
                "выполнение post-migration hooks требует подтверждения".to_string(),
            ));
        }

        let mut results = Vec::new();
        for rule in &self.config.app_rules {
            if !rule.enabled || rule.post_migration_hook.is_none() {
                continue;
            }

            self.cancel.check()?;
            let output = crate::applications::run_hook(rule, true)?;
            crate::log_info!(
                "app-hook",
                "hook приложения '{}': {}",
                rule.app_name,
                output.trim()
            );
            results.push((rule.app_name.clone(), output));
        }

        Ok(results)
    }

    /// Сохранить отчёт в настроенных форматах (§17).
    pub fn save_report(&self, report: &MigrationReport) -> (Vec<PathBuf>, Vec<String>) {
        let directory = self.report_directory();
        let formats = if self.config.report_formats.is_empty() {
            vec![ReportFormat::Json]
        } else {
            self.config.report_formats.clone()
        };

        let mut paths = Vec::new();
        let mut warnings = Vec::new();
        for format in formats {
            match report.save(&directory, format) {
                Ok(path) => paths.push(path),
                Err(error) => {
                    warnings.push(format!("отчёт '{}' не сохранён: {}", format.key(), error))
                }
            }
        }

        (paths, warnings)
    }

    /// Выполнить миграцию: резервная копия, выполнение, проверки, отчёт (§2).
    pub fn run(&mut self, observer: &dyn ProgressObserver) -> Result<MigrationReport> {
        let started = Instant::now();
        self.step = WizardStep::Progress;
        self.stage = WizardStage::Running;

        let id = uuid::Uuid::new_v4().to_string();
        let mut report = MigrationReport::new(
            id,
            self.config.mode,
            self.config.source_home.to_string_lossy().to_string(),
            self.config.target_home.to_string_lossy().to_string(),
        );
        report.components = self.config.components.clone();

        let history = self.begin_history(&report);

        match self.create_backup(observer) {
            Ok(Some(path)) => report.backup_path = Some(path.display().to_string()),
            Ok(None) => {}
            Err(error) => report
                .warnings
                .push(format!("резервная копия не создана: {}", error)),
        }

        let result = if self.cancel.is_cancelled() {
            Err(MigrationError::Cancelled)
        } else {
            self.execute(observer, &mut report)
        };

        if result.is_ok() && !self.config.dry_run {
            self.post_migration_checks(&mut report);
        }

        if let Some(compatibility) = self.ensure_compatibility() {
            report.compatibility = Some(compatibility.summary());
            report.warnings.extend(compatibility.warnings);
            report.warnings.extend(
                compatibility
                    .blockers
                    .into_iter()
                    .map(|blocker| format!("блокирующая несовместимость: {}", blocker)),
            );
        }

        report.stats.duration_ms = started.elapsed().as_millis();

        match result {
            Ok(()) if self.cancel.is_cancelled() => {
                report.cancelled = true;
                self.stage = WizardStage::Cancelled;
            }
            Ok(()) => {
                self.stage = WizardStage::Completed;
                self.step = WizardStep::Report;
            }
            Err(error) if error.is_cancelled() => {
                report.cancelled = true;
                self.stage = WizardStage::Cancelled;
                observer.on_message("операция отменена пользователем");
            }
            Err(error) => {
                self.stage = WizardStage::Failed;
                report.errors.push(error.to_string());
            }
        }

        let (paths, warnings) = self.save_report(&report);
        report.warnings.extend(warnings);
        self.report_paths = paths;
        self.report = Some(report.clone());
        self.finish_history(history, &report);

        Ok(report)
    }

    fn execute(
        &mut self,
        observer: &dyn ProgressObserver,
        report: &mut MigrationReport,
    ) -> Result<()> {
        match self.config.mode {
            MigrationMode::LocalArchive => self.run_create_archive(observer, report),
            MigrationMode::Restore => self.run_restore(observer, report),
            MigrationMode::SshDirect => self.run_ssh_direct(observer, report),
        }
    }

    /// Этап 9: создание архива из профиля.
    fn run_create_archive(
        &mut self,
        observer: &dyn ProgressObserver,
        report: &mut MigrationReport,
    ) -> Result<()> {
        let archive = self.require_archive()?.to_path_buf();
        let inventory = self.scan()?.clone();
        apply_scan_warnings(&inventory, report);

        let options = CreateArchiveOptions {
            output: archive,
            source_home: self.config.source_home.clone(),
            items: inventory.items.clone(),
            components: self.config.components.clone(),
            passphrase: self.config.passphrase.clone(),
            compression_level: self.config.compression_level,
            dry_run: self.config.dry_run,
            cancel: Some(self.cancel.clone()),
        };

        let created = ArchiveManager::create(&options, observer)?;
        report.stats.files_copied = created.manifest.total_files as u64;
        report.stats.bytes_copied = created.manifest.total_size;
        record_items(report, &inventory.items);
        Ok(())
    }

    /// Этап 9: восстановление из архива.
    fn run_restore(
        &mut self,
        observer: &dyn ProgressObserver,
        report: &mut MigrationReport,
    ) -> Result<()> {
        if self.config.target_home.as_os_str().is_empty() {
            return Err(MigrationError::InvalidInput(
                "не задан целевой домашний каталог".to_string(),
            ));
        }
        let archive = self.require_archive()?.to_path_buf();

        let options = RestoreOptions {
            archive,
            passphrase: self.config.passphrase.clone(),
            target_root: self.config.target_home.clone(),
            components: Some(self.config.components.clone()),
            strategy: self.config.conflict_strategy,
            verify_hash: self.config.verify,
            dry_run: self.config.dry_run,
            cancel: Some(self.cancel.clone()),
        };

        let result = ArchiveManager::restore(&options, observer)?;
        report.stats.files_copied = result.restored_files;
        report.stats.bytes_copied = result.restored_bytes;
        report.stats.files_skipped = result.skipped_files;
        report.stats.conflicts_resolved = result.conflicts.len() as u64;

        for conflict in &result.conflicts {
            let description = conflict.describe();
            report.conflicts.push(description.clone());
            report.warnings.push(description);
        }
        report.errors.extend(result.errors);
        Ok(())
    }

    /// Этап 9: прямая передача по SSH (rsync/scp каталогов компонентов).
    fn run_ssh_direct(
        &mut self,
        observer: &dyn ProgressObserver,
        report: &mut MigrationReport,
    ) -> Result<()> {
        if self.config.target_home.as_os_str().is_empty() {
            return Err(MigrationError::InvalidInput(
                "не задан целевой домашний каталог".to_string(),
            ));
        }
        let target =
            self.config.ssh.clone().ok_or_else(|| {
                MigrationError::InvalidInput("не задано SSH-подключение".to_string())
            })?;

        let inventory = self.scan()?.clone();
        apply_scan_warnings(&inventory, report);
        if self.config.include_private_ssh_keys && !inventory.private_keys.is_empty() {
            report.warnings.push(format!(
                "передаются приватные SSH-ключи: {}",
                inventory.private_keys.join(", ")
            ));
        }

        let transfer = SshTransfer::new(target);
        let options = SshTransferOptions {
            dry_run: self.config.dry_run,
            compress: true,
            rate_limit_kbps: self.config.rate_limit / 1024,
            delete_extra: false,
        };

        if !self.config.dry_run {
            transfer.check_connection()?;
        }

        for component in self.config.components.clone() {
            for root in component.default_paths(&self.config.source_home) {
                self.cancel.check()?;
                if !root.exists() {
                    continue;
                }

                // Каталог .ssh передаём пофайлово, чтобы не унести приватные ключи (§4).
                if root.is_dir() && is_ssh_dir(&root) && !self.config.include_private_ssh_keys {
                    for item in inventory
                        .items
                        .iter()
                        .filter(|item| item.relative.starts_with(".ssh"))
                    {
                        self.cancel.check()?;
                        let remote = format!(
                            "{}/{}",
                            self.config.target_home.display(),
                            item.relative.to_string_lossy()
                        );
                        observer.on_message(&format!("передача {}", item.source.display()));
                        let result = transfer.push(&item.source, &remote, &options)?;
                        report.stats.files_copied += result.files;
                        report.stats.bytes_copied += result.bytes;
                    }
                    continue;
                }

                let relative = crate::security::make_relative(&self.config.source_home, &root)?;
                let remote = format!(
                    "{}/{}",
                    self.config.target_home.display(),
                    relative.display()
                );
                observer.on_message(&format!("передача {}", root.display()));

                let result = transfer.push(&root, &remote, &options)?;
                report.stats.files_copied += result.files;
                report.stats.bytes_copied += result.bytes;
            }
        }

        Ok(())
    }

    /// Этап 10: проверка результата и рекомендации (§4, §11, §13).
    fn post_migration_checks(&mut self, report: &mut MigrationReport) {
        if self.config.mode != MigrationMode::Restore {
            return;
        }
        let target = self.config.target_home.clone();

        if self.config.components.contains(&ComponentType::SshKeys) {
            if let Err(error) = SshKeysScanner::fix_permissions(&target) {
                report
                    .warnings
                    .push(format!("не удалось настроить права SSH: {}", error));
            }
            match SshKeysScanner::new(&target).scan() {
                Ok(keys) => {
                    let unpaired: Vec<String> = keys
                        .iter()
                        .filter(|key| {
                            key.kind == crate::ssh_keys::SshKeyKind::Private && !key.paired
                        })
                        .map(|key| key.relative_path.clone())
                        .collect();
                    if !unpaired.is_empty() {
                        report.add_recommendation(format!(
                            "проверьте пары SSH-ключей (нет публичного): {}",
                            unpaired.join(", ")
                        ));
                    }
                }
                Err(error) => report
                    .warnings
                    .push(format!("не удалось проверить SSH-ключи: {}", error)),
            }
        }

        if self.config.components.contains(&ComponentType::Printers) {
            let printers = PrinterManager::list().unwrap_or_default();
            report.stats.printers = printers.len() as u64;
            report.restored_printers = printers
                .iter()
                .map(|printer| printer.name.clone())
                .collect();
            if printers.is_empty() {
                report.add_recommendation(
                    "принтеры не обнаружены: проверьте printers.conf и lpadmin (§13)".to_string(),
                );
            }
        }

        if self.config.components.contains(&ComponentType::Packages) {
            report.add_recommendation(format!(
                "список пакетов источника восстанавливайте через {} (§11)",
                PackageManager::detect().list_tool()
            ));
        }
    }

    /// Начать запись истории операции в SQLite (§16).
    fn begin_history(&self, report: &MigrationReport) -> Option<MigrationRecord> {
        let path = self.config.database_path.clone()?;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut record = MigrationRecord::new(
            operation_type(self.config.mode),
            report.source.clone(),
            report.target.clone(),
        );
        // Один идентификатор у отчёта и записи истории (§16).
        record.id = report.id.clone();
        record.components =
            serde_json::to_string(&report.components).unwrap_or_else(|_| "[]".to_string());
        record.mark_started();

        if let Ok(mut database) = DatabaseManager::new(&path) {
            let _ = database.save_migration(&record);
        }
        Some(record)
    }

    /// Завершить запись истории операции (§16).
    fn finish_history(&self, record: Option<MigrationRecord>, report: &MigrationReport) {
        let Some(mut record) = record else { return };
        let Some(path) = self.config.database_path.clone() else {
            return;
        };

        record.file_count = self
            .inventory
            .as_ref()
            .map(|inventory| inventory.total_files)
            .unwrap_or(report.stats.files_copied);
        record.total_size = self
            .inventory
            .as_ref()
            .map(|inventory| inventory.total_size)
            .unwrap_or(report.stats.bytes_copied);
        record.transferred_files = report.stats.files_copied;
        record.transferred_size = report.stats.bytes_copied;
        record.error_count = report.errors.len() as u32;
        record.warning_count = report.warnings.len() as u32;
        record.report_path = self
            .report_paths
            .first()
            .map(|path| path.display().to_string());
        record.backup_path = report.backup_path.clone();

        if report.cancelled {
            record.mark_cancelled();
        } else if report.is_success() {
            record.mark_completed();
        } else if record.transferred_files > 0 {
            record.mark_partially_completed();
        } else {
            record.mark_failed(report.errors.join("; "));
        }

        if let Ok(mut database) = DatabaseManager::new(&path) {
            let _ = database.save_migration(&record);
        }
    }
}

/// Собрать файлы каталога в инвентарь (относительно домашнего каталога).
fn collect_items(
    root: &Path,
    home: &Path,
    exclusions: &[glob::Pattern],
    inventory: &mut ScanInventory,
) -> Result<()> {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }

        let path = entry.path();
        let relative = crate::security::make_relative(home, path)?;
        let key = relative.to_string_lossy().replace('\\', "/");
        if is_excluded(&key, exclusions) {
            inventory.excluded.push(key);
            continue;
        }

        let metadata = entry.metadata()?;
        let is_symlink = entry.file_type().is_symlink();

        inventory.items.push(TransferItem {
            source: path.to_path_buf(),
            relative,
            size: metadata.len(),
            is_symlink,
            symlink_target: if is_symlink {
                std::fs::read_link(path).ok()
            } else {
                None
            },
            mode: crate::platform::file_mode(path),
        });
    }

    Ok(())
}

/// Флаг исключения для относительного пути (§3).
fn is_excluded(relative: &str, patterns: &[glob::Pattern]) -> bool {
    let with_root = format!("/{relative}");
    patterns
        .iter()
        .any(|pattern| pattern.matches(relative) || pattern.matches(&with_root))
}

/// Каталог `.ssh` домашнего каталога.
fn is_ssh_dir(path: &Path) -> bool {
    path.file_name().map(|name| name == ".ssh").unwrap_or(false)
}

/// Файл `.ssh/<имя>` — потенциальный приватный ключ (§4).
fn is_private_key(relative: &Path) -> bool {
    let mut parts = relative.iter();
    if parts.next().and_then(|part| part.to_str()) != Some(".ssh") {
        return false;
    }
    // Только файлы непосредственно в `.ssh`.
    if parts.next().is_none() || parts.next().is_some() {
        return false;
    }

    let name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_lowercase();

    if name.ends_with(".pub")
        || name.starts_with("known_hosts")
        || name == "config"
        || name == "authorized_keys"
        || name.contains('.')
    {
        return false;
    }

    // Всё остальное без расширения считаем потенциальным приватным ключом:
    // id_ed25519, id_rsa, deploy_key, google_compute_engine, …
    true
}

/// Файл `.ssh/authorized_keys` (§4 — переносится только по выбору).
fn is_authorized_keys(relative: &Path) -> bool {
    let in_ssh = relative.iter().next().and_then(|part| part.to_str()) == Some(".ssh");
    let name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("authorized_keys"))
        .unwrap_or(false);
    in_ssh && name
}

/// Применить SSH-политику (§4): элементы, приватные ключи и пропуски.
fn filter_ssh_items(
    items: Vec<TransferItem>,
    include_private: bool,
    include_authorized: bool,
) -> (Vec<TransferItem>, Vec<String>, Vec<String>) {
    let private_keys: Vec<String> = items
        .iter()
        .filter(|item| is_private_key(&item.relative))
        .map(|item| item.relative.to_string_lossy().replace('\\', "/"))
        .collect();

    let mut skipped = Vec::new();
    let kept: Vec<TransferItem> = items
        .into_iter()
        .filter(|item| {
            if !include_private && is_private_key(&item.relative) {
                return false;
            }
            if !include_authorized && is_authorized_keys(&item.relative) {
                skipped.push(format!(
                    "{}: authorized_keys переносится только по выбору пользователя",
                    item.relative.to_string_lossy().replace('\\', "/")
                ));
                return false;
            }
            true
        })
        .collect();

    (kept, private_keys, skipped)
}

/// Добавить в отчёт результаты сканирования (§3, §4).
fn apply_scan_warnings(inventory: &ScanInventory, report: &mut MigrationReport) {
    report.warnings.extend(inventory.errors.iter().cloned());
    if !inventory.excluded.is_empty() {
        report
            .warnings
            .push(format!("исключено файлов: {}", inventory.excluded.len()));
        report.skipped_files.extend(
            inventory
                .excluded
                .iter()
                .map(|path| format!("{}: исключение", path)),
        );
    }
    if !inventory.private_keys.is_empty() {
        report.skipped_files.extend(
            inventory
                .private_keys
                .iter()
                .map(|key| format!("{}: приватный SSH-ключ", key)),
        );
        report.add_recommendation(
            "приватные SSH-ключи перенесите вручную или включите явное согласие (§4)".to_string(),
        );
    }
    // §5: предупреждения правил приложений и рекомендация о перезапуске.
    for warning in &inventory.app_warnings {
        report.warnings.push(warning.clone());
    }
    if inventory
        .applications
        .iter()
        .any(|app| app.enabled && app.requires_restart && app.files > 0)
    {
        report.add_recommendation(
            "перезапустите перенесённые приложения, чтобы они подхватили настройки (§5)"
                .to_string(),
        );
    }
}

/// Разбивка переноса по приложениям (§5) и предупреждения включённых правил.
fn app_breakdown(items: &[TransferItem], rules: &[AppRule]) -> (Vec<AppBreakdown>, Vec<String>) {
    let mut breakdown = Vec::with_capacity(rules.len());
    let mut warnings = Vec::new();

    for rule in rules {
        if !rule.enabled {
            breakdown.push(AppBreakdown {
                name: rule.app_name.clone(),
                files: 0,
                bytes: 0,
                enabled: false,
                requires_restart: rule.requires_restart,
            });
            continue;
        }

        let mut files = 0u64;
        let mut bytes = 0u64;
        for item in items {
            let relative = item.relative.to_string_lossy().replace('\\', "/");
            if rule.matches_relative(&relative) {
                files += 1;
                bytes += item.size;
            }
        }

        if files > 0 {
            for warning in &rule.warnings {
                warnings.push(format!("{}: {}", rule.app_name, warning));
            }
        }

        breakdown.push(AppBreakdown {
            name: rule.app_name.clone(),
            files,
            bytes,
            enabled: true,
            requires_restart: rule.requires_restart,
        });
    }

    (breakdown, warnings)
}

/// Записать перенесённые файлы в отчёт (с ограничением длины).
fn record_items(report: &mut MigrationReport, items: &[TransferItem]) {
    const LIMIT: usize = 500;

    for item in items.iter().take(LIMIT) {
        report.record_transferred(item.relative.to_string_lossy().to_string());
    }
    if items.len() > LIMIT {
        report.add_recommendation(format!(
            "полный список из {} файлов — в манифесте архива",
            items.len()
        ));
    }
}

/// Название режима для сообщений и резервных копий.
fn mode_label(mode: MigrationMode) -> &'static str {
    match mode {
        MigrationMode::LocalArchive => "созданием архива",
        MigrationMode::Restore => "восстановлением",
        MigrationMode::SshDirect => "передачей по SSH",
    }
}

/// Тип операции для истории SQLite (§16).
fn operation_type(mode: MigrationMode) -> OperationType {
    match mode {
        MigrationMode::LocalArchive => OperationType::CreateArchive,
        MigrationMode::Restore => OperationType::Restore,
        MigrationMode::SshDirect => OperationType::SshMigration,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::CancelToken;
    use crate::database::MigrationStatus;
    use crate::file_transfer::NoProgress;
    use tempfile::tempdir;

    fn make_home(dir: &Path) -> PathBuf {
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("Documents")).expect("mkdir");
        std::fs::create_dir_all(home.join(".ssh")).expect("mkdir");
        std::fs::write(home.join("Documents/doc.txt"), b"doc").expect("write");
        std::fs::write(home.join(".ssh/config"), b"Host *\n").expect("write");
        home
    }

    /// Дом с «мусором» (§3) и SSH-ключами (§4).
    fn make_noisy_home(dir: &Path) -> PathBuf {
        let home = make_home(dir);
        std::fs::create_dir_all(home.join("Documents/.cache")).expect("mkdir");
        std::fs::write(home.join("Documents/.cache/blob"), b"cache").expect("write");
        std::fs::write(home.join("Documents/notes.tmp"), b"temp").expect("write");
        std::fs::write(home.join(".ssh/id_ed25519"), b"private").expect("write");
        std::fs::write(home.join(".ssh/id_ed25519.pub"), b"public").expect("write");
        std::fs::write(home.join(".ssh/authorized_keys"), b"ssh-ed25519 aaa").expect("write");
        home
    }

    fn base_config(dir: &Path, mode: MigrationMode, home: &Path) -> WizardConfig {
        let mut config = WizardConfig::new(mode, home);
        config.components = vec![ComponentType::Documents, ComponentType::SshKeys];
        config.report_dir = Some(dir.join("reports"));
        config.backup_dir = Some(dir.join("backups"));
        config.cancel = Some(CancelToken::new());
        config
    }

    #[test]
    fn test_scan_builds_inventory() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::LocalArchive, &home);
        config.components = vec![ComponentType::Documents, ComponentType::SshKeys];

        let mut wizard = MigrationWizard::new(config);
        let inventory = wizard.scan().expect("scan").clone();

        assert_eq!(wizard.stage(), WizardStage::Scanned);
        assert_eq!(inventory.total_files, 2);
        assert!(inventory
            .items
            .iter()
            .any(|item| item.relative == Path::new("Documents/doc.txt")));
        assert!(inventory
            .items
            .iter()
            .any(|item| item.relative == Path::new(".ssh/config")));
    }

    #[test]
    fn test_app_breakdown_reports_sizes() {
        let dir = tempdir().expect("tempdir");
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".config/chromium/Default")).expect("mkdir");
        std::fs::write(home.join(".config/chromium/Default/Preferences"), b"{}").expect("write");

        let mut config = base_config(dir.path(), MigrationMode::LocalArchive, &home);
        config.components = vec![ComponentType::AppConfigs];

        let mut wizard = MigrationWizard::new(config);
        let inventory = wizard.scan().expect("scan").clone();

        let chromium = inventory
            .applications
            .iter()
            .find(|app| app.name.contains("Chromium"))
            .expect("правило Chromium");
        assert!(chromium.enabled);
        assert_eq!(chromium.files, 1);
        assert!(chromium.bytes > 0);
    }

    #[test]
    fn test_disabled_app_rule_excludes_files() {
        let dir = tempdir().expect("tempdir");
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".config/libreoffice")).expect("mkdir");
        std::fs::write(
            home.join(".config/libreoffice/registrymodifications.xcu"),
            b"data",
        )
        .expect("write");

        let mut config = base_config(dir.path(), MigrationMode::LocalArchive, &home);
        config.components = vec![ComponentType::AppConfigs];
        for rule in config.app_rules.iter_mut() {
            if rule.app_name == "LibreOffice" {
                rule.enabled = false;
            }
        }

        let mut wizard = MigrationWizard::new(config);
        let inventory = wizard.scan().expect("scan").clone();

        assert!(
            !inventory
                .items
                .iter()
                .any(|item| item.relative.to_string_lossy().contains("libreoffice")),
            "файлы выключенного правила не должны попадать в перенос"
        );
        let libre = inventory
            .applications
            .iter()
            .find(|app| app.name == "LibreOffice")
            .expect("правило LibreOffice");
        assert!(!libre.enabled);
        assert_eq!(libre.files, 0);
    }

    #[test]
    fn test_app_hooks_require_confirmation() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let config = base_config(dir.path(), MigrationMode::LocalArchive, &home);

        let wizard = MigrationWizard::new(config);
        assert!(wizard.run_app_hooks(false).is_err());
        assert!(wizard.run_app_hooks(true).expect("hooks").is_empty());
    }

    #[test]
    fn test_scan_skips_exclusions_and_private_keys() {
        let dir = tempdir().expect("tempdir");
        let home = make_noisy_home(dir.path());
        let config = base_config(dir.path(), MigrationMode::LocalArchive, &home);

        let mut wizard = MigrationWizard::new(config);
        let inventory = wizard.scan().expect("scan").clone();

        let relative: Vec<String> = inventory
            .items
            .iter()
            .map(|item| item.relative.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(relative.contains(&"Documents/doc.txt".to_string()));
        assert!(relative.contains(&".ssh/id_ed25519.pub".to_string()));
        // §3: кэш и временные файлы не переносятся автоматически.
        assert!(!relative.iter().any(|path| path.contains(".cache")));
        assert!(!relative.iter().any(|path| path.ends_with(".tmp")));
        assert!(inventory.excluded.len() >= 2);
        // §4: приватный ключ и authorized_keys не попадают в перенос.
        assert!(inventory
            .private_keys
            .contains(&".ssh/id_ed25519".to_string()));
        assert!(!relative.contains(&".ssh/id_ed25519".to_string()));
        assert!(!relative.contains(&".ssh/authorized_keys".to_string()));
    }

    #[test]
    fn test_private_keys_with_explicit_consent() {
        let dir = tempdir().expect("tempdir");
        let home = make_noisy_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::LocalArchive, &home);
        config.include_private_ssh_keys = true;
        config.include_authorized_keys = true;

        let mut wizard = MigrationWizard::new(config);
        let inventory = wizard.scan().expect("scan").clone();
        let relative: Vec<String> = inventory
            .items
            .iter()
            .map(|item| item.relative.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(relative.contains(&".ssh/id_ed25519".to_string()));
        assert!(relative.contains(&".ssh/authorized_keys".to_string()));
        // Список приватных ключей формируется для подтверждения (§4).
        assert!(inventory
            .private_keys
            .contains(&".ssh/id_ed25519".to_string()));
    }

    #[test]
    fn test_preview_reports_plan() {
        let dir = tempdir().expect("tempdir");
        let home = make_noisy_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::LocalArchive, &home);
        config.archive_path = Some(dir.path().join("out.rmm"));

        let mut wizard = MigrationWizard::new(config);
        let plan = wizard.preview().expect("preview");

        assert_eq!(plan.mode, MigrationMode::LocalArchive);
        assert_eq!(plan.total_files, 3);
        assert!(plan.total_size > 0);
        assert!(!plan.by_component.is_empty());
        assert!(wizard.step() == WizardStep::Preview);
    }

    #[test]
    fn test_run_creates_archive_reports_and_history() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::LocalArchive, &home);
        config.archive_path = Some(dir.path().join("profile.rmm"));
        config.database_path = Some(dir.path().join("history.db"));
        config.report_formats = vec![ReportFormat::Json, ReportFormat::Html, ReportFormat::Text];

        let mut wizard = MigrationWizard::new(config);
        let report = wizard.run(&NoProgress).expect("run");

        assert!(report.is_success(), "{:?}", report.errors);
        assert!(!report.cancelled);
        assert_eq!(wizard.stage(), WizardStage::Completed);
        assert_eq!(report.stats.files_copied, 2);
        assert_eq!(wizard.report_paths().len(), 3);
        assert!(wizard
            .report_paths()
            .iter()
            .all(|path| path.exists() && path.parent().unwrap().ends_with("reports")));

        let database = DatabaseManager::new(dir.path().join("history.db")).expect("db");
        let stored = database
            .get_migration(&report.id)
            .expect("query")
            .expect("record");
        assert!(matches!(stored.status, MigrationStatus::Completed));
        assert_eq!(stored.transferred_files, 2);
        assert!(stored.report_path.is_some());
    }

    #[test]
    fn test_dry_run_creates_no_archive() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::LocalArchive, &home);
        config.archive_path = Some(dir.path().join("none.rmm"));
        config.dry_run = true;

        let mut wizard = MigrationWizard::new(config);
        let report = wizard.run(&NoProgress).expect("dry run");

        assert!(report.is_success(), "{:?}", report.errors);
        assert!(!dir.path().join("none.rmm").exists());
    }

    #[test]
    fn test_restore_round_trip_with_backup_and_conflicts() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let archive = dir.path().join("profile.rmm");

        let mut create = base_config(dir.path(), MigrationMode::LocalArchive, &home);
        create.archive_path = Some(archive.clone());
        MigrationWizard::new(create)
            .run(&NoProgress)
            .expect("create archive");
        assert!(archive.exists());

        // В цели уже есть файл: нужен резерв (§9) и разрешение конфликта.
        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("Documents")).expect("mkdir");
        std::fs::write(target.join("Documents/doc.txt"), b"old").expect("write");

        let mut restore = base_config(dir.path(), MigrationMode::Restore, &target);
        restore.target_home = target.clone();
        restore.archive_path = Some(archive);
        restore.conflict_strategy = ConflictStrategy::Replace;

        let mut wizard = MigrationWizard::new(restore);
        let report = wizard.run(&NoProgress).expect("restore");

        assert!(report.is_success(), "{:?}", report.errors);
        assert_eq!(report.stats.files_copied, 2);
        assert_eq!(report.stats.conflicts_resolved, 1);
        assert_eq!(report.conflicts.len(), 1);
        assert!(report.backup_path.is_some());
        assert_eq!(wizard.stage(), WizardStage::Completed);
        assert_eq!(
            std::fs::read(target.join("Documents/doc.txt")).expect("read"),
            b"doc"
        );
    }

    #[test]
    fn test_cancel_stops_before_execution() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::LocalArchive, &home);
        config.archive_path = Some(dir.path().join("cancelled.rmm"));
        config.database_path = Some(dir.path().join("history.db"));

        let token = config.cancel_token();
        let mut wizard = MigrationWizard::new(config);
        wizard.cancel();
        assert!(token.is_cancelled());

        let report = wizard.run(&NoProgress).expect("cancelled run");
        assert!(report.cancelled);
        assert_eq!(wizard.stage(), WizardStage::Cancelled);
        assert!(!dir.path().join("cancelled.rmm").exists());

        let database = DatabaseManager::new(dir.path().join("history.db")).expect("db");
        let stored = database
            .get_migration(&report.id)
            .expect("query")
            .expect("record");
        assert!(matches!(stored.status, MigrationStatus::Cancelled));
    }

    #[test]
    fn test_missing_archive_marks_failure() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::Restore, &home);
        config.target_home = dir.path().join("target");
        config.archive_path = Some(dir.path().join("absent.rmm"));

        let mut wizard = MigrationWizard::new(config);
        let report = wizard.run(&NoProgress).expect("run");

        assert!(!report.errors.is_empty());
        assert!(!report.is_success());
        assert_eq!(wizard.stage(), WizardStage::Failed);
    }

    #[test]
    fn test_ssh_direct_requires_connection() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::SshDirect, &home);
        config.target_home = dir.path().join("remote-home");

        let mut wizard = MigrationWizard::new(config);
        let report = wizard.run(&NoProgress).expect("run");

        assert_eq!(wizard.stage(), WizardStage::Failed);
        assert!(!report.errors.is_empty());
    }

    #[test]
    fn test_restore_requires_target_home() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::Restore, &home);
        config.archive_path = Some(dir.path().join("profile.rmm"));

        let mut wizard = MigrationWizard::new(config);
        let _report = wizard.run(&NoProgress).expect("run");
        assert_eq!(wizard.stage(), WizardStage::Failed);
    }

    #[test]
    fn test_step_navigation() {
        assert_eq!(WizardStep::all().len(), 12);
        assert_eq!(WizardStep::Welcome.previous(), None);
        assert_eq!(WizardStep::Finish.next(), None);
        assert_eq!(WizardStep::Welcome.next(), Some(WizardStep::Connection));
        assert_eq!(WizardStep::Finish.previous(), Some(WizardStep::Report));
        assert_eq!(WizardStep::Progress.index(), 8);
        assert!(!WizardStep::Preview.title().is_empty());
    }

    #[test]
    fn test_state_round_trip_without_secrets() {
        let dir = tempdir().expect("tempdir");
        let home = make_home(dir.path());
        let mut config = base_config(dir.path(), MigrationMode::SshDirect, &home);
        config.passphrase = Some("super-secret".to_string());

        let mut wizard = MigrationWizard::new(config);
        wizard.next_step();

        let path = dir.path().join("state.json");
        wizard.save_state(&path).expect("save");

        let text = std::fs::read_to_string(&path).expect("read");
        assert!(!text.contains("super-secret"), "пароль не сохраняется");
        assert!(!text.contains("passphrase"), "поле секрета отсутствует");

        let state = WizardState::load(&path).expect("load");
        let restored = MigrationWizard::restore_state(state);
        assert_eq!(restored.step(), WizardStep::Connection);
        assert_eq!(restored.config().mode, MigrationMode::SshDirect);
        assert!(restored.config().passphrase.is_none());
        assert!(!restored.cancel_token().is_cancelled());

        WizardState::clear(&path).expect("clear");
        assert!(!path.exists());
    }
}
