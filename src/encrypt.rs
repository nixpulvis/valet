use aes_siv::{
    Aes256SivAead, KeySizeUser, Nonce,
    aead::{Aead, Key as AesKey, KeyInit},
};
use argon2::Argon2;
use rand_core::{OsRng, RngCore};

pub const SALT_SIZE: usize = 16;
pub const NONCE_SIZE: usize = 16;
pub const KEY_SIZE: usize = 64; // 512 bit-key size, 256-bit security.

/// Represents some encrypted data.
#[derive(PartialEq, Eq)]
pub struct Encrypted {
    pub(crate) data: Vec<u8>,
    pub(crate) nonce: Vec<u8>,
}

/// A key is generated from a user record's salt and thier password.
//
// TODO: #6 keys should not be clonable.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Key(AesKey<Aes256SivAead>);

impl Key {
    pub fn new() -> Result<Self, Error> {
        Ok(Key(Aes256SivAead::generate_key(&mut OsRng)))
    }

    // TODO: Zeroize password
    pub fn from_password(password: String, salt: &[u8]) -> Result<Self, Error> {
        let argon2 = Argon2::default();
        assert_eq!(KEY_SIZE, Aes256SivAead::key_size());
        let mut output_key_material = [0u8; KEY_SIZE];
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut output_key_material)
            .map_err(|e| Error::KeyDerivation(format!("{}", e)))?;

        Ok(Key(AesKey::<Aes256SivAead>::clone_from_slice(
            &output_key_material,
        )))
    }

    pub(crate) fn generate_salt() -> [u8; SALT_SIZE] {
        let mut salt = [0; SALT_SIZE];
        OsRng.fill_bytes(&mut salt);
        salt
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Encrypted, Error> {
        // TODO: Better security will be achieved by storing a unique counter for this somewhere.
        let mut nonce_bytes = [0; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let cipher = Aes256SivAead::new(&self.0);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| Error::Encryption(format!("{}", e)))?;
        Ok(Encrypted {
            data: ciphertext,
            nonce: nonce.as_slice().into(),
        })
    }

    pub fn decrypt(&self, encrypted: &Encrypted) -> Result<Vec<u8>, Error> {
        let nonce = Nonce::from_slice(&encrypted.nonce);
        let cipher = Aes256SivAead::new(&self.0);
        let plaintext = cipher
            .decrypt(nonce, &encrypted.data[..])
            .map_err(|e| Error::Decryption(format!("{}", e)))?;
        Ok(plaintext)
    }
}

#[derive(Debug)]
pub enum Error {
    KeyDerivation(String),
    Encryption(String),
    Decryption(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_password() {
        let salt = Key::generate_salt();
        let key = Key::from_password("user1password".into(), &salt).expect("error generating key");

        assert_eq!(KEY_SIZE, key.0.len());
    }

    #[test]
    fn encrypt_decrypt_test() {
        let key = Key::new().expect("error generating key");

        let plaintext = b"this is a secret";
        let encrypted = key.encrypt(plaintext).expect("error encrypting");
        let decrypted = key.decrypt(&encrypted).expect("error dencrypting");

        assert_eq!(plaintext, &decrypted[..]);
    }
}
