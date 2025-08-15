use std::ops::Deref;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A safer wrapper for plaintext password strings.
///
/// This structure zeros it's memory on drop.
//
// TODO: Is there a way in the GUI to avoid cloning the password to send it to
// a async function?
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Password(String);

impl Password {
    pub fn empty() -> Self {
        Password(String::new())
    }

    pub fn as_mut(&mut self) -> &mut String {
        &mut self.0
    }
}

impl From<String> for Password {
    fn from(password: String) -> Self {
        Password(password)
    }
}

// Only allow passwords to be created from immutable static strings when
// testing.
#[cfg(test)]
impl From<&'static str> for Password {
    fn from(password: &'static str) -> Self {
        Password(password.into())
    }
}

impl From<&mut str> for Password {
    fn from(password: &mut str) -> Self {
        let zeroize = Password(password.into());
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
