//! Conditional current-graph primitive, not durable host selection authority.
use super::*;

/// Only Failed can describe a returned error after entering the void setter.
/// No error or result authorizes an automatic retry or rollback.
#[derive(Debug, PartialEq, Eq)]
pub enum AttackLeechWriteError {
    NotReady(String),
    Stale(String),
    Refused(String),
    Offline,
    Failed(String),
}

impl std::fmt::Display for AttackLeechWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady(text) | Self::Stale(text) | Self::Refused(text) | Self::Failed(text) => f.write_str(text),
            Self::Offline => f.write_str("A live offline manager is required"),
        }
    }
}
impl std::error::Error for AttackLeechWriteError {}

impl From<DiscoveryError> for AttackLeechWriteError {
    fn from(error: DiscoveryError) -> Self {
        match error {
            DiscoveryError::OfflineRequired => Self::Offline,
            DiscoveryError::NotReady(reason) => Self::NotReady(reason.into()),
            DiscoveryError::Runtime(error) => Self::NotReady(error.to_string()),
            error => Self::Refused(error.to_string()),
        }
    }
}

impl From<AttackLeechReadError> for AttackLeechWriteError {
    fn from(error: AttackLeechReadError) -> Self {
        match error {
            AttackLeechReadError::Contract(error) => error.into(),
            AttackLeechReadError::Unavailable(reason) => match reason {
                AttackLeechUnavailable::OfflineRequired | AttackLeechUnavailable::ManagerAbsent => Self::Offline,
                AttackLeechUnavailable::ManagerChanged | AttackLeechUnavailable::GraphChanged => Self::Stale(format!("Current field graph changed: {reason:?}")),
                AttackLeechUnavailable::UnsupportedStatsClass => Self::Refused("Unsupported dynamic stats class".into()),
                reason => Self::NotReady(format!("Current field graph is unavailable: {reason:?}")),
            },
        }
    }
}

fn project(expected: &AttackLeechObservation, target: AttackLeechTarget, value: f32) -> Result<AttackLeechObservation, AttackLeechWriteError> {
    let value = ObservedRate { raw_bits: value.to_bits() };
    let eligible = match target {
        AttackLeechTarget::AttackRate => value.attack_editor_value(),
        AttackLeechTarget::IncreasedLeechRate => value.leech_editor_value(),
    };
    if eligible.is_err() {
        return Err(AttackLeechWriteError::Refused("New rate must be finite and inside its source editor domain".into()));
    }
    let mut projected = *expected;
    match target {
        AttackLeechTarget::AttackRate => projected.attack_rate = value,
        AttackLeechTarget::IncreasedLeechRate => projected.increased_leech_rate = value,
    }
    if projected == *expected {
        return Err(AttackLeechWriteError::Refused("Rate already has these exact bits; no store is needed".into()));
    }
    Ok(projected)
}

// Sole first-attempt fence. The f32 slot stays initialized/live for the entire
// setter call. No callback below can return a definite pre-store classification.
unsafe fn attempt_attack_store<T>(
    api: raw::FieldWriteApi,
    object: NonNull<raw::Object>,
    field: NonNull<raw::FieldInfo>,
    value: f32,
    readback: impl FnOnce() -> Result<T, AttackLeechWriteError>,
) -> Result<T, AttackLeechWriteError> {
    // SAFETY: caller validated the exact mutable Single field and owns the graph.
    unsafe {
        api.set_f32(object, field, value);
    }
    readback().map_err(|error| AttackLeechWriteError::Failed(format!("Rate store attempted; complete readback unconfirmed: {error}")))
}

