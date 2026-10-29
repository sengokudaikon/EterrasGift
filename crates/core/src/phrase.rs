//! The words the app shows, kept apart from the code that shows them so they can be translated.
//!
//! Each is a `const` [`Phrase`] declared beside the code that uses it, and used by name; the views' own words are
//! `@tr` strings in `crates/ui/ui/strings/`. A phrase's English is its gettext msgid, without a context, as the views'
//! are: `cargo xtask phrases` collects both into `lang/eterras-gift.pot` for translators. No translation is bundled
//! yet, so a phrase reads as its English; the lookup goes in [`Phrase::get`] when one is.
//!
//! A phrase that takes values names them in braces, `"Added {name}."`, and [`Phrase::with`] fills them in, so a
//! translation can move them. The extractor reads `Phrase::new("…")` calls as written: keep the English one literal.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Phrase(&'static str);

impl Phrase {
    pub const fn new(english: &'static str) -> Self {
        Self(english)
    }

    /// In the user's language.
    pub fn get(self) -> &'static str {
        self.0
    }

    /// With each `{name}` replaced by its value; a name without a value stays as it is.
    pub fn with(self, values: &[(&str, &dyn fmt::Display)]) -> String {
        let mut out = String::with_capacity(self.get().len());
        let mut rest = self.get();
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let tail = &rest[open..];
            match tail.find('}').map(|close| (&tail[1..close], close)) {
                Some((name, close)) if let Some((_, value)) = values.iter().find(|(key, _)| *key == name) => {
                    out.push_str(&value.to_string());
                    rest = &tail[close + 1..];
                }
                _ => {
                    out.push('{');
                    rest = &tail[1..];
                }
            }
        }
        out.push_str(rest);
        out
    }
}

impl fmt::Display for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.get())
    }
}

impl From<Phrase> for String {
    fn from(phrase: Phrase) -> Self {
        phrase.get().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_fill_their_names_and_nothing_else() {
        const ADDED: Phrase = Phrase::new("Added {name}, {count} times. {missing} stays.");
        assert_eq!(ADDED.with(&[("name", &"Loom"), ("count", &3)]), "Added Loom, 3 times. {missing} stays.");
        assert_eq!(Phrase::new("No braces").with(&[]), "No braces");
        assert_eq!(Phrase::new("Open { only").with(&[("only", &1)]), "Open { only");
    }
}
