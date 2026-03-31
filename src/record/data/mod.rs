use bitcode::{Decode, Encode};
use std::{collections::HashMap, fmt, io};

use crate::{
    Lot,
    encrypt::{Encrypted, Key},
    record::Error,
};

#[derive(Encode, Decode, Debug, Eq, PartialEq)]
pub enum Data {
    // TODO: We should really generalize the concept of a "label" to allow for
    // the HashMap to be the label here. We can then store a single (or many)
    // passwords separately.
    // TODO: Use PasswordBuf here.
    Domain(String, HashMap<String, String>),
    Plain(String, String),
}

// TODO: Don't display passwords.
impl fmt::Display for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Data::Domain(label, attributes) => {
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
            Data::Plain(label, text) => {
                if text.contains("\n") {
                    write!(f, "{label}:\n{text}")
                } else {
                    write!(f, "{label}: {text}")
                }
            }
        }
    }
}

impl Data {
    pub fn domain(label: &str, values: HashMap<String, String>) -> Self {
        Self::Domain(label.into(), values)
    }

    pub fn plain(label: &str, value: &str) -> Self {
        Self::Plain(label.into(), value.into())
    }

    pub fn label(&self) -> &str {
        match self {
            Data::Domain(s, _) => &s,
            Data::Plain(s, _) => &s,
        }
    }

    pub fn password(&self) -> &str {
        match self {
            Data::Domain(_, attrs) => {
                if attrs.contains_key("password") {
                    &attrs["password"]
                } else if attrs.len() == 1 {
                    &attrs.iter().next().unwrap().1
                } else {
                    ""
                }
            }
            Data::Plain(_, s) => &s,
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
        let decoded = Data::decode(&decompressed)?;
        Ok(decoded)
    }

    pub fn encrypt(&self, key: &Key<Lot>) -> Result<Encrypted, Error> {
        self._encrypt(key, None)
    }

    pub fn encrypt_with_aad(&self, key: &Key<Lot>, aad: &[u8]) -> Result<Encrypted, Error> {
        self._encrypt(key, Some(aad))
    }

    fn _encrypt(&self, key: &Key<Lot>, aad: Option<&[u8]>) -> Result<Encrypted, Error> {
        let compressed = self.compress()?;
        if let Some(aad) = aad {
            key.encrypt_with_aad(&compressed, aad)
                .map_err(|e| Error::Encryption(e))
        } else {
            key.encrypt(&compressed).map_err(|e| Error::Encryption(e))
        }
    }

    pub fn decrypt(buf: &Encrypted, key: &Key<Lot>) -> Result<Self, Error> {
        Self::_decrypt(buf, key, None)
    }

    pub fn decrypt_with_aad(buf: &Encrypted, key: &Key<Lot>, aad: &[u8]) -> Result<Self, Error> {
        Self::_decrypt(buf, key, Some(aad))
    }

    fn _decrypt(buf: &Encrypted, key: &Key<Lot>, aad: Option<&[u8]>) -> Result<Self, Error> {
        let decrypted = if let Some(aad) = aad {
            key.decrypt_with_aad(buf, aad)
                .map_err(|e| Error::Encryption(e))?
        } else {
            key.decrypt(buf).map_err(|e| Error::Encryption(e))?
        };
        Self::decompress(&decrypted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lot::Lot;

    #[test]
    fn label() {
        let data = Data::plain("plain", "secret");
        assert_eq!("plain", data.label());
        let data = Data::domain("domain", HashMap::new());
        assert_eq!("domain", data.label());
    }

    #[test]
    fn encode_decode() {
        let data = Data::plain("label", "secret");
        let encoded = data.encode();
        let decoded = Data::decode(&encoded).expect("failed to decode");
        assert_eq!(data, decoded);
    }

    #[test]
    fn compress_decompress() {
        let data = Data::plain("label", "secret");
        let compressed = data.compress().expect("failed to compress");
        let decompressed = Data::decompress(&compressed).expect("failed to decompress");
        assert_eq!(data, decompressed);
    }

    #[test]
    fn encrypt_decrypt() {
        let lot = Lot::new("test");
        let data = Data::plain("label", "secret");
        let encrypted = data.encrypt(lot.key()).expect("failed to encrypt");
        let decrypted = Data::decrypt(&encrypted, lot.key()).expect("failed to decrypt");
        assert_eq!(data, decrypted);
    }

    #[test]
    fn encrypt_decrypt_with_aad() {
        let lot = Lot::new("test");
        let data = Data::plain("label", "secret");
        let aad = [1, 2, 3];
        let encrypted = data
            .encrypt_with_aad(lot.key(), &aad)
            .expect("failed to encrypt");
        let decrypted =
            Data::decrypt_with_aad(&encrypted, lot.key(), &aad).expect("failed to decrypt");
        assert_eq!(data, decrypted);
    }
}
