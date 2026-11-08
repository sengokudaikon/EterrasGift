//! Numbers as a language writes them, from Windows' symbols for an explicitly named locale: any language by name
//! ([`NumberSymbols::capture`]), and the language shown ([`select`], [`numbers`]). Never the current user's locale or its
//! overrides.
use std::io;
use std::sync::{Arc, LazyLock, RwLock};

/// What a language writes numbers with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumberSymbols {
    /// Between the whole part and the fraction: "." in English.
    pub decimal: String,
    /// Between groups of thousands: "," in English.
    pub group: String,
    /// Before a number above zero, when it shows a sign.
    pub positive: String,
    /// Before a number below zero.
    pub negative: String,
}

static ENGLISH: LazyLock<Arc<NumberSymbols>> = LazyLock::new(|| Arc::new(NumberSymbols::english()));
static SHOWN: RwLock<Option<Arc<NumberSymbols>>> = RwLock::new(None);

/// The symbols shown numbers use: the shown language's, else English's.
pub fn numbers() -> Arc<NumberSymbols> {
    SHOWN.read().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_else(|| ENGLISH.clone())
}

/// Shown numbers follow `language` from now on, as the views' do; English when it is None or Windows has no
/// symbols for it.
pub fn select(language: Option<&str>) {
    let symbols = language
        .and_then(|language| NumberSymbols::capture(language).ok())
        .filter(|symbols| !symbols.decimal.is_empty() && symbols.decimal != symbols.group);
    *SHOWN.write().unwrap_or_else(|p| p.into_inner()) = symbols.map(Arc::new);
}

impl NumberSymbols {
    /// English's, shown without a translation.
    pub fn english() -> Self {
        Self {
            decimal: ".".into(),
            group: ",".into(),
            positive: "+".into(),
            negative: "-".into(),
        }
    }

    /// `language`'s symbols, as Windows has them for that locale name, such as "de" or "pt-BR". The user's own overrides are
    /// left out. Fails for a name Windows doesn't know, and always off Windows.
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

    /// `language`'s symbols, as Windows has them for that locale name. Off Windows there are none, so it always fails.
    #[cfg(not(windows))]
    pub fn capture(_: &str) -> io::Result<Self> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "Windows numeric locales are unavailable"))
    }

    /// "0.##": at most two decimals, none when whole; a midpoint rounds away from zero, as .NET's formatting does.
    pub fn plain(&self, value: f64) -> String {
        let text = format!("{:.2}", (value * 100.0).round() / 100.0);
        let text = text.trim_end_matches('0').trim_end_matches('.');
        let text = if text == "-0" { "0" } else { text };
        text.replace('.', &self.decimal).replace('-', &self.negative)
    }

    /// "#,0.##": [`plain`](Self::plain) with its thousands grouped.
    pub fn grouped(&self, value: f64) -> String {
        let text = self.plain(value.abs());
        let (whole, fraction) = text.split_once(&self.decimal).map_or((text.as_str(), None), |(whole, fraction)| (whole, Some(fraction)));
        let mut out = if value < 0.0 && text != "0" { self.negative.clone() } else { String::new() };
        for (i, digit) in whole.chars().enumerate() {
            if i > 0 && (whole.len() - i) % 3 == 0 {
                out.push_str(&self.group);
            }
            out.push(digit);
        }
        if let Some(fraction) = fraction {
            out.push_str(&self.decimal);
            out.push_str(fraction);
        }
        out
    }

    /// A typed number: spaces and group symbols are ignored, the decimal symbol is the point; anything else is no number.
    pub fn parse(&self, text: &str) -> Option<f64> {
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let text = if self.group.is_empty() { text } else { text.replace(&self.group, "") };
        text.replace(&self.decimal, ".")
            .replace(&self.negative, "-")
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_numbers() {
        let english = NumberSymbols::english();
        assert_eq!([2.0, 0.1, -30.0, 0.125].map(|value| english.plain(value)), ["2", "0.1", "-30", "0.13"]);
        assert_eq!(
            [12400.0, 1234567.5, -1.0, 999.0, -0.001].map(|value| english.grouped(value)),
            ["12,400", "1,234,567.5", "-1", "999", "0"]
        );
        assert_eq!(english.parse(" 12,400 "), Some(12400.0));
        assert_eq!(english.parse("-2.5"), Some(-2.5));
        assert_eq!(english.parse(""), None);
        assert_eq!(english.parse("abc"), None);
    }

    #[test]
    fn another_languages_symbols() {
        let german = NumberSymbols {
            decimal: ",".into(),
            group: ".".into(),
            positive: "+".into(),
            negative: "-".into(),
        };
        assert_eq!(german.plain(0.5), "0,5");
        assert_eq!(german.grouped(-1234567.25), "-1.234.567,25");
        assert_eq!(german.parse("1.234,5"), Some(1234.5));
        assert_eq!(german.parse("1,5"), Some(1.5));
    }
}
