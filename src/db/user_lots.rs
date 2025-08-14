use sqlx::prelude::FromRow;

/// Represents a row in the `user_lots` table.
#[derive(FromRow, Debug, PartialEq, Eq)]
pub(crate) struct SqlUserLot {
    pub(crate) username: String,
    pub(crate) lot: String,
    pub(crate) name: String,
    pub(crate) data: Vec<u8>,
    pub(crate) nonce: Vec<u8>,
}

use crate::db::{Database, Error};

impl SqlUserLot {
    /// Upsert a user's encrypted lot key.
    #[must_use]
    pub(crate) async fn upsert(&self, db: &Database) -> Result<Self, Error> {
        sqlx::query_as(
            r"
            INSERT INTO user_lots (username, lot, name, data, nonce)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(username, lot) DO UPDATE SET
                name = excluded.name,
                data = excluded.data,
                nonce = excluded.nonce
            RETURNING username, lot, name, data, nonce
            ",
        )
        .bind(&self.username)
        .bind(&self.lot)
        .bind(&self.name)
        .bind(&self.data[..])
        .bind(&self.nonce[..])
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    /// Select a user's encrypted lot key by lot uuid.
    #[must_use]
    #[cfg(test)]
    pub(crate) async fn select(db: &Database, username: &str, lot: &str) -> Result<Self, Error> {
        sqlx::query_as(
            r"
            SELECT username, lot, name, data, nonce
            FROM user_lots
            WHERE username = ? AND lot = ?
            ",
        )
        .bind(username)
        .bind(lot)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    /// Select a user's encrypted lot by name.
    #[must_use]
    pub(crate) async fn select_by_name(
        db: &Database,
        username: &str,
        name: &str,
    ) -> Result<Self, Error> {
        sqlx::query_as(
            r"
            SELECT username, lot, name, data, nonce
            FROM user_lots
            WHERE username = ? AND name = ?
            ",
        )
        .bind(username)
        .bind(name)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    /// Select all of a user's encrypted lot keys.
    #[must_use]
    pub(crate) async fn select_all(db: &Database, username: &str) -> Result<Vec<Self>, Error> {
        sqlx::query_as(
            r"
            SELECT username, lot, name, data, nonce
            FROM user_lots
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

        let lot_1 = SqlLot { uuid: "1".into() };
        lot_1.insert(&db).await.expect("failed to insert lot");

        let ul_a = SqlUserLot {
            username: user.username.clone(),
            lot: lot_1.uuid.clone(),
            name: "Lot A".into(),
            data: b"userlotakey".to_vec(),
            nonce: b"userlotanonce".to_vec(),
        };
        let inserted = ul_a.upsert(&db).await.expect("failed to upsert user_lot");
        assert_eq!(inserted, ul_a);

        // Select and check
        let selected = SqlUserLot::select(&db, &user.username, &lot_1.uuid)
            .await
            .expect("failed to select user_lot");
        assert_eq!(selected, ul_a);

        let lot_2 = SqlLot { uuid: "2".into() };
        lot_2.insert(&db).await.expect("failed to insert lot");

        let ul_b = SqlUserLot {
            username: user.username.clone(),
            lot: lot_2.uuid.clone(),
            name: "Lot B".into(),
            data: b"userlotbdata".to_vec(),
            nonce: b"userlotbnonce".to_vec(),
        };
        let inserted = ul_b.upsert(&db).await.expect("failed to upsert user_lot");
        assert_eq!(inserted, ul_b);

        // Select all and check
        let selected = SqlUserLot::select_all(&db, &user.username)
            .await
            .expect("failed to select user_lot");
        assert_eq!(selected, vec![ul_a, ul_b]);
    }
}
