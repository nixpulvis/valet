use crate::{
    db::{self, Database},
    encrypt::{self, Encrypted, Key},
    record::{self, Record},
    user::User,
    uuid::Uuid,
};
use std::fmt;

pub const DEFAULT_LOT: &'static str = "main";

/// An encrypted collection of secrets.
#[derive(PartialEq, Eq)]
pub struct Lot {
    uuid: Uuid<Self>,
    name: String,
    records: Vec<Record>,
    key: Key<Self>,
}

impl Lot {
    pub fn new(name: &str) -> Self {
        Lot {
            uuid: Uuid::now(),
            name: name.into(),
            records: Vec::new(),
            key: Key::generate(),
        }
    }

    pub fn uuid(&self) -> &Uuid<Self> {
        &self.uuid
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn key(&self) -> &Key<Self> {
        &self.key
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn records_mut(&mut self) -> &mut Vec<Record> {
        &mut self.records
    }

    /// Save this lot and its records to the database.
    pub async fn save(&self, db: &Database, user: &User) -> Result<(), Error> {
        let sql_lot = db::lots::SqlLot {
            uuid: self.uuid.to_string(),
            name: self.name.clone(),
        };
        sql_lot.upsert(&db).await?;

        let encrypted = user.key().encrypt(self.key.as_bytes())?;
        let sql_user_lot_key = db::user_lot_keys::SqlUserLotKey {
            username: user.username().into(),
            lot: self.uuid().to_string(),
            data: encrypted.data,
            nonce: encrypted.nonce,
        };
        sql_user_lot_key.insert(&db).await?;

        // TODO: Collect errors and report after.
        for record in &self.records {
            record.save(&db, self).await?;
        }

        Ok(())
    }

    pub async fn load(db: &Database, name: &str, user: &User) -> Result<Self, Error> {
        let sql_lot = db::lots::SqlLot::select_by_name(&db, name).await?;
        let sql_user_lot_key =
            db::user_lot_keys::SqlUserLotKey::select(&db, user.username(), &sql_lot.uuid).await?;
        let encrypted = Encrypted {
            data: sql_user_lot_key.data,
            nonce: sql_user_lot_key.nonce,
        };
        let key_bytes = user.key().decrypt(&encrypted)?;
        let mut lot = Lot {
            uuid: Uuid::parse(&sql_lot.uuid)?,
            name: sql_lot.name,
            records: Vec::new(),
            key: Key::from_bytes(&key_bytes),
        };
        lot.records = Record::load_all(&db, &lot).await?;
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

#[derive(Debug)]
pub enum Error {
    Uuid(crate::uuid::Error),
    Encrypt(encrypt::Error),
    Record(record::Error),
    Database(db::Error),
}

impl From<crate::uuid::Error> for Error {
    fn from(err: crate::uuid::Error) -> Self {
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
    use crate::{db::Database, record::RecordData};

    #[test]
    fn new() {
        let lot = Lot::new("lot a");
        assert_eq!(36, lot.uuid.to_string().len());
        assert!(lot.records().is_empty());
    }

    #[tokio::test]
    async fn save() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".into())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot = Lot::new("lot a");
        lot.records
            .push(Record::new(&lot, RecordData::plain("a", "1")));
        lot.save(&db, &user).await.expect("failed to save lot");
        lot.records
            .push(Record::new(&lot, RecordData::plain("b", "2")));
        lot.save(&db, &user).await.expect("failed to save lot");
    }

    #[tokio::test]
    async fn user_lot_key() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".into())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");

        let sql_user_lot_key =
            db::user_lot_keys::SqlUserLotKey::select(&db, user.username(), &lot.uuid().to_string())
                .await
                .expect("failed to select user lot key");
        let encrypted = Encrypted {
            data: sql_user_lot_key.data,
            nonce: sql_user_lot_key.nonce,
        };
        let lot_key = Key::<Lot>::from_bytes(
            &user
                .key()
                .decrypt(&encrypted)
                .expect("failed to decrypted lot key"),
        );
        assert_eq!(lot.key().as_bytes(), lot_key.as_bytes());
    }

    #[tokio::test]
    #[ignore]
    async fn load() {
        unimplemented!();
    }

    #[tokio::test]
    #[ignore]
    async fn load_all() {
        unimplemented!();
    }
}
