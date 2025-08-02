use crate::db::records::SqlRecord;
use crate::db::{self, Database};
use crate::encrypt::{self, Encrypted};
use crate::lot::{Lot, LotKey};
use bitcode::{Decode, Encode};
use std::collections::HashMap;
use std::{fmt, io};
use uuid::Uuid;

pub struct Record {
    pub(crate) lot: Uuid,
    pub(crate) uuid: Uuid,
    pub(crate) data: RecordData,
}

impl Record {
    pub fn new(lot: &Lot, data: RecordData) -> Self {
        Record {
            lot: lot.uuid().clone(),
            uuid: Uuid::now_v7(),
            data,
        }
    }

    pub fn uuid(&self) -> &Uuid {
        &self.uuid
    }

    pub fn lot(&self) -> &Uuid {
        &self.lot
    }

    pub fn data(&self) -> &RecordData {
        &self.data
    }

    pub fn encrypt(&self, key: &LotKey) -> Result<Encrypted, Error> {
        self.data.encrypt(key)
    }

    /// Save this record to the database.
    pub async fn save(&self, db: &Database, lot: &Lot) -> Result<Uuid, Error> {
        let uuid = self.uuid.clone();
        let encrypted = self.data.encrypt(lot.key())?;
        let sql_record = SqlRecord {
            lot: self.lot.to_string(),
            uuid: self.uuid.to_string(),
            data: encrypted.data,
            nonce: encrypted.nonce,
        };
        sql_record.upsert(&db).await?;
        Ok(uuid)
    }

    /// Insert this record into a lot and save it to the database.
    pub async fn insert(self, db: &Database, lot: &mut Lot) -> Result<Uuid, Error> {
        let uuid = self.save(&db, lot).await?;
        lot.records_mut().push(self);
        Ok(uuid)
    }

    // TODO: Return a vec of errors?
    pub async fn load_all(db: &Database, lot: &Lot) -> Result<Vec<Self>, Error> {
        let sql_records =
            db::records::SqlRecord::select_by_lot(&db, &lot.uuid().to_string()).await?;

        let mut records = Vec::new();
        for sql_record in sql_records {
            let encrypted = Encrypted {
                data: sql_record.data,
                nonce: sql_record.nonce,
            };
            let data = RecordData::decrypt(&encrypted, lot.key())?;
            let record = Record {
                lot: lot.uuid().clone(),
                uuid: Uuid::parse_str(&sql_record.uuid)?,
                data,
            };
            records.push(record);
        }

        Ok(records)
    }
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid && self.data == other.data && self.lot == other.lot
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
            .field("lot", &self.lot)
            .field("data", &self.data)
            .finish()
    }
}

#[derive(Encode, Decode, Debug, Eq, PartialEq)]
pub enum RecordData {
    Domain(String, HashMap<String, String>),
    Plain(String, String),
}

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

    pub fn encrypt(&self, key: &LotKey) -> Result<Encrypted, Error> {
        let compressed = self.compress()?;
        key.encrypt(&compressed).map_err(|e| Error::Encryption(e))
    }

    pub fn decrypt(buf: &Encrypted, key: &LotKey) -> Result<Self, Error> {
        let decrypted = key.decrypt(buf).map_err(|e| Error::Encryption(e))?;
        Self::decompress(&decrypted)
    }
}

#[derive(Debug)]
pub enum Error {
    MissingLot,
    Uuid(uuid::Error),
    Database(db::Error),
    Encoding(bitcode::Error),
    Compression(io::Error),
    Encryption(encrypt::Error),
}

impl From<uuid::Error> for Error {
    fn from(err: uuid::Error) -> Self {
        Error::Uuid(err)
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
    use crate::{
        encrypt::Key,
        lot::{Lot, LotKey},
        user::User,
    };

    #[test]
    fn new() {
        let lot = Lot::new("test");
        let record = Record::new(&lot, RecordData::plain("foo", "bar"));
        assert_eq!(lot.uuid(), &record.lot);
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
        let key = LotKey(Key::new());
        let record = Record::new(&lot, RecordData::plain("foo", "bar"));
        let encrypted = record.encrypt(&key).expect("failed to encrypt");
        let decrypted_data = RecordData::decrypt(&encrypted, &key).expect("failed to decrypt");
        assert_eq!(record.data, decrypted_data);
    }

    #[tokio::test]
    #[ignore]
    async fn save() {}

    #[tokio::test]
    async fn insert() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".into()).expect("failed to make user");
        let mut lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let inserted_uuid = Record::new(&lot, RecordData::plain("foo", "bar"))
            .insert(&db, &mut lot)
            .await
            .expect("failed to insert record");
        assert_eq!(lot.uuid(), &lot.records()[0].lot);
        assert_eq!(inserted_uuid, lot.records()[0].uuid);
    }

    #[tokio::test]
    async fn load_all() {
        let db = Database::new("sqlite://:memory:")
            .await
            .expect("failed to create database");
        let user = User::new("nixpulvis", "password".into()).expect("failed to make user");
        let mut lot = Lot::new("lot a");
        lot.save(&db, &user).await.expect("failed to save lot");
        let record = Record::new(&lot, RecordData::plain("foo", "bar"));
        let inserted_uuid = record
            .insert(&db, &mut lot)
            .await
            .expect("failed to insert record");
        let records = Record::load_all(&db, &lot)
            .await
            .expect("failed to load records");
        assert_eq!(lot.uuid(), &records[0].lot);
        assert_eq!(inserted_uuid, records[0].uuid);
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
