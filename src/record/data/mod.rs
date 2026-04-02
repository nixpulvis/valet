use crate::{
    Lot,
    encrypt::{Encrypted, Key},
    password::Password,
    record::Error,
};
use bitcode::{Decode, Encode};
use std::{collections::HashMap, fmt, io};

#[derive(Encode, Decode, Debug, Eq, PartialEq)]
pub struct Data {
    label: Label,
    password: Password,
    extra: HashMap<String, String>,
}

impl fmt::Display for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label)
    }
}

impl Data {
    pub fn new(label: Label, password: Password) -> Result<Self, Error> {
        if !password.is_valid() {
            return Err(Error::InvalidPassword);
        }
        Ok(Data {
            label,
            password,
            extra: HashMap::new(),
        })
    }

    pub fn add_extra(mut self, attr: String, value: String) -> Self {
        self.extra.insert(attr, value);
        self
    }

    pub fn with_extra(mut self, extra: HashMap<String, String>) -> Self {
        self.extra = extra;
        self
    }

    pub fn label(&self) -> &Label {
        &self.label
    }

    pub fn password<'a>(&'a self) -> &'a Password {
        &self.password
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

mod label;
pub use self::label::Label;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lot::Lot;

    #[test]
    fn label() {
        let data = Data::new(Label::Simple("label".into()), "secretpass".try_into().unwrap()).unwrap();
        assert_eq!("label", format!("{}", data.label()));
    }

    #[test]
    fn extra() {
        let data = Data::new(Label::Simple("label".into()), "secretpass".try_into().unwrap())
            .unwrap()
            .add_extra("foo".into(), "bar".into())
            .add_extra("foo".into(), "bar".into());
        assert_eq!(data.extra.len(), 1);
        assert_eq!(data.extra["foo"], "bar");
    }

    #[test]
    fn encode_decode() {
        let data = Data::new(Label::Simple("label".into()), "secretpass".try_into().unwrap()).unwrap();
        let encoded = data.encode();
        let decoded = Data::decode(&encoded).expect("failed to decode");
        assert_eq!(data, decoded);
    }

    #[test]
    fn compress_decompress() {
        let data = Data::new(Label::Simple("label".into()), "secretpass".try_into().unwrap()).unwrap();
        let compressed = data.compress().expect("failed to compress");
        let decompressed = Data::decompress(&compressed).expect("failed to decompress");
        assert_eq!(data, decompressed);
    }

    #[test]
    fn encrypt_decrypt() {
        let lot = Lot::new("test");
        let data = Data::new(Label::Simple("label".into()), "secretpass".try_into().unwrap()).unwrap();
        let encrypted = data.encrypt(lot.key()).expect("failed to encrypt");
        let decrypted = Data::decrypt(&encrypted, lot.key()).expect("failed to decrypt");
        assert_eq!(data, decrypted);
    }

    #[test]
    fn encrypt_decrypt_with_aad() {
        let lot = Lot::new("test");
        let data = Data::new(Label::Simple("label".into()), "secretpass".try_into().unwrap()).unwrap();
        let aad = [1, 2, 3];
        let encrypted = data
            .encrypt_with_aad(lot.key(), &aad)
            .expect("failed to encrypt");
        let decrypted =
            Data::decrypt_with_aad(&encrypted, lot.key(), &aad).expect("failed to decrypt");
        assert_eq!(data, decrypted);
    }

    #[test]
    fn new_rejects_invalid_password() {
        let invalid_password: Password = "short".try_into().unwrap();
        let result = Data::new(Label::Simple("label".into()), invalid_password);
        assert!(matches!(result, Err(Error::InvalidPassword)));
    }
}
