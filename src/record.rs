use crate::encrypt::{self, Encrypted};
use crate::lot::LotKey;
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
    pub fn new(lot: Uuid, data: RecordData) -> Self {
        Record {
            lot,
            uuid: Uuid::now_v7(),
            data,
        }
    }

    pub fn lot(&self) -> &Uuid {
        &self.lot
    }

    pub fn uuid(&self) -> &Uuid {
        &self.uuid
    }

    pub fn data(&self) -> &RecordData {
        &self.data
    }

    pub fn encrypt(&self, key: &LotKey) -> Result<Encrypted, Error> {
        self.data.encrypt(key)
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
        // Only display label and data, as we don't have a reference to the lot name.
        write!(f, "{}::{}", self.lot, self.data)
    }
}

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Record")
            .field("uuid", &self.uuid)
            .field("lot_uuid", &self.lot)
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
    use crate::{
        encrypt::Key,
        lot::{Lot, LotKey},
    };
    use uuid::Uuid;

    #[test]
    fn new() {
        let lot_uuid = Uuid::now_v7();
        let record = Record::new(lot_uuid, RecordData::plain("foo", "bar"));
        assert_eq!(lot_uuid, record.lot);
        assert_eq!(36, record.uuid.to_string().len());
        match record.data {
            RecordData::Plain(ref label, ref value) => {
                assert_eq!("foo", label);
                assert_eq!("bar", value);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn encrypt_decrypt() {
        let lot_uuid = Uuid::now_v7();
        let key = LotKey(Key::new());
        let record = Record::new(lot_uuid, RecordData::plain("foo", "bar"));
        let encrypted = record.encrypt(&key).expect("failed to encrypt");
        let decrypted_data = RecordData::decrypt(&encrypted, &key).expect("failed to decrypt");
        assert_eq!(record.data, decrypted_data);
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
        let lot = Lot::new("test");
        let data = RecordData::plain("label", "secret");
        let encrypted = data.encrypt(lot.key()).expect("failed to encrypt");
        let decrypted = RecordData::decrypt(&encrypted, lot.key()).expect("failed to decrypt");
        assert_eq!(data, decrypted);
    }
}
