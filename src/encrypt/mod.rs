use rand_core::{OsRng, RngCore};

// This value can be anything really, but is generally recommended to be about
// 128-bits. The idea is that it just needs to contain more entropy than the
// user's password.
pub(crate) const SALT_SIZE: usize = 128 / 8;

pub(crate) fn generate_salt() -> [u8; SALT_SIZE] {
    let mut salt = [0; SALT_SIZE];
    OsRng.fill_bytes(&mut salt);
    salt
}

/// Represents some encrypted data, which can be decrypted again.
#[derive(Debug, PartialEq, Eq)]
pub struct Encrypted {
    pub(crate) data: Vec<u8>,
    pub(crate) nonce: Vec<u8>,
}

#[derive(Debug)]
pub enum Error {
    KeyDerivation(String),
    Encryption(String),
    Decryption(String),
}

mod key;
pub use self::key::Key;
mod password;
pub use self::password::Password;
