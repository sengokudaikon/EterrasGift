//! The build side of `spiderland-phrases`, for an app's build script and its tasks. It names no app or game: what is
//! being built comes in as arguments.
//!
//! - [`bundle_catalogs`]: a build script turns `<lang>/<language>/LC_MESSAGES/<domain>.po` into the table of `Catalog`s
//!   the app hands to `Translations`.
//! - [`Template`]: every word in the sources, in one gettext template for translators.

#![forbid(unsafe_code)]

mod catalogs;
mod template;

pub use catalogs::bundle_catalogs;
pub use template::Template;
