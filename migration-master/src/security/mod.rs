//! Модуль безопасности для работы с шифрованием и хешированием

use std::path::{Path, PathBuf};
use std::io::{Read, Write, BufReader, BufWriter};
use sha2::{Sha256, Digest};
use age::{Encryptor, Decryptor, Identity, passphrase::ProtectedIdentity};
use serde::{Deserialize, Serialize};
use crate::error::{MigrationError, Result};

/// Результат вычисления хеша
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashResult {
    /// Путь к файлу
    pub path: String,
    /// SHA-256 хеш в hex формате
    pub sha256: String,
    /// Размер файла в байтах
    pub size: u64,
}

/// Вычисление SHA-256 хеша файла
pub fn compute_sha256(path: impl AsRef<Path>) -> Result<HashResult> {
    let path = path.as_ref();
    let file = std::fs::File::open(path)
        .map_err(|e| MigrationError::FileNotFound(format!("{}: {}", path.display(), e)))?;
    
    let metadata = file.metadata()?;
    let size = metadata.len();
    
    let mut hasher = Sha256::new();
    let mut reader = BufReader::new(file);
    let mut buffer = vec![0u8; 8192];
    
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    
    let hash = hasher.finalize();
    let hex_hash = hex::encode(hash);
    
    Ok(HashResult {
        path: path.to_string_lossy().to_string(),
        sha256: hex_hash,
        size,
    })
}

/// Вычисление SHA-256 хеша из данных
pub fn compute_sha256_from_data(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash = hasher.finalize();
    hex::encode(hash)
}

/// Проверка соответствия хеша
pub fn verify_sha256(path: impl AsRef<Path>, expected_hash: &str) -> Result<bool> {
    let result = compute_sha256(path)?;
    Ok(result.sha256 == expected_hash)
}

/// Настройки шифрования
#[derive(Debug, Clone)]
pub struct EncryptionConfig {
    /// Пароль для шифрования
    pub password: String,
    /// Уровень сложности пароля (0-4)
    pub password_strength: u8,
}

impl EncryptionConfig {
    pub fn new(password: impl Into<String>) -> Self {
        let password = password.into();
        let strength = estimate_password_strength(&password);
        
        Self {
            password,
            password_strength: strength,
        }
    }

    /// Оценка сложности пароля
    pub fn estimate_strength(password: &str) -> u8 {
        estimate_password_strength(password)
    }
}

/// Оценка сложности пароля (упрощённая версия zxcvbn)
fn estimate_password_strength(password: &str) -> u8 {
    let mut score = 0u8;
    
    // Длина
    if password.len() >= 8 {
        score += 1;
    }
    if password.len() >= 12 {
        score += 1;
    }
    if password.len() >= 16 {
        score += 1;
    }
    
    // Разнообразие символов
    let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_special = password.chars().any(|c| !c.is_alphanumeric());
    
    let variety_count = [has_lower, has_upper, has_digit, has_special]
        .iter()
        .filter(|&&b| b)
        .count();
    
    score = score.saturating_add(variety_count as u8);
    
    // Ограничиваем максимум 4
    score.min(4)
}

/// Шифрование файла с использованием age
pub fn encrypt_file(
    input_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    password: &str,
) -> Result<()> {
    let input_path = input_path.as_ref();
    let output_path = output_path.as_ref();
    
    // Создаём output директорию если нужно
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    let input_file = std::fs::File::open(input_path)?;
    let output_file = std::fs::File::create(output_path)?;
    
    let encryptor = Encryptor::with_user_passphrase(password);
    
    let mut writer = encryptor.wrap_output(BufWriter::new(output_file))
        .map_err(|e| MigrationError::Encryption(e.to_string()))?;
    
    let mut reader = BufReader::new(input_file);
    let mut buffer = vec![0u8; 8192];
    
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        writer.write_all(&buffer[..bytes_read])?;
    }
    
    writer.finish()
        .map_err(|e| MigrationError::Encryption(e.to_string()))?;
    
    Ok(())
}

/// Расшифрование файла с использованием age
pub fn decrypt_file(
    input_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    password: &str,
) -> Result<()> {
    let input_path = input_path.as_ref();
    let output_path = output_path.as_ref();
    
    // Создаём output директорию если нужно
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    
    let input_file = std::fs::File::open(input_path)?;
    let output_file = std::fs::File::create(output_path)?;
    
    let decryptor = Decryptor::new(BufReader::new(input_file))
        .map_err(|e| MigrationError::Encryption(e.to_string()))?;
    
    let identity = ProtectedIdentity::from_passphrase(password);
    
    let mut reader = decryptor.decrypt(&[&identity as &dyn Identity])
        .map_err(|e| MigrationError::Encryption(e.to_string()))?
        .ok_or_else(|| MigrationError::Encryption("Неверный пароль".to_string()))?;
    
    let mut writer = BufWriter::new(output_file);
    let mut buffer = vec![0u8; 8192];
    
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        writer.write_all(&buffer[..bytes_read])?;
    }
    
    writer.flush()?;
    
    Ok(())
}

