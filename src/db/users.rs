use crate::db::{Database, Error};
use crate::encrypt::{Encrypted, SALT_SIZE};
use crate::user::{self, User};

pub struct Users;

impl Users {
    pub async fn register(db: &Database, username: &str, password: &str) -> Result<(), Error> {
        let user = User::new(username, password)?;

        sqlx::query(
            r"
            INSERT INTO users (username, salt, validation_data, validation_nonce)
            VALUES (?, ?, ?, ?)
            ",
        )
        .bind(&user.username)
        .bind(&user.salt[..])
        .bind(&user.validation.data[..])
        .bind(&user.validation.nonce[..])
        .execute(db.pool())
        .await?;

        Ok(())
    }

    pub async fn get(db: &Database, username: &str, password: &str) -> Result<User, Error> {
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

        let salt: [u8; SALT_SIZE] = salt.try_into().expect("TODO: Need our own error type");
        let validation = Encrypted { data, nonce };
        Ok(User::load(username, password, salt, validation)?)
    }
}
