use crate::encrypt::{self, Encrypted, Key};
use crate::lot::Lot;
use bitcode::{Decode, Encode};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::{fmt, io};
use uuid::Uuid;

#[derive(PartialEq, Eq)]
pub struct Record {
    pub lot: Rc<Lot>,
    pub uuid: Uuid,
    pub data: RecordData,
}

impl Record {
    pub fn new(lot: Rc<Lot>, data: RecordData) -> Rc<RefCell<Self>> {
        let uuid = Uuid::now_v7();
        Rc::new(RefCell::new(Record { lot, uuid, data }))
    }
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.lot.name, self.data)
    }
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Record")
            .field("lot", &self.lot.uuid)
            .field("uuid", &self.uuid)
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
    pub fn domain(index: &str, values: HashMap<String, String>) -> Self {
        Self::Domain(index.into(), values)
    }

    pub fn plain(index: &str, value: &str) -> Self {
        Self::Plain(index.into(), value.into())
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
        let lot = Lot::new("lot a").expect("failed to create lot");
        let record = Record::new(lot.clone(), RecordData::plain("foo", "bar"));
        assert_eq!(lot.uuid, record.borrow().lot.uuid);
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
    fn data_encode_decode() {
        let record_data = RecordData::plain("index", "secret");
        let encoded = record_data.encode();
        let decoded = RecordData::decode(&encoded).expect("failed to decode");
        assert_eq!(record_data, decoded);
    }

    #[test]
    fn data_compress_decompress() {
        let record_data = RecordData::plain("index", "secret");
        let compressed = record_data.compress().expect("failed to compress");
        let decompressed = RecordData::decompress(&compressed).expect("failed to decompress");
        assert_eq!(record_data, decompressed);
    }

    #[test]
    fn data_encrypt_decrypt() {
        let user = User::new("nixpulvis", "password".into()).expect("failed to make user");
        let record_data = RecordData::plain("index", "secret");
        let encrypted = record_data.encrypt(user.key()).expect("failed to encrypt");
        let decrypted = RecordData::decrypt(&encrypted, user.key()).expect("failed to decrypt");
        assert_eq!(record_data, decrypted);
    }
}
