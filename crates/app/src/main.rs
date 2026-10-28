#![windows_subsystem = "windows"]
#![deny(unsafe_code)]
//! Eterra's Gift: the composition of the settings, the companion, the trainer and the window.

mod application;
mod companion;
mod startup;
mod trainer;

use std::process::ExitCode;

fn main() -> Result<ExitCode, slint::PlatformError> {
    let Some(startup) = startup::Startup::open() else {
        return Ok(ExitCode::SUCCESS);
    };
    application::run(startup)
}
