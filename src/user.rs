use aes_siv::{Aes256SivAead, KeySizeUser, aead::Key};
use argon2::Argon2;
use rand::{Rng, rngs::OsRng};

const SALT_SIZE: usize = 16;
const CREDENTIAL_SIZE: usize = 64;

/// Usernames and the salt for their password are store in a database.
pub struct Registration {
    username: String,
    salt: [u8; SALT_SIZE],
}

impl Registration {
    pub fn new(username: &str) -> Result<Self, ()> {
        let mut salt = [0; SALT_SIZE];
        let mut rng = OsRng::new().map_err(|_| ())?;
        rng.try_fill(&mut salt).map_err(|_| ())?;
        Ok(Registration {
            username: username.into(),
            salt: salt,
        })
    }
}

/// A credential is generated from a user's registration and thier password.
pub struct Credential(Key<Aes256SivAead>);

impl Credential {
    pub fn new(registration: &Registration, password: &str) -> Result<Self, ()> {
        let argon2 = Argon2::default();
        assert_eq!(CREDENTIAL_SIZE, Aes256SivAead::key_size());
        let mut output_key_material = [0u8; CREDENTIAL_SIZE];
        argon2
            .hash_password_into(
                password.as_bytes(),
                &registration.salt,
                &mut output_key_material,
            )
            .map_err(|_| ())?;

        Ok(Credential(Key::<Aes256SivAead>::clone_from_slice(
            &output_key_material,
        )))
    }

    pub fn key(&self) -> &Key<Aes256SivAead> {
        &self.0
    }
}

#[test]
fn new_credential() {
    let registration = Registration::new("user1").expect("error registering user");
    let credential =
        Credential::new(&registration, "user1password").expect("error generating credential");

    assert_eq!(CREDENTIAL_SIZE, credential.0.len());
}
