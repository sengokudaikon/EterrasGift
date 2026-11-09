//! What the screens' tests share: a worker's snapshot as a test builds it.

use eg_trainer::backend::{BackendSnapshot, CatalogState, ForgeState, GameFiles, Phase, ProfileLock};
use eg_trainer::session::Cleanup;
use std::collections::BTreeMap;

/// The worker at `revision` in `phase`, with nothing found or held.
pub(crate) fn snapshot(revision: u64, phase: Phase) -> BackendSnapshot {
    BackendSnapshot {
        fields: BTreeMap::new(),
        revision,
        generation: 0,
        enabled: true,
        monitoring: false,
        auto_connect: false,
        override_enabled: false,
        phase,
        activity: None,
        detected: None,
        attached: None,
        retained_helper: None,
        gate: None,
        cleanup: Cleanup::NotNeeded,
        runtime_status: None,
        error: None,
        catalog: CatalogState::Missing,
        game_files: GameFiles::NotConfigured,
        forge: ForgeState::Unobserved,
        forge_read_sequence: 0,
        mappings: vec![],
        unique_preview: None,
        capabilities: Default::default(),
        last_write: None,
        undo_forge: None,
        undo_forge_depth: 0,
        undo_forge_needs_override: false,
        editing: None,
        profiles: Default::default(),
        unsaved: None,
        profile_lock: ProfileLock::Editable,
        hooks: BTreeMap::new(),
    }
}
