#![windows_subsystem = "windows"]
#![deny(unsafe_code)]
//! `eterras-gift.exe`: the launcher. With no mode it opens the preparation window, prepares a generation and runs the app
//! supervised; `prepare [--root R]` builds or reuses a generation in the console and exits.

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let code = match arguments.first().map(String::as_str) {
        Some("prepare") => eterras_gift_launcher::prepare_check(&arguments[1..]),
        _ => eterras_gift_launcher::gui::run(arguments),
    };
    std::process::exit(code);
}
