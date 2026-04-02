use crate::{
    db::{self, Database},
    encrypt::{self, Encrypted, Key, SALT_SIZE},
    lot::{self, Lot},
    password::Password,
};
use sea_orm::{ActiveValue::Set, QuerySelect, entity::prelude::*};
use std::{fmt::Debug, fmt::Formatter};

const VALIDATION: &[u8] = b"VALID";

/// A user of valet, who is uniquely identified by username.
///
/// As is standard practice with password handling, the user's password provided
/// to either [`User::new`] or [`User::load`] is never saved anywhere and is
/// kept in memory for as little time as possible.
///
/// The user's password (and a random saved "salt") is used to derive the _user
/// key_, i.e. [`Key<User>`]. To generate this key we use a common Key
/// Derivation Function (KDF), namely [`argon2`][argon2]. Each user record saves
/// it's random salt value in order to prevent users with the same password from
/// getting the same key, and thus opening up the scheme to ["rainbow table"][1]
/// attacks.
///
/// In addition to the salt, each user also stores a short encrypted validation
/// string which is used to authenticate the user. Simply being able
/// to decrypt the string is enough to verify the user, since we use
/// ["Authenticated Encryption"][2] (the AE in AEAD).
///
/// [1]: https://en.wikipedia.org/wiki/Rainbow_table
/// [2]: https://en.wikipedia.org/wiki/Authenticated_encryption
/// [argon2]: https://docs.rs/argon2/latest/argon2
#[derive(PartialEq, Eq)]
pub struct User {
    username: String,
    salt: [u8; SALT_SIZE],
    validation: Encrypted,
    key: Key<Self>,
}

impl User {
    pub fn new(username: &str, password: Password) -> Result<Self, Error> {
        if !password.is_valid() {
            return Err(Error::InvalidPassword);
        }
        let salt = encrypt::generate_salt();
        let key = Key::from_password(&password, &salt)?;
        let validation = key.encrypt_with_aad(VALIDATION, User::aad(username))?;
        Ok(User {
            username: username.into(),
            salt,
            validation,
            key,
        })
    }

    pub async fn register(self, db: &Database) -> Result<Self, Error> {
        let active = self::orm::ActiveModel {
            username: Set(self.username.clone()),
            salt: Set(self.salt.to_vec()),
            validation_data: Set(self.validation.data.clone()),
            validation_nonce: Set(self.validation.nonce.clone()),
        };
        self::orm::Entity::insert(active)
            .exec(db.connection())
            .await?;
        Ok(self)
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn key(&self) -> &Key<Self> {
        &self.key
    }

    pub fn validate(&self) -> bool {
        if let Ok(v) = self
            .key()
            .decrypt_with_aad(&self.validation, User::aad(&self.username))
        {
            v == VALIDATION // This should never be false.
        } else {
            false
        }
    }

    pub async fn load<'a>(
        db: &'a Database,
        username: &'a str,
        password: Password,
    ) -> Result<Self, Error> {
        let model = self::orm::Entity::find_by_id(username.to_owned())
            .one(db.connection())
            .await?
            .ok_or(Error::NotFound)?;

        let key = Key::from_password(&password, &model.salt[..])?;
        let validation = Encrypted {
            data: model.validation_data,
            nonce: model.validation_nonce,
        };
        let user = User {
            username: model.username,
            salt: model.salt.try_into().map_err(|_| Error::SaltError)?,
            validation,
            key,
        };
        if user.validate() {
            Ok(user)
        } else {
            Err(Error::Invalid)
        }
    }

    /// Load all of this user's lots.
    ///
    /// This function as well as [`Lot::load`] and [`Lot::load_all`] utilize the
    /// `user_lots` SQL table to determine lot membership as well as to access
    /// the user encrypted lot key for each lot.
    ///
    /// For more information, see [`Lot`].
    pub async fn lots(&self, db: &Database) -> Result<Vec<Lot>, Error> {
        Ok(Lot::load_all(&db, self).await?)
    }

    /// Return the list of registered usernames from the database.
    pub async fn list(db: &Database) -> Result<Vec<String>, Error> {
        self::orm::Entity::find()
            .select_only()
            .column(self::orm::Column::Username)
            .into_tuple::<String>()
            .all(db.connection())
            .await
            .map_err(Into::into)
    }

    fn aad(username: &str) -> &[u8] {
        username.as_bytes()
    }
}

impl Debug for User {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("username", &self.username)
            .finish()
    }
}

#[derive(Debug)]
pub enum Error {
    NotFound,
    Invalid,
    InvalidPassword,
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

impl From<sea_orm::DbErr> for Error {
    fn from(err: sea_orm::DbErr) -> Self {
        Error::Database(err.into())
    }
}

impl From<lot::Error> for Error {
    fn from(err: lot::Error) -> Self {
        Error::Lot(err)
    }
}

#[cfg(feature = "orm")]
pub mod orm;
#[cfg(not(feature = "orm"))]
pub(crate) mod orm;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::time::{Duration, Instant};

    #[test]
    fn new_validate() {
        let user =
            User::new("alice", "password".try_into().unwrap()).expect("failed to create user");
        assert!(user.validate());
    }

    #[test]
    fn invalid() {
        let mut user =
            User::new("alice", "password".try_into().unwrap()).expect("failed to create user");
        let imposter =
            User::new("charlie", "password".try_into().unwrap()).expect("failed to create user");
        user.validation = imposter
            .key()
            .encrypt(VALIDATION)
            .expect("failed to encrypt");
        assert!(!user.validate());
    }

    #[test]
    fn new_is_slow() {
        let start = Instant::now();
        User::new("alice", "password".try_into().unwrap()).expect("failed to create user");
        let duration = start.elapsed();
        assert!(duration > Duration::from_millis(200));
    }

    #[test]
    fn new_rejects_invalid_password() {
        let invalid_password: Password = "short".try_into().unwrap();
        let result = User::new("alice", invalid_password);
        assert!(matches!(result, Err(Error::InvalidPassword)));
    }

    #[tokio::test]
    async fn register_load() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");

        let password: Password = "password".try_into().unwrap();
        let user = User::new("alice", Password::from(password.clone()))
            .expect("failed to create user")
            .register(&db)
            .await
            .expect("failed to register user");

        let loaded = User::load(&db, &user.username, password)
            .await
            .expect("failed to load user");

        assert_eq!(user, loaded);
    }

    #[tokio::test]
    async fn lots() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot_a = Lot::new("lot a");
        lot_a.save(&db, &user).await.expect("failed to save lot");
        let lot_b = Lot::new("lot b");
        lot_b.save(&db, &user).await.expect("failed to save lot");

        let lots = user.lots(&db).await.expect("failed to load lots");
        assert_eq!(lots, vec![lot_a, lot_b]);
    }

    #[tokio::test]
    async fn list() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        User::new("alice", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        User::new("bob", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let list = User::list(&db).await.expect("failed to list users");
        assert_eq!(["alice", "bob"], &list[..]);
    }
}
