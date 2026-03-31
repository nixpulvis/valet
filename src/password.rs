use bitcode::{Decode, Encode};
use std::fmt;
use std::pin::Pin;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const LENGTH: usize = 255;

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
#[derive(Encode, Decode, Zeroize, ZeroizeOnDrop, Clone, Debug, Eq, PartialEq)]
pub struct Password(Pin<Box<[u8; LENGTH]>>);

impl Password {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_bytes<'a>(&'a self) -> &'a [u8] {
        &*self.0
    }

    pub fn as_bytes_mut<'a>(&'a mut self) -> &'a mut [u8] {
        &mut *self.0
    }

    pub fn as_str<'a>(&'a self) -> &'a str {
        let utf8 = unsafe { str::from_utf8_unchecked(&*self.0) };
        let null_pos = utf8.find(|c| c == '\0').unwrap_or(utf8.len());
        &utf8[0..null_pos]
    }

    pub fn as_str_mut<'a>(&'a mut self) -> &'a mut str {
        let utf8 = unsafe { str::from_utf8_unchecked_mut(&mut *self.0) };
        let null_pos = utf8.find(|c| c == '\0').unwrap_or(utf8.len());
        &mut utf8[0..null_pos]
    }
}

// TODO: We shouldn't even allow empty passwords at all, or provide a
// Password::valid method.
impl Default for Password {
    fn default() -> Self {
        Password(Box::pin([0; LENGTH]))
    }
}

impl From<&str> for Password {
    fn from(str: &str) -> Self {
        let mut buf = [0; LENGTH];
        for (d, s) in buf.iter_mut().zip(str.bytes()) {
            *d = s;
        }
        Password(Box::pin(buf))
    }
}

// TODO: Consider a more explicit way to reveal passwords.
impl fmt::Display for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(utf8) = str::from_utf8(&*self.0) {
            let null_pos = utf8.find(|c| c == '\0').unwrap_or(utf8.len());
            write!(f, "{}", &utf8[0..null_pos])
        } else {
            write!(f, "<invalid utf8>")
        }
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
    fn from_str() {
        let password_string = String::from("password");
        let password = Password::from(password_string.as_str());
        assert_eq!(&password.as_bytes()[0..8], password_string.as_bytes());
    }

    #[test]
    fn encode_decode() {
        let password = Password::from("password");
        let encoded = bitcode::encode(&password);
        println!("{:?}", encoded);
        let decoded: Password = bitcode::decode(&encoded).unwrap();
        println!("{:?}", decoded);
    }
}
