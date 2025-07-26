use crate::{
    db::{self, Database},
    encrypt::{self, Encrypted, Key, SALT_SIZE},
    lot,
};

const VALIDATION: &[u8] = b"VALID";

/// Usernames and the salt for their password are store in a database.
///
/// A short validation string is also saved which is used to authenticate the
/// user.
#[derive(Debug, PartialEq, Eq)]
pub struct User {
    pub username: String,
    pub salt: [u8; SALT_SIZE],
    pub validation: Encrypted,
    key: Key,
}

impl User {
    // TODO: Zeroize password
    pub fn new(username: &str, password: String) -> Result<Self, Error> {
        let salt = Key::generate_salt()?;
        let key = Key::from_password(password, &salt)?;
        let validation = key.encrypt(VALIDATION)?;
        Ok(User {
            username: username.into(),
            salt,
            validation,
            key,
        })
    }

    pub fn key(&self) -> &Key {
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
    pub async fn register(&self, db: &Database) -> Result<(), Error> {
        let sql_user = db::users::SqlUser {
            username: self.username.clone(),
            salt: self.salt.to_vec(),
            validation_data: self.validation.data.clone(),
            validation_nonce: self.validation.nonce.clone(),
        };
        sql_user.insert(&db).await.map(|_| ()).map_err(|e| e.into())
    }

    // TODO: Zeroize password
    pub async fn load(db: &Database, username: &str, password: String) -> Result<Self, Error> {
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
    // pub async fn lots(&self, db: &Database) -> Result<Vec<Lot>, Error> {
    //     let sql_lots = db::lots::SqlLot::select_by_user(&db, &self.username).await?;
    //     let mut lots = vec![];
    //     for sql_lot in sql_lots {
    //         let mut lot = Lot {
    //             username: sql_lot.username,
    //             uuid: Uuid::from_str(&sql_lot.uuid).map_err(|e| lot::Error::Uuid(e))?,
    //             records: vec![],
    //             key: self.key.clone(),
    //         };
    //         // lot.load_records(&db).await?;
    //         lots.push(lot);
    //     }
    //     Ok(lots)
    // }
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

    #[test]
    fn new_validate() {
        let user = User::new("alice", "password".into()).expect("failed to create user");
        assert!(user.validate());
    }

    // TODO: Test key derivation time.

    #[tokio::test]
    async fn register_load() {
        let password = "password".to_string();
        let user = User::new("alice", password.clone()).expect("failed to create user");
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        user.register(&db).await.expect("failed to register user");

        let loaded = User::load(&db, &user.username, password)
            .await
            .expect("failed to load user");
        assert_eq!(user, loaded);
    }
}
