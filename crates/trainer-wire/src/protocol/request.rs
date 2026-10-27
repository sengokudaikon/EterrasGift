//! Request bodies are sequential fields. Only the common envelope has fixed positions.
use super::*;
use crate::{
    fields::{AttackLeechObservation, AttackLeechTarget, ObservedObjectIdentity, ObservedRate},
    mapping::{AffixMappingDraft, AffixMappingQuery, ForgeDescriptionRequest, MappingStart, UniquePreviewRequest, UniqueRollChange},
    write::{AffixRollChange, AffixRolls, StatusField},
};

const MAX_BODY: usize = MAX_EXPECTED_BYTES + 256;

#[derive(Default)]
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
        let mut body = Writer::default();
        if let Some(dispatcher) = self.dispatcher() {
            body.u64(dispatcher.rva);
            body.0.extend_from_slice(&dispatcher.bytes);
        }
        let operation = match self {
            Self::Discover => 6,
            Self::Unload => 25,
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
                body.byte(*value);
                26
            }
            Self::ReadGameplayHook { expected_manager, hook, .. } => {
                body.u64(*expected_manager);
                body.byte(*hook as u8);
                27
            }
            Self::Status { .. } => 7,
            Self::ClearHooks { .. } => 15,
            Self::ReadForge { .. } => 18,
            // 19 (item limits) and 21 (catalog pages) are retired: the definitions come from the game's files.
            Self::ReadLocale { .. } => 20,
            Self::ReadAttackLeech { expected_manager, .. } => {
                body.u64(*expected_manager);
                23
            }
            Self::DescribeImplicits(query) | Self::DescribeUnique(query) => {
                body.u64(query.manager());
                body.snapshot(&query.expected);
                if matches!(self, Self::DescribeUnique(_)) {
                    4
                } else {
                    ForgeDescriptionRequest::OPERATION
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
                UniquePreviewRequest::OPERATION
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
                AffixMappingQuery::OPERATION
            }
            Self::SetAttackLeechRate(write) => {
                body.byte(match write.target() {
                    AttackLeechTarget::AttackRate => 1,
                    AttackLeechTarget::IncreasedLeechRate => 2,
                });
                body.u32(write.desired_bits());
                let expected = write.expected();
                for object in [expected.manager, expected.actor, expected.stats] {
                    body.u64(object.wrapper_token());
                    body.u64(object.native_token());
                }
                body.u32(expected.attack_rate.raw_bits);
                body.u32(expected.increased_leech_rate.raw_bits);
                SET_ATTACK_LEECH_RATE_OPERATION
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
                        ADD_AFFIX_OPERATION
                    }
                    ForgeMutation::RemoveAffix { slot, id } => {
                        body.byte(slot);
                        body.u16(id);
                        REMOVE_AFFIX_OPERATION
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
                        CHANGE_AFFIX_OPERATION
                    }
                    ForgeMutation::AffixRolls { rolls, unrestricted } => {
                        body.flag(unrestricted);
                        body.byte(rolls.changes().len() as u8);
                        for change in rolls.changes() {
                            body.byte(change.slot);
                            body.byte(change.value);
                        }
                        CHANGE_AFFIX_ROLLS_OPERATION
                    }
                    ForgeMutation::Status { field, value, unrestricted } => {
                        body.byte(field as u8);
                        body.byte(value);
                        body.flag(unrestricted);
                        SET_STATUS_OPERATION
                    }
                    ForgeMutation::Base {
                        item_type,
                        sub_type,
                        unrestricted,
                    } => {
                        body.byte(item_type);
                        body.u16(sub_type);
                        body.flag(unrestricted);
                        CHANGE_BASE_OPERATION
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
                        CHANGE_IDENTITY_OPERATION
                    }
                };
                body.blob(write.expected_bytes());
                operation
            }
        };
        let mut frame = Writer::default();
        frame.u32(MAGIC);
        frame.u32(operation);
        frame.u32(body.0.len() as u32);
        frame.0.extend(body.0);
        frame.0
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
        if !matches!(
            operation,
            1 | 2 | 4 | 5 | 6 | 7 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 17 | 18 | 19 | 20 | 21 | 23 | 24 | 25 | 26 | 27
        ) || length > MAX_BODY
            || (matches!(operation, 6 | 25) && length != 0)
            || (!matches!(operation, 6 | 25) && length < 24)
        {
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
        if matches!(operation, 6 | 25) {
            body.finish()?;
            return Ok(if operation == 6 { Self::Discover } else { Self::Unload });
        }
        let dispatcher = DispatcherSpec {
            rva: body.u64()?,
            bytes: body.array()?,
        };
        let request = match operation {
            26 | 27 => {
                let expected_manager = body.u64()?;
                if expected_manager == 0 {
                    return Err(ProtocolError::InvalidData);
                }
                let hook = crate::hooks::GameplayHook::decode(body.byte()?)?;
                if operation == 26 {
                    Self::SetGameplayHook {
                        dispatcher,
                        expected_manager,
                        hook,
                        on: body.flag()?,
                        value: hook.validate_value(body.byte()?)?,
                    }
                } else {
                    Self::ReadGameplayHook {
                        dispatcher,
                        expected_manager,
                        hook,
                    }
                }
            }
            7 => Self::Status { dispatcher },
            15 => Self::ClearHooks { dispatcher },
            18 => Self::ReadForge { dispatcher },
            20 => Self::ReadLocale { dispatcher },
            23 => {
                let expected_manager = body.u64()?;
                if expected_manager == 0 {
                    return Err(ProtocolError::InvalidData);
                }
                Self::ReadAttackLeech { dispatcher, expected_manager }
            }
            4 | 12 => {
                let manager = body.u64()?;
                let query = ForgeDescriptionRequest::from_checked(dispatcher, manager, body.snapshot()?)?;
                if operation == 4 { Self::DescribeUnique(query) } else { Self::DescribeImplicits(query) }
            }
            5 => {
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
            9 => {
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
            24 => {
                let target = match body.byte()? {
                    1 => AttackLeechTarget::AttackRate,
                    2 => AttackLeechTarget::IncreasedLeechRate,
                    _ => return Err(ProtocolError::InvalidData),
                };
                let desired = body.u32()?;
                let mut object = || ObservedObjectIdentity::new(body.u64()?, body.u64()?);
                let expected = AttackLeechObservation {
                    manager: object()?,
                    actor: object()?,
                    stats: object()?,
                    attack_rate: ObservedRate { raw_bits: body.u32()? },
                    increased_leech_rate: ObservedRate { raw_bits: body.u32()? },
                };
                Self::SetAttackLeechRate(AttackLeechRateWrite::new(dispatcher, expected, target, desired)?)
            }
            _ => {
                let manager = body.u64()?;
                let mutation = match operation {
                    ADD_AFFIX_OPERATION => ForgeMutation::AddAffix {
                        id: body.u16()?,
                        tier: body.byte()?,
                        roll: body.byte()?,
                        seal: body.byte()?,
                        unrestricted: body.flag()?,
                    },
                    REMOVE_AFFIX_OPERATION => ForgeMutation::RemoveAffix {
                        slot: body.byte()?,
                        id: body.u16()?,
                    },
                    CHANGE_AFFIX_OPERATION => ForgeMutation::Affix {
                        slot: body.byte()?,
                        id: body.u16()?,
                        tier: body.byte()?,
                        roll: body.byte()?,
                        seal: body.byte()?,
                        unrestricted: body.flag()?,
                    },
                    CHANGE_AFFIX_ROLLS_OPERATION => {
                        let unrestricted = body.flag()?;
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
                        ForgeMutation::AffixRolls {
                            rolls: AffixRolls::new(&changes)?,
                            unrestricted,
                        }
                    }
                    SET_STATUS_OPERATION => ForgeMutation::Status {
                        field: StatusField::from_wire(u32::from(body.byte()?))?,
                        value: body.byte()?,
                        unrestricted: body.flag()?,
                    },
                    CHANGE_BASE_OPERATION => ForgeMutation::Base {
                        item_type: body.byte()?,
                        sub_type: body.u16()?,
                        unrestricted: body.flag()?,
                    },
                    CHANGE_IDENTITY_OPERATION => ForgeMutation::Identity {
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
