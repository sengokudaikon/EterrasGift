//! Request bodies are sequential fields. Only the common envelope has fixed positions.
use super::*;
use crate::{
    mapping::{
        AffixMappingDraft, AffixMappingQuery, ForgeDescriptionRequest, MappingAddress, MappingStart, MappingTarget, ModifierMappingQuery, UniquePreviewRequest, UniqueRollChange,
    },
    new_affixes::{MAX_PER_REQUEST, MAX_PROPERTIES, MAX_TEXT, NewAffix, StatLine},
    write::{AffixRollChange, AffixRolls, RollArray, StatusField},
};

const MAX_BODY: usize = MAX_EXPECTED_BYTES + 256;

struct Writer(Vec<u8>);
impl Writer {
    fn byte(&mut self, value: u8) {
        self.0.push(value);
    }
    fn flag(&mut self, value: bool) {
        self.byte(u8::from(value));
    }
    fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn blob(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        self.0.extend_from_slice(bytes);
    }
    fn snapshot(&mut self, snapshot: &forge::CheckedSnapshot) {
        self.blob(snapshot.bytes());
    }
}
impl Reader<'_> {
    fn flag(&mut self) -> Result<bool, ProtocolError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ProtocolError::InvalidData),
        }
    }
    fn u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn snapshot(&mut self) -> Result<forge::CheckedSnapshot, ProtocolError> {
        let length = self.u32()? as usize;
        forge::CheckedSnapshot::decode(self.take(length)?)
    }
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        // The header's operation and length are filled in once the body is written.
        let mut body = Writer(Vec::with_capacity(REQUEST_HEADER_SIZE + 64));
        body.u32(MAGIC);
        body.u32(0);
        body.u32(0);
        if let Some(dispatcher) = self.dispatcher() {
            body.u64(dispatcher.rva);
            body.0.extend_from_slice(&dispatcher.bytes);
        }
        let operation = match self {
            Self::Discover => op::DISCOVER,
            Self::Unload => op::UNLOAD,
            Self::SetGameplayHook {
                expected_manager,
                hook,
                on,
                value,
                ..
            } => {
                body.u64(*expected_manager);
                body.byte(*hook as u8);
                body.flag(*on);
                body.u64(*value as u64);
                if hook.value_kind().wide() || *value > u64::MAX as crate::hooks::HookValue {
                    body.u64((*value >> 64) as u64);
                }
                op::SET_GAMEPLAY_HOOK
            }
            Self::ReadGameplayHook { expected_manager, hook, .. } => {
                body.u64(*expected_manager);
                body.byte(*hook as u8);
                op::READ_GAMEPLAY_HOOK
            }
            Self::RegisterAffixes { expected_manager, affixes, .. } => {
                body.u64(*expected_manager);
                body.byte(affixes.len() as u8);
                for affix in affixes {
                    body.u16(affix.id);
                    body.blob(affix.name.as_bytes());
                    body.blob(affix.display_name.as_bytes());
                    body.u32(affix.standard_effect.to_bits());
                    body.byte(affix.properties.len() as u8);
                    for line in &affix.properties {
                        body.byte(line.property);
                        body.byte(line.special_tag);
                        body.u32(line.tags as u32);
                        body.u32(line.extra_tag as u32);
                        body.u32(line.modifier_type as u32);
                        body.u32(line.minimum.to_bits());
                        body.u32(line.maximum.to_bits());
                    }
                }
                op::REGISTER_AFFIXES
            }
            Self::Status { .. } => op::STATUS,
            Self::ClearHooks { .. } => op::CLEAR_HOOKS,
            Self::ReadForge { .. } => op::READ_FORGE,
            Self::ReadFields { expected_manager, set, .. } => {
                body.u64(*expected_manager);
                body.byte(*set as u8);
                op::READ_FIELDS
            }
            Self::WriteFields { edit, .. } => {
                body.blob(&edit.expected.encode().expect("validated field edit"));
                body.u32(edit.values.len() as u32);
                for value in &edit.values {
                    body.flag(value.is_some());
                    if let Some(value) = value {
                        body.u32(*value as u32);
                    }
                }
                op::WRITE_FIELDS
            }
            Self::ReadLocale { .. } => op::READ_LOCALE,
            Self::DescribeImplicits(query) | Self::DescribeUnique(query) => {
                body.u64(query.manager());
                body.snapshot(&query.expected);
                if matches!(self, Self::DescribeUnique(_)) {
                    op::DESCRIBE_UNIQUE
                } else {
                    op::DESCRIBE_IMPLICITS
                }
            }
            Self::PreviewUnique(query) => {
                body.u64(query.manager());
                body.byte(query.changes().len() as u8);
                for change in query.changes() {
                    body.byte(change.index);
                    body.byte(change.value);
                }
                body.snapshot(&query.target.expected);
                op::PREVIEW_UNIQUE
            }
            Self::MapRoll(query) => {
                body.u64(query.manager());
                body.byte(query.slot());
                body.flag(matches!(query.start(), MappingStart::Samples(_)));
                if let MappingStart::Samples(start) = query.start() {
                    body.byte(start);
                }
                body.flag(query.draft().is_some());
                if let Some(draft) = query.draft() {
                    body.u16(draft.id);
                    body.byte(draft.tier);
                    body.byte(draft.roll);
                    body.byte(draft.seal);
                }
                body.snapshot(&query.target.expected);
                op::MAP_ROLL
            }
            Self::MapModifierRoll(query) => {
                let address = query.address();
                body.u64(query.manager());
                body.byte(address.target as u8);
                body.byte(address.index);
                body.flag(address.line.is_some());
                if let Some(line) = address.line {
                    body.u32(line);
                }
                body.flag(matches!(query.start(), MappingStart::Samples(_)));
                if let MappingStart::Samples(start) = query.start() {
                    body.byte(start);
                }
                body.byte(query.draft().len() as u8);
                for change in query.draft() {
                    body.byte(change.index);
                    body.byte(change.value);
                }
                body.snapshot(&query.target.expected);
                op::MAP_MODIFIER_ROLL
            }
            Self::EditForge(write) => {
                body.u64(write.expected_manager());
                let operation = match write.mutation() {
                    ForgeMutation::AddAffix {
                        id,
                        tier,
                        roll,
                        seal,
                        unrestricted,
                    } => {
                        body.u16(id);
                        body.byte(tier);
                        body.byte(roll);
                        body.byte(seal);
                        body.flag(unrestricted);
                        op::ADD_AFFIX
                    }
                    ForgeMutation::RemoveAffix { slot, id, restore_potential } => {
                        body.byte(slot);
                        body.u16(id);
                        body.flag(restore_potential.is_some());
                        if let Some(potential) = restore_potential {
                            body.byte(potential);
                        }
                        op::REMOVE_AFFIX
                    }
                    ForgeMutation::Affix {
                        slot,
                        id,
                        tier,
                        roll,
                        seal,
                        unrestricted,
                    } => {
                        body.byte(slot);
                        body.u16(id);
                        body.byte(tier);
                        body.byte(roll);
                        body.byte(seal);
                        body.flag(unrestricted);
                        op::CHANGE_AFFIX
                    }
                    ForgeMutation::AffixRolls { rolls, unrestricted } => {
                        body.flag(unrestricted);
                        body.byte(rolls.changes().len() as u8);
                        for change in rolls.changes() {
                            body.byte(change.slot);
                            body.byte(change.value);
                        }
                        op::CHANGE_AFFIX_ROLLS
                    }
                    ForgeMutation::ModifierRolls { array, rolls, unrestricted } => {
                        body.byte(array as u8);
                        body.flag(unrestricted);
                        body.byte(rolls.changes().len() as u8);
                        for change in rolls.changes() {
                            body.byte(change.slot);
                            body.byte(change.value);
                        }
                        op::CHANGE_MODIFIER_ROLLS
                    }
                    ForgeMutation::Status { field, value, unrestricted } => {
                        body.byte(field as u8);
                        body.byte(value);
                        body.flag(unrestricted);
                        op::SET_STATUS
                    }
                    ForgeMutation::Base {
                        item_type,
                        sub_type,
                        unrestricted,
                    } => {
                        body.byte(item_type);
                        body.u16(sub_type);
                        body.flag(unrestricted);
                        op::CHANGE_BASE
                    }
                    ForgeMutation::Identity {
                        id,
                        sub_type,
                        rarity,
                        unrestricted,
                    } => {
                        body.u16(id);
                        body.byte(sub_type);
                        body.byte(rarity);
                        body.flag(unrestricted);
                        op::CHANGE_IDENTITY
                    }
                };
                body.blob(write.expected_bytes());
                operation
            }
        };
        let length = (body.0.len() - REQUEST_HEADER_SIZE) as u32;
        body.0[4..8].copy_from_slice(&operation.to_le_bytes());
        body.0[8..12].copy_from_slice(&length.to_le_bytes());
        body.0
    }
    pub fn decode_header(bytes: &[u8]) -> Result<RequestHeader, ProtocolError> {
        if bytes.len() != REQUEST_HEADER_SIZE {
            return Err(ProtocolError::InvalidData);
        }
        let mut reader = Reader::new(bytes);
        if reader.u32()? != MAGIC {
            return Err(ProtocolError::InvalidMagic);
        }
        let operation = reader.u32()?;
        let length = reader.u32()? as usize;
        // Every body but Discover's and Unload's starts with the dispatcher (rva and 16 bytes).
        const DISPATCHER: usize = 8 + 16;
        let exact = match operation {
            op::DISCOVER | op::UNLOAD => Some(0),
            op::STATUS | op::CLEAR_HOOKS | op::READ_FORGE | op::READ_LOCALE => Some(DISPATCHER),
            op::READ_GAMEPLAY_HOOK => Some(DISPATCHER + 8 + 1),
            op::READ_FIELDS => Some(DISPATCHER + 8 + 1),
            op::WRITE_FIELDS if (DISPATCHER..=crate::field_edits::MAX_BYTES + 32 * 1024).contains(&length) => Some(length),
            op::SET_GAMEPLAY_HOOK if [DISPATCHER + 8 + 1 + 1 + 8, DISPATCHER + 8 + 1 + 1 + 16].contains(&length) => Some(length),
            // A conditional item request carries its expected item, whose size varies.
            op::CHANGE_AFFIX
            | op::CHANGE_AFFIX_ROLLS
            | op::CHANGE_MODIFIER_ROLLS
            | op::DESCRIBE_UNIQUE
            | op::PREVIEW_UNIQUE
            | op::MAP_ROLL
            | op::MAP_MODIFIER_ROLL
            | op::SET_STATUS
            | op::CHANGE_BASE
            | op::DESCRIBE_IMPLICITS
            | op::ADD_AFFIX
            | op::REMOVE_AFFIX
            | op::CHANGE_IDENTITY
            | op::REGISTER_AFFIXES => None,
            _ => return Err(ProtocolError::InvalidData),
        };
        let valid = match exact {
            Some(exact) => length == exact,
            None => (DISPATCHER..=MAX_BODY).contains(&length),
        };
        if !valid {
            return Err(ProtocolError::InvalidData);
        }
        Ok(RequestHeader { operation, body_len: length })
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let header = Self::decode_header(bytes.get(..REQUEST_HEADER_SIZE).ok_or(ProtocolError::InvalidData)?)?;
        if bytes.len() != header.total_len() {
            return Err(ProtocolError::InvalidData);
        }
        Self::decode_body(header.operation, &bytes[REQUEST_HEADER_SIZE..])
    }
    pub(super) fn decode_body(operation: u32, bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut body = Reader::new(bytes);
        if matches!(operation, op::DISCOVER | op::UNLOAD) {
            body.finish()?;
            return Ok(if operation == op::DISCOVER { Self::Discover } else { Self::Unload });
        }
        let dispatcher = DispatcherSpec {
            rva: body.u64()?,
            bytes: body.array()?,
        };
        let request = match operation {
            op::SET_GAMEPLAY_HOOK | op::READ_GAMEPLAY_HOOK => {
                let expected_manager = body.u64()?;
                if expected_manager == 0 {
                    return Err(ProtocolError::InvalidData);
                }
                let hook = crate::hooks::GameplayHook::decode(body.byte()?)?;
                if operation == op::SET_GAMEPLAY_HOOK {
                    Self::SetGameplayHook {
                        dispatcher,
                        expected_manager,
                        hook,
                        on: body.flag()?,
                        value: {
                            let mut value = crate::hooks::HookValue::from(body.u64()?);
                            if hook.value_kind().wide() {
                                value |= crate::hooks::HookValue::from(body.u64()?) << 64;
                            }
                            hook.validate_value(value)?
                        },
                    }
                } else {
                    Self::ReadGameplayHook {
                        dispatcher,
                        expected_manager,
                        hook,
                    }
                }
            }
            op::REGISTER_AFFIXES => {
                let expected_manager = body.u64()?;
                let count = usize::from(body.byte()?);
                if expected_manager == 0 || !(1..=MAX_PER_REQUEST).contains(&count) {
                    return Err(ProtocolError::InvalidData);
                }
                let mut affixes = Vec::with_capacity(count);
                for _ in 0..count {
                    affixes.push(new_affix(&mut body)?);
                }
                Self::RegisterAffixes {
                    dispatcher,
                    expected_manager,
                    affixes,
                }
            }
            op::STATUS => Self::Status { dispatcher },
            op::CLEAR_HOOKS => Self::ClearHooks { dispatcher },
            op::READ_FORGE => Self::ReadForge { dispatcher },
            op::READ_FIELDS => {
                let expected_manager = body.u64()?;
                if expected_manager == 0 {
                    return Err(ProtocolError::InvalidData);
                }
                Self::ReadFields {
                    dispatcher,
                    expected_manager,
                    set: crate::field_edits::FieldSet::decode(body.byte()?)?,
                }
            }
            op::WRITE_FIELDS => {
                let length = body.u32()? as usize;
                let expected = crate::field_edits::Snapshot::decode(body.take(length)?)?;
                let count = body.u32()? as usize;
                if count != expected.fields.len() {
                    return Err(ProtocolError::InvalidData);
                }
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(if body.flag()? { Some(i32::from_le_bytes(body.array()?)) } else { None });
                }
                let edit = crate::field_edits::Edit { expected, values };
                edit.validate()?;
                Self::WriteFields { dispatcher, edit: Box::new(edit) }
            }
            op::READ_LOCALE => Self::ReadLocale { dispatcher },
            op::DESCRIBE_UNIQUE | op::DESCRIBE_IMPLICITS => {
                let manager = body.u64()?;
                let query = ForgeDescriptionRequest::from_checked(dispatcher, manager, body.snapshot()?)?;
                if operation == op::DESCRIBE_UNIQUE {
                    Self::DescribeUnique(query)
                } else {
                    Self::DescribeImplicits(query)
                }
            }
            op::PREVIEW_UNIQUE => {
                let manager = body.u64()?;
                let count = body.byte()?;
                if !(1..=8).contains(&count) {
                    return Err(ProtocolError::InvalidData);
                }
                let mut changes = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    changes.push(UniqueRollChange {
                        index: body.byte()?,
                        value: body.byte()?,
                    });
                }
                Self::PreviewUnique(UniquePreviewRequest::from_target(
                    ForgeDescriptionRequest::from_checked(dispatcher, manager, body.snapshot()?)?,
                    changes,
                )?)
            }
            op::MAP_ROLL => {
                let manager = body.u64()?;
                let slot = body.byte()?;
                let start = if body.flag()? { MappingStart::Samples(body.byte()?) } else { MappingStart::ContextOnly };
                let draft = if body.flag()? {
                    Some(AffixMappingDraft {
                        id: body.u16()?,
                        tier: body.byte()?,
                        roll: body.byte()?,
                        seal: body.byte()?,
                    })
                } else {
                    None
                };
                Self::MapRoll(AffixMappingQuery::from_target(
                    ForgeDescriptionRequest::from_checked(dispatcher, manager, body.snapshot()?)?,
                    slot,
                    start,
                    draft,
                )?)
            }
            op::MAP_MODIFIER_ROLL => {
                let manager = body.u64()?;
                let target = MappingTarget::try_from(i32::from(body.byte()?))?;
                let index = body.byte()?;
                let line = if body.flag()? { Some(body.u32()?) } else { None };
                let start = if body.flag()? { MappingStart::Samples(body.byte()?) } else { MappingStart::ContextOnly };
                let count = body.byte()?;
                if count > 7 {
                    return Err(ProtocolError::InvalidData);
                }
                let mut draft = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    draft.push(UniqueRollChange {
                        index: body.byte()?,
                        value: body.byte()?,
                    });
                }
                Self::MapModifierRoll(ModifierMappingQuery::from_target(
                    ForgeDescriptionRequest::from_checked(dispatcher, manager, body.snapshot()?)?,
                    MappingAddress { target, index, line },
                    start,
                    draft,
                )?)
            }
            _ => {
                let manager = body.u64()?;
                let mutation = match operation {
                    op::ADD_AFFIX => ForgeMutation::AddAffix {
                        id: body.u16()?,
                        tier: body.byte()?,
                        roll: body.byte()?,
                        seal: body.byte()?,
                        unrestricted: body.flag()?,
                    },
                    op::REMOVE_AFFIX => ForgeMutation::RemoveAffix {
                        slot: body.byte()?,
                        id: body.u16()?,
                        restore_potential: if body.flag()? { Some(body.byte()?) } else { None },
                    },
                    op::CHANGE_AFFIX => ForgeMutation::Affix {
                        slot: body.byte()?,
                        id: body.u16()?,
                        tier: body.byte()?,
                        roll: body.byte()?,
                        seal: body.byte()?,
                        unrestricted: body.flag()?,
                    },
                    op::CHANGE_AFFIX_ROLLS => {
                        let unrestricted = body.flag()?;
                        ForgeMutation::AffixRolls {
                            rolls: roll_changes(&mut body)?,
                            unrestricted,
                        }
                    }
                    op::CHANGE_MODIFIER_ROLLS => {
                        let array = RollArray::from_wire(body.byte()?)?;
                        let unrestricted = body.flag()?;
                        ForgeMutation::ModifierRolls {
                            array,
                            rolls: roll_changes(&mut body)?,
                            unrestricted,
                        }
                    }
                    op::SET_STATUS => ForgeMutation::Status {
                        field: StatusField::from_wire(u32::from(body.byte()?))?,
                        value: body.byte()?,
                        unrestricted: body.flag()?,
                    },
                    op::CHANGE_BASE => ForgeMutation::Base {
                        item_type: body.byte()?,
                        sub_type: body.u16()?,
                        unrestricted: body.flag()?,
                    },
                    op::CHANGE_IDENTITY => ForgeMutation::Identity {
                        id: body.u16()?,
                        sub_type: body.byte()?,
                        rarity: body.byte()?,
                        unrestricted: body.flag()?,
                    },
                    _ => return Err(ProtocolError::InvalidData),
                };
                Self::EditForge(ForgeWrite::from_checked(dispatcher, manager, body.snapshot()?, mutation)?)
            }
        };
        body.finish()?;
        Ok(request)
    }
}

