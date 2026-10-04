//! Модуль безопасности: хеширование (SHA-256), шифрование (age),
//! генерация паролей, защита от path traversal и управление правами файлов.
//!
//! Пароли и passphrase никогда не сохраняются на диск и не попадают в журнал.

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use rand::distributions::Alphanumeric;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::error::{MigrationError, Result};
use crate::platform;

/// Результат вычисления хеша файла.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HashResult {
    /// Путь к файлу
    pub path: String,
    /// SHA-256 хеш в hex формате
    pub sha256: String,
    /// Размер файла в байтах
    pub size: u64,
}

/// Вычисление SHA-256 хеша файла.
pub fn compute_sha256(path: impl AsRef<Path>) -> Result<HashResult> {
    let path = path.as_ref();
    let file = std::fs::File::open(path)
        .map_err(|e| MigrationError::FileNotFound(format!("{}: {}", path.display(), e)))?;

    let size = file.metadata()?.len();
    let sha256 = hash_reader(BufReader::new(file))?;

    Ok(HashResult {
        path: path.to_string_lossy().to_string(),
        sha256,
        size,
    })
}

/// Вычисление SHA-256 для любых данных в памяти.
pub fn compute_sha256_from_data(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Потоковое вычисление SHA-256.
pub fn hash_reader(mut reader: impl Read) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// Проверка соответствия хеша файла ожидаемому значению.
pub fn verify_sha256(path: impl AsRef<Path>, expected_hash: &str) -> Result<bool> {
    let result = compute_sha256(path)?;
    Ok(result.sha256.eq_ignore_ascii_case(expected_hash.trim()))
}

/// Проверка соответствия ожидаемому значению с понятной ошибкой.
pub fn ensure_checksum(path: impl AsRef<Path>, expected_hash: &str) -> Result<()> {
    let path = path.as_ref();
    if verify_sha256(path, expected_hash)? {
        Ok(())
    } else {
        Err(MigrationError::ChecksumMismatch(format!(
            "{} (ожидалось {}, получено другое значение)",
            path.display(),
            expected_hash
        )))
    }
}

/// Замена секрета на маску для журналов и отчётов.
pub fn redact_secret(_secret: &str) -> String {
    "***".to_string()
}

/// Настройки шифрования.
#[derive(Debug, Clone)]
pub struct EncryptionConfig {
    /// Пароль для шифрования
    pub password: String,
    /// Оценка сложности пароля (0-4)
    pub password_strength: u8,
}

impl EncryptionConfig {
    /// Создать конфигурацию и оценить сложность пароля.
    pub fn new(password: impl Into<String>) -> Self {
        let password = password.into();
        let password_strength = estimate_password_strength(&password);
        Self {
            password,
            password_strength,
        }
    }

    /// Оценка сложности пароля.
    pub fn estimate_strength(password: &str) -> u8 {
        estimate_password_strength(password)
    }

    /// Является ли пароль достаточно надёжным (оценка >= 3).
    pub fn is_strong(&self) -> bool {
        self.password_strength >= 3
    }
}

/// Оценка сложности пароля по шкале 0-4 (упрощённый аналог zxcvbn).
pub fn estimate_password_strength(password: &str) -> u8 {
    if password.is_empty() {
        return 0;
    }

    let mut score = 0u8;

    if password.len() >= 8 {
        score += 1;
    }
    if password.len() >= 12 {
        score += 1;
    }
    if password.len() >= 16 {
        score += 1;
    }

    let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_special = password
        .chars()
        .any(|c| !c.is_alphanumeric() && !c.is_whitespace());

    let variety = [has_lower, has_upper, has_digit, has_special]
        .iter()
        .filter(|&&v| v)
        .count() as u8;

    score = score.saturating_add(variety);

    // Слабые шаблоны снижают оценку
    let lowered = password.to_lowercase();
    for weak in ["password", "qwerty", "123456", "пароль", "admin", "letmein"] {
        if lowered.contains(weak) {
            score = score.saturating_sub(2);
        }
    }

    score.min(4)
}

/// Текстовая оценка сложности пароля для интерфейса.
pub fn password_strength_label(score: u8) -> &'static str {
    match score {
        0 | 1 => "очень слабый",
        2 => "слабый",
        3 => "хороший",
        _ => "надёжный",
    }
}

