use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;

pub const SERVICE_NAME: &str = "document_investigator";
pub const KEY_NAME: &str = "master_key";

pub trait KeyProvider: Send + Sync {
    fn get_or_create_master_key(&self) -> Result<[u8; 32], String>;
}

pub struct KeyringProvider;

impl KeyProvider for KeyringProvider {
    fn get_or_create_master_key(&self) -> Result<[u8; 32], String> {
        let entry = keyring::Entry::new(SERVICE_NAME, KEY_NAME)
            .map_err(|e| format!("Failed to initialize keyring entry: {e}"))?;

        match entry.get_password() {
            Ok(hex_key) => {
                let bytes = hex::decode(&hex_key)
                    .map_err(|e| format!("Failed to decode key hex from keyring: {e}"))?;
                if bytes.len() != 32 {
                    return Err("Stored master key has invalid length".to_string());
                }
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                Ok(key)
            }
            Err(_) => {
                let mut key = [0u8; 32];
                rand::thread_rng().fill_bytes(&mut key);
                let hex_key = hex::encode(key);
                let _ = entry.set_password(&hex_key);
                Ok(key)
            }
        }
    }
}

pub struct MemoryKeyProvider {
    key: [u8; 32],
}

impl MemoryKeyProvider {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    pub fn random() -> Self {
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        Self { key }
    }
}

impl KeyProvider for MemoryKeyProvider {
    fn get_or_create_master_key(&self) -> Result<[u8; 32], String> {
        Ok(self.key)
    }
}

pub struct EncryptedPayload {
    pub ciphertext_hex: String,
    pub nonce_hex: String,
}

pub fn encrypt_password(plaintext: &str, master_key: &[u8; 32]) -> Result<EncryptedPayload, String> {
    let cipher = Aes256Gcm::new_from_slice(master_key)
        .map_err(|e| format!("Failed to create cipher: {e}"))?;
    
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| format!("Encryption error: {e}"))?;

    Ok(EncryptedPayload {
        ciphertext_hex: hex::encode(ciphertext),
        nonce_hex: hex::encode(nonce_bytes),
    })
}

pub fn decrypt_password(
    ciphertext_hex: &str,
    nonce_hex: &str,
    master_key: &[u8; 32],
) -> Result<String, String> {
    let cipher = Aes256Gcm::new_from_slice(master_key)
        .map_err(|e| format!("Failed to create cipher: {e}"))?;

    let ciphertext = hex::decode(ciphertext_hex)
        .map_err(|e| format!("Invalid ciphertext hex: {e}"))?;
    let nonce_bytes = hex::decode(nonce_hex)
        .map_err(|e| format!("Invalid nonce hex: {e}"))?;

    if nonce_bytes.len() != 12 {
        return Err("Invalid nonce length; expected 12 bytes".to_string());
    }

    let nonce = Nonce::from_slice(&nonce_bytes);
    let decrypted_bytes = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| "Decryption failed (bad key or corrupted data)".to_string())?;

    String::from_utf8(decrypted_bytes)
        .map_err(|e| format!("Decrypted bytes are not valid UTF-8: {e}"))
}

mod hex {
    pub fn encode<T: AsRef<[u8]>>(data: T) -> String {
        let bytes = data.as_ref();
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    pub fn decode(hex: &str) -> Result<Vec<u8>, String> {
        if hex.len() % 2 != 0 {
            return Err("Odd hex length".to_string());
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for i in (0..hex.len()).step_by(2) {
            let byte = u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| format!("Invalid hex character at {i}: {e}"))?;
            bytes.push(byte);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_key_provider() {
        let provider = MemoryKeyProvider::random();
        let key1 = provider.get_or_create_master_key().unwrap();
        let key2 = provider.get_or_create_master_key().unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_encryption_decryption_roundtrip() {
        let key = [42u8; 32];
        let original_pwd = "SecretPassw0rd!#123";

        let encrypted = encrypt_password(original_pwd, &key).expect("Encryption should succeed");
        assert!(!encrypted.ciphertext_hex.is_empty());
        assert_eq!(encrypted.nonce_hex.len(), 24); // 12 bytes * 2 hex chars

        let decrypted = decrypt_password(&encrypted.ciphertext_hex, &encrypted.nonce_hex, &key)
            .expect("Decryption should succeed");
        assert_eq!(decrypted, original_pwd);
    }

    #[test]
    fn test_decryption_fails_with_wrong_key() {
        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        let original_pwd = "MyConfidentialData";

        let encrypted = encrypt_password(original_pwd, &key1).unwrap();
        let result = decrypt_password(&encrypted.ciphertext_hex, &encrypted.nonce_hex, &key2);
        assert!(result.is_err());
    }

    #[test]
    fn test_decryption_fails_with_corrupted_ciphertext() {
        let key = [7u8; 32];
        let original_pwd = "TestPassword";

        let encrypted = encrypt_password(original_pwd, &key).unwrap();
        let mut corrupted_hex = encrypted.ciphertext_hex;
        if let Some(first_char) = corrupted_hex.chars().next() {
            let replacement = if first_char == '0' { '1' } else { '0' };
            corrupted_hex.replace_range(0..1, &replacement.to_string());
        }

        let result = decrypt_password(&corrupted_hex, &encrypted.nonce_hex, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_hex_encode_decode() {
        let raw = b"hello world 123";
        let encoded = hex::encode(raw);
        let decoded = hex::decode(&encoded).unwrap();
        assert_eq!(decoded, raw);

        assert!(hex::decode("abc").is_err());
        assert!(hex::decode("zz").is_err());
    }
}
