use sqlx::prelude::FromRow;

/// Represents a row in the `user_lot_keys` table.
#[derive(FromRow, Debug, PartialEq, Eq)]
pub(crate) struct SqlUserLotKey {
    pub(crate) username: String,
    pub(crate) lot: String,
    pub(crate) data: Vec<u8>,
    pub(crate) nonce: Vec<u8>,
}

use crate::db::{Database, Error};

impl SqlUserLotKey {
    /// Upsert a user's encrypted lot key.
    #[must_use]
    pub(crate) async fn upsert(&self, db: &Database) -> Result<Self, Error> {
        sqlx::query_as(
            r"
            INSERT INTO user_lot_keys (username, lot, data, nonce)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(username, lot) DO UPDATE SET
                data = excluded.data,
                nonce = excluded.nonce
            RETURNING username, lot, data, nonce
            ",
        )
        .bind(&self.username)
        .bind(&self.lot)
        .bind(&self.data[..])
        .bind(&self.nonce[..])
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    /// Select a user's encrypted lot key by username and lot.
    #[must_use]
    pub(crate) async fn select(db: &Database, username: &str, lot: &str) -> Result<Self, Error> {
        sqlx::query_as(
            r"
            SELECT username, lot, data, nonce
            FROM user_lot_keys
            WHERE username = ? AND lot = ?
            ",
        )
        .bind(username)
        .bind(lot)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, lots::SqlLot, users::SqlUser};

    #[tokio::test]
    async fn upsert_and_select() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");

        let user = SqlUser {
            username: "alice".into(),
            salt: b"salty".to_vec(),
            validation_data: b"valdata".to_vec(),
            validation_nonce: b"valnonce".to_vec(),
        };
        user.insert(&db).await.expect("failed to insert user");

        let lot = SqlLot {
            uuid: "lot-uuid-1".into(),
            name: "Lot 1".into(),
        };
        lot.upsert(&db).await.expect("failed to insert lot");

        let key = SqlUserLotKey {
            username: user.username.clone(),
            lot: lot.uuid.clone(),
            data: b"userlotkey".to_vec(),
            nonce: b"userlotnonce".to_vec(),
        };
        let inserted = key
            .upsert(&db)
            .await
            .expect("failed to upsert user_lot_key");
        assert_eq!(inserted, key);

        // Select and check
        let selected = SqlUserLotKey::select(&db, &user.username, &lot.uuid)
            .await
            .expect("failed to select user_lot_key");
        assert_eq!(selected, key);
    }
}
