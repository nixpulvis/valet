use std::{fmt::Debug, fmt::Formatter};

use crate::{
    db::{self, Database},
    encrypt::{self, Encrypted, Key, Password, SALT_SIZE},
    lot::{self, Lot},
};

const VALIDATION: &[u8] = b"VALID";

/// A user of valet, who is uniquely identified by username.
///
/// As is standard practice with password handling, the user's password provided
/// to either [`User::new`] or [`User::load`] is never saved anywhere and is
/// kept in memory for as little time as possible.
///
/// The user's password (and a random saved "salt") is used to derive the "user
/// key", i.e. [`Key<User>`]. To generate this key we use a common Key
/// Derivation Function (KDF), namely [`argon2`]. Each user record saves it's
/// random salt value in order to prevent users with the same password from
/// getting the same key, and thus opening up the scheme to ["rainbow table"][1]
/// attacks.
///
/// In addition to the salt, each user also stores a short encrypted validation
/// string which is used to authenticate the user. Simply being able
/// to decrpyt the string is enough to verify the user, since we use
/// ["Authenticated Encryption"][2] (the AE in AEAD).
///
/// [1]: https://en.wikipedia.org/wiki/Rainbow_table
/// [2]: https://en.wikipedia.org/wiki/Authenticated_encryption
#[derive(PartialEq, Eq)]
pub struct User {
    username: String,
    salt: [u8; SALT_SIZE],
    validation: Encrypted,
    key: Key<Self>,
}

impl User {
    pub fn new(username: &str, password: Password) -> Result<Self, Error> {
        let salt = Key::<Self>::generate_salt();
        let key = Key::from_password(password, &salt)?;
        let validation = key.encrypt(VALIDATION)?;
        Ok(User {
            username: username.into(),
            salt,
            validation,
            key,
        })
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn key(&self) -> &Key<Self> {
        &self.key
    }

    pub fn validate(&self) -> bool {
        if let Ok(v) = self.key().decrypt(&self.validation) {
            v == VALIDATION // This should never be false.
        } else {
            false
        }
    }

    // TODO: Return type, insert or update info.
    pub async fn register(self, db: &Database) -> Result<Self, Error> {
        let sql_user = db::users::SqlUser {
            username: self.username.clone(),
            salt: self.salt.to_vec(),
            validation_data: self.validation.data.clone(),
            validation_nonce: self.validation.nonce.clone(),
        };
        sql_user.insert(&db).await?;
        Ok(self)
    }

    pub async fn load(db: &Database, username: &str, password: Password) -> Result<Self, Error> {
        let sql_user = db::users::SqlUser::select(&db, &username).await?;
        let key = Key::from_password(password, &sql_user.salt[..])?;
        let validation = Encrypted {
            data: sql_user.validation_data,
            nonce: sql_user.validation_nonce,
        };
        let user = User {
            username: sql_user.username,
            salt: sql_user.salt.try_into().map_err(|_| Error::SaltError)?,
            validation,
            key,
        };
        if user.validate() {
            Ok(user)
        } else {
            Err(Error::Invalid)
        }
    }

    // TODO: Use user_lot_keys join table
    pub async fn lots(&self, db: &Database) -> Result<Vec<Lot>, Error> {
        Ok(Lot::load_all(&db, self).await?)
    }
}

impl Debug for User {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("username", &self.username)
            .finish()
    }
}

// fn load_and_clobber_user(sql_user: SqlUser, password: String) -> Result<User, Error> {
//     let salt: [u8; SALT_SIZE] = sql_user.salt.try_into().map_err(|_| Error::SaltError)?;
//     let validation = Encrypted {
//         data: sql_user.validation_data,
//         nonce: sql_user.validation_nonce,
//     };
//     let user = User::load(sql_user.username, &password, salt, validation)?;
//     unsafe {
//         Self::clobber_password(password);
//     }
//     Ok(user)
// }

// // We make no attempt to create valid UTF-8 strings here, this is
// // just to protect the memory.
// unsafe fn clobber_password(mut password: String) {
//     unsafe {
//         let bytes = password.as_mut_vec();
//         if let Ok(mut rng) = OsRng::new()
//             && let Ok(_) = rng.try_fill(&mut bytes[..])
//         {
//         } else {
//             panic!("Critical failure.")
//         }
//     }
// }

#[derive(Debug)]
pub enum Error {
    Invalid,
    SaltError,
    Encrypt(encrypt::Error),
    Database(db::Error),
    Lot(lot::Error),
}

impl From<encrypt::Error> for Error {
    fn from(err: encrypt::Error) -> Self {
        Error::Encrypt(err)
    }
}

impl From<db::Error> for Error {
    fn from(err: db::Error) -> Self {
        Error::Database(err)
    }
}

impl From<lot::Error> for Error {
    fn from(err: lot::Error) -> Self {
        Error::Lot(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::time::{Duration, Instant};

    #[test]
    fn new_validate() {
        let user = User::new("alice", "password".into()).expect("failed to create user");
        assert!(user.validate());
    }

    #[test]
    fn invalid() {
        let mut user = User::new("alice", "password".into()).expect("failed to create user");
        let imposter = User::new("charlie", "password".into()).expect("failed to create user");
        user.validation = imposter
            .key()
            .encrypt(VALIDATION)
            .expect("failed to encrypt");
        assert!(!user.validate());
    }

    #[test]
    fn new_is_slow() {
        let start = Instant::now();
        User::new("alice", "password".into()).expect("failed to create user");
        let duration = start.elapsed();
        assert!(duration > Duration::from_millis(200));
    }

    #[tokio::test]
    async fn register_load() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");

        let password = "password";
        let user = User::new("alice", password.into())
            .expect("failed to create user")
            .register(&db)
            .await
            .expect("failed to register user");

        let loaded = User::load(&db, &user.username, password.into())
            .await
            .expect("failed to load user");

        assert_eq!(user, loaded);
    }
}
