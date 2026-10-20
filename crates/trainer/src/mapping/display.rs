//! Host parsing of the exact game display, using the captured locale's Windows
//! numeric symbols. No current-user locale or raw-quantity fallback is used.
//! The supported game uses the standard CultureInfo symbols for its language;
//! a custom game NumberFormatInfo remains an unsupported formatting capability.
use super::DisplayScalarParser;
use eg_core::numeric_locale::NumberSymbols;

pub struct CapturedDisplayParser {
    language: String,
    symbols: Symbols,
}
struct Symbols {
    decimal: String,
    group: String,
    positive: String,
    negative: String,
}
impl CapturedDisplayParser {
    pub fn new(language: &str) -> Result<Self, String> {
        if language.is_empty() || language.contains('\0') {
            return Err("The game's numeric locale is unavailable.".into());
        }
        Ok(Self {
            language: language.into(),
            symbols: {
                let mut symbols = NumberSymbols::capture(language).map_err(|_| "The game's numeric locale is unavailable.")?;
                // CultureData.GetNFIValues applies the same normalization:
                // Windows commonly reports an empty positive sign.
                if symbols.positive.is_empty() {
                    symbols.positive = "+".into();
                }
                if symbols.decimal.is_empty() || symbols.negative.is_empty() {
                    return Err("The game's numeric locale is unavailable.".into());
                }
                Symbols {
                    decimal: symbols.decimal,
                    group: symbols.group,
                    positive: symbols.positive,
                    negative: symbols.negative,
                }
            },
        })
    }
}
impl DisplayScalarParser for CapturedDisplayParser {
    fn parse(&mut self, language: &str, display: &str, unit: &str) -> Result<f64, String> {
        if language != self.language {
            return Err("The game's numeric locale changed during mapping.".into());
        }
        let scalar = display.strip_suffix(unit).ok_or("The modifier display does not end in its exact unit.")?;
        self.symbols.parse(scalar).ok_or_else(|| "The modifier has no unambiguous scalar display value.".into())
    }
}
impl Symbols {
    fn sign<'a>(&self, text: &'a str, output: &mut String) -> &'a str {
        if let Some(rest) = text.strip_prefix(&self.negative) {
            output.push('-');
            rest
        } else if let Some(rest) = text.strip_prefix(&self.positive) {
            output.push('+');
            rest
        } else {
            text
        }
    }
    fn parse(&self, text: &str) -> Option<f64> {
        // NumberStyles.Float allows ASCII numeric whitespace, one leading
        // sign, decimal and exponent; AllowThousands applies before decimal.
        let whitespace = |c: char| c == ' ' || ('\t'..='\r').contains(&c);
        let mut text = text.trim_matches(whitespace);
        let mut output = String::with_capacity(text.len());
        text = self.sign(text, &mut output);
        let mut decimal = false;
        let mut digits = 0;
        while !text.is_empty() {
            let first = text.as_bytes()[0];
            if first.is_ascii_digit() {
                output.push(char::from(first));
                digits += 1;
                text = &text[1..];
            } else if !decimal && let Some(rest) = text.strip_prefix(&self.decimal) {
                decimal = true;
                output.push('.');
                text = rest;
            } else if !decimal
                && digits != 0
                && let Some(rest) = self.group_rest(text)
            {
                text = rest;
            } else if matches!(first, b'e' | b'E') && digits != 0 {
                output.push('e');
                text = self.sign(&text[1..], &mut output);
                if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                output.push_str(text);
                break;
            } else {
                return None;
            }
        }
        if digits == 0 {
            return None;
        }
        output.parse::<f64>().ok().filter(|n| n.is_finite())
    }
    fn group_rest<'a>(&self, text: &'a str) -> Option<&'a str> {
        if self.group.is_empty() {
            return None;
        }
        text.strip_prefix(&self.group).or_else(|| {
            // NLS and ICU differ between NBSP and narrow NBSP for French.
            // .NET accepts an ordinary space for these grouping symbols too.
            // This equivalence is only enabled for a captured space separator.
            if matches!(self.group.as_str(), " " | "\u{a0}" | "\u{202f}") {
                text.strip_prefix([' ', '\u{a0}', '\u{202f}'])
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_scanner_keeps_display_units_sign_exponent_and_grouping_semantics() {
        let symbols = Symbols {
            decimal: ",".into(),
            group: "\u{a0}".into(),
            positive: "+".into(),
            negative: "-".into(),
        };
        for text in ["1\u{a0}234,5", "1\u{202f}234,5", "1 234,5", "\t+1234,5\r", "1,2345e+3"] {
            assert_eq!(symbols.parse(text), Some(1234.5), "{text:?}");
        }
        for text in ["", "NaN", "Infinity", "1,2,3", "1.2", " 1234 -", "(1234)", "1,2 3", "1e", "1e9999", "١٢"] {
            assert_eq!(symbols.parse(text), None, "{text:?}");
        }
    }
    #[cfg(windows)]
    #[test]
    fn captured_cultures_parse_exact_display_and_never_fall_back() {
        for (language, display) in [
            ("en", "1,234.5%"),
            ("en-GB", "1,234.5%"),
            ("de-DE", "1.234,5%"),
            ("fr-FR", "1\u{202f}234,5%"),
            ("fr", "1\u{a0}234,5%"),
        ] {
            let mut parser = CapturedDisplayParser::new(language).unwrap();
            assert_eq!(parser.parse(language, display, "%"), Ok(1234.5));
            assert!(parser.parse(language, display, "kg").is_err());
            assert!(parser.parse("different", display, "%").is_err());
        }
        for language in ["", "en\0US", "not-a-supported-game-locale"] {
            assert!(CapturedDisplayParser::new(language).is_err(), "{language:?}");
        }
    }
}