impl RuntimeAccess {
    // Internal caller inherits set_attack_leech's exact mutation/runtime contract.
    unsafe fn attack_mutable_field(&self, graph: &AttackGraph<'_, '_>, target: AttackLeechTarget) -> Result<NonNull<raw::FieldInfo>, AttackLeechWriteError> {
        // SAFETY: inherits set_attack_leech's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let (owner, name, read_field) = match target {
                AttackLeechTarget::AttackRate => (graph.character_stats, c"attackRate", graph.attack_field),
                AttackLeechTarget::IncreasedLeechRate => (graph.base_stats, c"increasedLeechRate", graph.leech_field),
            };
            let field = self.attack_field(owner, name, graph.single, false)?;
            let ty = (self.field_type)(field);
            if ty.is_null()
                || (self.type_byref)(ty)
                || (self.type_class)(ty) != graph.single
                || (self.type_code)(ty) != 0x0c
                || (self.field_parent)(field) != owner
                || (self.field_flags)(field) & 0x70 != 0
                || field != read_field
            {
                return Err(AttackLeechWriteError::Refused("Rate is not the exact mutable declared System.Single/R4 field".into()));
            }
            NonNull::new(field.cast()).ok_or_else(|| AttackLeechWriteError::Refused("Rate field is absent".into()))
        }
    }

    /// One conditional current-graph field store with complete owned readback.
    /// No getter, constructor, refresh, save, callback or reapplication is requested.
    ///
    /// # Safety
    /// Every read_attack_leech runtime/mainthread/metadata/nonmoving-GC condition
    /// applies, now with exclusive WRITE permission for the complete retained graph
    /// and all relevant statics through store/readback. Attachment and both export
    /// tables belong to the same initialized source-bound runtime. Caller explicitly
    /// authorizes this one selected current-graph mutation, and excludes all other
    /// writers, graph/native lifetime changes, reentrancy, shutdown and callbacks.
    /// Field::SetValue must implement the source-audited direct R4 assignment with
    /// no conversion, managed body, or retained argument pointer. Root/metadata/field
    /// runtime APIs obey their existing contracts throughout.
    /// Owned-fixture admission may independently establish an initialized Unity main
    /// thread Update callback plus every exact runtime/static, exclusive graph/scalar,
    /// native-lifetime, no-job, no-reentrancy and no-shutdown condition above. Update
    /// is not claimed to follow ExecuteTasks. Production uses the after-original
    /// admitted-dispatcher route; this alternate proof does not admit arbitrary callbacks.
    /// Expected tokens are compared only. Roots acquired by THIS CALL prevent managed
    /// wrapper collection; native-object lifetime remains an external caller obligation.
    /// They do not prove the objects were continuously alive since a prior
    /// HOST observation. Full token/rawbit equality cannot rule out cross-request
    /// ABA; caller must separately establish current selection authority/ownership.
    /// A host read sequence/generation alone is insufficient. No retained observation
    /// ticket/lifecycle mechanism is provided here. A containing wire operation must
    /// independently admit its conditional CURRENT-player mutation; this primitive
    /// and expected tokens do not grant that authority. Such an operation need not
    /// promise historical selection continuity or request-consumption/replay state.
    /// After entering the void setter, every returned failure is uncertain; this
    /// function cannot recover arbitrary SEH/native faults, unwind or guarantee rollback.
    pub unsafe fn set_attack_leech(
        &self,
        attached: &AttachedThread<'_>,
        write_api: raw::FieldWriteApi,
        expected: &AttackLeechObservation,
        target: AttackLeechTarget,
        value: f32,
    ) -> Result<AttackLeechObservation, AttackLeechWriteError> {
        let projected = project(expected, target, value)?;
        // SAFETY: inherits set_attack_leech's retained runtime/metadata, attached thread and synchronous rooted graph-access contract.
        unsafe {
            let graph = self.attack_graph(attached, expected.manager.wrapper_token())?;
            let field = self.attack_mutable_field(&graph, target)?;
            let object = target_object(&graph)?;
            // No metadata work or managed body follows this last complete guarded
            // observation before the store: compare all six tokens and both bits.
            let before = self.attack_graph_snapshot(&graph)?;
            if before != *expected {
                return Err(AttackLeechWriteError::Stale("Complete current field observation differs from expected".into()));
            }
            attempt_attack_store(write_api, object, field, value, || {
                let actual = self.attack_graph_snapshot(&graph)?;
                if actual != projected {
                    return Err(AttackLeechWriteError::Failed("Complete current graph/readback differs from projected observation".into()));
                }
                Ok(actual)
            })
        }
    }
}

fn target_object(graph: &AttackGraph<'_, '_>) -> Result<NonNull<raw::Object>, AttackLeechWriteError> {
    target(&graph.stats).map_err(AttackLeechWriteError::from)
}
