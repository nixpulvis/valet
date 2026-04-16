#[cfg(feature = "db")]
use crate::db::{self, Database};
use crate::{
    encrypt::{self, Encrypted, Key},
    lot::Lot,
    password::Password,
    uuid::Uuid,
};
use bitcode::{Decode, Encode};
#[cfg(feature = "db")]
use sea_orm::{IntoActiveModel, entity::prelude::*, sea_query::OnConflict};
use std::{fmt, io};

#[derive(Encode, Decode)]
pub struct Record {
    pub(crate) uuid: Uuid<Self>,
    pub(crate) lot_uuid: Uuid<Lot>,
    pub(crate) data: Data,
}

impl Record {
    pub fn new(lot: &Lot, data: Data) -> Self {
        Record {
            uuid: Uuid::now(),
            lot_uuid: lot.uuid().clone(),
            data,
        }
    }

    pub fn uuid(&self) -> &Uuid<Self> {
        &self.uuid
    }

    pub fn lot_uuid(&self) -> &Uuid<Lot> {
        &self.lot_uuid
    }

    pub fn data(&self) -> &Data {
        &self.data
    }

    pub fn label(&self) -> &Label {
        self.data.label()
    }

    pub fn password(&self) -> &Password {
        self.data.password()
    }

    pub fn encrypt(&self, key: &Key<Lot>) -> Result<Encrypted, Error> {
        let aad = Record::aad(&self.uuid.to_string(), &self.lot_uuid.to_string());
        self.data.encrypt_with_aad(key, &aad)
    }

    pub fn decrypt(
        uuid: Uuid<Self>,
        lot_uuid: Uuid<Lot>,
        encrypted: &Encrypted,
        key: &Key<Lot>,
    ) -> Result<Self, Error> {
        let aad = Record::aad(&uuid.to_string(), &lot_uuid.to_string());
        let data = Data::decrypt_with_aad(encrypted, key, &aad)?;
        Ok(Record {
            uuid,
            lot_uuid,
            data,
        })
    }

    /// Save this record to the database and return its uuid.
    #[cfg(feature = "db")]
    pub async fn upsert(&self, db: &Database, lot: &Lot) -> Result<Uuid<Self>, Error> {
        let uuid = self.uuid.clone();
        let aad = Record::aad(&self.uuid.to_string(), &self.lot_uuid.to_string());
        let encrypted = self.data.encrypt_with_aad(lot.key(), &aad)?;
        // TODO: Only Set values that changed? To do this we'd need to track the
        // changed values in this Record struct... which seems redundant, since
        // the orm::ActiveModel already does that. Perhaps we can figure out a
        // nice way to avoid the duplicate Record and orm::(Active)Model structs
        // down the road.
        let model = self::orm::Model {
            uuid: self.uuid.to_string(),
            lot_uuid: self.lot_uuid.to_string(),
            data: encrypted.data,
            nonce: encrypted.nonce,
        };
        let active = model.into_active_model();
        let on_conflict = OnConflict::column(self::orm::Column::Uuid)
            .update_columns([
                self::orm::Column::LotUuid,
                self::orm::Column::Data,
                self::orm::Column::Nonce,
            ])
            .to_owned();
        self::orm::Entity::insert(active)
            .on_conflict(on_conflict)
            .exec_with_returning(db.connection())
            .await?;

        Ok(uuid)
    }

    /// Delete this record from the database.
    #[cfg(feature = "db")]
    pub async fn delete(&self, db: &Database) -> Result<(), Error> {
        self::orm::Entity::delete_by_id(self.uuid.to_string())
            .exec(db.connection())
            .await?;
        Ok(())
    }

    #[cfg(feature = "db")]
    pub async fn load_all(db: &Database, lot: &Lot) -> Result<Vec<Self>, Error> {
        let models = self::orm::Entity::find()
            .filter(self::orm::Column::LotUuid.eq(lot.uuid().to_string()))
            .all(db.connection())
            .await?;

        let mut records = Vec::new();
        for model in models {
            let encrypted = Encrypted {
                data: model.data,
                nonce: model.nonce,
            };
            let record = Record::decrypt(
                Uuid::parse(&model.uuid)?,
                lot.uuid().clone(),
                &encrypted,
                lot.key(),
            )?;
            records.push(record);
        }

        Ok(records)
    }

