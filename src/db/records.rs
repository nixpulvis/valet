use crate::db::Database;
use crate::db::Error;
use sqlx::prelude::FromRow;

#[derive(FromRow, Debug, PartialEq, Eq)]
pub(crate) struct SqlRecord {
    pub(crate) lot: String,
    pub(crate) uuid: String,
    pub(crate) data: Vec<u8>,
    pub(crate) nonce: Vec<u8>,
}

impl SqlRecord {
    pub(crate) async fn select_by_lot(db: &Database, lot: &str) -> Result<Vec<SqlRecord>, Error> {
        sqlx::query_as(
            r"
            SELECT lot, uuid, data, nonce
            FROM records
            WHERE lot = ?
            ",
        )
        .bind(lot)
        .fetch_all(db.pool())
        .await
        .map_err(|e| e.into())
    }

    /// Inserts or updates a record and it's encrypted data.
    ///
    /// This function, unlike [`db::lots::SqlLot::upsert`] will update the
    /// record's encrypted data.
    pub(crate) async fn upsert(&self, db: &Database) -> Result<SqlRecord, Error> {
        sqlx::query_as(
            r"
            INSERT INTO records (lot, uuid, data, nonce)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(uuid) DO UPDATE SET
                lot = excluded.lot,
                data = excluded.data,
                nonce = excluded.nonce
            RETURNING lot, uuid, data, nonce
            ",
        )
        .bind(&self.lot)
        .bind(&self.uuid)
        .bind(&self.data[..])
        .bind(&self.nonce[..])
        .fetch_one(db.pool())
        .await
        .map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, lots::SqlLot};

    #[tokio::test]
    async fn upsert() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = SqlLot {
            uuid: "123".into(),
            name: "a lot".into(),
        };
        lot.upsert(&db).await.expect("failed to insert lot");
        let record = SqlRecord {
            lot: lot.uuid.clone(),
            uuid: "a_record".into(),
            data: b"encrypted".into(),
            nonce: b"something".into(),
        };
        let upserted = record.upsert(&db).await.expect("failed to upsert record");
        assert_eq!(upserted, record);
    }

    #[tokio::test]
    async fn select_by_lot() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = SqlLot {
            uuid: "123".into(),
            name: "a lot".into(),
        };
        lot.upsert(&db).await.expect("failed to insert lot");
        let record_a = SqlRecord {
            lot: lot.uuid.clone(),
            uuid: "a_record".into(),
            data: b"encrypted".into(),
            nonce: b"something".into(),
        };
        record_a.upsert(&db).await.expect("failed to upsert record");
        let record_b = SqlRecord {
            lot: lot.uuid.clone(),
            uuid: "b_record".into(),
            data: b"encrypted".into(),
            nonce: b"something".into(),
        };
        record_b.upsert(&db).await.expect("failed to upsert record");
        let selected = SqlRecord::select_by_lot(&db, &lot.uuid)
            .await
            .expect("failed to get uuid");
        assert_eq!(selected, vec![record_a, record_b]);
    }
}
