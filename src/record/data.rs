use crate::{encrypt::Stash, lot::Lot, password::Password};
use bitcode::{Decode, Encode};
use std::collections::HashMap;

/// A record's secret payload: the password plus any attributes that are only
/// meaningful once the record is opened.
///
/// `Data` is encrypted as a unit and is never read by [`RecordIndex`].
/// Contrast with [`Label::extra`], which holds key/value pairs that need to be
/// searchable via the index without decrypting `Data`.
///
/// Rule of thumb: if a field drives search or disambiguation (e.g. `username`,
/// `url`), put it on [`Label::extra`]. If it is opaque supplementary content
/// (e.g. `notes`, recovery codes, TOTP secrets), put it on [`Data::extra`].
///
/// [`Label::extra`]: crate::record::Label::extra
/// [`RecordIndex`]: crate::record::RecordIndex
#[derive(Encode, Decode, Debug, Eq, PartialEq)]
pub struct Data {
    password: Password,
    /// Opaque supplementary attributes. Encrypted as part of the enclosing
    /// [`Data`] under the lot key (see [`Stash<Lot>`]); not visible to
    /// [`RecordIndex`](crate::record::RecordIndex). See the [`Data`] type
    /// docs for when to use this vs. [`Label::extra`](crate::record::Label::extra).
    extra: HashMap<String, String>,
}

impl Stash<Lot> for Data {}

impl Data {
    pub fn new(password: Password) -> Self {
        Data {
            password,
            extra: HashMap::new(),
        }
    }

    pub fn add_extra(mut self, attr: String, value: String) -> Self {
        self.extra.insert(attr, value);
        self
    }

    pub fn with_extra(mut self, extra: HashMap<String, String>) -> Self {
        self.extra = extra;
        self
    }

    pub fn password(&self) -> &Password {
        &self.password
    }

    pub fn extra(&self) -> &HashMap<String, String> {
        &self.extra
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lot::Lot;

    #[test]
    fn extra() {
        let data = Data::new("secret".try_into().unwrap())
            .add_extra("foo".into(), "bar".into())
            .add_extra("foo".into(), "bar".into());
        assert_eq!(data.extra.len(), 1);
        assert_eq!(data.extra["foo"], "bar");
    }

    #[test]
    fn encode_decode() {
        let data = Data::new("secret".try_into().unwrap());
        let encoded = data.encode();
        let decoded = Data::decode(&encoded).expect("failed to decode");
        assert_eq!(data, decoded);
    }

    #[test]
    fn compress_decompress() {
        let data = Data::new("secret".try_into().unwrap());
        let compressed = data.compress().expect("failed to compress");
        let decompressed = Data::decompress(&compressed).expect("failed to decompress");
        assert_eq!(data, decompressed);
    }

    #[test]
    fn encrypt_decrypt() {
        let lot = Lot::new("test");
        let data = Data::new("secret".try_into().unwrap());
        let encrypted = data.encrypt(lot.key()).expect("failed to encrypt");
        let decrypted = Data::decrypt(&encrypted, lot.key()).expect("failed to decrypt");
        assert_eq!(data, decrypted);
    }

    #[test]
    fn encrypt_decrypt_with_aad() {
        let lot = Lot::new("test");
        let data = Data::new("secret".try_into().unwrap());
        let aad = [1, 2, 3];
        let encrypted = data
            .encrypt_with_aad(lot.key(), &aad)
            .expect("failed to encrypt");
        let decrypted =
            Data::decrypt_with_aad(&encrypted, lot.key(), &aad).expect("failed to decrypt");
        assert_eq!(data, decrypted);
    }
}
