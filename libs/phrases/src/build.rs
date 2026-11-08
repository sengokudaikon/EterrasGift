//! The build side, for an app's build script and its tasks. What is being built comes in as arguments.
//!
//! - [`bundle_catalogs`]: a build script turns `<lang>/<language>/LC_MESSAGES/<domain>.po` into the table of `Catalog`s
//!   the app hands to `Translations`.
//! - [`Template`]: every word in the sources, in one gettext template for translators. It fails with an [`Error`].

mod catalogs;
mod template;

pub use catalogs::bundle_catalogs;
pub use template::{Error, Template, Written};
