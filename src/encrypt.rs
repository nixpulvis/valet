use aes_siv::{
    Aes256SivAead, KeySizeUser, Nonce,
    aead::{Aead, Key, KeyInit},
};
use argon2::Argon2;
use rand::{Rng, rngs::OsRng};

pub const SALT_SIZE: usize = 16;
pub const NONCE_SIZE: usize = 16;
pub const CREDENTIAL_SIZE: usize = 64;

pub struct Encrypted {
    pub data: Vec<u8>,
    pub nonce: Vec<u8>,
}

/// A credential is generated from a user's registration and thier password.
pub struct Credential(Key<Aes256SivAead>);

impl Credential {
    pub fn new(password: &str, salt: &[u8]) -> Result<Self, ()> {
        let argon2 = Argon2::default();
        assert_eq!(CREDENTIAL_SIZE, Aes256SivAead::key_size());
        let mut output_key_material = [0u8; CREDENTIAL_SIZE];
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut output_key_material)
            .map_err(|_| ())?;

        Ok(Credential(Key::<Aes256SivAead>::clone_from_slice(
            &output_key_material,
        )))
    }

    pub fn key(&self) -> &Key<Aes256SivAead> {
        &self.0
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Encrypted, ()> {
        // TODO: Better security will be achieved by storing a unique counter for this somewhere.
        let mut nonce_bytes = [0; NONCE_SIZE];
        let mut rng = OsRng::new().map_err(|_| ())?;
        rng.try_fill(&mut nonce_bytes).map_err(|_| ())?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        let cipher = Aes256SivAead::new(self.key());
        let ciphertext = cipher.encrypt(nonce, plaintext).map_err(|_| ())?;
        Ok(Encrypted {
            data: ciphertext,
            nonce: nonce.as_slice().into(),
        })
    }

    pub fn decrypt(&self, encrypted: &Encrypted) -> Result<Vec<u8>, ()> {
        let nonce = Nonce::from_slice(&encrypted.nonce);
        let cipher = Aes256SivAead::new(self.key());
        let plaintext = cipher.decrypt(nonce, &encrypted.data[..]).map_err(|_| ())?;
        Ok(plaintext)
    }

    pub(crate) fn generate_salt() -> Result<[u8; SALT_SIZE], ()> {
        let mut salt = [0; SALT_SIZE];
        let mut rng = OsRng::new().map_err(|_| ())?;
        rng.try_fill(&mut salt).map_err(|_| ())?;
        Ok(salt)
    }
}

#[test]
fn new_credential() {
    let salt = Credential::generate_salt().expect("error generating salt");
    let credential = Credential::new("user1password", &salt).expect("error generating credential");

    assert_eq!(CREDENTIAL_SIZE, credential.0.len());
}

#[test]
fn encrypt_decrypt_test() {
    let salt = Credential::generate_salt().expect("error generating salt");
    let credential = Credential::new("user1password", &salt).expect("error generating credentials");

    let plaintext = b"this is a secret";
    let encrypted = credential.encrypt(plaintext).expect("error encrypting");
    let decrypted = credential.decrypt(&encrypted).expect("error dencrypting");

    assert_eq!(plaintext, &decrypted[..]);
}
