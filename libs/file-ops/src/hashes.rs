//! SHA-256 in one spelling: upper-case hex, for records to store and compare.

use crate::stamp::FileStamp;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub fn hex_upper(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut text = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 0xF)] as char);
    }
    text
}

pub fn of_bytes(bytes: &[u8]) -> String {
    hex_upper(Sha256::digest(bytes).as_slice())
}

/// Stream a reader to EOF without allocating its complete contents.
pub fn of_reader(reader: impl Read) -> io::Result<String> {
    digest_reader::<Sha256>(reader)
}

/// A file's hash, read in pieces rather than whole.
pub fn of_file(path: &Path) -> io::Result<String> {
    of_reader(File::open(path)?)
}

/// Any digest of a reader's bytes to EOF, in upper-case hex, without allocating its complete contents.
pub fn digest_reader<D: Digest>(mut reader: impl Read) -> io::Result<String> {
    let mut hash = D::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let count = match reader.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(hex_upper(hash.finalize().as_slice()));
        }
        hash.update(&buffer[..count]);
    }
}

/// Any digest of a file whose stamp was `before` when it was asked for; `None` if the stamp has moved once it is read, as
/// the hash may then be of neither version.
pub fn digest_unchanged<D: Digest>(path: &Path, before: &FileStamp) -> io::Result<Option<String>> {
    let hash = digest_reader::<D>(File::open(path)?)?;
    Ok(before.unchanged(path)?.then_some(hash))
}

/// Whether the text is a SHA-256 in hex, as a file name or a saved value must be before it is trusted.
pub fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_upper_case_hex() {
        assert_eq!(of_bytes(b"abc"), "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD");
        assert!(is_sha256(&of_bytes(b"abc")));
        assert!(!is_sha256("BA7816BF"));
        assert!(!is_sha256(&"G".repeat(64)));
    }

    #[test]
    fn a_digest_is_kept_only_while_the_stamp_holds() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        std::fs::write(&path, "abc").unwrap();
        let before = FileStamp::of(&path).unwrap();
        assert_eq!(digest_unchanged::<Sha256>(&path, &before).unwrap().unwrap(), of_bytes(b"abc"));
        std::fs::write(&path, "abcd").unwrap();
        assert_eq!(digest_unchanged::<Sha256>(&path, &before).unwrap(), None);
    }
}
