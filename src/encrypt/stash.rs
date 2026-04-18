use crate::encrypt::{Encrypted, Error, Key};
use bitcode::{DecodeOwned, Encode};
use std::io;

/// Types that serialize to a bitcode + snap-compressed byte buffer and seal
/// under a [`Key<T>`] via AES-GCM-SIV.
///
/// The type parameter `T` is the key's scope marker; it carries no data and
/// only constrains which [`Key<T>`] this stash will accept at the type level.
///
/// Typical use:
///
/// ```ignore
/// #[derive(Encode, Decode)]
/// struct Foo { /* ... */ }
///
/// impl Stash<Lot> for Foo {}
///
/// let foo = Foo { /* ... */ };
/// let sealed = foo.encrypt_with_aad(lot.key(), b"aad")?;
/// let foo2 = Foo::decrypt_with_aad(&sealed, lot.key(), b"aad")?;
/// ```
pub trait Stash<T>: Encode + DecodeOwned + Sized {
    fn encode(&self) -> Vec<u8> {
        bitcode::encode(self)
    }

    fn decode(buf: &[u8]) -> Result<Self, Error> {
        bitcode::decode(buf).map_err(Error::Decoding)
    }

    fn compress(&self) -> Result<Vec<u8>, Error> {
        let encoded = self.encode();
        let mut compressed = Vec::new();
        let mut encoder = snap::read::FrameEncoder::new(encoded.as_slice());
        io::copy(&mut encoder, &mut compressed).map_err(Error::Compression)?;
        Ok(compressed)
    }

    fn decompress(buf: &[u8]) -> Result<Self, Error> {
        let mut decompressed = Vec::new();
        let mut decoder = snap::read::FrameDecoder::new(buf);
        io::copy(&mut decoder, &mut decompressed).map_err(Error::Decompression)?;
        Self::decode(&decompressed)
    }

    fn encrypt(&self, key: &Key<T>) -> Result<Encrypted, Error> {
        key.encrypt(&self.compress()?)
    }

    fn encrypt_with_aad(&self, key: &Key<T>, aad: &[u8]) -> Result<Encrypted, Error> {
        key.encrypt_with_aad(&self.compress()?, aad)
    }

    fn decrypt(buf: &Encrypted, key: &Key<T>) -> Result<Self, Error> {
        Self::decompress(&key.decrypt(buf)?)
    }

    fn decrypt_with_aad(buf: &Encrypted, key: &Key<T>, aad: &[u8]) -> Result<Self, Error> {
        Self::decompress(&key.decrypt_with_aad(buf, aad)?)
    }
}
