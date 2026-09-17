//! Модуль сканирования пользовательского профиля

use std::path::{Path, PathBuf};
use std::fs;
use walkdir::WalkDir;
use serde::{Deserialize, Serialize};
use crate::config::{ComponentType, DEFAULT_EXCLUSIONS};
use crate::error::{MigrationError, Result};
use glob::Pattern;

/// Информация о файле
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    /// Полный путь к файлу
    pub path: String,
    /// Относительный путь
    pub relative_path: String,
    /// Размер в байтах
    pub size: u64,
    /// Является ли директорией
    pub is_dir: bool,
    /// Время последней модификации
    pub modified: Option<u64>,
    /// Владелец (UID)
    pub uid: Option<u32>,
    /// Группа (GID)
    pub gid: Option<u32>,
    /// Права доступа
    pub mode: Option<u32>,
    /// Тип компонента
    pub component: ComponentType,
}

/// Результат сканирования профиля
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileScanResult {
    /// Путь к домашнему каталогу
    pub home_dir: PathBuf,
    /// Пользователь
    pub username: String,
    /// UID пользователя
    pub uid: u32,
    /// GID пользователя
    pub gid: u32,
    /// Список файлов
    pub files: Vec<FileInfo>,
    /// Общее количество файлов
    pub total_files: usize,
    /// Общий размер в байтах
    pub total_size: u64,
    /// Количество по компонентам
    pub files_by_component: std::collections::HashMap<String, usize>,
    /// Размер по компонентам
    pub size_by_component: std::collections::HashMap<String, u64>,
    /// Исключённые пути
    pub excluded_paths: Vec<String>,
    /// Ошибки сканирования
    pub errors: Vec<String>,
}

impl ProfileScanResult {
    /// Получение размера в человекочитаемом формате
    pub fn total_size_human(&self) -> String {
        human_bytes::human_bytes(self.total_size as f64)
    }
}

/// Сканер пользовательского профиля
pub struct ProfileScanner {
    home_dir: PathBuf,
    username: String,
    uid: u32,
    gid: u32,
    components: Vec<ComponentType>,
    exclusions: Vec<String>,
    include_hidden: bool,
}

impl ProfileScanner {
    /// Создание нового сканера
    pub fn new() -> Result<Self> {
        let username = whoami::username();
        let home_dir = std::env::var("HOME")
            .map(PathBuf::from)
            .or_else(|_| dirs::home_dir().ok_or(MigrationError::FileNotFound("Домашняя директория не найдена".into())))?;
        
        // Получаем UID/GID из метаданных
        #[cfg(unix)]
        let (uid, gid) = {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::metadata(&home_dir)?;
            (metadata.uid(), metadata.gid())
        };
        
        #[cfg(not(unix))]
        let (uid, gid) = (1000, 1000);

        Ok(Self {
            home_dir,
            username,
            uid,
            gid,
            components: ComponentType::default_components(),
            exclusions: DEFAULT_EXCLUSIONS.iter().map(|s| s.to_string()).collect(),
            include_hidden: true,
        })
    }

    /// Установка компонентов для сканирования
    pub fn with_components(mut self, components: Vec<ComponentType>) -> Self {
        self.components = components;
        self
    }

    /// Добавление исключений
    pub fn with_exclusions(mut self, exclusions: Vec<String>) -> Self {
        self.exclusions.extend(exclusions);
        self
    }

    /// Включение скрытых файлов
    pub fn with_hidden_files(mut self, include: bool) -> Self {
        self.include_hidden = include;
        self
    }

    /// Выполнение сканирования
    pub fn scan(&self) -> Result<ProfileScanResult> {
        let mut files = Vec::new();
        let mut excluded_paths = Vec::new();
        let mut errors = Vec::new();
        let mut total_size = 0u64;
        let mut files_by_component: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut size_by_component: std::collections::HashMap<String, u64> = std::collections::HashMap::new();

        // Сканируем каждый компонент
        for &component in &self.components {
            let paths = self.get_component_paths(component);
            
            for path in paths {
                if !path.exists() {
                    continue;
                }

                match self.scan_directory(&path, component) {
                    Ok(scan_files) => {
                        for file in scan_files {
                            // Проверяем исключения
                            if self.is_excluded(&file.relative_path) {
                                excluded_paths.push(file.relative_path.clone());
                                continue;
                            }

                            // Проверяем скрытые файлы
                            if !self.include_hidden && self.is_hidden_file(&file.path) {
                                continue;
                            }

                            total_size += file.size;
                            if !file.is_dir {
                                let comp_name = format!("{:?}", component);
                                *files_by_component.entry(comp_name.clone()).or_insert(0) += 1;
                                *size_by_component.entry(comp_name).or_insert(0) += file.size;
                            }
                            
                            files.push(file);
                        }
                    }
                    Err(e) => {
                        errors.push(format!("Ошибка сканирования {:?}: {}", component, e));
                    }
                }
            }
        }

        Ok(ProfileScanResult {
            home_dir: self.home_dir.clone(),
            username: self.username.clone(),
            uid: self.uid,
            gid: self.gid,
            total_files: files.len(),
            total_size,
            files_by_component,
            size_by_component,
            excluded_paths,
            errors,
            files,
        })
    }

