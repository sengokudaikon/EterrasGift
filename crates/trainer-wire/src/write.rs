//! Owned conditional Forge writes and bounded native acknowledgement decoding.
//! These codecs grant no process access or write permission. Host generation,
//! profile, policy and main-thread runtime admission remain independent checks.

#![forbid(unsafe_code)]

use crate::{
    forge::{self, AffixSnapshot, ForgeSnapshot},
    protocol::{DispatcherSpec, MAX_RESPONSE_SIZE, ProtocolError, REQUEST_HEADER_SIZE, Response, ResponseCode},
};

pub const CHANGE_AFFIX_OPERATION: u32 = 1;

pub const CHANGE_AFFIX_ROLLS_OPERATION: u32 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AffixRollChange {
    pub slot: u8,
    pub value: u8,
}

/// A bounded, nonempty batch with distinct slots. Slots still need a fresh item
/// and definition check in the helper; construction grants no memory access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffixRolls {
    changes: [AffixRollChange; 32],
    len: u8,
}
impl AffixRolls {
    pub fn new(changes: &[AffixRollChange]) -> Result<Self, ProtocolError> {
        if changes.is_empty() || changes.len() > 32 {
            return invalid();
        }
        let mut seen = 0_u32;
        let mut result = Self {
            changes: [AffixRollChange::default(); 32],
            len: changes.len() as u8,
        };
        for (index, change) in changes.iter().copied().enumerate() {
            if change.slot >= 32 || seen & (1_u32 << change.slot) != 0 {
                return invalid();
            }
            seen |= 1_u32 << change.slot;
            result.changes[index] = change;
        }
        Ok(result)
    }
    pub fn changes(&self) -> &[AffixRollChange] {
        &self.changes[..usize::from(self.len)]
    }
}

pub const SET_STATUS_OPERATION: u32 = 10;
pub const CHANGE_BASE_OPERATION: u32 = 11;
pub const ADD_AFFIX_OPERATION: u32 = 13;
pub const REMOVE_AFFIX_OPERATION: u32 = 14;
pub const CHANGE_IDENTITY_OPERATION: u32 = 17;

