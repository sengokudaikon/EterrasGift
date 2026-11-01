//! Reading binary file formats: [`Reader`], a bounds-checked cursor that never reads past its bytes, [`read_at`] for
//! positioned file reads, and [`Error`], which says whether bytes are broken, unsupported or simply not what was expected.

#![forbid(unsafe_code)]

mod error;
pub mod pe;
mod reader;

pub use error::{Error, Result};
pub use reader::{Endian, Reader, read_at};
