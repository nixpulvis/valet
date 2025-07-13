use aes_siv::{
    Aes256SivAead, KeySizeUser, Nonce,
    aead::{Aead, Key, KeyInit},
};
use argon2::Argon2;
use rand::{Rng, rngs::OsRng};

const SALT_SIZE: usize = 16;
const CREDENTIAL_SIZE: usize = 64;

/// Usernames and the salt for their password are store in a database.
pub struct Registration {
    username: String,
    salt: [u8; SALT_SIZE],
}

impl Registration {
    pub fn new(username: &str) -> Result<Self, ()> {
        let mut salt = [0; SALT_SIZE];
        let mut rng = OsRng::new().map_err(|_| ())?;
        rng.try_fill(&mut salt).map_err(|_| ())?;
        Ok(Registration {
            username: username.into(),
            salt: salt,
        })
    }

    // pub fn validate(&self, password: &str) -> bool {
    //     let credential = Credential::new(self, password);
    // }
}

/// A credential is generated from a user's registration and thier password.
pub struct Credential(Key<Aes256SivAead>);

impl Credential {
    pub fn new(registration: &Registration, password: &str) -> Result<Self, ()> {
        let argon2 = Argon2::default();
        assert_eq!(CREDENTIAL_SIZE, Aes256SivAead::key_size());
        let mut output_key_material = [0u8; CREDENTIAL_SIZE];
        argon2
            .hash_password_into(
                password.as_bytes(),
                &registration.salt,
                &mut output_key_material,
            )
            .map_err(|_| ())?;

        Ok(Credential(Key::<Aes256SivAead>::clone_from_slice(
            &output_key_material,
        )))
    }

    pub fn key(&self) -> &Key<Aes256SivAead> {
        &self.0
    }
}

#[test]
fn new_credential() {
    let registration = Registration::new("user1").expect("error registering user");
    let credential =
        Credential::new(&registration, "user1password").expect("error generating credential");

    assert_eq!(CREDENTIAL_SIZE, credential.0.len());
}

const NONCE_SIZE: usize = 16;

pub struct Encrypted {
    data: Vec<u8>,
    nonce: Vec<u8>,
}

impl Credential {
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
}

#[test]
fn encrypt_decrypt_test() {
    use crate::prelude::Registration;

    let registration = Registration::new("user1").expect("error registering user");
    let credential =
        Credential::new(&registration, "user1password").expect("error generating credentials");

    let plaintext = b"this is a secret";
    let encrypted = credential.encrypt(plaintext).expect("error encrypting");
    let decrypted = credential.decrypt(&encrypted).expect("error dencrypting");

    assert_eq!(plaintext, &decrypted[..]);
}
