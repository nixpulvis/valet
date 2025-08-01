use crate::db::{Database, Error};
use sqlx::prelude::FromRow;

#[derive(FromRow, Debug, PartialEq, Eq)]
pub(crate) struct SqlLot {
    pub(crate) uuid: String,
    pub(crate) name: String,
    pub(crate) key_data: Vec<u8>,
    pub(crate) key_nonce: Vec<u8>,
}

impl SqlLot {
    /// Insert or update a lot.
    ///
    /// This function will only update the lots name if it already exists. The
    /// UUID and key information will remain the same. See [`update_key`] for
    /// details if you need to change a lots key.
    #[must_use]
    pub async fn upsert(&self, db: &Database) -> Result<SqlLot, Error> {
        sqlx::query_as(
            r#"
            INSERT INTO lots (uuid, name, key_data, key_nonce)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(uuid) DO UPDATE SET
                name = excluded.name
            RETURNING uuid, name, key_data, key_nonce
            "#,
        )
        .bind(&self.uuid)
        .bind(&self.name)
        .bind(&self.key_data)
        .bind(&self.key_nonce)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    #[must_use]
    #[allow(unused)]
    pub async fn update_key(&self, db: &Database) {
        unimplemented!()
    }

    #[must_use]
    pub async fn select_by_name(db: &Database, name: &str) -> Result<SqlLot, Error> {
        sqlx::query_as(
            r"
            SELECT uuid, name, key_data, key_nonce
            FROM lots
            WHERE name = ?
            ",
        )
        .bind(name)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    // TODO: Use user_lot_keys to load.
    // #[must_use]
    // pub async fn select_by_user(db: &Database, username: &str) -> Result<Vec<SqlLot>, Error> {
    //     sqlx::query_as(
    //         r"
    //         SELECT username, uuid
    //         FROM lots
    //         WHERE username = ?
    //         ",
    //     )
    //     .bind(username)
    //     .fetch_all(db.pool())
    //     .await
    //     .map_err(|e| e.into())
    // }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    #[tokio::test]
    async fn upsert() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = SqlLot {
            uuid: "123".into(),
            name: "a lot".into(),
            key_data: b"keydata".to_vec(),
            key_nonce: b"keynonce".to_vec(),
        };
        let mut inserted = lot.upsert(&db).await.expect("failed to insert lot");
        assert_eq!(inserted, lot);
        inserted.name = "b lot".into();
        let upserted = inserted.upsert(&db).await.expect("failed to insert lot");
        assert_eq!("b lot", upserted.name);
    }

    #[tokio::test]
    async fn select_by_name() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = SqlLot {
            uuid: "123".into(),
            name: "a lot".into(),
            key_data: b"keydata".to_vec(),
            key_nonce: b"keynonce".to_vec(),
        };
        lot.upsert(&db).await.expect("failed to insert lot");
        let selected = SqlLot::select_by_name(&db, &lot.name)
            .await
            .expect("failed to get name");
        assert_eq!(selected, lot);
    }

    // #[tokio::test]
    // async fn select_by_user() {
    //     let db = Database::new("sqlite://:memory:")
    //         .await
    //         .expect("failed to create database");
    // }
}
