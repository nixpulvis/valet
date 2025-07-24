use std::str::FromStr;

use crate::{
    db::{self, Database, records::SqlRecord},
    encrypt::{Encrypted, Key},
    record::{self, Record, RecordData},
    user::User,
};
use uuid::Uuid;

/// An encrypted collection of secrets.
pub struct Lot {
    pub username: String,
    pub uuid: Uuid,
    pub records: Vec<Record>,
    pub(crate) key: Key,
}

impl Lot {
    pub fn new(user: &User) -> Self {
        let uuid = Uuid::now_v7();
        Lot {
            username: user.username.clone(),
            uuid,
            records: vec![],
            key: user.key().clone(),
        }
    }

    pub fn key(&self) -> &Key {
        &self.key
    }

    /// Save this lot and it's records to the database.
    pub async fn save(&self, db: &Database) -> Result<(), Error> {
        let sql_lot = db::lots::SqlLot {
            username: self.username.clone(),
            uuid: self.uuid.to_string(),
        };
        sql_lot.insert(&db).await?;

        for record in self.records.iter() {
            // TODO: Collect errors and report after.
            self.upsert_record(&db, &record).await?;
        }

        Ok(())
    }

    pub async fn load(db: &Database, uuid: &Uuid, user: &User) -> Result<Self, Error> {
        // TODO: We hardly need this table at all.
        db::lots::SqlLot::select_by_uuid(&db, &uuid.to_string()).await?;
        let mut lot = Lot {
            username: user.username.clone(),
            uuid: uuid.clone(),
            records: vec![],
            key: user.key().clone(),
        };
        lot.load_records(&db).await?;
        Ok(lot)
    }

    // TODO: Return a vec of errors?
    pub async fn load_records(&mut self, db: &Database) -> Result<(), Error> {
        let sql_records =
            db::records::SqlRecord::select_by_lot(&db, &self.uuid.to_string()).await?;
        for sql_record in sql_records {
            let encrypted = Encrypted {
                data: sql_record.data,
                nonce: sql_record.nonce,
            };
            let data = RecordData::decrypt(&encrypted, self.key())?;
            let record = Record {
                lot: self.uuid,
                uuid: Uuid::from_str(&sql_record.uuid).map_err(|e| record::Error::Uuid(e))?,
                data,
            };
            self.records.push(record);
        }
        Ok(())
    }

    pub async fn insert_record(&mut self, db: &Database, data: RecordData) -> Result<(), Error> {
        let record = Record::new(self, data);
        self.upsert_record(&db, &record).await?;
        self.records.push(record);
        Ok(())
    }

    pub async fn remove_record(&self, _db: &Database, _record: Record) -> Result<(), Error> {
        unimplemented!();
    }

    // TODO: result type, create or update info.
    async fn upsert_record(&self, db: &Database, record: &Record) -> Result<(), Error> {
        let encrypted = record.data.encrypt(self.key())?;
        let sql_record = SqlRecord {
            lot: self.uuid.to_string(),
            uuid: record.uuid.to_string(),
            data: encrypted.data,
            nonce: encrypted.nonce,
        };
        sql_record.upsert(&db).await?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum Error {
    Uuid(uuid::Error),
    Record(record::Error),
    Database(db::Error),
}

impl From<uuid::Error> for Error {
    fn from(err: uuid::Error) -> Self {
        Error::Uuid(err)
    }
}

impl From<record::Error> for Error {
    fn from(err: record::Error) -> Self {
        Error::Record(err)
    }
}

impl From<db::Error> for Error {
    fn from(err: db::Error) -> Self {
        Error::Database(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::Database, record::RecordData, user::User};

    #[test]
    fn new() {
        let user = User::new("alice", "password").expect("failed to create user");
        let lot = Lot::new(&user);
        assert_eq!(user.username, lot.username);
        assert_eq!(36, lot.uuid.to_string().len());
        assert!(lot.records.is_empty());
        // TODO: #6
        // assert_ne!(user.key(), lot.key());
    }

    #[tokio::test]
    async fn save_empty() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("alice", "password").expect("failed to create user");
        user.register(&db).await.expect("failed to register user");
        let lot = Lot::new(&user);
        lot.save(&db).await.expect("failed to save lot");
    }

    #[tokio::test]
    #[ignore]
    async fn save_filled() {
        unimplemented!();
    }

    #[tokio::test]
    #[ignore]
    async fn load_empty() {
        unimplemented!();
    }

    #[tokio::test]
    #[ignore]
    async fn load_filled() {
        unimplemented!();
    }

    #[tokio::test]
    async fn insert_record() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("alice", "password").expect("failed to create user");
        user.register(&db).await.expect("failed to register user");
        let mut lot = Lot::new(&user);
        lot.save(&db).await.expect("failed to save lot");
        lot.insert_record(&db, RecordData::plain("foo", "bar"))
            .await
            .expect("failed to insert record");
        let record = &lot.records[0];
        assert_eq!(lot.uuid, record.lot);
        assert_eq!(36, record.uuid.to_string().len());
        match record.data {
            RecordData::Plain(ref label, ref value) => {
                assert_eq!("foo", label);
                assert_eq!("bar", value);
            }
            _ => assert!(false),
        }
    }

    #[tokio::test]
    async fn load_records() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("alice", "password").expect("failed to create user");
        user.register(&db).await.expect("failed to register user");
        let mut lot = Lot::new(&user);
        lot.save(&db).await.expect("failed to save lot");
        lot.load_records(&db).await.expect("failed to load records");
        assert!(lot.records.is_empty());
        lot.insert_record(&db, RecordData::plain("foo", "bar"))
            .await
            .expect("failed to insert record");
        lot.records = vec![]; // clear the records, since insert_record added it.
        lot.load_records(&db).await.expect("failed to load records");
        let record = &lot.records[0];
        assert_eq!(lot.uuid, record.lot);
        assert_eq!(36, record.uuid.to_string().len());
        match record.data {
            RecordData::Plain(ref label, ref value) => {
                assert_eq!("foo", label);
                assert_eq!("bar", value);
            }
            _ => assert!(false),
        }
    }
}
