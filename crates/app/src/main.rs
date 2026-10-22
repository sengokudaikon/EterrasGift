#![windows_subsystem = "windows"]
#![deny(unsafe_code)]
//! Eterra's Gift: the composition (`spec/architecture.md` §5).

mod application;
mod companion;
mod startup;
#[allow(unsafe_code)] // Composition of the explicitly unsafe native trainer adapter.
mod trainer;

fn main() -> Result<(), slint::PlatformError> {
    let Some(startup) = startup::Startup::open() else {
        return Ok(());
    };
    application::run(startup)
}
