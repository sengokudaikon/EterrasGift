//! The trainer's screens without a window: what each shows and how it answers the user, as `spiderland` features. The worker
//! and the helper are reached only through effects, and a window only through the UI crate's hosts.
//!
//! `trainer_pages` is the feature behind the pages of hooks and field editors, whose rows are `pages`; `connection` says how
//! the connection stands; `settings` is the trainer's part of the Settings page. `session` is the port those rows talk to, and `backend_session` is that port on the real worker.

pub mod backend_session;
pub mod connection;
pub mod forge;
pub mod pages;
pub mod profiles;
pub mod registry;
pub mod session;
pub mod settings;
pub mod trainer_pages;
