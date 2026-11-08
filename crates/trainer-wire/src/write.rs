//! Owned conditional Forge writes and bounded native acknowledgement decoding.
//! These codecs grant no process access or write permission. Process identity,
//! profile, policy and main-thread runtime admission remain independent checks.

#![forbid(unsafe_code)]

use crate::{
    codec::{Reader, Wire, Writer, wire_enum},
    forge::{self, AffixSnapshot, ForgeSnapshot},
    protocol::{DispatcherSpec, MAX_RESPONSE_SIZE, ProtocolError, REQUEST_HEADER_SIZE, Response, ResponseCode, op},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AffixRollChange {
    pub slot: u8,
    pub value: u8,
}

/// A bounded, nonempty batch with distinct slots (an affix's slot, or a unique or implicit roll's index). Slots still need a fresh item
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

wire_enum! {
    /// The item's own roll arrays: the unique (or set) definition's rolls and the base's implicits.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum RollArray: u8 {
        Unique = 1,
        Implicit = 2,
    }
}
impl RollArray {
    /// How many rolls the game's array holds at most.
    pub fn capacity(self) -> usize {
        match self {
            Self::Unique => forge::MAX_UNIQUE_ROLLS,
            Self::Implicit => forge::MAX_IMPLICIT_ROLLS,
        }
    }
}

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
        /// The legendary potential a legendary gets back when the removal leaves it a unique: an Undo of the upgrade
        /// (`rules::affix_list_outcome` spends it).
        restore_potential: Option<u8>,
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
    /// Roll positions in one of the item's roll arrays; which modifiers they belong to is the game's to say.
    ModifierRolls {
        array: RollArray,
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
            | Self::ModifierRolls { unrestricted, .. }
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
                crate::rules::reorder_affixes(item);
                (item.rarity, item.status) = crate::rules::affix_list_outcome(item, &item.affixes);
            }
            Self::RemoveAffix { slot, restore_potential, .. } => {
                if usize::from(slot) < item.affixes.len() {
                    item.affixes.remove(usize::from(slot));
                }
                crate::rules::reorder_affixes(item);
                let legendary = item.rarity == 9;
                (item.rarity, item.status) = crate::rules::affix_list_outcome(item, &item.affixes);
                if let Some(potential) = restore_potential
                    && legendary
                    && item.rarity == 7
                {
                    item.status.legendary_potential = potential;
                }
            }
            Self::Affix { slot, id, tier, roll, seal, .. } => {
                if let Some(affix) = item.affixes.get_mut(usize::from(slot)) {
                    affix.id = id;
                    affix.tier = u16::from(tier);
                    affix.roll = roll;
                    affix.seal = i32::from(seal);
                }
                crate::rules::reorder_affixes(item);
                (item.rarity, item.status) = crate::rules::affix_list_outcome(item, &item.affixes);
            }
            Self::AffixRolls { rolls, .. } => {
                for change in rolls.changes() {
                    if let Some(affix) = item.affixes.get_mut(usize::from(change.slot)) {
                        affix.roll = change.value;
                    }
                }
            }
            Self::ModifierRolls { array, rolls, .. } => {
                let target = match array {
                    RollArray::Unique => item.identity.as_mut().map(|identity| &mut identity.rolls),
                    RollArray::Implicit => Some(&mut item.implicit_rolls),
                };
                if let Some(target) = target {
                    for change in rolls.changes() {
                        if let Some(roll) = target.get_mut(usize::from(change.slot)) {
                            *roll = change.value;
                        }
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
            Self::ModifierRolls { array, rolls, .. } => rolls.changes().iter().all(|change| usize::from(change.slot) < array.capacity()),
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

wire_enum! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum StatusField: i32 {
        ForgingPotential = 0,
        LegendaryPotential = 1,
        WeaversWill = 2,
        Corrupted = 3,
        Ruined = 4,
        LevelRequirement = 5,
        WeaversTouch = 6,
    }
}
impl StatusField {
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
}

/// A roll batch on the wire: its count, then each slot and value.
impl Wire for AffixRolls {
    fn write(&self, w: &mut Writer) {
        w.byte(self.len);
        for change in self.changes() {
            w.byte(change.slot);
            w.byte(change.value);
        }
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        let count = r.count(32)?;
        if count == 0 {
            return invalid();
        }
        let mut changes = Vec::with_capacity(count);
        for _ in 0..count {
            changes.push(AffixRollChange {
                slot: r.byte()?,
                value: r.byte()?,
            });
        }
        Self::new(&changes)
    }
}

impl ForgeMutation {
    /// The operation that carries it.
    pub(crate) fn operation(self) -> u32 {
        match self {
            Self::AddAffix { .. } => op::ADD_AFFIX,
            Self::RemoveAffix { .. } => op::REMOVE_AFFIX,
            Self::Affix { .. } => op::CHANGE_AFFIX,
            Self::AffixRolls { .. } => op::CHANGE_AFFIX_ROLLS,
            Self::ModifierRolls { .. } => op::CHANGE_MODIFIER_ROLLS,
            Self::Status { .. } => op::SET_STATUS,
            Self::Base { .. } => op::CHANGE_BASE,
            Self::Identity { .. } => op::CHANGE_IDENTITY,
        }
    }
    fn write(self, w: &mut Writer) {
        match self {
            Self::AddAffix {
                id,
                tier,
                roll,
                seal,
                unrestricted,
            } => {
                w.u16(id);
                w.byte(tier);
                w.byte(roll);
                w.byte(seal);
                w.flag(unrestricted);
            }
            Self::RemoveAffix { slot, id, restore_potential } => {
                w.byte(slot);
                w.u16(id);
                w.flag(restore_potential.is_some());
                if let Some(potential) = restore_potential {
                    w.byte(potential);
                }
            }
            Self::Affix {
                slot,
                id,
                tier,
                roll,
                seal,
                unrestricted,
            } => {
                w.byte(slot);
                w.u16(id);
                w.byte(tier);
                w.byte(roll);
                w.byte(seal);
                w.flag(unrestricted);
            }
            Self::AffixRolls { rolls, unrestricted } => {
                w.flag(unrestricted);
                w.put(&rolls);
            }
            Self::ModifierRolls { array, rolls, unrestricted } => {
                w.byte(array as u8);
                w.flag(unrestricted);
                w.put(&rolls);
            }
            Self::Status { field, value, unrestricted } => {
                w.byte(field as u8);
                w.byte(value);
                w.flag(unrestricted);
            }
            Self::Base {
                item_type,
                sub_type,
                unrestricted,
            } => {
                w.byte(item_type);
                w.u16(sub_type);
                w.flag(unrestricted);
            }
            Self::Identity {
                id,
                sub_type,
                rarity,
                unrestricted,
            } => {
                w.u16(id);
                w.byte(sub_type);
                w.byte(rarity);
                w.flag(unrestricted);
            }
        }
    }
    fn read(operation: u32, r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        Ok(match operation {
            op::ADD_AFFIX => Self::AddAffix {
                id: r.u16()?,
                tier: r.byte()?,
                roll: r.byte()?,
                seal: r.byte()?,
                unrestricted: r.flag()?,
            },
            op::REMOVE_AFFIX => Self::RemoveAffix {
                slot: r.byte()?,
                id: r.u16()?,
                restore_potential: if r.flag()? { Some(r.byte()?) } else { None },
            },
            op::CHANGE_AFFIX => Self::Affix {
                slot: r.byte()?,
                id: r.u16()?,
                tier: r.byte()?,
                roll: r.byte()?,
                seal: r.byte()?,
                unrestricted: r.flag()?,
            },
            op::CHANGE_AFFIX_ROLLS => {
                let unrestricted = r.flag()?;
                Self::AffixRolls { rolls: r.get()?, unrestricted }
            }
            op::CHANGE_MODIFIER_ROLLS => {
                let array = RollArray::try_from(r.byte()?)?;
                let unrestricted = r.flag()?;
                Self::ModifierRolls {
                    array,
                    rolls: r.get()?,
                    unrestricted,
                }
            }
            op::SET_STATUS => Self::Status {
                field: StatusField::try_from(i32::from(r.byte()?))?,
                value: r.byte()?,
                unrestricted: r.flag()?,
            },
            op::CHANGE_BASE => Self::Base {
                item_type: r.byte()?,
                sub_type: r.u16()?,
                unrestricted: r.flag()?,
            },
            op::CHANGE_IDENTITY => Self::Identity {
                id: r.u16()?,
                sub_type: r.byte()?,
                rarity: r.byte()?,
                unrestricted: r.flag()?,
            },
            _ => return invalid(),
        })
    }
}

impl ForgeWrite {
    /// Its body after the dispatcher: the manager, the mutation's fields, then the expected item.
    pub(crate) fn write(&self, w: &mut Writer) {
        w.u64(self.expected_manager);
        self.mutation.write(w);
        w.put(&self.expected);
    }
    pub(crate) fn read(dispatcher: DispatcherSpec, operation: u32, r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        let manager = r.u64()?;
        let mutation = ForgeMutation::read(operation, r)?;
        Self::from_checked(dispatcher, manager, r.get()?, mutation)
    }
}

/// A conditional write's acknowledgement, classified from its bytes alone. No result authorizes a retry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply<T> {
    /// Complete native acknowledgement for the same target. This is provisional;
    /// only the session's separate read may publish Confirmed or NotKept.
    Confirmed(T),
    /// Trusted helper contract raises these codes only before the store boundary.
    /// This is never permission for an automatic write retry.
    NotApplied { code: ResponseCode, message: String },
    /// Store may have happened; retain diagnostics and require explicit fresh read.
    Uncertain { reason: String },
}
pub type WriteReply = Reply<ForgeSnapshot>;

/// Classifies a write's reply. `confirm` reads a Done payload as the acknowledged observation, or says why it is not
/// one. A refusal that cannot be parsed cannot prove that nothing ran, so it stays uncertain.
pub fn classify<T>(response: &Response, confirm: impl FnOnce(&[u8]) -> Result<T, &'static str>) -> Reply<T> {
    let uncertain = |reason: &str| Reply::Uncertain { reason: reason.into() };
    if response.payload.len() > MAX_RESPONSE_SIZE {
        return uncertain("response exceeds frame bound");
    }
    if response.code == ResponseCode::Done {
        return confirm(&response.payload).map_or_else(uncertain, Reply::Confirmed);
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
    Reply::NotApplied {
        code: response.code,
        message: message.into(),
    }
}

pub fn classify_response(response: &Response, request: &ForgeWrite) -> WriteReply {
    classify(response, |payload| match forge::decode(payload) {
        Ok(Some(actual)) if actual.item == request.expected().item && actual.individual_id == request.expected().individual_id => Ok(actual),
        Ok(_) => Err("write acknowledgement has no current target"),
        Err(_) => Err("invalid write readback payload"),
    })
}
