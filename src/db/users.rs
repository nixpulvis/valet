use crate::db::{Database, Error};
use sqlx::prelude::FromRow;

#[derive(FromRow, Debug, PartialEq, Eq)]
pub(crate) struct SqlUser {
    pub(crate) username: String,
    pub(crate) salt: Vec<u8>,
    pub(crate) validation_data: Vec<u8>,
    pub(crate) validation_nonce: Vec<u8>,
}

impl SqlUser {
    #[must_use]
    pub(crate) async fn insert(&self, db: &Database) -> Result<Self, Error> {
        sqlx::query_as(
            r"
            INSERT INTO users (username, salt, validation_data, validation_nonce)
            VALUES (?, ?, ?, ?)
            RETURNING username, salt, validation_data, validation_nonce
            ",
        )
        .bind(&self.username)
        .bind(&self.salt[..])
        .bind(&self.validation_data[..])
        .bind(&self.validation_nonce[..])
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    #[must_use]
    pub(crate) async fn select(db: &Database, username: &str) -> Result<SqlUser, Error> {
        sqlx::query_as(
            r"
            SELECT username, salt, validation_data, validation_nonce
            FROM users
            WHERE username = ?
            ",
        )
        .bind(username)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[tokio::test]
    async fn insert() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = SqlUser {
            username: "alice".into(),
            salt: b"low sodium".into(),
            validation_data: b"test".into(),
            validation_nonce: b"not".into(),
        };
        let inserted = user.insert(&db).await.expect("failed to insert user");
        assert_eq!(inserted, user);
    }

    #[tokio::test]
    async fn select() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = SqlUser {
            username: "alice".into(),
            salt: b"low sodium".into(),
            validation_data: b"test".into(),
            validation_nonce: b"not".into(),
        };
        user.insert(&db).await.expect("failed to insert user");
        let selected = SqlUser::select(&db, &user.username)
            .await
            .expect("failed to create user");
        assert_eq!(selected, user);
    }
}
