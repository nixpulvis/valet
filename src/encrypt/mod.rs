use aes_gcm_siv::aead;
use rand_core::{OsRng, RngCore};
use std::io;

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

/// AES-GCM-SIV nonce size in bytes. Fixed at 96 bits; used to split packed
/// `nonce || ciphertext` blobs.
#[cfg(feature = "db")]
pub(crate) const NONCE_SIZE: usize = 12;

#[cfg(feature = "db")]
impl Encrypted {
    /// Pack `nonce || ciphertext` into a single blob for storage where a
    /// separate nonce column is not desired.
    pub(crate) fn pack(&self) -> Vec<u8> {
        debug_assert_eq!(self.nonce.len(), NONCE_SIZE);
        let mut out = Vec::with_capacity(self.nonce.len() + self.data.len());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.data);
        out
    }

    /// Split a packed blob back into nonce + ciphertext. Panics in debug
    /// builds if `bytes` is shorter than [`NONCE_SIZE`].
    pub(crate) fn unpack(bytes: &[u8]) -> Self {
        debug_assert!(bytes.len() >= NONCE_SIZE);
        let (nonce, data) = bytes.split_at(NONCE_SIZE);
        Encrypted {
            data: data.to_vec(),
            nonce: nonce.to_vec(),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    KeyDerivation(String),
    Encryption(aead::Error),
    Decryption(aead::Error),
    Decoding(bitcode::Error),
    Compression(io::Error),
    Decompression(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::KeyDerivation(s) => write!(f, "key derivation: {s}"),
            Error::Encryption(e) => write!(f, "encryption: {e}"),
            Error::Decryption(e) => write!(f, "decryption: {e}"),
            Error::Decoding(e) => write!(f, "decoding: {e}"),
            Error::Compression(e) => write!(f, "compression: {e}"),
            Error::Decompression(e) => write!(f, "decompression: {e}"),
        }
    }
}

impl std::error::Error for Error {}

mod key;
mod stash;
pub use self::key::Key;
pub use self::stash::Stash;
