//! A bounds-checked cursor over a byte slice, where every read either succeeds in full or fails without moving, and
//! positioned reads from a file.

use crate::error::{Error, Result};
use std::fs::File;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    pub endian: Endian,
}

macro_rules! numbers {
    ($($name:ident: $type:ty),* $(,)?) => {
        $(
            pub fn $name(&mut self) -> Result<$type> {
                let bytes = self.take(size_of::<$type>())?.try_into().expect("taken to size");
                Ok(match self.endian {
                    Endian::Little => <$type>::from_le_bytes(bytes),
                    Endian::Big => <$type>::from_be_bytes(bytes),
                })
            }
        )*
    };
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8], endian: Endian) -> Self {
        Self { bytes, position: 0, endian }
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    pub fn seek(&mut self, position: usize) -> Result<()> {
        if position > self.bytes.len() {
            return Err(Error::malformed(format!("seek to {position} past the end ({})", self.bytes.len())));
        }
        self.position = position;
        Ok(())
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        if count > self.remaining() {
            return Err(Error::malformed(format!("{count} bytes needed at {}, {} left", self.position, self.remaining())));
        }
        let taken = &self.bytes[self.position..self.position + count];
        self.position += count;
        Ok(taken)
    }

    /// Moves to the next multiple of `to`.
    pub fn align(&mut self, to: usize) -> Result<()> {
        let padding = (to - self.position % to) % to;
        self.take(padding).map(|_| ())
    }

    numbers!(u8: u8, i8: i8, u16: u16, i16: i16, u32: u32, i32: i32, u64: u64, i64: i64, f32: f32, f64: f64);

    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }

    /// A count that must fit in what is left, given the smallest size one element can take.
    pub fn count(&mut self, element_size: usize) -> Result<usize> {
        let count = self.i32()?;
        let count = usize::try_from(count).map_err(|_| Error::malformed(format!("negative count {count}")))?;
        if count.saturating_mul(element_size.max(1)) > self.remaining() {
            return Err(Error::malformed(format!("count {count} exceeds the {} bytes left", self.remaining())));
        }
        Ok(count)
    }

    pub fn cstring(&mut self) -> Result<String> {
        let rest = &self.bytes[self.position..];
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::malformed(format!("unterminated string at {}", self.position)))?;
        let text = std::str::from_utf8(&rest[..end]).map_err(|_| Error::malformed(format!("string at {} is not UTF-8", self.position)))?;
        self.position += end + 1;
        Ok(text.to_owned())
    }
}

/// Fills `out` from `at` in `file`, without moving its cursor.
#[cfg(windows)]
pub fn read_at(file: &File, mut at: u64, mut out: &mut [u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !out.is_empty() {
        match file.seek_read(out, at)? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            read => {
                out = &mut out[read..];
                at += read as u64;
            }
        }
    }
    Ok(())
}
#[cfg(unix)]
pub fn read_at(file: &File, at: u64, out: &mut [u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, out, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_endians_and_refuses_short_input_without_moving() {
        let mut reader = Reader::new(&[1, 0, 0, 0, 0, 0, 0, 2], Endian::Little);
        assert_eq!(reader.u32().unwrap(), 1);
        reader.endian = Endian::Big;
        assert_eq!(reader.u32().unwrap(), 2);
        assert!(reader.u8().is_err());
        assert_eq!(reader.position(), 8);
    }

    #[test]
    fn aligns_counts_and_strings() {
        let mut reader = Reader::new(b"ab\0\0\x02\0\0\0xy", Endian::Little);
        assert_eq!(reader.cstring().unwrap(), "ab");
        reader.align(4).unwrap();
        assert_eq!(reader.position(), 4);
        assert_eq!(reader.count(1).unwrap(), 2);
        let mut reader = Reader::new(b"\x09\0\0\0xy", Endian::Little);
        assert!(reader.count(1).is_err(), "a count larger than the bytes left is refused");
        assert!(Reader::new(b"abc", Endian::Little).cstring().is_err());
    }
}
