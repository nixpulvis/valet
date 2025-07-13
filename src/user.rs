use crate::encrypt::{Credential, Encrypted, SALT_SIZE};

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

    pub fn new(username: &str, password: &str) -> Result<Self, ()> {
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
    ) -> Result<Self, ()> {
        let user = User {
            username,
            salt,
            validation,
            credential: Credential::new(password, &salt[..])
                .expect("TODO: Need our own error type"),
        };
        if user.validate().expect("TODO") {
            Ok(user)
        } else {
            Err(())
        }
    }

    pub fn validate(&self) -> Result<bool, ()> {
        if let Ok(v) = self.credential().decrypt(&self.validation) {
            Ok(v == VALIDATION)
        } else {
            Ok(false)
        }
    }
}