/// Генерация криптографически стойкого случайного пароля.
pub fn generate_password(length: usize) -> String {
    let length = length.clamp(12, 128);
    let mut rng = rand::thread_rng();

    let mut password: String = (0..length)
        .map(|_| char::from(rng.sample(Alphanumeric)))
        .collect();

    // Гарантируем наличие строчных, прописных символов и цифр
    if !password.chars().any(|c| c.is_ascii_lowercase()) {
        password.push(char::from(rng.sample(Alphanumeric)).to_ascii_lowercase());
    }
    if !password.chars().any(|c| c.is_ascii_uppercase()) {
        password.push(char::from(rng.sample(Alphanumeric)).to_ascii_uppercase());
    }
    if !password.chars().any(|c| c.is_ascii_digit()) {
        password.push(char::from(rng.gen_range(b'0'..=b'9')) as char);
    }

    password
}

/// Преобразование пароля в секретный тип age.
fn secret(password: &str) -> age::secrecy::SecretString {
    age::secrecy::SecretString::from(password.to_string())
}

/// Шифрование файла паролем (age, scrypt).
pub fn encrypt_file(
    input_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    password: &str,
) -> Result<()> {
    let input_path = input_path.as_ref();
    let output_path = output_path.as_ref();

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let input_file = std::fs::File::open(input_path)?;
    let output_file = std::fs::File::create(output_path)?;

    let encryptor = age::Encryptor::with_user_passphrase(secret(password));
    let mut writer = encryptor
        .wrap_output(BufWriter::new(output_file))
        .map_err(|e| MigrationError::Encryption(e.to_string()))?;

    std::io::copy(&mut BufReader::new(input_file), &mut writer)?;
    writer
        .finish()
        .map_err(|e| MigrationError::Encryption(e.to_string()))?;

    Ok(())
}

/// Расшифрование файла паролем (age, scrypt).
pub fn decrypt_file(
    input_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    password: &str,
) -> Result<()> {
    let input_path = input_path.as_ref();
    let output_path = output_path.as_ref();

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let input_file = std::fs::File::open(input_path)?;
    let decryptor = age::Decryptor::new(BufReader::new(input_file))
        .map_err(|e| MigrationError::Decryption(e.to_string()))?;

    let identity = age::scrypt::Identity::new(secret(password));
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|e| MigrationError::Decryption(e.to_string()))?;

    let mut writer = BufWriter::new(std::fs::File::create(output_path)?);
    std::io::copy(&mut reader, &mut writer)?;
    writer.flush()?;

    Ok(())
}

/// Шифрование данных в памяти.
pub fn encrypt_data(data: &[u8], password: &str) -> Result<Vec<u8>> {
    let recipient = age::scrypt::Recipient::new(secret(password));
    age::encrypt(&recipient, data).map_err(|e| MigrationError::Encryption(e.to_string()))
}

/// Расшифрование данных в памяти.
pub fn decrypt_data(encrypted_data: &[u8], password: &str) -> Result<Vec<u8>> {
    let identity = age::scrypt::Identity::new(secret(password));
    age::decrypt(&identity, encrypted_data)
        .map_err(|e| MigrationError::Decryption(format!("не удалось расшифровать данные: {}", e)))
}

/// Лексическая нормализация пути без обращения к файловой системе.
/// Убирает `.` и разрешает `..` только внутри пути.
pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    result.push("..");
                }
            }
            other => result.push(other.as_os_str()),
        }
    }

    result
}

/// Проверка, что относительный путь безопасен для распаковки архива.
///
/// Запрещены: абсолютные пути, компоненты `..`, префиксы томов Windows,
/// пустой путь и символы `\0`.
pub fn sanitize_relative_path(path: &Path) -> Result<PathBuf> {
    let raw = path.to_string_lossy();
    if raw.contains('\0') {
        return Err(MigrationError::PathTraversal(raw.to_string()));
    }

    let mut result = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Normal(part) => result.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(MigrationError::PathTraversal(raw.to_string()));
            }
        }
    }

    if result.as_os_str().is_empty() {
        return Err(MigrationError::PathTraversal(format!(
            "пустой путь в архиве: {}",
            raw
        )));
    }

    Ok(result)
}

