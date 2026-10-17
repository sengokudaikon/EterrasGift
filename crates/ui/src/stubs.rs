//! Stand-ins for the functionality behind the pages that is not ported yet, so the UI can be driven and judged.
//! Each callback says what it will do once ported (TODO) and keeps just enough local state for the view to react.

use crate::{Connection, MainWindow, ModsState, TrainerState};
use slint::ComponentHandle;

pub fn install(window: &MainWindow, trainer: bool) {
    mods(window);
    connection(window, trainer);
}

fn not_ported(what: &str) {
    eprintln!("TODO: {what} is not ported yet");
}

fn mods(window: &MainWindow) {
    let mods = window.global::<ModsState>();
    // TODO(mods): the mod library (spec/companion.md §6): file picker, Explorer, Latest.log, with-mods start.
    mods.on_add_mod(|| not_ported("Add mod"));
    mods.on_open_folder(|| not_ported("Open folder"));
    mods.on_open_log(|| not_ported("Open log"));
    mods.on_play(|| not_ported("Play offline with mods"));
}

fn connection(window: &MainWindow, trainer: bool) {
    let connection = window.global::<Connection>();
    connection.set_present(trainer);
    connection.set_connected(false);
    connection.set_text("Not connected".into());
    connection.set_show_action(true);
    connection.set_action_text("Connect".into());
    // TODO(trainer): the session's states and Connect / Retry (spec/trainer.md §3).
    connection.on_connect(|| not_ported("Connect"));
    let trainer = window.global::<TrainerState>();
    trainer.set_empty_title("Not connected".into());
    trainer.set_empty_detail("Connect, then place an item in the forge.".into());
}
