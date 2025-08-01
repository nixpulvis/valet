use std::{fmt, ops::Deref, str::FromStr};

use crate::{
    db::{self, Database, records::SqlRecord},
    encrypt::{self, Encrypted, Key},
    record::{self, Record, RecordData},
    user::User,
};
use uuid::Uuid;

pub const DEFAULT_LOT: &'static str = "main";

/// An encrypted collection of secrets.
#[derive(PartialEq, Eq)]
pub struct Lot {
    uuid: Uuid,
    name: String,
    records: Vec<Record>,
    key: LotKey,
}

impl Lot {
    pub fn new(name: &str) -> Self {
        Lot {
            uuid: Uuid::now_v7(),
            name: name.into(),
            records: Vec::new(),
            key: LotKey(Key::new()),
        }
    }

    pub fn uuid(&self) -> &Uuid {
        &self.uuid
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> &LotKey {
        &self.key
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Save this lot and its records to the database.
    pub async fn save(&self, db: &Database, user: &User) -> Result<(), Error> {
        let encrypted = user.key().encrypt(self.key.as_bytes())?;
        let sql_lot = db::lots::SqlLot {
            uuid: self.uuid.to_string(),
            name: self.name.clone(),
            key_data: encrypted.data,
            key_nonce: encrypted.nonce,
        };

        sql_lot.upsert(&db).await?;

        // TODO: Collect errors and report after.
        for record in &self.records {
            self.upsert_record(&db, record).await?;
        }

        Ok(())
    }

    pub async fn load(db: &Database, name: &str, user: &User) -> Result<Self, Error> {
        let sql_lot = db::lots::SqlLot::select_by_name(&db, name).await?;
        let encrypted = Encrypted {
            data: sql_lot.key_data,
            nonce: sql_lot.key_nonce,
        };
        let key_bytes = user.key().decrypt(&encrypted)?;
        let mut lot = Lot {
            uuid: Uuid::parse_str(&sql_lot.uuid)?,
            name: sql_lot.name,
            records: Vec::new(),
            key: LotKey(Key::from_bytes(&key_bytes)),
        };
        lot.load_records(&db).await?;
        Ok(lot)
    }

    pub async fn load_all(db: &Database, user: &User) -> Result<Vec<Self>, Error> {
        // let sql_lots = db::lots::SqlLot::select_by_user(&db, &self.username).await?;
        // let mut lots = vec![];
        // for sql_lot in sql_lots {
        //     let mut lot = Lot {
        //         username: sql_lot.username,
        //         uuid: Uuid::from_str(&sql_lot.uuid).map_err(|e| lot::Error::Uuid(e))?,
        //         records: vec![],
        //         key: self.key.clone(),
        //     };
        //     // lot.load_records(&db).await?;
        //     lots.push(lot);
        // }
        // Ok(lots)

        let lot = Self::load(&db, DEFAULT_LOT, &user).await?;
        Ok(vec![lot])
    }

    /// Decrypt a record from this lot.
    ///
    /// This function returns a *new* record with a unique UUID.
    // TODO: Each time you save a record it's new, with history preserved.
    pub fn decrypt_record(&self, encrypted: &Encrypted) -> Result<Record, Error> {
        Ok(Record::new(
            self.uuid,
            RecordData::decrypt(encrypted, self.key())?,
        ))
    }

    /// Insert a new record into this lot, save to DB, and return a reference to it.
    pub async fn insert_record(&mut self, db: &Database, data: RecordData) -> Result<Uuid, Error> {
        let record = Record::new(self.uuid, data);
        let uuid = record.uuid.clone();
        self.upsert_record(&db, &record).await?;
        self.records.push(record);
        Ok(uuid)
    }

    pub async fn remove_record(&mut self, _db: &Database, _record_uuid: Uuid) -> Result<(), Error> {
        // TODO: Implement record removal and DB update
        unimplemented!();
    }

    // TODO: Return a vec of errors?
    async fn load_records(&mut self, db: &Database) -> Result<(), Error> {
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

    // TODO: result type, create or update info.
    async fn upsert_record(&self, db: &Database, record: &Record) -> Result<(), Error> {
        let encrypted = record.data().encrypt(self.key())?;
        let sql_record = SqlRecord {
            lot: self.uuid.to_string(),
            uuid: record.uuid().to_string(),
            data: encrypted.data,
            nonce: encrypted.nonce,
        };
        sql_record.upsert(&db).await?;
        Ok(())
    }
}

impl fmt::Debug for Lot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lot")
            .field("uuid", &self.uuid)
            .field("name", &self.name)
            .field("records", &self.records)
            .finish()
    }
}

#[derive(PartialEq, Eq)]
pub struct LotKey(pub(crate) Key);

impl Deref for LotKey {
    type Target = Key;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug)]
pub enum Error {
    Uuid(uuid::Error),
    Encrypt(encrypt::Error),
    Record(record::Error),
    Database(db::Error),
}

impl From<uuid::Error> for Error {
    fn from(err: uuid::Error) -> Self {
        Error::Uuid(err)
    }
}

impl From<encrypt::Error> for Error {
    fn from(err: encrypt::Error) -> Self {
        Error::Encrypt(err)
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
    use crate::db::Database;

    #[test]
    fn new() {
        let lot = Lot::new("lot a");
        assert_eq!(36, lot.uuid.to_string().len());
        assert!(lot.records().is_empty());
        // TODO: #6
        // assert_ne!(user.key(), lot.key());
    }

    #[tokio::test]
    async fn save() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".into()).expect("failed to make user");
        let mut lot = Lot::new("lot a");
        lot.records
            .push(Record::new(lot.uuid, RecordData::plain("a", "1")));
        lot.save(&db, &user).await.expect("failed to save lot");
        lot.records
            .push(Record::new(lot.uuid, RecordData::plain("b", "2")));
        lot.save(&db, &user).await.expect("failed to save lot");
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
        let user = User::new("nixpulvis", "password".into()).expect("failed to make user");
        let mut lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let record_uuid = lot
            .insert_record(&db, RecordData::plain("foo", "bar"))
            .await
            .expect("failed to insert record");
        assert_eq!(lot.records()[0].uuid, record_uuid);
    }

    #[tokio::test]
    async fn load_records() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");

        let user = User::new("nixpulvis", "password".into()).expect("failed to make user");

        // Create a lot.
        let mut lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");

        // Load records should be empty.
        lot.load_records(&db).await.expect("failed to load records");
        assert!(lot.records().is_empty());

        // Insert a record.
        let inserted_uuid = lot
            .insert_record(&db, RecordData::plain("foo", "bar"))
            .await
            .expect("failed to insert record");
        lot.records.clear();
        lot.load_records(&db).await.expect("failed to load records");

        // Check inserted record is the same as the loaded one.
        let record = &lot.records()[0];
        assert_eq!(lot.uuid, record.lot);
        assert_eq!(inserted_uuid, record.uuid);
    }
}
