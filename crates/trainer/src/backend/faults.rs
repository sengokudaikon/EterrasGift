//! Fault evidence is recorded before publication barriers, including drained writes.
use super::*;
use eg_core::fault::FaultDetails;
use serde_json::{Value, json};

impl<P: ProcessLocator, C: Connector, L: LogSource + Send + 'static, K: Clock> BackendWorker<P, C, L, K> {
    pub(super) fn log_forge_outcome(&self, plan: &ForgePlan, status: &WriteStatus) {
        let Some(faults) = &self.faults else { return };
        let (outcome, failure, observed) = match status {
            WriteStatus::NotKept(actual) => ("not kept; the readback item is the game's state", None, Some(actual)),
            WriteStatus::Uncertain { message } => ("unconfirmed; do not automatically retry", Some(message.as_str()), None),
            _ => return,
        };
        let diagnostics = plan.inspect_readback(observed);
        let before = plan.before();
        let details = FaultDetails {
            code: Some(if observed.is_some() { "EditNotKept" } else { "WriteUncertain" }.into()),
            write_outcome: Some(outcome.into()),
            expected: Some(json!({
                "process": {"pid": before.process().pid, "created": before.process().created, "executable": before.process().executable},
                "generation": before.generation(), "item": forge_record(before.snapshot()),
            })),
            edit: Some(edit(plan.edit_intent())),
            readback: Some(json!({
                "policy": format!("{:?}", diagnostics.policy), "item": observed.map(forge_record),
                "matchingSlots": diagnostics.matching_slots,
                "differences": diagnostics.differences.iter().map(|d| json!({"field": d.field, "expected": d.expected, "actual": d.actual})).collect::<Vec<_>>(),
            })),
            ..Default::default()
        };
        faults.write(
            "Forge write",
            failure,
            Some(&details),
            Some(if observed.is_some() {
                "The game's observed state is retained; drafts and Undo are cleared. The session may remain connected."
            } else {
                "Drafts and Undo are cleared. Release the session and explicitly reconnect; never replay this write."
            }),
        );
    }
}

/// The item as the game reported it, as fault reports record it. Where it lies in the game's memory is left out: it
/// means nothing outside that run.
pub fn forge_record(value: &ForgeSnapshot) -> Value {
    let status = &value.status;
    json!({
        "individualId": value.individual_id, "fingerprint": value.fingerprint, "name": value.name,
        "itemType": value.item_type, "subType": value.sub_type, "rarity": value.rarity, "classRequirement": value.class_requirement,
        "affixes": value.affixes.iter().map(|a| json!({"slot": a.slot, "id": a.id, "tier": a.tier, "roll": a.roll, "kind": a.kind, "seal": a.seal, "name": a.name, "special": a.special})).collect::<Vec<_>>(),
        "implicitRolls": value.implicit_rolls, "identity": value.identity.as_ref().map(|i| json!({"id": i.id, "rolls": i.rolls})),
        "status": {"forgingPotential": status.forging_potential, "forgingPotentialType": status.forging_potential_type,
            "legendaryPotential": status.legendary_potential, "weaversWill": status.weavers_will, "weaversTouch": status.weavers_touch,
            "corrupted": status.corrupted, "ruined": status.ruined, "levelRequirement": status.level_requirement},
    })
}

fn edit(value: &crate::item::edit::ItemEdit) -> Value {
    use crate::item::edit::ItemEdit;
    match value {
        ItemEdit::Affix(e) => {
            json!({"kind": "Affix", "slot": e.slot, "id": e.id, "tier": e.tier, "roll": e.roll, "seal": e.seal as i32, "allowIncompatible": e.allow_incompatible})
        }
        ItemEdit::AddAffix(e) => {
            json!({"kind": "AddAffix", "id": e.id, "tier": e.tier, "roll": e.roll, "seal": e.seal as i32, "affixKind": e.kind as i32, "category": e.category, "allowIncompatible": e.allow_incompatible})
        }
        ItemEdit::RemoveAffix(e) => json!({"kind": "RemoveAffix", "slot": e.slot, "id": e.id}),
        ItemEdit::Roll(e) => {
            json!({"kind": "Roll", "target": e.target as i32, "changes": e.changes.iter().map(|c| json!({"index": c.index, "value": c.value})).collect::<Vec<_>>(), "allowIncompatible": e.allow_incompatible})
        }
        ItemEdit::Status(e) => json!({"kind": "Status", "field": format!("{:?}", e.field), "value": e.value, "unrestricted": e.unrestricted}),
        ItemEdit::BaseType(e) => json!({"kind": "BaseType", "subType": e.sub_type, "itemType": e.item_type, "allowIncompatible": e.allow_incompatible}),
        ItemEdit::UniqueIdentity(e) => json!({"kind": "UniqueIdentity", "uniqueId": e.unique_id, "subType": e.sub_type, "rarity": e.rarity, "unrestricted": e.unrestricted}),
    }
}
