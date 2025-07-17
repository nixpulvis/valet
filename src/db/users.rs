use crate::db::{Database, Error};
use crate::encrypt::{Encrypted, SALT_SIZE};
use crate::user::User;

pub struct Users;

impl Users {
    /// TODO
    ///
    /// Takes ownership of `password` because the returned `User`
    /// has it's key and we shouldn't be reusing the password after
    /// this point.
    pub async fn register(db: &Database, username: &str, password: String) -> Result<User, Error> {
        let user = User::new(username, &password)?;

        let (username, salt, data, nonce): (String, Vec<u8>, Vec<u8>, Vec<u8>) = sqlx::query_as(
            r"
            INSERT INTO users (username, salt, validation_data, validation_nonce)
            VALUES (?, ?, ?, ?)
            RETURNING username, salt, validation_data, validation_nonce
            ",
        )
        .bind(&user.username)
        .bind(&user.salt[..])
        .bind(&user.validation.data[..])
        .bind(&user.validation.nonce[..])
        .fetch_one(db.pool())
        .await?;

        let salt: [u8; SALT_SIZE] = salt.try_into().map_err(|_| Error::SaltError)?;
        let validation = Encrypted { data, nonce };
        Ok(User::load(username, &password, salt, validation)?)
    }

    /// TODO
    ///
    /// Takes ownership of `password` because the returned `User`
    /// has it's key and we shouldn't be reusing the password after
    /// this point.
    pub async fn get(db: &Database, username: &str, password: String) -> Result<User, Error> {
        let (username, salt, data, nonce): (String, Vec<u8>, Vec<u8>, Vec<u8>) = sqlx::query_as(
            r"
            SELECT username, salt, validation_data, validation_nonce
            FROM users
            WHERE username = ?
            ",
        )
        .bind(username)
        .fetch_one(db.pool())
        .await?;

        let salt: [u8; SALT_SIZE] = salt.try_into().map_err(|_| Error::SaltError)?;
        let validation = Encrypted { data, nonce };
        Ok(User::load(username, &password, salt, validation)?)
    }
}
