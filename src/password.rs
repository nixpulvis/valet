use bitcode::{Decode, Encode};
use std::fmt;
use std::pin::Pin;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const MIN_LENGTH: usize = 8;
pub const MAX_LENGTH: usize = 255;

/// A safe wrapper for plaintext password strings.
///
/// This structure zeros it's memory on drop. It also prevents moving the
/// underlying heap allocated string buffer. To do this it's a fixed allocation
/// of 255 bytes.
//
// TODO: Implement Secret for unpinned larger secrets.
//
// TODO: Is there a way in the GUI to avoid cloning the password to send it to
// a async function?
#[derive(Encode, Decode, Zeroize, ZeroizeOnDrop, Clone, Eq, PartialEq)]
pub struct Password(Pin<Box<[u8; MAX_LENGTH]>>);

impl Password {
    /// Generate a random 20-character password from `[A-Za-z0-9!@#$%^&*]`.
    ///
    /// Uses rejection sampling to avoid modulo bias.
    pub fn generate() -> Self {
        use rand_core::{OsRng, RngCore};
        const CHARSET: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*";
        let mut rng = OsRng;
        let mut buf = [0u8; 1];
        let mut password = String::with_capacity(20);
        while password.len() < 20 {
            rng.fill_bytes(&mut buf);
            let idx = buf[0] as usize;
            if idx < 256 - (256 % CHARSET.len()) {
                password.push(CHARSET[idx % CHARSET.len()] as char);
            }
        }
        password.as_str().try_into().unwrap()
    }

    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }

    // TODO: Impose some more requirements
    pub fn is_valid(&self) -> bool {
        self.as_bytes().len() >= MIN_LENGTH
    }

    pub fn as_bytes(&self) -> &[u8] {
        let null_pos = self.0.iter().position(|c| *c == 0).unwrap_or(self.0.len());
        &self.0[0..null_pos]
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        let null_pos = self.0.iter().position(|c| *c == 0).unwrap_or(self.0.len());
        &mut self.0[0..null_pos]
    }

    pub fn as_str(&self) -> &str {
        unsafe { str::from_utf8_unchecked(self.as_bytes()) }
    }

    /// # Safety
    ///
    /// The caller must ensure that any writes through the returned
    /// `&mut str` leave valid UTF-8 in the underlying buffer.
    pub unsafe fn as_str_mut(&mut self) -> &mut str {
        unsafe { str::from_utf8_unchecked_mut(self.as_bytes_mut()) }
    }
}

// TODO: We shouldn't even allow empty passwords at all, or provide a
// Password::valid method.
impl Default for Password {
    fn default() -> Self {
        Password(Box::pin([0; MAX_LENGTH]))
    }
}

impl TryFrom<&str> for Password {
    type Error = ();

    fn try_from(str: &str) -> Result<Self, Self::Error> {
        if str.len() > MAX_LENGTH {
            return Err(());
        }
        let mut buf = [0; MAX_LENGTH];
        for (d, s) in buf.iter_mut().zip(str.bytes()) {
            *d = s;
        }
        Ok(Password(Box::pin(buf)))
    }
}

// TODO: Consider a more explicit way to reveal passwords.
impl fmt::Display for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(utf8) = str::from_utf8(&*self.0) {
            let null_pos = utf8.find('\0').unwrap_or(utf8.len());
            write!(f, "{}", &utf8[0..null_pos])
        } else {
            write!(f, "<invalid utf8>")
        }
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(***)")
    }
}

#[cfg(feature = "gui")]
use eframe::egui::TextBuffer;
#[cfg(feature = "gui")]
use std::{any::TypeId, ops::Range};

#[cfg(feature = "gui")]
impl TextBuffer for Password {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        self.as_str()
    }

    fn insert_text(&mut self, text: &str, char_index: usize) -> usize {
        if char_index + text.len() > self.0.len() {
            return 0;
        }
        self.0[char_index..].rotate_right(text.len());
        for (d, s) in self.0[char_index..].iter_mut().zip(text.as_bytes()) {
            *d = *s;
        }
        text.len()
    }

    fn delete_char_range(&mut self, char_range: Range<usize>) {
        let l = self.0.len();
        self.0[char_range.start..].rotate_left(char_range.len());
        self.0[l - char_range.len()..].fill(0);
    }

    fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_empty() {
        let password: Password = "".try_into().unwrap();
        assert!(password.is_empty());
    }

    #[test]
    fn is_valid() {
        let valid: Password = "12345678".try_into().unwrap();
        assert!(valid.is_valid());
        let invalid: Password = "123".try_into().unwrap();
        assert!(!invalid.is_valid());
    }

    #[test]
    fn from_str() {
        let password_string = String::from("password");
        let password: Password = password_string.as_str().try_into().unwrap();
        assert_eq!(&password.as_bytes()[0..8], password_string.as_bytes());
    }

    #[test]
    fn debug_redacts_plaintext() {
        let password: Password = "hunter2hunter2".try_into().unwrap();
        let rendered = format!("{password:?}");
        assert!(!rendered.contains("hunter2"));
    }

    #[test]
    fn encode_decode() {
        let password: Password = "password".try_into().unwrap();
        let encoded = bitcode::encode(&password);
        println!("{:?}", encoded);
        let decoded: Password = bitcode::decode(&encoded).unwrap();
        println!("{:?}", decoded);
    }
}
