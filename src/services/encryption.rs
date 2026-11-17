use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose, Engine as _};
use rand::Rng;

use crate::error::{AppError, AppResult};

pub struct EncryptionService {
    key: Key<Aes256Gcm>,
}

impl EncryptionService {
    /// Create from config
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self::new(&config.encryption_key).unwrap()
    }

    /// Create a new encryption service with the given secret key
    pub fn new(secret: &str) -> AppResult<Self> {
        // Derive a 256-bit key from the secret
        let mut key_bytes = [0u8; 32];
        let secret_bytes = secret.as_bytes();

        // Simple key derivation - in production, use proper KDF like PBKDF2
        for (i, byte) in secret_bytes.iter().cycle().take(32).enumerate() {
            key_bytes[i] = *byte;
        }

        let key = Key::<Aes256Gcm>::from_slice(&key_bytes);

        Ok(Self { key: *key })
    }

    /// Encrypt a string and return base64-encoded ciphertext
    pub fn encrypt(&self, plaintext: &str) -> AppResult<String> {
        let cipher = Aes256Gcm::new(&self.key);

        // Generate a random nonce
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt the plaintext
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| AppError::Internal(format!("Encryption failed: {e}")))?;

        // Combine nonce + ciphertext and encode as base64
        let mut result = nonce_bytes.to_vec();
        result.extend_from_slice(&ciphertext);

        Ok(general_purpose::STANDARD.encode(result))
    }

    /// Decrypt a base64-encoded ciphertext
    pub fn decrypt(&self, encrypted: &str) -> AppResult<String> {
        let cipher = Aes256Gcm::new(&self.key);

        // Decode from base64
        let data = general_purpose::STANDARD
            .decode(encrypted)
            .map_err(|e| AppError::Internal(format!("Invalid encrypted data: {e}")))?;

        if data.len() < 12 {
            return Err(AppError::Internal("Invalid encrypted data length".to_string()));
        }

        // Split nonce and ciphertext
        let (nonce_bytes, ciphertext) = data.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        // Decrypt
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| AppError::Internal(format!("Decryption failed: {e}")))?;

        String::from_utf8(plaintext)
            .map_err(|e| AppError::Internal(format!("Invalid UTF-8 in decrypted data: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let service = EncryptionService::new("test_secret_key_123").unwrap();
        let plaintext = "sk-test-api-key-1234567890";

        let encrypted = service.encrypt(plaintext).unwrap();
        assert_ne!(encrypted, plaintext);

        let decrypted = service.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_different_encryption_results() {
        let service = EncryptionService::new("test_secret_key_123").unwrap();
        let plaintext = "test_data";

        let encrypted1 = service.encrypt(plaintext).unwrap();
        let encrypted2 = service.encrypt(plaintext).unwrap();

        // Different nonces should produce different ciphertexts
        assert_ne!(encrypted1, encrypted2);

        // But both should decrypt to the same plaintext
        assert_eq!(service.decrypt(&encrypted1).unwrap(), plaintext);
        assert_eq!(service.decrypt(&encrypted2).unwrap(), plaintext);
    }

    #[test]
    fn test_invalid_encrypted_data() {
        let service = EncryptionService::new("test_secret_key_123").unwrap();

        let result = service.decrypt("invalid_base64_!@#$");
        assert!(result.is_err());
    }
}
