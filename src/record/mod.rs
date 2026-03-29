use crate::{
    db::{self, Database},
    encrypt::{self, Encrypted, Key},
    lot::Lot,
    uuid::Uuid,
};
use bitcode::{Decode, Encode};
use sea_orm::{IntoActiveModel, entity::prelude::*, sea_query::OnConflict};
use std::collections::HashMap;
use std::{fmt, io};

pub struct Record {
    pub(crate) uuid: Uuid<Self>,
    pub(crate) lot_uuid: Uuid<Lot>,
    pub(crate) data: RecordData,
}

impl Record {
    pub fn new(lot: &Lot, data: RecordData) -> Self {
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

    pub fn lot(&self) -> Lot {
        unimplemented!()
    }

    pub fn data(&self) -> &RecordData {
        &self.data
    }

    pub fn label(&self) -> &str {
        self.data.label()
    }

    // TODO: Should be a Password type
    pub fn password(&self) -> &str {
        self.data.password()
    }

    pub fn encrypt(&self, key: &Key<Lot>) -> Result<Encrypted, Error> {
        self.data.encrypt(key)
    }

    /// Save this record to the database and return its uuid.
    pub async fn upsert(&self, db: &Database, lot: &Lot) -> Result<Uuid<Self>, Error> {
        let uuid = self.uuid.clone();
        let encrypted = self.data.encrypt(lot.key())?;
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
    pub async fn delete(&self, db: &Database) -> Result<(), Error> {
        self::orm::Entity::delete_by_id(self.uuid.to_string())
            .exec(db.connection())
            .await?;
        Ok(())
    }

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
            let data = RecordData::decrypt(&encrypted, lot.key())?;
            let record = Record {
                lot_uuid: lot.uuid().clone(),
                uuid: Uuid::parse(&model.uuid)?,
                data,
            };
            records.push(record);
        }

        Ok(records)
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

#[derive(Encode, Decode, Debug, Eq, PartialEq)]
pub enum RecordData {
    // TODO: We should really generalize the concept of a "label" to allow for
    // the HashMap to be the label here. We can then store a single (or many)
    // passwords separately.
    // TODO: Use PasswordBuf here.
    Domain(String, HashMap<String, String>),
    Plain(String, String),
}

// TODO: Don't display passwords.
impl fmt::Display for RecordData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordData::Domain(label, attributes) => {
                write!(f, "{label}: {{ ")?;
                for (i, (k, v)) in attributes.iter().enumerate() {
                    write!(f, "{k}: {v}")?;
                    if i < attributes.len() - 1 {
                        write!(f, ", ")?;
                    }
                }
                write!(f, " }}")?;
                Ok(())
            }
            RecordData::Plain(label, text) => {
                if text.contains("\n") {
                    write!(f, "{label}:\n{text}")
                } else {
                    write!(f, "{label}: {text}")
                }
            }
        }
    }
}

impl RecordData {
    pub fn domain(label: &str, values: HashMap<String, String>) -> Self {
        Self::Domain(label.into(), values)
    }

    pub fn plain(label: &str, value: &str) -> Self {
        Self::Plain(label.into(), value.into())
    }

    pub fn label(&self) -> &str {
        match self {
            RecordData::Domain(s, _) => &s,
            RecordData::Plain(s, _) => &s,
        }
    }

