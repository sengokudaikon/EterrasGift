//! The one cursor pair every message is written and read with, and the [`Wire`] trait a payload implements beside its
//! type. Integers are little-endian, a flag is one 0/1 byte, and a text is its UTF-8 length (a u32, or a u16 where
//! [`Writer::short_text`] says so) followed by its bytes. The bounds a text or list must keep are the message's, passed
//! to the reader, which checks each before it takes or allocates anything.

use crate::protocol::{MAX_RESPONSE_SIZE, ProtocolError};

pub(crate) fn invalid<T>() -> Result<T, ProtocolError> {
    Err(ProtocolError::InvalidData)
}

/// A payload that writes and reads itself: one place for both directions.
pub(crate) trait Wire: Sized {
    fn write(&self, w: &mut Writer);
    fn read(r: &mut Reader<'_>) -> Result<Self, ProtocolError>;
}

/// Writes a message. Writing cannot fail: what a message must be is checked before it is written, and its whole size
/// by [`Writer::finish`].
#[derive(Default)]
pub(crate) struct Writer(pub(crate) Vec<u8>);

impl Writer {
    pub(crate) fn raw(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
    pub(crate) fn byte(&mut self, value: u8) {
        self.0.push(value);
    }
    pub(crate) fn flag(&mut self, value: bool) {
        self.byte(u8::from(value));
    }
    pub(crate) fn u16(&mut self, value: u16) {
        self.raw(&value.to_le_bytes());
    }
    pub(crate) fn u32(&mut self, value: u32) {
        self.raw(&value.to_le_bytes());
    }
    pub(crate) fn i32(&mut self, value: i32) {
        self.raw(&value.to_le_bytes());
    }
    pub(crate) fn u64(&mut self, value: u64) {
        self.raw(&value.to_le_bytes());
    }
    pub(crate) fn f32(&mut self, value: f32) {
        self.raw(&value.to_le_bytes());
    }
    pub(crate) fn f64(&mut self, value: f64) {
        self.raw(&value.to_le_bytes());
    }
    /// A u32 length, then the bytes.
    pub(crate) fn blob(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        self.raw(bytes);
    }
    pub(crate) fn text(&mut self, text: &str) {
        self.blob(text.as_bytes());
    }
    /// A text, refused when longer than `limit`.
    pub(crate) fn text_within(&mut self, text: &str, limit: usize) -> Result<(), ProtocolError> {
        if text.len() > limit {
            return invalid();
        }
        self.text(text);
        Ok(())
    }
    /// A u16 length, then the bytes.
    pub(crate) fn short_text(&mut self, text: &str) {
        self.u16(text.len() as u16);
        self.raw(text.as_bytes());
    }
    pub(crate) fn put(&mut self, value: &impl Wire) {
        value.write(self);
    }
    /// The message, if it fits in a reply.
    pub(crate) fn finish(self) -> Result<Vec<u8>, ProtocolError> {
        if self.0.len() > MAX_RESPONSE_SIZE { invalid() } else { Ok(self.0) }
    }
}

/// Reads a message, refusing whatever it does not hold exactly.
pub(crate) struct Reader<'a> {
    remaining: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(remaining: &'a [u8]) -> Self {
        Self { remaining }
    }
    /// A reader over a reply, which is refused whole when it is larger than a reply may be.
    pub(crate) fn reply(payload: &'a [u8]) -> Result<Self, ProtocolError> {
        if payload.len() > MAX_RESPONSE_SIZE { invalid() } else { Ok(Self::new(payload)) }
    }
    pub(crate) fn take(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let (value, remaining) = self.remaining.split_at_checked(length).ok_or(ProtocolError::InvalidData)?;
        self.remaining = remaining;
        Ok(value)
    }
    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        self.take(N)?.try_into().map_err(|_| ProtocolError::InvalidData)
    }
    pub(crate) fn byte(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.array::<1>()?[0])
    }
    pub(crate) fn flag(&mut self) -> Result<bool, ProtocolError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => invalid(),
        }
    }
    pub(crate) fn u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    pub(crate) fn i32(&mut self) -> Result<i32, ProtocolError> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    pub(crate) fn u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    pub(crate) fn f32(&mut self) -> Result<f32, ProtocolError> {
        Ok(f32::from_le_bytes(self.array()?))
    }
    pub(crate) fn f64(&mut self) -> Result<f64, ProtocolError> {
        Ok(f64::from_le_bytes(self.array()?))
    }
    /// A one-byte count of at most `limit`.
    pub(crate) fn count(&mut self, limit: usize) -> Result<usize, ProtocolError> {
        let count = usize::from(self.byte()?);
        if count > limit { invalid() } else { Ok(count) }
    }
    /// A u32 count of at most `limit`.
    pub(crate) fn count32(&mut self, limit: usize) -> Result<usize, ProtocolError> {
        let count = self.u32()? as usize;
        if count > limit { invalid() } else { Ok(count) }
    }
    /// A u32 length of at most `limit`, then the bytes.
    pub(crate) fn blob(&mut self, limit: usize) -> Result<&'a [u8], ProtocolError> {
        let length = self.count32(limit)?;
        self.take(length)
    }
    pub(crate) fn text(&mut self, limit: usize) -> Result<&'a str, ProtocolError> {
        std::str::from_utf8(self.blob(limit)?).map_err(|_| ProtocolError::InvalidData)
    }
    /// A u16 length of at most `limit`, then the bytes.
    pub(crate) fn short_text(&mut self, limit: usize) -> Result<&'a str, ProtocolError> {
        let length = usize::from(self.u16()?);
        if length > limit {
            return invalid();
        }
        std::str::from_utf8(self.take(length)?).map_err(|_| ProtocolError::InvalidData)
    }
    pub(crate) fn get<T: Wire>(&mut self) -> Result<T, ProtocolError> {
        T::read(self)
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
    /// Nothing may follow what was read.
    pub(crate) fn finish(self) -> Result<(), ProtocolError> {
        if self.remaining.is_empty() { Ok(()) } else { invalid() }
    }
}

/// An enum whose variants are numbers on the wire: the enum, and its `TryFrom` the number, which refuses any other.
macro_rules! wire_enum {
    ($(#[$meta:meta])* $vis:vis enum $name:ident: $repr:ident { $($(#[$variant_meta:meta])* $variant:ident = $value:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[repr($repr)]
        $vis enum $name {
            $($(#[$variant_meta])* $variant = $value),+
        }
        impl TryFrom<$repr> for $name {
            type Error = $crate::protocol::ProtocolError;
            fn try_from(value: $repr) -> Result<Self, Self::Error> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err($crate::protocol::ProtocolError::InvalidData),
                }
            }
        }
    };
}
pub(crate) use wire_enum;
