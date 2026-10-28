//! Typed access to one game object each, over discovery's bindings. An accessor finds its object, checks it is the
//! object it expects and copies what it holds; it knows nothing of the trainer's features.

pub(crate) mod character;
pub(crate) mod item;
mod limits;
mod locale;
pub(crate) mod player;
pub mod presence;