    pub fn password(&self) -> &str {
        match self {
            RecordData::Domain(_, attrs) => {
                if attrs.contains_key("password") {
                    &attrs["password"]
                } else if attrs.len() == 1 {
                    &attrs.iter().next().unwrap().1
                } else {
                    ""
                }
            }
            RecordData::Plain(_, s) => &s,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        bitcode::encode(self)
    }

    pub fn decode(buf: &[u8]) -> Result<Self, Error> {
        bitcode::decode(buf).map_err(|e| Error::Encoding(e))
    }

    pub fn compress(&self) -> Result<Vec<u8>, Error> {
        let mut compressed = Vec::new();
        let encoded = self.encode();
        let mut encoder = snap::read::FrameEncoder::new(encoded.as_slice());
        io::copy(&mut encoder, &mut compressed).map_err(|e| Error::Compression(e))?;
        Ok(compressed)
    }

    pub fn decompress(buf: &[u8]) -> Result<Self, Error> {
        let mut decompressed = Vec::new();
        let mut decoder = snap::read::FrameDecoder::new(buf);
        io::copy(&mut decoder, &mut decompressed).map_err(|e| Error::Compression(e))?;
        let decoded = RecordData::decode(&decompressed)?;
        Ok(decoded)
    }

    pub fn encrypt(&self, key: &Key<Lot>) -> Result<Encrypted, Error> {
        let compressed = self.compress()?;
        key.encrypt(&compressed).map_err(|e| Error::Encryption(e))
    }

    pub fn decrypt(buf: &Encrypted, key: &Key<Lot>) -> Result<Self, Error> {
        let decrypted = key.decrypt(buf).map_err(|e| Error::Encryption(e))?;
        Self::decompress(&decrypted)
    }
}

#[derive(Debug)]
pub enum Error {
    MissingLot,
    Uuid(crate::uuid::Error),
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

impl From<db::Error> for Error {
    fn from(err: db::Error) -> Self {
        Error::Database(err)
    }
}

impl From<sea_orm::DbErr> for Error {
    fn from(err: sea_orm::DbErr) -> Self {
        Error::Database(err.into())
    }
}

#[cfg(feature = "orm")]
pub mod orm;
#[cfg(not(feature = "orm"))]
pub(crate) mod orm;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encrypt::Key, lot::Lot, pw, user::User};

    #[test]
    fn new() {
        let lot = Lot::new("test");
        let record = Record::new(&lot, RecordData::plain("foo", "bar"));
        assert_eq!(lot.uuid(), &record.lot_uuid);
        assert_eq!(36, record.uuid.to_string().len());
        match record.data {
            RecordData::Plain(ref label, ref value) => {
                assert_eq!("foo", label);
                assert_eq!("bar", value);
            }
            _ => unreachable!(),
        }
    }

    // TODO: Record::decrypt (does #14 help get the UUID?)
    #[test]
    fn encrypt_decrypt() {
        let lot = Lot::new("test");
        let key = Key::<Lot>::generate();
        let record = Record::new(&lot, RecordData::plain("foo", "bar"));
        let encrypted = record.encrypt(&key).expect("failed to encrypt");
        let decrypted_data = RecordData::decrypt(&encrypted, &key).expect("failed to decrypt");
        assert_eq!(record.data, decrypted_data);
    }

    #[tokio::test]
    async fn insert() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", pw!("password"))
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let inserted_uuid = Record::new(&lot, RecordData::plain("foo", "bar"))
            .upsert(&db, &lot)
            .await
            .expect("failed to upsert record");
        let records = lot.records(&db).await.expect("failed to load records");
        assert_eq!(lot.uuid(), &records[0].lot_uuid);
        assert_eq!(inserted_uuid, records[0].uuid);
    }

    #[tokio::test]
    async fn load_all() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", pw!("password"))
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let record = Record::new(&lot, RecordData::plain("foo", "bar"));
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

    #[tokio::test]
    async fn delete() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", pw!("password"))
            .expect("failed to make user")
            .register(&db)
            .await
            .expect("failed to register user");
        let lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let record = Record::new(&lot, RecordData::plain("foo", "bar"));
        record
            .upsert(&db, &lot)
            .await
            .expect("failed to upsert record");
        record.delete(&db).await.expect("failed to delete record");
        let records = lot.records(&db).await.expect("failed to load records");
        assert!(records.is_empty());
    }

    #[test]
    fn data_label() {
        let data = RecordData::plain("plain", "secret");
        assert_eq!("plain", data.label());
        let data = RecordData::domain("domain", HashMap::new());
        assert_eq!("domain", data.label());
    }

    #[test]
    fn data_encode_decode() {
        let data = RecordData::plain("label", "secret");
        let encoded = data.encode();
        let decoded = RecordData::decode(&encoded).expect("failed to decode");
        assert_eq!(data, decoded);
    }

    #[test]
    fn data_compress_decompress() {
        let data = RecordData::plain("label", "secret");
        let compressed = data.compress().expect("failed to compress");
        let decompressed = RecordData::decompress(&compressed).expect("failed to decompress");
        assert_eq!(data, decompressed);
    }

    #[test]
    fn data_encrypt_decrypt() {
        let lot = Lot::new("test");
        let data = RecordData::plain("label", "secret");
        let encrypted = data.encrypt(lot.key()).expect("failed to encrypt");
        let decrypted = RecordData::decrypt(&encrypted, lot.key()).expect("failed to decrypt");
        assert_eq!(data, decrypted);
    }
}
