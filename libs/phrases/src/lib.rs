//! The words and numbers an app shows, kept apart from the code that shows them so they can be translated. The
//! translations themselves are the app's, whose build script bundles them into a table of [`Catalog`]s (`build`, below),
//! which [`Translations`] checks and [`Translations::select`] shows. The crate has no words of its own: every phrase is the
//! app's.
//!
//! Words are declared with [`phrases!`] beside the code that uses them, and used by name. Words in a table or an argument,
//! with no name worth giving, are written in place as `Phrase::new("…")`. A phrase's English is its gettext msgid, so an
//! extractor can collect the words for translators and the English stays a literal.
//!
//! A phrase that takes values names them in braces, `"Added {name}."`, and [`Phrase::with`] fills them in, so a
//! translation can move them. A count goes through a [`Plural`], whose `{n}` picks the form.
//!
//! The `build` feature adds `build`, for a build script and the task that writes the template: `build::bundle_catalogs`
//! writes the table of catalogs, and `build::Template` collects every shown word into one gettext template. The README says
//! how the two sides fit together.

#![deny(unsafe_code)]

#[cfg(feature = "build")]
pub mod build;
mod catalog;
#[allow(unsafe_code)] // Windows/FFI boundary; the rest of the crate denies unsafe.
pub mod locale;
mod macros;
mod phrase;
mod plural_rule;

pub use catalog::{Catalog, Error, Translations};
pub use phrase::{Phrase, Plural};
