use std::{cell::RefCell, ops::Deref, rc::Rc, str::FromStr};

use crate::{
    db::{self, Database, records::SqlRecord},
    encrypt::{self, Encrypted, Key},
    record::{self, Record, RecordData},
    user::User,
};
use uuid::Uuid;

pub const DEFAULT_LOT: &'static str = "main";

/// An encrypted collection of secrets.
#[derive(Debug, PartialEq, Eq)]
pub struct Lot {
    pub uuid: Uuid,
    pub name: String,
    pub records: RefCell<Vec<Rc<RefCell<Record>>>>,
    pub(crate) key: Key,
}

impl Lot {
    pub fn new(name: &str) -> Rc<Self> {
        let uuid = Uuid::now_v7();
        Rc::new(Lot {
            uuid,
            name: name.into(),
            records: RefCell::new(vec![]),
            key: Key::new(),
        })
    }

    pub fn key(&self) -> &Key {
        &self.key
    }

    /// Save this lot and it's records to the database.
    // TODO: Probably don't need a .save method, we need a
    // create method which also makes the join table entry.
    pub async fn save(self: &Rc<Self>, db: &Database) -> Result<(), Error> {
        let sql_lot = db::lots::SqlLot {
            uuid: self.uuid.to_string(),
            name: self.name.clone(),
        };
        sql_lot.insert(&db).await?;

        for record in self.records.borrow().iter() {
            // TODO: Collect errors and report after.
            self.upsert_record(&db, record.clone()).await?;
        }

        Ok(())
    }

    pub async fn load(db: &Database, name: &str, user: &User) -> Result<Rc<Self>, Error> {
        let sql_lot = db::lots::SqlLot::select_by_name(&db, name).await?;
        let lot = Rc::new(Lot {
            uuid: Uuid::parse_str(&sql_lot.uuid)?,
            name: sql_lot.name,
            records: RefCell::new(vec![]),
            // TODO: #6 decrypt key stored in user_lot_keys
            key: user.key().clone(),
        });
        lot.load_records(&db).await?;
        Ok(lot)
    }

    pub async fn load_all(db: &Database, user: &User) -> Result<Vec<Rc<Self>>, Error> {
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

    pub async fn insert_record(
        self: &Rc<Self>,
        db: &Database,
        data: RecordData,
    ) -> Result<Rc<RefCell<Record>>, Error> {
        let record = Record::new(self.clone(), data);
        self.upsert_record(&db, record.clone()).await?;
        self.records.borrow_mut().push(record);
        let index = self.records.borrow().len() - 1;
        let record = self.records.borrow()[index].clone();
        Ok(record)
    }

    pub async fn remove_record(
        self: Rc<Self>,
        _db: &Database,
        _record: Record,
    ) -> Result<(), Error> {
        unimplemented!();
    }

    // TODO: Return a vec of errors?
    async fn load_records(self: &Rc<Self>, db: &Database) -> Result<(), Error> {
        let sql_records =
            db::records::SqlRecord::select_by_lot(&db, &self.uuid.to_string()).await?;

        for sql_record in sql_records {
            let encrypted = Encrypted {
                data: sql_record.data,
                nonce: sql_record.nonce,
            };
            let data = RecordData::decrypt(&encrypted, self.key())?;
            let record = Rc::new(RefCell::new(Record {
                lot: self.clone(),
                uuid: Uuid::from_str(&sql_record.uuid).map_err(|e| record::Error::Uuid(e))?,
                data,
            }));
            self.records.borrow_mut().push(record);
        }

        Ok(())
    }

    // TODO: result type, create or update info.
    async fn upsert_record(&self, db: &Database, record: Rc<RefCell<Record>>) -> Result<(), Error> {
        let encrypted = record.borrow().data.encrypt(self.key())?;
        let sql_record = SqlRecord {
            lot: self.uuid.to_string(),
            uuid: record.borrow().uuid.to_string(),
            data: encrypted.data,
            nonce: encrypted.nonce,
        };
        sql_record.upsert(&db).await?;
        Ok(())
    }
}

pub struct LotKey(Key);

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
        assert!(lot.records.borrow().is_empty());
        // TODO: #6
        // assert_ne!(user.key(), lot.key());
    }

    #[tokio::test]
    async fn save_empty() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let lot = Lot::new("lot a");
        lot.records
            .borrow_mut()
            .push(Record::new(lot.clone(), RecordData::plain("a", "b")));
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
        let lot = Lot::new("lot a");
        lot.save(&db).await.expect("failed to save lot");
        let record = lot
            .insert_record(&db, RecordData::plain("foo", "bar"))
            .await
            .expect("failed to insert record");
        // assert_eq!(lot.uuid, record.lot);
        assert_eq!(lot.records.borrow()[0].borrow().uuid, record.borrow().uuid);
    }

    #[tokio::test]
    async fn load_records() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");

        // Create a lot.
        let lot = Lot::new("lot a");
        lot.save(&db).await.expect("failed to save lot");

        // Load records should be empty.
        lot.load_records(&db).await.expect("failed to load records");
        assert!(lot.records.borrow().is_empty());

        // Insert a record.
        let inserted = lot
            .insert_record(&db, RecordData::plain("foo", "bar"))
            .await
            .expect("failed to insert record");
        lot.records.borrow_mut().clear();
        lot.load_records(&db).await.expect("failed to load records");

        // Check inserted record is the same as the loaded one.
        let record = &lot.records.borrow()[0];
        assert_eq!(lot.uuid, record.borrow().lot.uuid);
        assert_eq!(inserted.borrow().uuid, record.borrow().uuid)
    }
}