/// Преобразование абсолютного пути в относительный внутри базового каталога.
pub fn make_relative(base: &Path, path: &Path) -> Result<PathBuf> {
    let base = lexical_normalize(base);
    let path = lexical_normalize(path);

    path.strip_prefix(&base)
        .map(|p| p.to_path_buf())
        .map_err(|_| MigrationError::traversal(&path))
}

/// Проверка, что путь находится внутри базового каталога (без обращения к ФС).
pub fn is_within(base: &Path, candidate: &Path) -> bool {
    let base = lexical_normalize(base);
    let candidate = lexical_normalize(candidate);
    candidate.starts_with(&base)
}

/// Безопасное соединение базового каталога и относительного пути.
pub fn safe_join(base: &Path, relative: &Path) -> Result<PathBuf> {
    let relative = sanitize_relative_path(relative)?;
    let joined = base.join(&relative);

    if !is_within(base, &joined) {
        return Err(MigrationError::traversal(&joined));
    }

    Ok(joined)
}

/// Валидация пути относительно базового каталога (совместимость с прежним API).
pub fn validate_path(base: &Path, path: &Path) -> Result<PathBuf> {
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };

    let base_norm = lexical_normalize(base);
    let full_norm = lexical_normalize(&full);

    if !full_norm.starts_with(&base_norm) {
        return Err(MigrationError::traversal(&full_norm));
    }

    Ok(full_norm)
}

/// Установить приватные права на файл и при необходимости исправить владельца.
pub fn enforce_private_permissions(path: &Path, expected_uid: Option<u32>) -> Result<()> {
    platform::ensure_private_file(path)?;

    if let Some(uid) = expected_uid {
        if !is_owned_by(path, uid) {
            platform::set_owner(path, uid, platform::current_gid())?;
        }
    }

    Ok(())
}

