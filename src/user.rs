use std::{fmt::Debug, fmt::Formatter, ops::Deref};

use crate::{
    db::{self, Database},
    encrypt::{self, Encrypted, Key, SALT_SIZE},
    lot::{self, Lot},
};

const VALIDATION: &[u8] = b"VALID";

/// Usernames and the salt for their password are store in a database.
///
/// A short validation string is also saved which is used to authenticate the
/// user.
#[derive(PartialEq, Eq)]
pub struct User {
    pub username: String,
    salt: [u8; SALT_SIZE],
    validation: Encrypted,
    key: UserKey,
}

impl User {
    // TODO: Zeroize password
    pub fn new(username: &str, password: String) -> Result<Self, Error> {
        let salt = Key::generate_salt();
        let key = UserKey(Key::from_password(password, &salt)?);
        let validation = key.encrypt(VALIDATION)?;
        Ok(User {
            username: username.into(),
            salt,
            validation,
            key,
        })
    }

    pub fn key(&self) -> &UserKey {
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

    // TODO: Zeroize password
    pub async fn load(db: &Database, username: &str, password: String) -> Result<Self, Error> {
        let sql_user = db::users::SqlUser::select(&db, &username).await?;
        let key = UserKey(Key::from_password(password, &sql_user.salt[..])?);
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

#[derive(PartialEq, Eq)]
pub struct UserKey(pub(crate) Key);

impl Deref for UserKey {
    type Target = Key;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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
        user.validation = user.key().encrypt(b"invalid").expect("failed to encrypt");
        assert!(!user.validate());
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

        let password = "password".to_string();
        let user = User::new("alice", password.clone())
            .expect("failed to create user")
            .register(&db)
            .await
            .expect("failed to register user");

        let loaded = User::load(&db, &user.username, password)
            .await
            .expect("failed to load user");

        assert_eq!(user, loaded);
    }
}
