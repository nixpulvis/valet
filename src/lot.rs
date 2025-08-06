use crate::{
    db::{self, Database, lots::SqlLot, user_lot_keys::SqlUserLotKey},
    encrypt::{self, Encrypted, Key},
    record::{self, Record},
    user::User,
    uuid::Uuid,
};
use std::fmt;

pub const DEFAULT_LOT: &'static str = "main";

/// An encrypted collection of secrets.
///
/// Each lot has its own _lot key_, i.e. [`Key<Lot>`] which is used to encrypt
/// all of the records within the lot. Users with access to a lot obtain the lot
/// key through the `user_lot_keys` SQL table.
///
/// Example `user_lot_keys` table:
///
/// | username | lot |    data    |   nonce    |
/// |----------|-----|------------|------------|
/// | Alice    | `a` | `tvuZQ1XS` | `6jLC3aP9` |
/// | Alice    | `b` | `LyZJM8GA` | `SCW2EWjc` |
/// | Bob      | `a` | `dWPiZfO9` | `oQ/2Y845` |
///
/// The lot keys they derive:
///
/// |  Key   | `Decrypt_A` is Alice's            | `Decrypt_B` is Bob's              |
/// |--------|-----------------------------------|-----------------------------------|
/// | `Ka`   | `= Decrypt_A(tvuZQ1XS, 6jLC3aP9)` | `= Decrypt_B(dWPiZfO9, oQ/2Y845)` |
/// | `Kb`   | `= Decrypt_A(LyZJM8GA, SCW2EWjc)` | N/A                               |
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
    pub async fn save(&self, db: &Database, user: &User) -> Result<Uuid<Self>, Error> {
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
        sql_user_lot_key.upsert(&db).await?;

        // TODO: Collect errors and report after.
        for record in &self.records {
            record.save(&db, self).await?;
        }

        Ok(self.uuid.clone())
    }

    /// Load a user's lot by name.
    pub async fn load(db: &Database, name: &str, user: &User) -> Result<Self, Error> {
        let sql_lot = SqlLot::select_by_name(&db, name).await?;
        let sql_ulk = SqlUserLotKey::select(&db, user.username(), &sql_lot.uuid).await?;
        let lot = Self::decrypt_and_build(&db, &user, sql_lot, sql_ulk).await?;
        Ok(lot)
    }

    /// Load a user's lots.
    pub async fn load_all(db: &Database, user: &User) -> Result<Vec<Self>, Error> {
        let sql_ulks = SqlUserLotKey::select_all(&db, user.username()).await?;
        let mut lots = Vec::new();
        for sql_ulk in sql_ulks {
            let sql_lot = SqlLot::select(db, &sql_ulk.lot).await?;
            let lot = Self::decrypt_and_build(&db, &user, sql_lot, sql_ulk).await?;
            lots.push(lot);
        }
        Ok(lots)
    }

    async fn decrypt_and_build(
        db: &Database,
        user: &User,
        sql_lot: SqlLot,
        sql_ulk: SqlUserLotKey,
    ) -> Result<Lot, Error> {
        let encrypted = Encrypted {
            data: sql_ulk.data,
            nonce: sql_ulk.nonce,
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
    async fn create_load() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".into())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot_a = Lot::new("lot a");
        // Save the lot without any records.
        lot_a.save(&db, &user).await.expect("failed to save lot");
        // Insert a record.
        Record::new(&lot_a, RecordData::plain("a", "1"))
            .insert(&db, &mut lot_a)
            .await
            .expect("failed to insert record");
        // Manually insert a record.
        lot_a
            .records
            .push(Record::new(&lot_a, RecordData::plain("b", "2")));
        lot_a.save(&db, &user).await.expect("failed to save lot");

        let lot_b = Lot::load(&db, lot_a.name(), &user)
            .await
            .expect("failed to load lot");
        assert_eq!(lot_a.records, lot_b.records);
    }

    #[tokio::test]
    async fn create_load_all() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".into())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let mut lot_a = Lot::new("lot a");
        lot_a.save(&db, &user).await.expect("failed to save lot");
        Record::new(&lot_a, RecordData::plain("a", "1"))
            .insert(&db, &mut lot_a)
            .await
            .expect("failed to insert record");
        let mut lot_b = Lot::new("lot b");
        lot_b.save(&db, &user).await.expect("failed to save lot");
        Record::new(&lot_b, RecordData::plain("b", "2"))
            .insert(&db, &mut lot_b)
            .await
            .expect("failed to insert record");

        let lots = Lot::load_all(&db, &user)
            .await
            .expect("failed to load lots");
        assert_eq!(lots, vec![lot_a, lot_b]);
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
        let lot_key = get_user_lot_key(&db, &user, &lot).await;
        assert_eq!(lot.key().as_bytes(), lot_key.as_bytes());
    }

    #[tokio::test]
    async fn user_lot_key_update() {
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
        let lot_key_a = get_user_lot_key(&db, &user, &lot).await;
        lot.key = Key::<Lot>::generate();
        // Update lot key, user_lot_key, and reencrypt all records.
        lot.save(&db, &user).await.expect("failed to save lot");
        let lot_key_b = get_user_lot_key(&db, &user, &lot).await;
        assert_ne!(lot_key_a.as_bytes(), lot_key_b.as_bytes());
        // Ensure the records got reencrypted and we can still access them.
        let lot = Lot::load(&db, lot.name(), &user)
            .await
            .expect("failed to load lot");
        assert_eq!(1, lot.records.len());
        assert_eq!("a", lot.records[0].data.label());
    }

    /// Returns the lot key for a given user/lot as decrypted from the
    /// user_lot_keys table.
    async fn get_user_lot_key(db: &Database, user: &User, lot: &Lot) -> Key<Lot> {
        let sql_user_lot_key =
            db::user_lot_keys::SqlUserLotKey::select(&db, user.username(), &lot.uuid().to_string())
                .await
                .expect("failed to select user lot key");
        let encrypted = Encrypted {
            data: sql_user_lot_key.data,
            nonce: sql_user_lot_key.nonce,
        };
        Key::<Lot>::from_bytes(
            &user
                .key()
                .decrypt(&encrypted)
                .expect("failed to decrypted lot key"),
        )
    }
}
