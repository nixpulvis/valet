use crate::db::Database;
use crate::encrypt::Encrypted;
use sqlx::Error;

pub struct Lots;

impl Lots {
    pub async fn create(db: &Database, username: &str, encrypted: &Encrypted) -> Result<(), Error> {
        sqlx::query(
            r"
            INSERT OR REPLACE INTO lots (username, uuid, data, nonce)
            VALUES (?, ?, ?, ?)
            ",
        )
        .bind(username)
        .bind(1)
        .bind(&encrypted.data[..])
        .bind(&encrypted.nonce[..])
        .execute(db.pool())
        .await?;

        Ok(())
    }

    // TODO: Pass a UUID argument and use that for the WHERE.
    pub async fn encrypted(db: &Database, username: &str) -> Result<Encrypted, Error> {
        let (data, nonce): (Vec<u8>, Vec<u8>) = sqlx::query_as(
            r"
            SELECT data, nonce
            FROM lots
            WHERE username = ?
            ",
        )
        .bind(username)
        .fetch_one(db.pool())
        .await?;

        Ok(Encrypted { data, nonce })
    }
}
