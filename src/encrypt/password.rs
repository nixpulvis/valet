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
