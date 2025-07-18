use aes_siv::{
    Aes256SivAead, KeySizeUser, Nonce,
    aead::{Aead, Key as AesKey, KeyInit},
};
use argon2::Argon2;
use rand::{Rng, rngs::OsRng};

pub const SALT_SIZE: usize = 16;
pub const NONCE_SIZE: usize = 16;
pub const KEY_SIZE: usize = 64;

/// Represents some encrypted data.
pub struct Encrypted {
    pub data: Vec<u8>,
    pub nonce: Vec<u8>,
}

/// A key is generated from a user record's salt and thier password.
pub struct Key(AesKey<Aes256SivAead>);

impl Key {
    pub(crate) fn generate_salt() -> Result<[u8; SALT_SIZE], Error> {
        let mut salt = [0; SALT_SIZE];
        let mut rng = OsRng::new().map_err(|e| Error::KeyDerivation(e.msg.into()))?;
        rng.try_fill(&mut salt)
            .map_err(|e| Error::KeyDerivation(e.msg.into()))?;
        Ok(salt)
    }

    pub fn new(password: &str, salt: &[u8]) -> Result<Self, Error> {
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

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Encrypted, Error> {
        // TODO: Better security will be achieved by storing a unique counter for this somewhere.
        let mut nonce_bytes = [0; NONCE_SIZE];
        let mut rng = OsRng::new().map_err(|e| Error::Encryption(format!("{}", e)))?;
        rng.try_fill(&mut nonce_bytes)
            .map_err(|e| Error::Encryption(format!("{}", e)))?;
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

#[test]
fn new_credential() {
    let salt = Key::generate_salt().expect("error generating salt");
    let credential = Key::new("user1password", &salt).expect("error generating credential");

    assert_eq!(KEY_SIZE, credential.0.len());
}

#[test]
fn encrypt_decrypt_test() {
    let salt = Key::generate_salt().expect("error generating salt");
    let credential = Key::new("user1password", &salt).expect("error generating credentials");

    let plaintext = b"this is a secret";
    let encrypted = credential.encrypt(plaintext).expect("error encrypting");
    let decrypted = credential.decrypt(&encrypted).expect("error dencrypting");

    assert_eq!(plaintext, &decrypted[..]);
}