/// Проверить, принадлежит ли файл указанному пользователю.
pub fn is_owned_by(path: &Path, uid: u32) -> bool {
    let (owner, _) = platform::owner_uid_gid(path);
    owner == Some(uid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_compute_sha256() {
        let dir = tempdir().expect("tempdir");
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").expect("write");

        let result = compute_sha256(&file).expect("hash");
        assert_eq!(
            result.sha256,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(result.size, 11);
    }

    #[test]
    fn test_compute_sha256_from_data_matches_file_hash() {
        let dir = tempdir().expect("tempdir");
        let file = dir.path().join("data.bin");
        let data = vec![7u8; 4096];
        std::fs::write(&file, &data).expect("write");

        assert_eq!(
            compute_sha256(&file).expect("hash").sha256,
            compute_sha256_from_data(&data)
        );
    }

    #[test]
    fn test_verify_sha256() {
        let dir = tempdir().expect("tempdir");
        let file = dir.path().join("data.bin");
        std::fs::write(&file, b"abc").expect("write");

        let hash = compute_sha256(&file).expect("hash").sha256;
        assert!(verify_sha256(&file, &hash).expect("verify"));
        assert!(verify_sha256(&file, &hash.to_uppercase()).expect("verify upper"));
        assert!(!verify_sha256(&file, "deadbeef").expect("verify"));

        let err = ensure_checksum(&file, "deadbeef").unwrap_err();
        assert!(matches!(err, MigrationError::ChecksumMismatch(_)));
    }

    #[test]
    fn test_encrypt_decrypt_data_round_trip() {
        let plaintext = "секретные данные профиля".as_bytes();
        let encrypted = encrypt_data(plaintext, "Пароль123!").expect("encrypt");
        assert_ne!(encrypted, plaintext);

        let decrypted = decrypt_data(&encrypted, "Пароль123!").expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_password_fails() {
        let encrypted = encrypt_data(b"data", "правильный-пароль").expect("encrypt");
        assert!(decrypt_data(&encrypted, "неверный-пароль").is_err());
    }

    #[test]
    fn test_encrypt_decrypt_file_round_trip() {
        let dir = tempdir().expect("tempdir");
        let input = dir.path().join("input.txt");
        let encrypted = dir.path().join("output.rmm");
        let decrypted = dir.path().join("restored.txt");

        let content = b"profile data";
        std::fs::write(&input, content).expect("write");

        encrypt_file(&input, &encrypted, "pass-word-123").expect("encrypt");
        assert!(encrypted.exists());

        decrypt_file(&encrypted, &decrypted, "pass-word-123").expect("decrypt");
        assert_eq!(std::fs::read(&decrypted).expect("read"), content);
    }

    #[test]
    fn test_password_strength_scale() {
        assert_eq!(estimate_password_strength(""), 0);
        assert!(estimate_password_strength("123") < 2);
        assert!(estimate_password_strength("password123") < 4);
        assert!(estimate_password_strength("Str0ng-P@ssw0rd-2026") >= 3);
        assert_eq!(password_strength_label(4), "надёжный");
    }

    #[test]
    fn test_encryption_config() {
        let config = EncryptionConfig::new("Str0ng-P@ssw0rd-2026");
        assert!(config.is_strong());
        assert_eq!(config.password, "Str0ng-P@ssw0rd-2026");
    }

    #[test]
    fn test_generated_password_is_strong_and_unique() {
        let first = generate_password(24);
        let second = generate_password(24);

        assert_ne!(first, second);
        assert!(first.len() >= 24);
        assert!(estimate_password_strength(&first) >= 3);
    }

    #[test]
    fn test_sanitize_relative_path_rejects_traversal() {
        assert_eq!(
            sanitize_relative_path(Path::new("Documents/file.txt")).expect("ok"),
            PathBuf::from("Documents/file.txt")
        );
        assert_eq!(
            sanitize_relative_path(Path::new("./Documents/./file.txt")).expect("ok"),
            PathBuf::from("Documents/file.txt")
        );

        assert!(sanitize_relative_path(Path::new("../etc/passwd")).is_err());
        assert!(sanitize_relative_path(Path::new("Documents/../../etc/passwd")).is_err());
        assert!(sanitize_relative_path(Path::new("")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_sanitize_relative_path_rejects_absolute() {
        assert!(sanitize_relative_path(Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn test_safe_join_stays_inside_base() {
        let base = PathBuf::from("/home/user");
        assert_eq!(
            safe_join(&base, Path::new("Documents/a.txt")).expect("join"),
            PathBuf::from("/home/user/Documents/a.txt")
        );
        assert!(safe_join(&base, Path::new("../../etc/passwd")).is_err());
        assert!(safe_join(&base, Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn test_make_relative() {
        let base = PathBuf::from("/home/user");
        assert_eq!(
            make_relative(&base, Path::new("/home/user/Documents/a.txt")).expect("rel"),
            PathBuf::from("Documents/a.txt")
        );
        assert!(make_relative(&base, Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn test_validate_path() {
        let base = PathBuf::from("/home/user");
        assert!(validate_path(&base, Path::new("Documents/file.txt")).is_ok());
        assert!(validate_path(&base, Path::new("../etc/passwd")).is_err());
    }

    #[test]
    fn test_is_within() {
        let base = PathBuf::from("/home/user");
        assert!(is_within(&base, Path::new("/home/user/.ssh/id_rsa")));
        assert!(!is_within(&base, Path::new("/home/other/.ssh/id_rsa")));
    }

    #[test]
    fn test_lexical_normalize() {
        assert_eq!(
            lexical_normalize(Path::new("/home/user/./Documents/../file.txt")),
            PathBuf::from("/home/user/file.txt")
        );
    }

    #[test]
    fn test_redact_secret_hides_value() {
        let redacted = redact_secret("super-secret");
        assert_eq!(redacted, "***");
        assert!(!redacted.contains("secret"));
    }

    #[cfg(unix)]
    #[test]
    fn test_enforce_private_permissions() {
        let dir = tempdir().expect("tempdir");
        let file = dir.path().join("id_ed25519");
        std::fs::write(&file, "PRIVATE KEY").expect("write");

        enforce_private_permissions(&file, Some(platform::current_uid())).expect("permissions");
        assert_eq!(platform::file_mode(&file).map(|m| m & 0o777), Some(0o600));
    }
}
