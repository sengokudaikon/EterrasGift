//! The launcher's preparation window behind `PreparationState` (spec/shell-and-settings.md §11): Eterra's tree follows how
//! far preparation has really come, ash drifts while it works, and the state shows in words beneath. When it stops, the
//! reason in full and the two ways on. The launcher (`crates/launcher`) drives it from its supervisor; its captures
//! drive the same supervisor on a fake preparation and app.

use crate::decorations::{Ash, Decorations};
use crate::{PreparationState, PreparationWindow};
use eg_core::bloom::BloomPhase;
use slint::ComponentHandle;
use std::rc::Rc;

/// Where preparation is (the C# launcher's `PreparationStage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Checking,
    Building,
    WaitingForGame,
    Discovering,
    Validating,
    Ready,
    Blocked,
    Restarting,
}

impl Stage {
    /// The tree follows how far preparation has really come; it never runs ahead of it.
    pub fn phase(self) -> BloomPhase {
        match self {
            Stage::Building => BloomPhase::Opening,
            Stage::Ready => BloomPhase::Bloom,
            Stage::Blocked => BloomPhase::Wilted,
            _ => BloomPhase::Budding,
        }
    }
}

pub struct PreparationView {
    window: PreparationWindow,
    decorations: Rc<Decorations<PreparationWindow>>,
}

impl PreparationView {
    pub fn new(window: PreparationWindow) -> PreparationView {
        let decorations = Decorations::install(&window);
        crate::apply_system_settings(&window);
        PreparationView { window, decorations }
    }

    pub fn window(&self) -> &PreparationWindow {
        &self.window
    }

    /// Working again from the start: the reason and the ways on go, the ash comes back.
    pub fn start(&self, message: &str) {
        let state = self.window.global::<PreparationState>();
        state.set_detail("".into());
        self.report(Stage::Checking, message);
    }

    /// Preparation moved on: say where it is, and let the tree and the ash follow.
    pub fn report(&self, stage: Stage, message: &str) {
        let state = self.window.global::<PreparationState>();
        state.set_message(message.into());
        state.set_stopped(stage == Stage::Blocked);
        let ash = if stage == Stage::Blocked { Ash::Hidden } else { Ash::Drifting };
        self.decorations.follow(stage.phase(), ash);
    }

    /// The latest line of output while it works, on one line.
    pub fn show_detail(&self, line: &str) {
        self.window.global::<PreparationState>().set_detail(line.into());
    }

    /// A note that needs no stop, such as why Settings cannot open yet.
    pub fn say(&self, message: &str) {
        self.window.global::<PreparationState>().set_message(message.into());
    }

    /// Preparation stopped: wilted and still, the reason in full, and the two ways on. Without a `message` the last one
    /// stays.
    pub fn stop(&self, reason: &str, message: Option<&str>) {
        let state = self.window.global::<PreparationState>();
        let message = message.map_or_else(|| state.get_message(), Into::into);
        state.set_reason(reason.into());
        self.report(Stage::Blocked, &message);
    }

    pub fn on_retry(&self, retry: impl Fn() + 'static) {
        self.window.global::<PreparationState>().on_retry(retry);
    }

    pub fn on_open_settings(&self, open: impl Fn() + 'static) {
        self.window.global::<PreparationState>().on_open_settings(open);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_follows_the_stage() {
        assert_eq!(Stage::Checking.phase(), BloomPhase::Budding);
        assert_eq!(Stage::Building.phase(), BloomPhase::Opening);
        assert_eq!(Stage::WaitingForGame.phase(), BloomPhase::Budding);
        assert_eq!(Stage::Ready.phase(), BloomPhase::Bloom);
        assert_eq!(Stage::Blocked.phase(), BloomPhase::Wilted);
        assert_eq!(Stage::Restarting.phase(), BloomPhase::Budding);
    }
}
