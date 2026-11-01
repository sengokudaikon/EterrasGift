//! The words and numbers the app shows, kept apart from the code that shows them so they can be translated. It names
//! neither the app nor the game; the translations themselves are `eg-ui`'s, whose build script bundles
//! `lang/<language>/LC_MESSAGES/eg-ui.po` into the views and into a table of [`Catalog`]s, which [`Translations`] checks
//! and [`Translations::select`] shows.
//!
//! Words are declared with [`phrases!`] beside the code that uses them, and used by name; the views' own words are `@tr`
//! strings in `crates/ui/ui/strings/`. Words in a table or an argument, with no name worth giving, are written in place
//! as `Phrase::new("…")`. A phrase's English is its gettext msgid, as the views' are: `cargo xtask phrases` collects both
//! into `lang/eterras-gift.pot` for translators, so the English stays a literal.
//!
//! A phrase that takes values names them in braces, `"Added {name}."`, and [`Phrase::with`] fills them in, so a
//! translation can move them. A count goes through a [`Plural`], whose `{n}` picks the form; a list through [`list`].

#![deny(unsafe_code)]

mod catalog;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod locale;
mod macros;
mod phrase;
mod plural_rule;

pub use catalog::{Catalog, Translations};
pub use phrase::{Conjunction, Phrase, Plural, list};