    /// Получение путей для компонента
    fn get_component_paths(&self, component: ComponentType) -> Vec<PathBuf> {
        match component {
            ComponentType::Desktop => vec![self.home_dir.join("Desktop"), self.home_dir.join("Рабочий стол")],
            ComponentType::Documents => vec![self.home_dir.join("Documents"), self.home_dir.join("Документы")],
            ComponentType::Downloads => vec![self.home_dir.join("Downloads"), self.home_dir.join("Загрузки")],
            ComponentType::Pictures => vec![self.home_dir.join("Pictures"), self.home_dir.join("Изображения")],
            ComponentType::Videos => vec![self.home_dir.join("Videos"), self.home_dir.join("Видео")],
            ComponentType::Music => vec![self.home_dir.join("Music"), self.home_dir.join("Музыка")],
            ComponentType::Templates => vec![self.home_dir.join("Templates"), self.home_dir.join("Шаблоны")],
            ComponentType::AppConfigs => vec![self.home_dir.join(".config")],
            ComponentType::AppData => vec![self.home_dir.join(".local").join("share")],
            ComponentType::LocalBin => vec![self.home_dir.join(".local").join("bin")],
            ComponentType::LocalApps => vec![self.home_dir.join(".local").join("share").join("applications")],
            ComponentType::Themes => vec![self.home_dir.join(".themes")],
            ComponentType::Icons => vec![self.home_dir.join(".icons")],
            ComponentType::Fonts => vec![self.home_dir.join(".fonts"), self.home_dir.join(".local").join("share").join("fonts")],
            ComponentType::SshKeys => vec![self.home_dir.join(".ssh")],
            _ => vec![],
        }
    }

    /// Сканирование директории
    fn scan_directory(&self, dir: &Path, component: ComponentType) -> Result<Vec<FileInfo>> {
        let mut files = Vec::new();
        
        if !dir.exists() {
            return Ok(files);
        }

        let walker = WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // Пропускаем symlink на сокеты и устройства
                let ft = e.file_type();
                if ft.is_symlink() {
                    // Проверяем куда ведёт symlink
                    if let Ok(target) = fs::read_link(e.path()) {
                        let target_str = target.to_string_lossy();
                        if target_str.starts_with("/proc/") 
                            || target_str.starts_with("/sys/")
                            || target_str.starts_with("/dev/")
                        {
                            return false;
                        }
                    }
                }
                true
            });

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    files.push(FileInfo {
                        path: String::new(),
                        relative_path: String::new(),
                        size: 0,
                        is_dir: false,
                        modified: None,
                        uid: None,
                        gid: None,
                        mode: None,
                        component,
                    });
                    continue;
                }
            };

            let path = entry.path().to_path_buf();
            let relative_path = path.strip_prefix(&self.home_dir)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            #[cfg(unix)]
            let (uid, gid, mode) = {
                use std::os::unix::fs::MetadataExt;
                (Some(metadata.uid()), Some(metadata.gid()), Some(metadata.mode()))
            };
            
            #[cfg(not(unix))]
            let (uid, gid, mode) = (None, None, None);

            let size = if metadata.is_dir() { 0 } else { metadata.len() };
            let modified = metadata.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());

            files.push(FileInfo {
                path: path.to_string_lossy().to_string(),
                relative_path,
                size,
                is_dir: metadata.is_dir(),
                modified,
                uid,
                gid,
                mode,
                component,
            });
        }

        Ok(files)
    }

    /// Проверка пути на соответствие исключениям
    fn is_excluded(&self, path: &str) -> bool {
        for pattern in &self.exclusions {
            if let Ok(pat) = Pattern::new(pattern) {
                if pat.matches(path) {
                    return true;
                }
            }
        }
        false
    }

    /// Проверка является ли файл скрытым
    fn is_hidden_file(&self, path: &str) -> bool {
        Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
    }

    /// Быстрая оценка размера без детального сканирования
    pub fn quick_estimate(&self) -> Result<(usize, u64)> {
        let mut total_files = 0;
        let mut total_size = 0u64;

        for &component in &self.components {
            for path in self.get_component_paths(component) {
                if path.exists() {
                    let (files, size) = self.estimate_directory(&path)?;
                    total_files += files;
                    total_size += size;
                }
            }
        }

        Ok((total_files, total_size))
    }

    /// Оценка размера директории
    fn estimate_directory(&self, dir: &Path) -> Result<(usize, u64)> {
        let mut files = 0;
        let mut size = 0u64;

        for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    files += 1;
                    size += metadata.len();
                }
            }
        }

        Ok((files, size))
    }
}

impl Default for ProfileScanner {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_profile_scanner_creation() {
        let scanner = ProfileScanner::new();
        assert!(scanner.is_ok());
    }

    #[test]
    fn test_quick_estimate() {
        let dir = tempdir().unwrap();
        let test_file = dir.path().join("test.txt");
        std::fs::write(&test_file, "hello").unwrap();

        // Тест с временной директорией
        let mut scanner = ProfileScanner::new().unwrap();
        scanner.home_dir = dir.path().to_path_buf();
        
        let result = scanner.quick_estimate();
        assert!(result.is_ok());
    }
}
