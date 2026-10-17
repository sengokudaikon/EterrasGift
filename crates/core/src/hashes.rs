//! SHA-256 as the app spells it everywhere: upper-case hex (`spec/data-and-persistence.md` §3).

use crate::paths;
use sha2::{Digest, Sha256};
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

/// The hash of the text's UTF-8 bytes.
pub fn of_text(text: &str) -> String {
    of_bytes(text.as_bytes())
}

/// Whether the text is a SHA-256 in hex, as a file name or a saved value must be before it is trusted.
pub fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A folder's identity: the same for the folder written with or without a trailing separator, in any case.
pub fn folder_identity(folder: &Path) -> String {
    of_text(&paths::normalized(folder).to_string_lossy().to_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_upper_case_hex() {
        assert_eq!(of_text("abc"), "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD");
        assert!(is_sha256(&of_text("abc")));
        assert!(!is_sha256("BA7816BF"));
        assert!(!is_sha256(&"G".repeat(64)));
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_is_the_same_however_its_end_and_case_are_spelled() {
        assert_eq!(folder_identity(Path::new(r"C:\Games\Last Epoch\")), folder_identity(Path::new(r"c:\games\last epoch")));
    }
}
