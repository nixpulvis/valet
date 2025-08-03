use crate::encrypt::{Encrypted, Error, Password};
use aes_gcm_siv::{
    Aes256GcmSiv, KeySizeUser, Nonce,
    aead::{Aead, Key as AesKey, KeyInit, generic_array::typenum::Unsigned},
};
use argon2::Argon2;
use rand_core::{OsRng, RngCore};
use std::marker::PhantomData;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A generic symmetric key used to achive privacy and integrity.
///
/// This struct is generic over any type `T` to allow users to specify functions
/// which expect spesific kinds of keys. For example, [`User::key`] returns a
/// [`Key<User>`], whereas [`Lot::key`] returns a [`Key<Lot>`]. This helps
/// prevent accidental misuse of keys.
///
/// [`User::key`]: crate::user::User::key
/// [`Lot::key`]: crate::lot::Lot::key
//
// TODO: #15
// Aes256 has a 512-bit key size, and achieves 256-bit security.
#[derive(PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Key<T>(AesKey<Aes256GcmSiv>, PhantomData<T>);

impl<T> Key<T> {
    pub fn new() -> Self {
        Key(Aes256GcmSiv::generate_key(&mut OsRng), PhantomData)
    }

    pub fn from_password(password: Password, salt: &[u8]) -> Result<Self, Error> {
        let argon2 = Argon2::default();
        let mut output_key_material = [0u8; <Aes256GcmSiv as KeySizeUser>::KeySize::USIZE];
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut output_key_material)
            .map_err(|e| Error::KeyDerivation(format!("{}", e)))?;

        Ok(Key(
            AesKey::<Aes256GcmSiv>::clone_from_slice(&output_key_material),
            PhantomData,
        ))
    }

    /// Construct a Key from a slice of bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Key(AesKey::<Aes256GcmSiv>::clone_from_slice(bytes), PhantomData)
    }

    /// Returns this key as a slice of bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Encrypted, Error> {
        let mut nonce = Nonce::default();
        OsRng.fill_bytes(&mut nonce.as_mut_slice());

        let cipher = Aes256GcmSiv::new(&self.0);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| Error::Encryption(format!("{}", e)))?;
        Ok(Encrypted {
            data: ciphertext,
            nonce: nonce.as_slice().into(),
        })
    }

    pub fn decrypt(&self, encrypted: &Encrypted) -> Result<Vec<u8>, Error> {
        let nonce = Nonce::from_slice(&encrypted.nonce);
        let cipher = Aes256GcmSiv::new(&self.0);
        let plaintext = cipher
            .decrypt(nonce, &encrypted.data[..])
            .map_err(|e| Error::Decryption(format!("{}", e)))?;
        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encrypt::generate_salt;

    #[test]
    fn from_password() {
        let salt = generate_salt();
        let key =
            Key::<()>::from_password("user1password".into(), &salt).expect("error generating key");
        assert_eq!(256 / 8, key.0.len());
    }

    #[test]
    fn encrypt_decrypt_test() {
        let key = Key::<()>::new();
        let plaintext = b"this is a secret";
        let encrypted = key.encrypt(plaintext).expect("error encrypting");
        let decrypted = key.decrypt(&encrypted).expect("error dencrypting");
        assert_eq!(plaintext, &decrypted[..]);
    }

    #[test]
    fn as_from_bytes_test() {
        let key_a = Key::<()>::new();
        let bytes = key_a.as_bytes();
        let key_b = Key::<()>::from_bytes(bytes);
        // The same key still shouldn't produce the same ciphertext. We
        // shouldn't panic though.
        assert_ne!(
            key_a.encrypt(b"").expect("error encrypting"),
            key_b.encrypt(b"").expect("error encrypting")
        );
    }

    #[test]
    #[should_panic]
    fn from_bytes_panic_test() {
        let key = Key::<()>::new();
        let bytes = key.as_bytes();
        Key::<()>::from_bytes(&bytes[0..5]);
    }
}
