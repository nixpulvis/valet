use rand::{Rng, rngs::OsRng};

use crate::db::{Database, Error};
use crate::encrypt::{Encrypted, SALT_SIZE};
use crate::user::User;

pub struct Users;

impl Users {
    /// TODO
    ///
    /// Takes ownership of `password` because the returned `User`
    /// has it's key and we shouldn't be reusing the password after
    /// this point.
    pub async fn register(db: &Database, username: &str, password: String) -> Result<User, Error> {
        let user = User::new(username, &password)?;

        let (username, salt, data, nonce): (String, Vec<u8>, Vec<u8>, Vec<u8>) = sqlx::query_as(
            r"
            INSERT INTO users (username, salt, validation_data, validation_nonce)
            VALUES (?, ?, ?, ?)
            RETURNING username, salt, validation_data, validation_nonce
            ",
        )
        .bind(&user.username)
        .bind(&user.salt[..])
        .bind(&user.validation.data[..])
        .bind(&user.validation.nonce[..])
        .fetch_one(db.pool())
        .await?;

        Self::load_and_clobber_user(username, password, salt, data, nonce)
    }

    /// TODO
    ///
    /// Takes ownership of `password` because the returned `User`
    /// has it's key and we shouldn't be reusing the password after
    /// this point.
    pub async fn get(db: &Database, username: &str, password: String) -> Result<User, Error> {
        let (username, salt, data, nonce): (String, Vec<u8>, Vec<u8>, Vec<u8>) = sqlx::query_as(
            r"
            SELECT username, salt, validation_data, validation_nonce
            FROM users
            WHERE username = ?
            ",
        )
        .bind(username)
        .fetch_one(db.pool())
        .await?;

        Self::load_and_clobber_user(username, password, salt, data, nonce)
    }

    fn load_and_clobber_user(
        username: String,
        password: String,
        salt: Vec<u8>,
        data: Vec<u8>,
        nonce: Vec<u8>,
    ) -> Result<User, Error> {
        let salt: [u8; SALT_SIZE] = salt.try_into().map_err(|_| Error::SaltError)?;
        let validation = Encrypted { data, nonce };
        let user = User::load(username, &password, salt, validation)?;
        unsafe {
            Self::clobber_password(password);
        }
        Ok(user)
    }

    // We make no attempt to create valid UTF-8 strings here, this is
    // just to protect the memory, after this is called no uses of
    // `password` should be made.
    unsafe fn clobber_password(mut password: String) {
        unsafe {
            let bytes = password.as_mut_vec();
            if let Ok(mut rng) = OsRng::new()
                && let Ok(_) = rng.try_fill(&mut bytes[..])
            {
            } else {
                panic!("Critical failure.")
            }
        }
    }
}
