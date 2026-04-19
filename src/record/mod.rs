#[cfg(feature = "db")]
use crate::db::{self, Database};
#[cfg(feature = "db")]
use crate::encrypt::{Encrypted, Stash};
use crate::{encrypt, lot::Lot, password::Password, uuid::Uuid};
use bitcode::{Decode, Encode};
#[cfg(feature = "db")]
use sea_orm::{IntoActiveModel, entity::prelude::*, sea_query::OnConflict};
use std::fmt;

#[derive(Encode, Decode)]
pub struct Record {
    pub(crate) uuid: Uuid<Self>,
    pub(crate) lot_uuid: Uuid<Lot>,
    pub(crate) label: Label,
    pub(crate) data: Data,
}

impl Record {
    pub fn new(lot: &Lot, label: Label, data: Data) -> Self {
        Record {
            uuid: Uuid::now(),
            lot_uuid: lot.uuid().clone(),
            label,
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
        &self.label
    }

    pub fn password(&self) -> &Password {
        self.data.password()
    }

    #[cfg(feature = "db")]
    pub(crate) fn label_aad(record_uuid: &Uuid<Self>, lot_uuid: &Uuid<Lot>) -> Vec<u8> {
        [
            b"l".as_slice(),
            record_uuid.to_uuid().as_bytes(),
            lot_uuid.to_uuid().as_bytes(),
        ]
        .concat()
    }

    #[cfg(feature = "db")]
    pub(crate) fn data_aad(record_uuid: &Uuid<Self>, lot_uuid: &Uuid<Lot>) -> Vec<u8> {
        [
            b"d".as_slice(),
            record_uuid.to_uuid().as_bytes(),
            lot_uuid.to_uuid().as_bytes(),
        ]
        .concat()
    }

    /// Save this record to the database and return its uuid.
    #[cfg(feature = "db")]
    pub async fn upsert(&self, db: &Database, lot: &Lot) -> Result<Uuid<Self>, Error> {
        let uuid = self.uuid.clone();
        let data_aad = Record::data_aad(&self.uuid, &self.lot_uuid);
        let data_encrypted = self.data.encrypt_with_aad(lot.key(), &data_aad)?;
        let label_aad = Record::label_aad(&self.uuid, &self.lot_uuid);
        let label_encrypted = self.label.encrypt_with_aad(lot.key(), &label_aad)?;
        let model = self::orm::Model {
            uuid: self.uuid.to_string(),
            lot_uuid: self.lot_uuid.to_string(),
            label: label_encrypted.data,
            label_nonce: label_encrypted.nonce,
            data: data_encrypted.data,
            data_nonce: data_encrypted.nonce,
        };
        let active = model.into_active_model();
        let on_conflict = OnConflict::column(self::orm::Column::Uuid)
            .update_columns([
                self::orm::Column::LotUuid,
                self::orm::Column::Label,
                self::orm::Column::LabelNonce,
                self::orm::Column::Data,
                self::orm::Column::DataNonce,
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

    /// Load a single record by UUID, decrypting both its label and its
    /// password-bearing data.
    ///
    /// This is the only public path that materializes a [`Password`]. Callers
    /// should reach this function exactly when they intend to expose the
    /// secret (e.g. copy-to-clipboard, reveal-in-UI, CLI `get`). Listing and
    /// searching should go through [`RecordIndex`] instead.
    #[cfg(feature = "db")]
    pub async fn show(db: &Database, lot: &Lot, uuid: &Uuid<Self>) -> Result<Option<Self>, Error> {
        let Some(model) = self::orm::Entity::find_by_id(uuid.to_string())
            .one(db.connection())
            .await?
        else {
            return Ok(None);
        };
        match Self::from_model(model, lot) {
            Ok(record) => Ok(Some(record)),
            Err(Error::LotMismatch { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Load every record in a lot with full decryption. Used internally for
    /// lot-key rotation; external consumers should use [`RecordIndex`] +
    /// [`Record::show`].
    #[cfg(feature = "db")]
    pub(crate) async fn load_all(db: &Database, lot: &Lot) -> Result<Vec<Self>, Error> {
        let models = self::orm::Entity::find()
            .filter(self::orm::Column::LotUuid.eq(lot.uuid().to_string()))
            .all(db.connection())
            .await?;
        models
            .into_iter()
            .map(|model| Self::from_model(model, lot))
            .collect()
    }

    /// Decrypt a stored row into a [`Record`], verifying that `model` belongs
    /// to `lot`. Returns [`Error::LotMismatch`] if the row's `lot_uuid` is not
    /// the lot passed in.
    #[cfg(feature = "db")]
    fn from_model(model: self::orm::Model, lot: &Lot) -> Result<Self, Error> {
        let uuid = Uuid::<Self>::parse(&model.uuid)?;
        let lot_uuid = Uuid::<Lot>::parse(&model.lot_uuid)?;
        if &lot_uuid != lot.uuid() {
            return Err(Error::LotMismatch {
                expected: lot.uuid().clone(),
                actual: lot_uuid,
            });
        }
        let label = Label::decrypt_with_aad(
            &Encrypted {
                data: model.label,
                nonce: model.label_nonce,
            },
            lot.key(),
            &Record::label_aad(&uuid, &lot_uuid),
        )?;
        let data = Data::decrypt_with_aad(
            &Encrypted {
                data: model.data,
                nonce: model.data_nonce,
            },
            lot.key(),
            &Record::data_aad(&uuid, &lot_uuid),
        )?;
        Ok(Record {
            uuid,
            lot_uuid,
            label,
            data,
        })
    }
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
            && self.lot_uuid == other.lot_uuid
            && self.label == other.label
            && self.data == other.data
    }
}
impl Eq for Record {}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label)
    }
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Record")
            .field("uuid", &self.uuid)
            .field("lot", &self.lot_uuid)
            .field("label", &self.label)
            .field("data", &self.data)
            .finish()
    }
}

#[derive(Debug)]
pub enum Error {
    MissingLot,
    #[cfg(feature = "db")]
    LotMismatch {
        expected: Uuid<Lot>,
        actual: Uuid<Lot>,
    },
    Uuid(crate::uuid::Error),
    #[cfg(feature = "db")]
    Database(db::Error),
    Encryption(encrypt::Error),
}

impl From<encrypt::Error> for Error {
    fn from(err: encrypt::Error) -> Self {
        Error::Encryption(err)
    }
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
pub use self::data::Data;

pub(crate) mod label;
pub use self::label::{Label, LabelName};

#[cfg(feature = "db")]
mod index;
#[cfg(feature = "db")]
pub use self::index::RecordIndex;

#[cfg(feature = "db")]
pub mod query;
#[cfg(feature = "db")]
pub use self::query::{Path, Query};

#[cfg(all(feature = "db", feature = "orm"))]
pub mod orm;
#[cfg(all(feature = "db", not(feature = "orm")))]
pub(crate) mod orm;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lot::Lot;
    #[cfg(feature = "db")]
    use crate::{db::Database, user::User};

    #[test]
    fn new() {
        let lot = Lot::new("test");
        let record = Record::new(
            &lot,
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        );
        assert_eq!(lot.uuid(), &record.lot_uuid);
        assert_eq!(36, record.uuid.to_string().len());
        assert_eq!(record.label(), &"foo".parse::<Label>().unwrap());
        assert_eq!(record.password().to_string(), "bar");
    }

    #[cfg(feature = "db")]
    #[test]
    fn label_and_data_aad_differ() {
        // Same record identity, different AAD prefix. A label blob must not
        // authenticate as a data blob even under the same key.
        let uuid = Uuid::<Record>::parse("00000000-0000-0000-0000-000000000001").unwrap();
        let lot_uuid = Uuid::<Lot>::parse("00000000-0000-0000-0000-000000000002").unwrap();
        assert_ne!(
            Record::label_aad(&uuid, &lot_uuid),
            Record::data_aad(&uuid, &lot_uuid),
        );
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn show_roundtrip() {
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
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        );
        let uuid = record
            .upsert(&db, &lot)
            .await
            .expect("failed to upsert record");
        let loaded = Record::show(&db, &lot, &uuid)
            .await
            .expect("failed to show record")
            .expect("record missing");
        assert_eq!(loaded, record);
    }

    #[cfg(feature = "db")]
    #[tokio::test]
    async fn show_wrong_lot_returns_none() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".try_into().unwrap())
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot_a = Lot::new("lot a");
        lot_a.save(&db, &user).await.expect("failed to save lot");
        let lot_b = Lot::new("lot b");
        lot_b.save(&db, &user).await.expect("failed to save lot");
        let uuid = Record::new(
            &lot_a,
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        )
        .upsert(&db, &lot_a)
        .await
        .expect("failed to upsert record");
        assert!(
            Record::show(&db, &lot_b, &uuid)
                .await
                .expect("failed to show")
                .is_none()
        );
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
            "foo".parse::<Label>().unwrap(),
            Data::new("bar".try_into().unwrap()),
        );
        let uuid = record
            .upsert(&db, &lot)
            .await
            .expect("failed to upsert record");
        record.delete(&db).await.expect("failed to delete record");
        assert!(
            Record::show(&db, &lot, &uuid)
                .await
                .expect("failed to show record")
                .is_none()
        );
    }
}