/// Only operations implemented by the helper. Wire numbers remain internal to
/// transport; domain planning separately checks catalog compatibility/override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForgeMutation {
    AddAffix {
        id: u16,
        tier: u8,
        roll: u8,
        seal: u8,
        unrestricted: bool,
    },
    RemoveAffix {
        slot: u8,
        id: u16,
    },
    Affix {
        slot: u8,
        id: u16,
        tier: u8,
        roll: u8,
        seal: u8,
        unrestricted: bool,
    },
    AffixRolls {
        rolls: AffixRolls,
        unrestricted: bool,
    },
    Status {
        field: StatusField,
        value: u8,
        unrestricted: bool,
    },
    Base {
        item_type: u8,
        sub_type: u16,
        unrestricted: bool,
    },
    Identity {
        id: u16,
        sub_type: u8,
        rarity: u8,
        unrestricted: bool,
    },
}
impl ForgeMutation {
    pub fn unrestricted(self) -> bool {
        match self {
            Self::RemoveAffix { .. } => false,
            Self::AddAffix { unrestricted, .. }
            | Self::Affix { unrestricted, .. }
            | Self::AffixRolls { unrestricted, .. }
            | Self::Status { unrestricted, .. }
            | Self::Base { unrestricted, .. }
            | Self::Identity { unrestricted, .. } => unrestricted,
        }
    }
    pub fn project(self, item: &mut ForgeSnapshot) {
        match self {
            // Transport-only projection: constructor-owned kind/category/name
            // are resolved by the native runtime, not inferred by this codec.
            Self::AddAffix { id, tier, roll, seal, .. } => {
                item.affixes.push(AffixSnapshot {
                    slot: item.affixes.len() as u8,
                    id,
                    tier: u16::from(tier),
                    roll,
                    seal: i32::from(seal),
                    kind: 0,
                    special: 0,
                    name: String::new(),
                });
                reorder_affixes(item);
                item.rarity = affix_list_rarity(item, &item.affixes);
            }
            Self::RemoveAffix { slot, .. } => {
                if usize::from(slot) < item.affixes.len() {
                    item.affixes.remove(usize::from(slot));
                }
                reorder_affixes(item);
                item.rarity = affix_list_rarity(item, &item.affixes);
            }
            Self::Affix { slot, id, tier, roll, seal, .. } => {
                if let Some(affix) = item.affixes.get_mut(usize::from(slot)) {
                    affix.id = id;
                    affix.tier = u16::from(tier);
                    affix.roll = roll;
                    affix.seal = i32::from(seal);
                }
                reorder_affixes(item);
            }
            Self::AffixRolls { rolls, .. } => {
                for change in rolls.changes() {
                    if let Some(affix) = item.affixes.get_mut(usize::from(change.slot)) {
                        affix.roll = change.value;
                    }
                }
            }
            Self::Status { field, value, .. } => field.project(item, value),
            Self::Base { item_type, sub_type, .. } => {
                item.item_type = item_type;
                item.sub_type = sub_type;
            }
            Self::Identity { id, sub_type, rarity, .. } => {
                if let Some(identity) = &mut item.identity {
                    identity.id = id;
                }
                item.sub_type = u16::from(sub_type);
                item.rarity = rarity;
            }
        }
        item.fingerprint = item.legacy_fingerprint();
    }
    fn validate(self) -> Result<(), ProtocolError> {
        let valid = match self {
            Self::AddAffix { id, tier, seal, .. } => id <= 4095 && (1..=8).contains(&tier) && seal <= 3,
            Self::Affix { slot, id, tier, seal, .. } => slot < 32 && id <= 4095 && (1..=8).contains(&tier) && seal <= 3,
            Self::RemoveAffix { slot, .. } => slot < 32,
            Self::Status {
                field: StatusField::Corrupted | StatusField::Ruined,
                value,
                ..
            } => value <= 1,
            Self::Identity { rarity, .. } => matches!(rarity, 7..=9),
            _ => true,
        };
        if valid { Ok(()) } else { invalid() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum StatusField {
    ForgingPotential = 0,
    LegendaryPotential = 1,
    WeaversWill = 2,
    Corrupted = 3,
    Ruined = 4,
    LevelRequirement = 5,
    WeaversTouch = 6,
}
impl StatusField {
    pub fn from_wire(value: u32) -> Result<Self, ProtocolError> {
        match value {
            0 => Ok(Self::ForgingPotential),
            1 => Ok(Self::LegendaryPotential),
            2 => Ok(Self::WeaversWill),
            3 => Ok(Self::Corrupted),
            4 => Ok(Self::Ruined),
            5 => Ok(Self::LevelRequirement),
            6 => Ok(Self::WeaversTouch),
            _ => invalid(),
        }
    }
    pub fn requires_definition(self, item: &ForgeSnapshot) -> bool {
        matches!(self, Self::LegendaryPotential | Self::WeaversWill) && item.rarity == 7
    }
    /// Game status rules shared by planning and the native pre-store guard.
    /// `legendary_type` comes from the captured catalog on the host and getUnique
    /// inside the game. Neither an override nor an absent definition guesses it.
    pub fn allows(self, item: &ForgeSnapshot, limits: &crate::definitions::ItemLimits, value: i32, unrestricted: bool, legendary_type: Option<i32>) -> bool {
        if !(0..=255).contains(&value) {
            return false;
        }
        let status = &item.status;
        let in_range = |range: crate::definitions::ValueRange| (i32::from(range.minimum)..=i32::from(range.maximum)).contains(&value);
        match self {
            Self::ForgingPotential => !matches!(item.rarity, 7..=9) && in_range(limits.forging_potential),
            Self::LevelRequirement => !matches!(item.rarity, 7..=9) && in_range(limits.level_requirement),
            Self::Corrupted | Self::Ruined => matches!(value, 0 | 1),
            Self::WeaversTouch => unrestricted && (25..=33).contains(&item.item_type) && (value == 0 || in_range(limits.weavers_touch)),
            Self::LegendaryPotential | Self::WeaversWill => {
                let will = self == Self::WeaversWill;
                if will && unrestricted && item.rarity == 9 {
                    value <= i32::from(status.weavers_will.max(limits.weavers_will.maximum)) && (status.weavers_will > 0 || status.legendary_potential == 0 || value == 0)
                } else if item.rarity == 7 {
                    let forced = will && unrestricted && legendary_type == Some(0);
                    (legendary_type == Some(i32::from(will)) || forced)
                        && (if will { status.legendary_potential == 0 } else { status.weavers_will == 0 })
                        && (if will {
                            in_range(limits.weavers_will) || (forced && value == 0)
                        } else {
                            in_range(limits.legendary_potential)
                        })
                } else {
                    false
                }
            }
        }
    }
    pub fn value(self, item: &ForgeSnapshot) -> i32 {
        match self {
            Self::ForgingPotential => i32::from(item.status.forging_potential),
            Self::LegendaryPotential => i32::from(item.status.legendary_potential),
            Self::WeaversWill => i32::from(item.status.weavers_will),
            Self::Corrupted => i32::from(item.status.corrupted),
            Self::Ruined => i32::from(item.status.ruined),
            Self::LevelRequirement => item.status.level_requirement,
            Self::WeaversTouch => i32::from(item.status.weavers_touch),
        }
    }
    pub fn project(self, item: &mut ForgeSnapshot, value: u8) {
        match self {
            Self::ForgingPotential => item.status.forging_potential = value,
            Self::LegendaryPotential => item.status.legendary_potential = value,
            Self::WeaversWill => item.status.weavers_will = value,
            Self::Corrupted => item.status.corrupted = value != 0,
            Self::Ruined => item.status.ruined = value != 0,
            Self::LevelRequirement => item.status.level_requirement = i32::from(value),
            Self::WeaversTouch => item.status.weavers_touch = value,
        }
        item.fingerprint = item.legacy_fingerprint();
    }
}
pub const MAX_ERROR_TEXT: usize = 4096;
/// Exact maximum of the existing strict owned forge schema: fixed48, main name,
/// 32 affixes each20+name bytes,3 implicit rolls,optional identity3+8roll bytes.
/// This is a versioned schema limit, not an arbitrary caller-provided allocation.
pub const MAX_EXPECTED_BYTES: usize = 48 + forge::MAX_NAME_BYTES + forge::MAX_AFFIXES * (20 + forge::MAX_NAME_BYTES) + forge::MAX_IMPLICIT_ROLLS + 3 + forge::MAX_UNIQUE_ROLLS;
pub const MIN_EXPECTED_BYTES: usize = 48;
pub const MAX_REQUEST_BYTES: usize = REQUEST_HEADER_SIZE + MAX_EXPECTED_BYTES + 256;
fn invalid<T>() -> Result<T, ProtocolError> {
    Err(ProtocolError::InvalidData)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeWrite {
    dispatcher: DispatcherSpec,
    expected_manager: u64,
    expected: forge::CheckedSnapshot,
    mutation: ForgeMutation,
}
impl ForgeWrite {
    /// Validates/copies the complete expected item before any transport operation.
    /// Manager and item addresses are opaque identity tokens, never dereferenced.
    /// Eligibility and the current game's literal maxLevel remain native checks.
    pub fn level_requirement(dispatcher: DispatcherSpec, expected_manager: u64, expected: ForgeSnapshot, value: i32) -> Result<Self, ProtocolError> {
        Self::new(dispatcher, expected_manager, expected, StatusField::LevelRequirement, value)
    }
    pub fn new(dispatcher: DispatcherSpec, expected_manager: u64, expected: ForgeSnapshot, field: StatusField, value: i32) -> Result<Self, ProtocolError> {
        Self::with_override(dispatcher, expected_manager, expected, field, value, false)
    }
    pub fn with_override(
        dispatcher: DispatcherSpec,
        expected_manager: u64,
        expected: ForgeSnapshot,
        field: StatusField,
        value: i32,
        unrestricted: bool,
    ) -> Result<Self, ProtocolError> {
        let value = u8::try_from(value).map_err(|_| ProtocolError::InvalidData)?;
        Self::edit(dispatcher, expected_manager, expected, ForgeMutation::Status { field, value, unrestricted })
    }
    pub fn edit(dispatcher: DispatcherSpec, expected_manager: u64, expected: ForgeSnapshot, mutation: ForgeMutation) -> Result<Self, ProtocolError> {
        Self::from_checked(dispatcher, expected_manager, forge::CheckedSnapshot::new(expected)?, mutation)
    }
    pub(crate) fn from_checked(dispatcher: DispatcherSpec, expected_manager: u64, expected: forge::CheckedSnapshot, mutation: ForgeMutation) -> Result<Self, ProtocolError> {
        mutation.validate()?;
        if expected_manager == 0 {
            return invalid();
        }
        Ok(Self {
            dispatcher,
            expected_manager,
            expected,
            mutation,
        })
    }
    pub fn expected(&self) -> &ForgeSnapshot {
        &self.expected
    }
    pub fn expected_manager(&self) -> u64 {
        self.expected_manager
    }
    pub fn mutation(&self) -> ForgeMutation {
        self.mutation
    }
    pub fn unrestricted(&self) -> bool {
        self.mutation.unrestricted()
    }
    pub fn dispatcher(&self) -> DispatcherSpec {
        self.dispatcher
    }
    pub fn projected(&self) -> ForgeSnapshot {
        let mut projected = (*self.expected).clone();
        self.mutation.project(&mut projected);
        projected
    }
    /// Infallible apart from allocation: all fields and canonical tail are owned,
    /// immutable and validated by construction. No encoder unwrap of caller data.
    pub fn encode(&self) -> Vec<u8> {
        crate::protocol::Request::EditForge(self.clone()).encode()
    }
    pub(crate) fn expected_bytes(&self) -> &[u8] {
        self.expected.bytes()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteReply {
    /// Complete native acknowledgement for the same item. This is provisional;
    /// only the session's separate read may publish Confirmed or NotKept.
    Confirmed(ForgeSnapshot),
    /// Trusted helper contract raises these codes only before the store boundary.
    /// This is never permission for an automatic write retry.
    NotApplied { code: ResponseCode, message: String },
    /// Store may have happened; retain diagnostics and require explicit fresh read.
    Uncertain { reason: String },
}
fn uncertain(reason: impl Into<String>) -> WriteReply {
    WriteReply::Uncertain { reason: reason.into() }
}
pub fn classify_response(response: &Response, request: &ForgeWrite) -> WriteReply {
    if response.payload.len() > MAX_RESPONSE_SIZE {
        return uncertain("response exceeds frame bound");
    }
    if response.code == ResponseCode::Done {
        return match forge::decode(&response.payload) {
            Ok(Some(actual)) if actual.item == request.expected().item && actual.individual_id == request.expected().individual_id => WriteReply::Confirmed(actual),
            Ok(_) => uncertain("write acknowledgement has no current target"),
            Err(_) => uncertain("invalid write readback payload"),
        };
    }
    if response.payload.len() > MAX_ERROR_TEXT {
        return uncertain("failure text exceeds bound");
    }
    let Ok(message) = std::str::from_utf8(&response.payload) else {
        return uncertain("invalid UTF-8 failure text");
    };
    if response.code == ResponseCode::Failed {
        return uncertain(message);
    }
    WriteReply::NotApplied {
        code: response.code,
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutProblem {
    Capacity,
    UniqueCorrupted,
    LegendaryUncorrupted,
    UniqueSealed,
    MultipleSealed,
    MultipleCorrupted,
    PrimordialTier,
    NormalTierEight,
}
pub fn layout_problems(item: &ForgeSnapshot, affixes: &[AffixSnapshot]) -> Vec<LayoutProblem> {
    let mut problems = Vec::new();
    let sealed = affixes.iter().filter(|a| a.seal == 1).count();
    let corrupted = affixes.iter().filter(|a| a.seal == 3).count();
    if affixes.len() > if matches!(item.rarity, 7..=9) { 7 } else { 32 } {
        problems.push(LayoutProblem::Capacity)
    }
    if corrupted > 0 && matches!(item.rarity, 7 | 8) {
        problems.push(LayoutProblem::UniqueCorrupted)
    }
    if corrupted > 0 && item.rarity == 9 && !item.status.corrupted {
        problems.push(LayoutProblem::LegendaryUncorrupted)
    }
    if sealed > 0 && matches!(item.rarity, 7..=9) {
        problems.push(LayoutProblem::UniqueSealed)
    }
    if sealed > 1 {
        problems.push(LayoutProblem::MultipleSealed)
    }
    if corrupted > 1 {
        problems.push(LayoutProblem::MultipleCorrupted)
    }
    if affixes.iter().any(|a| a.seal == 2 && a.tier != 8) {
        problems.push(LayoutProblem::PrimordialTier)
    }
    if affixes.iter().any(|a| a.seal == 0 && a.tier == 8) {
        problems.push(LayoutProblem::NormalTierEight)
    }
    problems
}
/// Stable load grouping used by the game: regular seal, corrupted seal, remaining affixes.
pub fn affix_order_key(rarity: u8, seal: i32) -> u8 {
    if matches!(rarity, 7..=9) {
        u8::from(seal == 3)
    } else {
        match seal {
            1 => 0,
            3 => 1,
            _ => 2,
        }
    }
}

fn reorder_affixes(item: &mut ForgeSnapshot) {
    item.affixes.sort_by_key(|a| affix_order_key(item.rarity, a.seal));
    for (slot, affix) in item.affixes.iter_mut().enumerate() {
        affix.slot = slot as u8;
    }
}
/// Host expectation only: the native writer invokes the game's rarity method.
pub fn affix_list_rarity(item: &ForgeSnapshot, affixes: &[AffixSnapshot]) -> u8 {
    if matches!(item.rarity, 7..=9) || item.rarity > 6 {
        return item.rarity;
    }
    let seals = affixes.iter().map(|a| a.seal).filter(|s| *s != 0).collect::<std::collections::BTreeSet<_>>();
    affixes.len().saturating_sub(seals.len()).min(4) as u8
}
