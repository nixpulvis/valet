use crate::db::{Database, Error};
use sqlx::prelude::FromRow;

#[derive(FromRow, Debug, PartialEq, Eq)]
pub(crate) struct SqlLot {
    pub(crate) uuid: String,
    pub(crate) name: String,
}

impl SqlLot {
    #[must_use]
    pub async fn insert(&self, db: &Database) -> Result<SqlLot, Error> {
        sqlx::query_as(
            r"
            INSERT INTO lots (uuid, name)
            VALUES (?, ?)
            RETURNING uuid, name
            ",
        )
        .bind(&self.uuid)
        .bind(&self.name)
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
    async fn insert() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = SqlLot {
            uuid: "123".into(),
            name: "a lot".into(),
        };
        let inserted = lot.insert(&db).await.expect("failed to insert lot");
        assert_eq!(inserted, lot);
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
        lot.insert(&db).await.expect("failed to insert lot");
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
