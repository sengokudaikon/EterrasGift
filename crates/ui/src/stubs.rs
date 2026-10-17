//! Stand-ins for the functionality behind the pages that is not ported yet, so the UI can be driven and judged.
//! Each callback says what it will do once ported (TODO) and keeps just enough local state for the view to react.

use crate::{Connection, MainWindow, TrainerState};
use slint::ComponentHandle;

pub fn install(window: &MainWindow, trainer: bool) {
    connection(window, trainer);
}

fn not_ported(what: &str) {
    eprintln!("TODO: {what} is not ported yet");
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
