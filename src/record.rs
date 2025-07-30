use crate::encrypt::{self, Encrypted, Key};
use crate::lot::Lot;
use bitcode::{Decode, Encode};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::{fmt, io};
use uuid::Uuid;

pub struct Record {
    pub lot: Weak<Lot>,
    pub uuid: Uuid,
    pub data: RecordData,
}

impl Record {
    pub fn new(lot: Weak<Lot>, data: RecordData) -> Arc<RefCell<Self>> {
        let uuid = Uuid::now_v7();
        Arc::new(RefCell::new(Record { lot, uuid, data }))
    }

    pub fn encrypt(&self) -> Result<Encrypted, Error> {
        if let Some(lot) = self.lot.upgrade() {
            self.data.encrypt(lot.key())
        } else {
            Err(Error::MissingLot)
        }
    }
}

impl PartialEq for Record {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid && self.data == other.data
    }
}
impl Eq for Record {}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(lot) = self.lot.upgrade() {
            write!(f, "{}::{}", lot.name, self.data)
        } else {
            write!(f, "<missing>::{}", self.data)
        }
    }
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lot = if let Some(lot) = self.lot.upgrade() {
            &lot.name.clone()
        } else {
            "<missing>"
        };
        f.debug_struct("Record")
            .field("uuid", &self.uuid)
            .field("lot", &lot)
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
                // TODO
                write!(f, "{label}")?;
                for attribute in attributes {
                    write!(f, "{:?}", attribute)?;
                }
                Ok(())
            }
            RecordData::Plain(label, text) => {
                if text.contains("\n") {
                    write!(f, "{label}\n{text}")
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

    pub fn encrypt(&self, key: &Key) -> Result<Encrypted, Error> {
        let compressed = self.compress()?;
        key.encrypt(&compressed).map_err(|e| Error::Encryption(e))
    }

    pub fn decrypt(buf: &Encrypted, key: &Key) -> Result<Self, Error> {
        let decrypted = key.decrypt(buf).map_err(|e| Error::Encryption(e))?;
        Self::decompress(&decrypted)
    }
}

#[derive(Debug)]
pub enum Error {
    MissingLot,
    Uuid(uuid::Error),
    Encoding(bitcode::Error),
    Compression(io::Error),
    Encryption(encrypt::Error),
}

impl From<uuid::Error> for Error {
    fn from(err: uuid::Error) -> Self {
        Error::Uuid(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::User;

    #[test]
    fn new() {
        let lot = Lot::new("lot a");
        let record = Record::new(Arc::downgrade(&lot), RecordData::plain("foo", "bar"));
        assert_eq!(
            lot.uuid,
            record
                .borrow()
                .lot
                .upgrade()
                .expect("record's lot exists")
                .uuid
        );
        assert_eq!(36, record.borrow().uuid.to_string().len());
        match record.borrow().data {
            RecordData::Plain(ref label, ref value) => {
                assert_eq!("foo", label);
                assert_eq!("bar", value);
            }
            _ => assert!(false),
        }
    }

    #[test]
    fn encrypt_decrypt() {
        let lot = Lot::new("lot a");
        let record = lot.new_record(RecordData::plain("foo", "bar"));
        let encrypted = record.borrow().encrypt().expect("failed to encrypt");
        let decrypted = lot.decrypt_record(&encrypted).expect("failed to decrypt");
        if let (Some(a), Some(b)) = (
            record.borrow().lot.upgrade(),
            decrypted.borrow().lot.upgrade(),
        ) {
            assert_eq!(a, b);
        } else {
            assert!(false);
        }
        assert_ne!(record.borrow().uuid, decrypted.borrow().uuid);
        assert_eq!(record.borrow().data, decrypted.borrow().data);
    }

    #[test]
    fn formatting_without_lot() {
        let record = Record::new(Weak::new(), RecordData::plain("foo", "bar"));
        let debug = format!("{:?}", &record);
        assert!(debug.contains("<missing>"));
        let display = format!("{}", &record.borrow());
        assert!(display.contains("<missing>"));
    }

    #[test]
    fn label() {
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
        let user = User::new("nixpulvis", "password".into()).expect("failed to make user");
        let data = RecordData::plain("label", "secret");
        let encrypted = data.encrypt(user.key()).expect("failed to encrypt");
        let decrypted = RecordData::decrypt(&encrypted, user.key()).expect("failed to decrypt");
        assert_eq!(data, decrypted);
    }
}
