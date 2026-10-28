//! Text read from files that other programs, editors and people write.

/// UTF-8's byte-order mark, which Windows editors and .NET put at the start of a file and no parser here expects.
const BOM: &str = "\u{feff}";

/// The text without a leading byte-order mark.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix(BOM).unwrap_or(text)
}

/// The bytes without a leading UTF-8 byte-order mark, for text not yet decoded.
pub fn strip_bom_bytes(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(BOM.as_bytes()).unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_leading_mark_is_stripped() {
        assert_eq!(strip_bom("\u{feff}{}"), "{}");
        assert_eq!(strip_bom("{}\u{feff}"), "{}\u{feff}");
        assert_eq!(strip_bom_bytes(b"\xEF\xBB\xBF{}"), b"{}");
        assert_eq!(strip_bom_bytes(b"{}"), b"{}");
    }
}
