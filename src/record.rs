use crate::encrypt::{self, Encrypted, Key};
use bitcode::{Decode, Encode};
use std::collections::HashMap;
use std::io;

#[derive(Encode, Decode, Clone, Debug, Eq, PartialEq)]
pub enum Record {
    Domain(String, HashMap<String, String>),
    Plain(String, String),
}

impl Record {
    pub fn domain(index: &str, values: HashMap<String, String>) -> Self {
        Self::Domain(index.into(), values)
    }

    pub fn plain(index: &str, value: &str) -> Self {
        Self::Plain(index.into(), value.into())
    }

    pub fn label(&self) -> &str {
        match self {
            Record::Domain(s, _) => &s,
            Record::Plain(s, _) => &s,
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
        let decoded = Record::decode(&decompressed)?;
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
    Encoding(bitcode::Error),
    Compression(io::Error),
    Encryption(encrypt::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::User;

    #[test]
    fn encode_decode() {
        let record = Record::plain("index", "secret");
        let encoded = record.encode();
        let decoded = Record::decode(&encoded).expect("failed to decode");
        assert_eq!(record, decoded);
    }

    #[test]
    fn compress_decompress() {
        let record = Record::plain("index", "secret");
        let compressed = record.compress().expect("failed to compress");
        let decompressed = Record::decompress(&compressed).expect("failed to decompress");
        assert_eq!(record, decompressed);
    }

    #[test]
    fn encrypt_decrypt() {
        let user = User::new("nixpulvis", "password").expect("failed to make user");
        let record = Record::plain("index", "secret");
        let encrypted = record.encrypt(user.key()).expect("failed to encrypt");
        let decrypted = Record::decrypt(&encrypted, user.key()).expect("failed to decrypt");
        assert_eq!(record, decrypted);
    }
}
