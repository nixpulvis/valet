use aes_siv::{
    Aes256SivAead, KeySizeUser, Nonce,
    aead::generic_array::typenum::Unsigned,
    aead::{Aead, Key as AesKey, KeyInit},
};
use argon2::Argon2;
use rand_core::{OsRng, RngCore};
use std::marker::PhantomData;
use std::{ops::Deref, pin::Pin};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A safer wrapper for plaintext password strings.
///
/// This structure both pins it's reference and zeros the memory on drop.
//
// TODO: Is there a way in the GUI to avoid cloning the password to send it to
// a async function?
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Password(Pin<String>);

impl Password {
    pub fn empty() -> Self {
        Password(Pin::new(String::new()))
    }

    pub fn as_str(&self) -> &str {
        &*self.0
    }
}

impl From<String> for Password {
    fn from(password: String) -> Self {
        Password(Pin::new(password))
    }
}

// Only allow passwords to be created from immutable static strings when
// testing.
#[cfg(test)]
impl From<&'static str> for Password {
    fn from(password: &'static str) -> Self {
        Password(Pin::new(password.into()))
    }
}

impl From<&mut str> for Password {
    fn from(password: &mut str) -> Self {
        let zeroize = Password(Pin::new(password.into()));
        password.zeroize();
        zeroize
    }
}

impl Deref for Password {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

const NONCE_SIZE: usize = 16;
// This value can be anything really, but is generally recommended to be about
// 128-bits. The idea is that it just needs to contain more entropy than the
// user's password.
pub(crate) const SALT_SIZE: usize = 128 / 8;

/// Represents some encrypted data, which can be decrypted again.
#[derive(Debug, PartialEq, Eq)]
pub struct Encrypted {
    pub(crate) data: Vec<u8>,
    pub(crate) nonce: Vec<u8>,
}

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
pub struct Key<T>(AesKey<Aes256SivAead>, PhantomData<T>);

impl<T> Key<T> {
    pub fn new() -> Self {
        Key(Aes256SivAead::generate_key(&mut OsRng), PhantomData)
    }

    pub fn from_password(password: Password, salt: &[u8]) -> Result<Self, Error> {
        let argon2 = Argon2::default();
        let mut output_key_material = [0u8; <Aes256SivAead as KeySizeUser>::KeySize::USIZE];
        argon2
            .hash_password_into(password.as_bytes(), salt, &mut output_key_material)
            .map_err(|e| Error::KeyDerivation(format!("{}", e)))?;

        Ok(Key(
            AesKey::<Aes256SivAead>::clone_from_slice(&output_key_material),
            PhantomData,
        ))
    }

    /// Construct a Key from a slice of bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Key(
            AesKey::<Aes256SivAead>::clone_from_slice(bytes),
            PhantomData,
        )
    }

    /// Returns this key as a slice of bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub(crate) fn generate_salt() -> [u8; SALT_SIZE] {
        let mut salt = [0; SALT_SIZE];
        OsRng.fill_bytes(&mut salt);
        salt
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Encrypted, Error> {
        // TODO: Better security will be achieved by storing a unique counter
        // for this somewhere.
        let mut nonce_bytes = [0; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let cipher = Aes256SivAead::new(&self.0);
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| Error::Encryption(format!("{}", e)))?;
        Ok(Encrypted {
            data: ciphertext,
            nonce: nonce.as_slice().into(),
        })
    }

    pub fn decrypt(&self, encrypted: &Encrypted) -> Result<Vec<u8>, Error> {
        let nonce = Nonce::from_slice(&encrypted.nonce);
        let cipher = Aes256SivAead::new(&self.0);
        let plaintext = cipher
            .decrypt(nonce, &encrypted.data[..])
            .map_err(|e| Error::Decryption(format!("{}", e)))?;
        Ok(plaintext)
    }
}

#[derive(Debug)]
pub enum Error {
    KeyDerivation(String),
    Encryption(String),
    Decryption(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_password() {
        let salt = Key::<()>::generate_salt();
        let key =
            Key::<()>::from_password("user1password".into(), &salt).expect("error generating key");
        assert_eq!(512 / 8, key.0.len());
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
