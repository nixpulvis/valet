use crate::db::{Database, Error};
use sqlx::prelude::FromRow;

#[derive(FromRow, Debug, PartialEq, Eq)]
pub(crate) struct SqlLot {
    pub(crate) uuid: String,
    pub(crate) name: String,
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
            INSERT INTO lots (uuid, name)
            VALUES (?, ?)
            ON CONFLICT(uuid) DO UPDATE SET
                name = excluded.name
            RETURNING uuid, name
            "#,
        )
        .bind(&self.uuid)
        .bind(&self.name)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    #[must_use]
    pub async fn select(db: &Database, uuid: &str) -> Result<SqlLot, Error> {
        sqlx::query_as(
            r"
            SELECT uuid, name
            FROM lots
            WHERE uuid = ?
            ",
        )
        .bind(uuid)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    #[must_use]
    pub async fn select_by_name(db: &Database, name: &str) -> Result<SqlLot, Error> {
        sqlx::query_as(
            r"
            SELECT uuid, name
            FROM lots
            WHERE name = ?
            ",
        )
        .bind(name)
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
    async fn upsert() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = SqlLot {
            uuid: "123".into(),
            name: "a lot".into(),
        };
        let mut inserted = lot.upsert(&db).await.expect("failed to insert lot");
        assert_eq!(inserted, lot);
        inserted.name = "b lot".into();
        let upserted = inserted.upsert(&db).await.expect("failed to insert lot");
        assert_eq!("b lot", upserted.name);
    }

    #[tokio::test]
    async fn select() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = SqlLot {
            uuid: "123".into(),
            name: "a lot".into(),
        };
        lot.upsert(&db).await.expect("failed to insert lot");
        let selected = SqlLot::select(&db, &lot.uuid)
            .await
            .expect("failed to get name");
        assert_eq!(selected, lot);
    }

    #[tokio::test]
    async fn select_by_name() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = SqlLot {
            uuid: "123".into(),
            name: "a lot".into(),
        };
        lot.upsert(&db).await.expect("failed to insert lot");
        let selected = SqlLot::select_by_name(&db, &lot.name)
            .await
            .expect("failed to get name");
        assert_eq!(selected, lot);
    }
}
