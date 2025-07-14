use crate::encrypt::{self, Encrypted, Key, SALT_SIZE};

const VALIDATION: &[u8] = b"VALID";

/// Usernames and the salt for their password are store in a database.
///
/// A short validation string is also saved which is used to authenticate the
/// user.
pub struct User {
    pub username: String,
    pub salt: [u8; SALT_SIZE],
    pub validation: Encrypted,
    key: Key,
}

impl User {
    pub fn key(&self) -> &Key {
        &self.key
    }

    pub fn new(username: &str, password: &str) -> Result<Self, Error> {
        let salt = Key::generate_salt()?;
        let key = Key::new(password, &salt)?;
        let validation = key.encrypt(VALIDATION)?;
        Ok(User {
            username: username.into(),
            salt,
            validation,
            key,
        })
    }

    pub fn load(
        username: String,
        password: &str,
        salt: [u8; SALT_SIZE],
        validation: Encrypted,
    ) -> Result<Self, Error> {
        let user = User {
            username,
            salt,
            validation,
            key: Key::new(password, &salt[..])?,
        };
        if user.validate() {
            Ok(user)
        } else {
            Err(Error::InvalidPassword)
        }
    }

    pub fn validate(&self) -> bool {
        if let Ok(v) = self.key().decrypt(&self.validation) {
            v == VALIDATION // This should never be false.
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidPassword,
    Encrypt(encrypt::Error),
}

impl From<encrypt::Error> for Error {
    fn from(err: encrypt::Error) -> Self {
        Error::Encrypt(err)
    }
}
