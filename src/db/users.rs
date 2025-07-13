use crate::db::Database;
use crate::user::{Encrypted, Registration};
use sqlx::Error;

pub struct Users;

impl Users {
    pub async fn create(db: &Database, registration: &Registration) -> Result<(), Error> {
        sqlx::query(
            r"
            INSERT INTO users (username, salt, validation_data, validation_nonce)
            VALUES (?, ?, ?, ?)
            ",
        )
        .bind(&registration.username)
        .bind(&registration.salt[..])
        .bind(&registration.validation.data[..])
        .bind(&registration.validation.nonce[..])
        .execute(db.pool())
        .await?;

        Ok(())
    }

    pub async fn registration(db: &Database, username: &str) -> Result<Registration, Error> {
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

        let validation = Encrypted { data, nonce };
        let salt = salt.try_into().expect("TODO: Need our own error type");
        Ok(Registration {
            username,
            salt,
            validation,
        })
    }
}
