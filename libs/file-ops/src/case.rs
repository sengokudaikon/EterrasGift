//! File, folder and mod names compared without case, as the file system compares them.
//!
//! Windows folds one UTF-16 unit at a time through a table of its own: `é` is `É` and `σ` is `Σ`, but `ß` is not `SS`,
//! `ı` and `ſ` are not `I` and `S`, `ς` is not `Σ`, and nothing outside the Basic Multilingual Plane folds at all. Rust's
//! `to_uppercase` and `to_lowercase` follow Unicode instead and disagree on hundreds of characters, and comparing ASCII
//! only misses `é`. So the table is Windows' own, asked for once. An NTFS volume keeps a copy from when it was formatted,
//! so a very old volume can lag on letters added to Unicode since.
//!
//! This is for names. Hashes and fixed ASCII words (`.dll`) are fine with `eq_ignore_ascii_case`, and text a person
//! searches or sorts is not a name the file system has to agree with.

use std::sync::LazyLock;

/// What each UTF-16 unit folds to.
static UPPER: LazyLock<Vec<u16>> = LazyLock::new(|| {
    let units: Vec<u16> = (0..=u16::MAX).collect();
    windows_upper(&units).unwrap_or_else(|| {
        let ascii = |unit: u16| u8::try_from(unit).map_or(unit, |byte| u16::from(byte.to_ascii_uppercase()));
        units.into_iter().map(ascii).collect()
    })
});

/// The file system's casing: without `LCMAP_LINGUISTIC_CASING`, that is what `LCMAP_UPPERCASE` asks for.
#[cfg(windows)]
fn windows_upper(units: &[u16]) -> Option<Vec<u16>> {
    use std::ptr::null;
    use windows_sys::Win32::Globalization::{LCMAP_UPPERCASE, LCMapStringEx};
    const INVARIANT_LOCALE: [u16; 1] = [0];
    let length = i32::try_from(units.len()).ok()?;
    let mut upper = vec![0; units.len()];
    // SAFETY: both buffers hold `length` units, the locale name is terminated, and the other arguments are optional.
    let written = unsafe {
        LCMapStringEx(
            INVARIANT_LOCALE.as_ptr(),
            LCMAP_UPPERCASE,
            units.as_ptr(),
            length,
            upper.as_mut_ptr(),
            length,
            null(),
            null(),
            0,
        )
    };
    (written == length).then_some(upper)
}

#[cfg(not(windows))]
fn windows_upper(_units: &[u16]) -> Option<Vec<u16>> {
    None
}

/// One character as a name's comparison sees it.
pub fn fold(character: char) -> char {
    u16::try_from(u32::from(character))
        .ok()
        .and_then(|unit| char::from_u32(u32::from(UPPER[usize::from(unit)])))
        .unwrap_or(character)
}

/// Two names are one name.
pub fn same(first: &str, second: &str) -> bool {
    first == second || first.chars().map(fold).eq(second.chars().map(fold))
}

/// One spelling for every way of writing a name, for keeping names in a map or a set, or looking for one's start.
pub fn key(name: &str) -> String {
    name.chars().map(fold).collect()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Letters whose casing has not moved in decades, so a volume of any age answers alike.
    const NAMES: [(&str, &str, bool); 17] = [
        ("a", "A", true),
        ("é", "É", true),
        ("я", "Я", true),
        ("σ", "Σ", true),
        ("ß", "SS", false),
        ("ß", "ẞ", false),
        ("İ", "i", false),
        ("İ", "I", false),
        ("ı", "I", false),
        ("ı", "i", false),
        ("\u{212A}", "k", false),
        ("\u{212A}", "K", false),
        ("\u{212B}", "Å", false),
        ("ſ", "S", false),
        ("ς", "Σ", false),
        ("ǅ", "Ǆ", false),
        ("\u{10428}", "\u{10400}", false),
    ];

    #[test]
    fn names_compare_as_the_file_system_compares_them() {
        let folder = tempfile::tempdir().unwrap();
        for (first, second, alike) in NAMES {
            let file = folder.path().join(format!("x{first}.dll"));
            std::fs::write(&file, b"").unwrap();
            assert_eq!(folder.path().join(format!("x{second}.dll")).exists(), alike, "the file system on {first} and {second}");
            std::fs::remove_file(file).unwrap();
            assert_eq!(same(first, second), alike, "{first} and {second}");
            assert_eq!(key(first) == key(second), alike, "the keys of {first} and {second}");
        }
    }

    #[test]
    fn a_key_keeps_the_rest_of_a_name() {
        assert_eq!(key("Étude 2_x-y.dll"), "ÉTUDE 2_X-Y.DLL");
        assert_eq!(key("Straße"), "STRAßE");
        assert_eq!(key(""), "");
    }
}
