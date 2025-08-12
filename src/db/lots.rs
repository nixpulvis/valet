use crate::db::{Database, Error};
use sqlx::prelude::FromRow;

#[derive(FromRow, Debug, PartialEq, Eq)]
pub(crate) struct SqlLot {
    pub(crate) uuid: String,
}

impl SqlLot {
    /// Insert or update a lot.
    ///
    /// Currently there's nothing to update.
    #[must_use]
    pub async fn insert(&self, db: &Database) -> Result<SqlLot, Error> {
        sqlx::query_as(
            r#"
            INSERT INTO lots (uuid)
            VALUES (?)
            ON CONFLICT(uuid) DO NOTHING
            RETURNING uuid
            "#,
        )
        .bind(&self.uuid)
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }

    #[must_use]
    pub async fn select(db: &Database, uuid: &str) -> Result<Option<SqlLot>, Error> {
        sqlx::query_as(
            r"
            SELECT uuid
            FROM lots
            WHERE uuid = ?
            ",
        )
        .bind(uuid)
        .fetch_optional(db.pool())
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
        let lot = SqlLot { uuid: "123".into() };
        let inserted = lot.insert(&db).await.expect("failed to insert lot");
        assert_eq!(inserted, lot);
    }

    #[tokio::test]
    async fn select() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = SqlLot { uuid: "123".into() };
        lot.insert(&db).await.expect("failed to insert lot");
        let selected = SqlLot::select(&db, &lot.uuid)
            .await
            .expect("failed to select lot");
        assert_eq!(selected.unwrap(), lot);
    }
}
