//! The **Valet** password manager framework.
//!
//! TODO

/// Main storage for keys, passwords, and secrets.
#[derive(Debug)]
pub struct Lot;

#[cfg(feature = "gui")]
mod gui;
pub mod prelude;

use aes_siv::{
    Aes256SivAead, KeySizeUser, Nonce,
    aead::{Aead, Key, KeyInit},
};
use argon2::Argon2;
use rand::{Rng, rngs::OsRng};

const KEY_SIZE: usize = 64;
const NONCE_SIZE: usize = 16;

pub fn key_from_password(password: &str, salt: &str) -> Result<Aes256SivAead, ()> {
    let argon2 = Argon2::default();
    assert_eq!(KEY_SIZE, Aes256SivAead::key_size());
    let mut output_key_material = [0u8; KEY_SIZE];
    argon2
        .hash_password_into(
            password.as_bytes(),
            salt.as_bytes(),
            &mut output_key_material,
        )
        .map_err(|_| ())?;

    let key = Key::<Aes256SivAead>::from_slice(&output_key_material);
    Ok(Aes256SivAead::new(&key))
}

pub fn encrypt(cipher: &Aes256SivAead, message: &[u8]) -> Result<(Vec<u8>, Vec<u8>), ()> {
    // TODO: Better security will be achieved by storing a unique counter for this somewhere.
    let mut nonce_bytes = [0; NONCE_SIZE];
    let mut rng = OsRng::new().map_err(|_| ())?;
    rng.try_fill(&mut nonce_bytes).map_err(|_| ())?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, message).map_err(|_| ())?;
    Ok((ciphertext, nonce.as_slice().into()))
}

pub fn decrypt(cipher: &Aes256SivAead, bundle: (&[u8], &[u8])) -> Result<Vec<u8>, ()> {
    let nonce = Nonce::from_slice(bundle.1);
    cipher.decrypt(nonce, bundle.0).map_err(|_| ())
}

#[test]
fn key_from_password_test() {
    let cipher = key_from_password("user1password", "user1salt").expect("error generating key");
    let nonce = Nonce::from_slice(b"1234567890123456");

    let ciphertext = cipher
        .encrypt(nonce, b"plaintext".as_ref())
        .expect("error encrypting");
    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .expect("error decrypting");

    assert_eq!(&plaintext, b"plaintext");
}

#[test]
fn encrypt_decrypt_test() {
    let cipher = key_from_password("user1password", "user1salt").expect("error generating key");
    let ciphertext_bundle = encrypt(&cipher, b"plaintext").expect("error encrypting");
    let bundle = (&ciphertext_bundle.0[..], &ciphertext_bundle.1[..]);
    let plaintext = decrypt(&cipher, bundle).expect("error decrypting");
    assert_eq!(&plaintext, b"plaintext");
}
