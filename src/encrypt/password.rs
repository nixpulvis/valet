use std::pin::Pin;
use std::{marker::PhantomPinned, ops::Deref};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A safer wrapper for plaintext password strings.
///
/// This type both pins it's reference and zeros the memory on drop. When given
/// a [`PasswordBuf`] it's best to immediately convert it into a [`Password`] to
/// avoid accidental moves.
pub type Password<'a> = Pin<&'a mut PasswordBuf>;

#[macro_export]
macro_rules! pw {
    ($e:expr) => {
        std::pin::pin!($crate::encrypt::PasswordBuf::from($e))
    };
}

/// A safe wrapper for plaintext password strings.
///
/// This structure zeros it's memory on drop. It's safer to use a [`Password`]
/// since it also prevents accidental moves, which may copy the memory.
//
// TODO: Is there a way in the GUI to avoid cloning the password to send it to
// a async function?
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct PasswordBuf(String, PhantomPinned);

impl PasswordBuf {
    pub fn empty() -> Self {
        PasswordBuf(String::new(), PhantomPinned)
    }

    pub fn as_mut(&mut self) -> &mut String {
        &mut self.0
    }
}

impl From<String> for PasswordBuf {
    fn from(password: String) -> Self {
        PasswordBuf(password, PhantomPinned)
    }
}

// Only allow passwords to be created from immutable static strings when
// testing.
#[cfg(test)]
impl From<&'static str> for PasswordBuf {
    fn from(password: &'static str) -> Self {
        PasswordBuf(password.into(), PhantomPinned)
    }
}

impl From<&mut str> for PasswordBuf {
    fn from(password: &mut str) -> Self {
        let zeroize = PasswordBuf(password.into(), PhantomPinned);
        password.zeroize();
        zeroize
    }
}

impl Deref for PasswordBuf {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}
