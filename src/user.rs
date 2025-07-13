use crate::encrypt::{self, Credential, Encrypted, SALT_SIZE};

const VALIDATION: &[u8] = b"VALID";

/// Usernames and the salt for their password are store in a database.
pub struct User {
    pub username: String,
    pub salt: [u8; SALT_SIZE],
    pub validation: Encrypted,
    credential: Credential,
}

impl User {
    pub fn credential(&self) -> &Credential {
        &self.credential
    }

    pub fn new(username: &str, password: &str) -> Result<Self, Error> {
        let salt = Credential::generate_salt()?;
        let credential = Credential::new(password, &salt)?;
        let validation = credential.encrypt(VALIDATION)?;
        Ok(User {
            username: username.into(),
            salt,
            validation,
            credential,
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
            credential: Credential::new(password, &salt[..])?,
        };
        if user.validate() {
            Ok(user)
        } else {
            Err(Error::InvalidPassword)
        }
    }

    pub fn validate(&self) -> bool {
        if let Ok(v) = self.credential().decrypt(&self.validation) {
            v == VALIDATION
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
