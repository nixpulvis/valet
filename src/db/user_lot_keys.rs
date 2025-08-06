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

    /// Select a user's encrypted lot key by lot uuid.
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

    /// Select all of a user's encrypted lot keys.
    #[must_use]
    pub(crate) async fn select_all(db: &Database, username: &str) -> Result<Vec<Self>, Error> {
        sqlx::query_as(
            r"
            SELECT username, lot, data, nonce
            FROM user_lot_keys
            WHERE username = ?
            ",
        )
        .bind(username)
        .fetch_all(db.pool())
        .await
        .map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, lots::SqlLot, users::SqlUser};

    #[tokio::test]
    async fn upsert_and_selects() {
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

        let lot_a = SqlLot {
            uuid: "1".into(),
            name: "Lot A".into(),
        };
        lot_a.upsert(&db).await.expect("failed to insert lot");

        let key_a = SqlUserLotKey {
            username: user.username.clone(),
            lot: lot_a.uuid.clone(),
            data: b"userlotakey".to_vec(),
            nonce: b"userlotanonce".to_vec(),
        };
        let inserted = key_a
            .upsert(&db)
            .await
            .expect("failed to upsert user_lot_key");
        assert_eq!(inserted, key_a);

        // Select and check
        let selected = SqlUserLotKey::select(&db, &user.username, &lot_a.uuid)
            .await
            .expect("failed to select user_lot_key");
        assert_eq!(selected, key_a);

        let lot_b = SqlLot {
            uuid: "2".into(),
            name: "Lot B".into(),
        };
        lot_b.upsert(&db).await.expect("failed to insert lot");

        let key_b = SqlUserLotKey {
            username: user.username.clone(),
            lot: lot_b.uuid.clone(),
            data: b"userlotbdata".to_vec(),
            nonce: b"userlotbnonce".to_vec(),
        };
        let inserted = key_b
            .upsert(&db)
            .await
            .expect("failed to upsert user_lot_key");
        assert_eq!(inserted, key_b);

        // Select all and check
        let selected = SqlUserLotKey::select_all(&db, &user.username)
            .await
            .expect("failed to select user_lot_key");
        assert_eq!(selected, vec![key_a, key_b]);
    }
}
