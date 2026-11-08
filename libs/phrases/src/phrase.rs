//! A phrase and a plural as the code holds them, each filled in and shown in the user's language.

use crate::catalog::current;
use std::fmt;

/// A piece of English the user reads, shown in their language once a translation has it. The template finds it in the code
/// by its literal, so `english` must be one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Phrase {
    context: Option<&'static str>,
    english: &'static str,
}

impl Phrase {
    /// `english`, for translators to translate.
    pub const fn new(english: &'static str) -> Self {
        Self { context: None, english }
    }

    /// English that means something else elsewhere, told apart for translators by `context`.
    pub const fn ctx(context: &'static str, english: &'static str) -> Self {
        Self { context: Some(context), english }
    }

    /// In the user's language.
    pub fn get(self) -> &'static str {
        current().and_then(|language| language.text(self.context, self.english)).unwrap_or(self.english)
    }

    /// With each `{name}` replaced by its value; a name without a value stays as it is.
    pub fn with(self, values: &[(&str, &dyn fmt::Display)]) -> String {
        fill(self.get(), values)
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

/// A sentence about a count, in as many forms as the user's language has: `Plural::new("{n} file", "{n} files")`.
/// English has two, the first for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Plural {
    one: &'static str,
    other: &'static str,
}

impl Plural {
    /// English's two forms: `one` for a count of one, `other` for any other.
    pub const fn new(one: &'static str, other: &'static str) -> Self {
        Self { one, other }
    }

    /// The form for `n`, in the user's language.
    pub fn get(self, n: u64) -> &'static str {
        current()
            .and_then(|language| language.plural(self.one, n))
            .unwrap_or(if n == 1 { self.one } else { self.other })
    }

    /// The form for `n`, with `{n}` filled in.
    pub fn count(self, n: usize) -> String {
        self.with(n, &[])
    }

    /// The form for `n`, with `{n}` and each named value filled in.
    pub fn with(self, n: usize, values: &[(&str, &dyn fmt::Display)]) -> String {
        let mut all: Vec<(&str, &dyn fmt::Display)> = vec![("n", &n)];
        all.extend_from_slice(values);
        fill(self.get(n as u64), &all)
    }
}

fn fill(text: &str, values: &[(&str, &dyn fmt::Display)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrases;

    #[test]
    fn values_fill_their_names_and_nothing_else() {
        phrases! { Added: "Added {name}, {count} times. {missing} stays." }
        assert_eq!(Added.with(&[("name", &"notes.txt"), ("count", &3)]), "Added notes.txt, 3 times. {missing} stays.");
        assert_eq!(Phrase::new("No braces").with(&[]), "No braces");
        assert_eq!(Phrase::new("Open { only").with(&[("only", &1)]), "Open { only");
    }

    #[test]
    fn english_counts() {
        phrases! { Files: "{n} file" | "{n} files" }
        assert_eq!([Files.count(0), Files.count(1), Files.count(2)], ["0 files", "1 file", "2 files"]);
        assert_eq!(Plural::new("{n} file in {place}", "{n} files in {place}").with(3, &[("place", &"Logs")]), "3 files in Logs");
    }

    #[test]
    fn a_context_is_part_of_the_phrase() {
        assert_ne!(Phrase::ctx("key", "Delete"), Phrase::new("Delete"));
        assert_eq!(Phrase::ctx("key", "Delete").get(), "Delete");
    }
}