/// Шифрование данных в памяти
pub fn encrypt_data(data: &[u8], password: &str) -> Result<Vec<u8>> {
    let encryptor = Encryptor::with_user_passphrase(password);
    
    let mut encrypted = Vec::new();
    {
        let mut writer = encryptor.wrap_output(&mut encrypted)
            .map_err(|e| MigrationError::Encryption(e.to_string()))?;
        writer.write_all(data)?;
        writer.finish()
            .map_err(|e| MigrationError::Encryption(e.to_string()))?;
    }
    
    Ok(encrypted)
}

/// Расшифрование данных из памяти
pub fn decrypt_data(encrypted_data: &[u8], password: &str) -> Result<Vec<u8>> {
    let decryptor = Decryptor::new(encrypted_data)
        .map_err(|e| MigrationError::Encryption(e.to_string()))?;
    
    let identity = ProtectedIdentity::from_passphrase(password);
    
    let mut reader = decryptor.decrypt(&[&identity as &dyn Identity])
        .map_err(|e| MigrationError::Encryption(e.to_string()))?
        .ok_or_else(|| MigrationError::Encryption("Неверный пароль".to_string()))?;
    
    let mut decrypted = Vec::new();
    reader.read_to_end(&mut decrypted)?;
    
    Ok(decrypted)
}

/// Генерация безопасного временного пароля
pub fn generate_temp_password(length: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos() as u64;
    
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*"
        .chars()
        .collect();
    
    let mut password = String::with_capacity(length);
    let mut current_seed = seed;
    
    for _ in 0..length {
        current_seed = current_seed.wrapping_mul(1103515245).wrapping_add(12345);
        let index = (current_seed % chars.len() as u64) as usize;
        password.push(chars[index]);
    }
    
    password
}

/// Проверка пути на path traversal атаки
pub fn validate_path(base: &Path, path: &Path) -> Result<PathBuf> {
    // Нормализуем базовый путь
    let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    
    // Собираем полный путь
    let full_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    
    // Проверяем что путь начинается с базового
    let normalized = full_path.lexical_clean();
    
    if !normalized.starts_with(&base) {
        return Err(MigrationError::PathTraversal(
            format!("Путь выходит за пределы базовой директории: {}", path.display())
        ));
    }
    
    Ok(normalized)
}

/// Расширение для проверки пути
trait PathExt {
    fn lexical_clean(&self) -> PathBuf;
}

impl PathExt for PathBuf {
    fn lexical_clean(&self) -> PathBuf {
        let mut components = Vec::new();
        
        for component in self.components() {
            match component {
                std::path::Component::ParentDir => {
                    components.pop();
                }
                std::path::Component::CurDir => {}
                _ => components.push(component.as_os_str()),
            }
        }
        
        components.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    
    #[test]
    fn test_compute_sha256() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();
        
        let result = compute_sha256(&file_path).unwrap();
        assert_eq!(result.sha256, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
        assert_eq!(result.size, 11);
    }
    
    #[test]
    fn test_encrypt_decrypt_file() {
        let dir = tempdir().unwrap();
        let input_path = dir.path().join("input.txt");
        let encrypted_path = dir.path().join("encrypted.age");
        let decrypted_path = dir.path().join("decrypted.txt");
        
        let original_content = b"secret data";
        std::fs::write(&input_path, original_content).unwrap();
        
        encrypt_file(&input_path, &encrypted_path, "password123").unwrap();
        decrypt_file(&encrypted_path, &decrypted_path, "password123").unwrap();
        
        let decrypted_content = std::fs::read(&decrypted_path).unwrap();
        assert_eq!(original_content, decrypted_content.as_slice());
    }
    
    #[test]
    fn test_password_strength() {
        assert!(estimate_password_strength("123") < 2);
        assert!(estimate_password_strength("password123") >= 2);
        assert!(estimate_password_strength("Str0ngP@ssw0rd!") >= 4);
    }
    
    #[test]
    fn test_path_validation() {
        let base = PathBuf::from("/safe/base");
        let valid = PathBuf::from("subdir/file.txt");
        let invalid = PathBuf::from("../etc/passwd");
        
        assert!(validate_path(&base, &valid).is_ok());
        assert!(validate_path(&base, &invalid).is_err());
    }
}