    fn aad(record_uuid: &str, lot_uuid: &str) -> Vec<u8> {
        [record_uuid.as_bytes(), lot_uuid.as_bytes()].concat()
    }
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid && self.data == other.data && self.lot_uuid == other.lot_uuid
    }
}
impl Eq for Record {}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.data)
    }
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Record")
            .field("uuid", &self.uuid)
            .field("lot", &self.lot_uuid)
            .field("data", &self.data)
            .finish()
    }
}

#[derive(Debug)]
pub enum Error {
    MissingLot,
    Uuid(crate::uuid::Error),
    #[cfg(feature = "db")]
    Database(db::Error),
    Encoding(bitcode::Error),
    Compression(io::Error),
    Encryption(encrypt::Error),
}

impl From<crate::uuid::Error> for Error {
    fn from(err: crate::uuid::Error) -> Self {
        Error::Uuid(err)
    }
}

#[cfg(feature = "db")]
impl From<db::Error> for Error {
    fn from(err: db::Error) -> Self {
        Error::Database(err)
    }
}

#[cfg(feature = "db")]
impl From<sea_orm::DbErr> for Error {
    fn from(err: sea_orm::DbErr) -> Self {
        Error::Database(err.into())
    }
}

mod data;
pub use self::data::{Data, Label}; // TODO: Remove
// pub use self::data::{Data, Label, Secret};

#[cfg(all(feature = "db", feature = "orm"))]
pub mod orm;
#[cfg(all(feature = "db", not(feature = "orm")))]
pub(crate) mod orm;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "db")]
    use crate::{db::Database, user::User};
    use crate::{lot::Lot, record::data::Label};

    #[test]
    fn new() {
        let lot = Lot::new("test");
        let record = Record::new(
            &lot,
            Data::new(Label::Simple("foo".into()), "bar".try_into().unwrap()),
        );
        assert_eq!(lot.uuid(), &record.lot_uuid);
        assert_eq!(36, record.uuid.to_string().len());
        assert_eq!(record.data.label(), &Label::Simple("foo".into()));
        assert_eq!(record.data.password().to_string(), "bar");
    }

    #[test]
    fn encrypt_decrypt() {
        let lot = Lot::new("test");
        let record = Record::new(
            &lot,
            Data::new(Label::Simple("foo".into()), "bar".try_into().unwrap()),
        );
        let encrypted = record.encrypt(&lot.key()).expect("failed to encrypt");
        let decrypted = Record::decrypt(
            record.uuid.clone(),
            record.lot_uuid.clone(),
            &encrypted,
            &lot.key(),
        )
        .expect("failed to decrypt");
        assert_eq!(record, decrypted);
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn insert() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let inserted_uuid = Record::new(
            &lot,
            Data::new(Label::Simple("foo".into()), "bar".try_into().unwrap()),
        )
        .upsert(&db, &lot)
        .await
        .expect("failed to upsert record");
        let records = lot.records(&db).await.expect("failed to load records");
        assert_eq!(lot.uuid(), &records[0].lot_uuid);
        assert_eq!(inserted_uuid, records[0].uuid);
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn load_all() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let record = Record::new(
            &lot,
            Data::new(Label::Simple("foo".into()), "bar".try_into().unwrap()),
        );
        let inserted_uuid = record
            .upsert(&db, &lot)
            .await
            .expect("failed to upsert record");
        let records = Record::load_all(&db, &lot)
            .await
            .expect("failed to load records");
        assert_eq!(lot.uuid(), &records[0].lot_uuid);
        assert_eq!(inserted_uuid, records[0].uuid);
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn delete() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let record = Record::new(
            &lot,
            Data::new(Label::Simple("foo".into()), "bar".try_into().unwrap()),
        );
        record
            .upsert(&db, &lot)
            .await
            .expect("failed to upsert record");
        record.delete(&db).await.expect("failed to delete record");
        let records = lot.records(&db).await.expect("failed to load records");
        assert!(records.is_empty());
    }
}
