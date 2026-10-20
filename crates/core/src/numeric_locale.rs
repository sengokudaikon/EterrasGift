//! Numeric symbols from an explicitly named Windows locale. This never uses
//! the current-user locale, falls back to another name or applies user overrides.
use std::io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumberSymbols {
    pub decimal: String,
    pub group: String,
    pub positive: String,
    pub negative: String,
}
impl NumberSymbols {
    #[cfg(windows)]
    pub fn capture(language: &str) -> io::Result<Self> {
        use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_NOUSEROVERRIDE, LOCALE_SDECIMAL, LOCALE_SNEGATIVESIGN, LOCALE_SPOSITIVESIGN, LOCALE_STHOUSAND};
        if language.is_empty() || language.contains('\0') || language.len() > 256 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid numeric locale name"));
        }
        let name: Vec<u16> = language.encode_utf16().chain(Some(0)).collect();
        let read = |kind| -> io::Result<String> {
            let mut output = [0u16; 128];
            // SAFETY: exact NUL-terminated explicit name and a writable bounded
            // output buffer. GetLocaleInfoEx does not retain either pointer.
            let count = unsafe { GetLocaleInfoEx(name.as_ptr(), kind | LOCALE_NOUSEROVERRIDE, output.as_mut_ptr(), output.len() as i32) };
            if count == 0 {
                return Err(io::Error::last_os_error());
            }
            if count < 1 || count as usize > output.len() || output[count as usize - 1] != 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid locale symbol"));
            }
            String::from_utf16(&output[..count as usize - 1]).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        };
        Ok(Self {
            decimal: read(LOCALE_SDECIMAL)?,
            group: read(LOCALE_STHOUSAND)?,
            positive: read(LOCALE_SPOSITIVESIGN)?,
            negative: read(LOCALE_SNEGATIVESIGN)?,
        })
    }
    #[cfg(not(windows))]
    pub fn capture(_: &str) -> io::Result<Self> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Windows numeric locales are unavailable"))
    }
}