fn new_affix(body: &mut Reader<'_>) -> Result<NewAffix, ProtocolError> {
    let id = body.u16()?;
    let mut text = || -> Result<String, ProtocolError> {
        let length = body.u32()? as usize;
        if length > MAX_TEXT {
            return Err(ProtocolError::InvalidData);
        }
        std::str::from_utf8(body.take(length)?).map(str::to_owned).map_err(|_| ProtocolError::InvalidData)
    };
    let (name, display_name) = (text()?, text()?);
    let standard_effect = f32::from_bits(body.u32()?);
    let count = usize::from(body.byte()?);
    if !(1..=MAX_PROPERTIES).contains(&count) {
        return Err(ProtocolError::InvalidData);
    }
    let mut properties = Vec::with_capacity(count);
    for _ in 0..count {
        properties.push(StatLine {
            property: body.byte()?,
            special_tag: body.byte()?,
            tags: body.u32()? as i32,
            extra_tag: body.u32()? as i32,
            modifier_type: body.u32()? as i32,
            minimum: f32::from_bits(body.u32()?),
            maximum: f32::from_bits(body.u32()?),
        });
    }
    let affix = NewAffix {
        id,
        name,
        display_name,
        standard_effect,
        properties,
    };
    affix.validate()?;
    Ok(affix)
}

/// A roll batch: its count, then each index and position.
fn roll_changes(body: &mut Reader<'_>) -> Result<AffixRolls, ProtocolError> {
    let count = body.byte()?;
    if !(1..=32).contains(&count) {
        return Err(ProtocolError::InvalidData);
    }
    let mut changes = Vec::with_capacity(count as usize);
    for _ in 0..count {
        changes.push(AffixRollChange {
            slot: body.byte()?,
            value: body.byte()?,
        });
    }
    AffixRolls::new(&changes)
}
