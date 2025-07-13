use crate::Credential;
use aes_siv::{
    Aes256SivAead, Nonce,
    aead::{Aead, KeyInit},
};
use rand::{Rng, rngs::OsRng};

const NONCE_SIZE: usize = 16;

// TODO: Should this remain on bytes, or handle conversion from actual secret record datatypes.
pub struct Record(Vec<u8>);

impl Record {
    pub fn encrypt(&self, credential: &Credential) -> Result<EncryptedRecord, ()> {
        // TODO: Better security will be achieved by storing a unique counter for this somewhere.
        let mut nonce_bytes = [0; NONCE_SIZE];
        let mut rng = OsRng::new().map_err(|_| ())?;
        rng.try_fill(&mut nonce_bytes).map_err(|_| ())?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        let cipher = Aes256SivAead::new(credential.key());
        let ciphertext = cipher.encrypt(nonce, &self.0[..]).map_err(|_| ())?;
        Ok(EncryptedRecord {
            ciphertext,
            nonce: nonce.as_slice().into(),
        })
    }
}

pub struct EncryptedRecord {
    ciphertext: Vec<u8>,
    nonce: Vec<u8>,
}

impl EncryptedRecord {
    pub fn decrypt(&self, credential: &Credential) -> Result<Record, ()> {
        let nonce = Nonce::from_slice(&self.nonce);
        let cipher = Aes256SivAead::new(credential.key());
        let plaintext = cipher
            .decrypt(nonce, &self.ciphertext[..])
            .map_err(|_| ())?;
        Ok(Record(plaintext))
    }
}

#[test]
fn encrypt_decrypt_test() {
    use crate::prelude::Registration;

    let registration = Registration::new("user1").expect("error registering user");
    let credential =
        Credential::new(&registration, "user1password").expect("error generating credentials");

    let record = Record(b"this is a secret".into());
    let encrypted = record.encrypt(&credential).expect("error encrypting");
    let decrypted = encrypted.decrypt(&credential).expect("error dencrypting");

    assert_eq!(&record.0, &decrypted.0);
}
