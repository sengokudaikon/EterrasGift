//! The request envelope: the header (magic, operation, body length), then the dispatcher every body but Discover's and
//! Unload's starts with, then the fields of the payload it carries, in order. A payload with a type of its own writes
//! and reads itself beside that type ([`Body`], [`Wire`]); the hook, field and affix requests' few fields are here.
//!
//! A new request is a row in `operations!` (its number and body sizes), its `Request` variant with its arm in
//! `Request::dispatcher`, and its arms in `Request::encode` and `Request::decode_body`.
use super::*;
use crate::{
    codec::{Reader, Wire, Writer, invalid},
    hooks::{GameplayHook, HookValue},
    mapping::{AffixMappingQuery, ForgeDescriptionRequest, ModifierMappingQuery, UniquePreviewRequest},
    new_affixes::MAX_PER_REQUEST,
};

/// A request's payload after the envelope's dispatcher, which the payload keeps.
pub(crate) trait Body: Sized {
    fn write(&self, w: &mut Writer);
    fn read(dispatcher: DispatcherSpec, r: &mut Reader<'_>) -> Result<Self, ProtocolError>;
}

impl Wire for DispatcherSpec {
    fn write(&self, w: &mut Writer) {
        w.u64(self.rva);
        w.raw(&self.bytes);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, ProtocolError> {
        Ok(Self { rva: r.u64()?, bytes: r.array()? })
    }
}

/// The dispatcher's rva and 16 bytes.
const DISPATCHER: usize = 8 + 16;
/// A conditional item request: it carries its expected item, whose size varies.
const ITEM: Sizes = Sizes::Within(DISPATCHER, MAX_EXPECTED_BYTES + 256);

/// Each operation: its number, by name in [`op`], and the sizes its body may have, dispatcher included.
macro_rules! operations {
    ($($name:ident = $number:literal: $sizes:expr,)+) => {
        /// Every operation number the pipe carries, by name. A number never returns once retired.
        pub mod op {
            $(pub const $name: u32 = $number;)+
        }
        /// The sizes an operation's body may have; `None` for a number that is no operation.
        fn body_sizes(operation: u32) -> Option<Sizes> {
            match operation {
                $($number => Some($sizes),)+
                _ => None,
            }
        }
    };
}

operations! {
    CHANGE_AFFIX = 1: ITEM,
    CHANGE_AFFIX_ROLLS = 2: ITEM,
    DESCRIBE_UNIQUE = 4: ITEM,
    PREVIEW_UNIQUE = 5: ITEM,
    DISCOVER = 6: Sizes::Exactly(0),
    STATUS = 7: Sizes::Exactly(DISPATCHER),
    MAP_ROLL = 9: ITEM,
    SET_STATUS = 10: ITEM,
    CHANGE_BASE = 11: ITEM,
    DESCRIBE_IMPLICITS = 12: ITEM,
    ADD_AFFIX = 13: ITEM,
    REMOVE_AFFIX = 14: ITEM,
    CLEAR_HOOKS = 15: Sizes::Exactly(DISPATCHER),
    CHANGE_IDENTITY = 17: ITEM,
    READ_FORGE = 18: Sizes::Exactly(DISPATCHER),
    READ_LOCALE = 20: Sizes::Exactly(DISPATCHER),
    UNLOAD = 25: Sizes::Exactly(0),
    // The manager, the hook, whether it is on, and its value: eight bytes, or sixteen for a wide one.
    SET_GAMEPLAY_HOOK = 26: Sizes::OneOf(&[DISPATCHER + 8 + 1 + 1 + 8, DISPATCHER + 8 + 1 + 1 + 16]),
    READ_GAMEPLAY_HOOK = 27: Sizes::Exactly(DISPATCHER + 8 + 1),
    CHANGE_MODIFIER_ROLLS = 28: ITEM,
    MAP_MODIFIER_ROLL = 29: ITEM,
    REGISTER_AFFIXES = 30: ITEM,
    READ_FIELDS = 31: Sizes::Exactly(DISPATCHER + 8 + 1),
    WRITE_FIELDS = 32: Sizes::Within(DISPATCHER, crate::field_edits::MAX_BYTES + 32 * 1024),
    // The manager, the count, then one to every hook.
    READ_GAMEPLAY_HOOKS = 33: Sizes::Within(DISPATCHER + 8 + 1 + 1, DISPATCHER + 8 + 1 + GameplayHook::COUNT),
}

enum Sizes {
    Exactly(usize),
    OneOf(&'static [usize]),
    Within(usize, usize),
}
impl Sizes {
    fn admit(&self, length: usize) -> bool {
        match *self {
            Self::Exactly(size) => length == size,
            Self::OneOf(sizes) => sizes.contains(&length),
            Self::Within(minimum, maximum) => (minimum..=maximum).contains(&length),
        }
    }
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer(Vec::with_capacity(REQUEST_HEADER_SIZE + 64));
        w.u32(MAGIC);
        // The operation and length are filled in once the body is written.
        w.u32(0);
        w.u32(0);
        if let Some(dispatcher) = self.dispatcher() {
            w.put(&dispatcher);
        }
        let operation = match self {
            Self::Discover => op::DISCOVER,
            Self::Unload => op::UNLOAD,
            Self::Status { .. } => op::STATUS,
            Self::ClearHooks { .. } => op::CLEAR_HOOKS,
            Self::ReadForge { .. } => op::READ_FORGE,
            Self::ReadLocale { .. } => op::READ_LOCALE,
            Self::SetGameplayHook {
                expected_manager,
                hook,
                on,
                value,
                ..
            } => {
                w.u64(*expected_manager);
                w.byte(*hook as u8);
                w.flag(*on);
                w.u64(*value as u64);
                // A value wider than its hook takes is written whole, and refused when read.
                if hook.value_kind().wide() || *value > u64::MAX as HookValue {
                    w.u64((*value >> 64) as u64);
                }
                op::SET_GAMEPLAY_HOOK
            }
            Self::ReadGameplayHook { expected_manager, hook, .. } => {
                w.u64(*expected_manager);
                w.byte(*hook as u8);
                op::READ_GAMEPLAY_HOOK
            }
            Self::ReadGameplayHooks { expected_manager, hooks, .. } => {
                w.u64(*expected_manager);
                w.byte(hooks.len() as u8);
                for hook in hooks {
                    w.byte(*hook as u8);
                }
                op::READ_GAMEPLAY_HOOKS
            }
            Self::RegisterAffixes { expected_manager, affixes, .. } => {
                w.u64(*expected_manager);
                w.byte(affixes.len() as u8);
                for affix in affixes {
                    w.put(affix);
                }
                op::REGISTER_AFFIXES
            }
            Self::ReadFields { expected_manager, set, .. } => {
                w.u64(*expected_manager);
                w.byte(*set as u8);
                op::READ_FIELDS
            }
            Self::WriteFields { edit, .. } => {
                w.put(edit.as_ref());
                op::WRITE_FIELDS
            }
            Self::DescribeImplicits(query) => {
                query.write(&mut w);
                op::DESCRIBE_IMPLICITS
            }
            Self::DescribeUnique(query) => {
                query.write(&mut w);
                op::DESCRIBE_UNIQUE
            }
            Self::PreviewUnique(query) => {
                query.write(&mut w);
                op::PREVIEW_UNIQUE
            }
            Self::MapRoll(query) => {
                query.write(&mut w);
                op::MAP_ROLL
            }
            Self::MapModifierRoll(query) => {
                query.write(&mut w);
                op::MAP_MODIFIER_ROLL
            }
            Self::EditForge(write) => {
                write.write(&mut w);
                write.mutation().operation()
            }
        };
        let length = (w.0.len() - REQUEST_HEADER_SIZE) as u32;
        w.0[4..8].copy_from_slice(&operation.to_le_bytes());
        w.0[8..12].copy_from_slice(&length.to_le_bytes());
        w.0
    }

    pub fn decode_header(bytes: &[u8]) -> Result<RequestHeader, ProtocolError> {
        if bytes.len() != REQUEST_HEADER_SIZE {
            return invalid();
        }
        let mut r = Reader::new(bytes);
        if r.u32()? != MAGIC {
            return Err(ProtocolError::InvalidMagic);
        }
        let operation = r.u32()?;
        let length = r.u32()? as usize;
        if !body_sizes(operation).is_some_and(|sizes| sizes.admit(length)) {
            return invalid();
        }
        Ok(RequestHeader { operation, body_len: length })
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let header = Self::decode_header(bytes.get(..REQUEST_HEADER_SIZE).ok_or(ProtocolError::InvalidData)?)?;
        if bytes.len() != header.total_len() {
            return invalid();
        }
        Self::decode_body(header.operation, &bytes[REQUEST_HEADER_SIZE..])
    }

    pub(super) fn decode_body(operation: u32, bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut r = Reader::new(bytes);
        let request = match operation {
            op::DISCOVER => Self::Discover,
            op::UNLOAD => Self::Unload,
            _ => {
                let dispatcher = r.get::<DispatcherSpec>()?;
                match operation {
                    op::STATUS => Self::Status { dispatcher },
                    op::CLEAR_HOOKS => Self::ClearHooks { dispatcher },
                    op::READ_FORGE => Self::ReadForge { dispatcher },
                    op::READ_LOCALE => Self::ReadLocale { dispatcher },
                    op::SET_GAMEPLAY_HOOK => {
                        let expected_manager = manager(&mut r)?;
                        let hook = GameplayHook::decode(r.byte()?)?;
                        let on = r.flag()?;
                        let mut value = HookValue::from(r.u64()?);
                        if hook.value_kind().wide() {
                            value |= HookValue::from(r.u64()?) << 64;
                        }
                        Self::SetGameplayHook {
                            dispatcher,
                            expected_manager,
                            hook,
                            on,
                            value: hook.validate_value(value)?,
                        }
                    }
                    op::READ_GAMEPLAY_HOOK => Self::ReadGameplayHook {
                        dispatcher,
                        expected_manager: manager(&mut r)?,
                        hook: GameplayHook::decode(r.byte()?)?,
                    },
                    op::READ_GAMEPLAY_HOOKS => {
                        let expected_manager = manager(&mut r)?;
                        let count = r.count(GameplayHook::COUNT)?;
                        if count == 0 {
                            return invalid();
                        }
                        let mut hooks = Vec::with_capacity(count);
                        for _ in 0..count {
                            let hook = GameplayHook::decode(r.byte()?)?;
                            // Ascending, so no hook is read twice in one turn.
                            if hooks.last().is_some_and(|last| *last as u8 >= hook as u8) {
                                return invalid();
                            }
                            hooks.push(hook);
                        }
                        Self::ReadGameplayHooks {
                            dispatcher,
                            expected_manager,
                            hooks,
                        }
                    }
                    op::REGISTER_AFFIXES => {
                        let expected_manager = manager(&mut r)?;
                        let count = r.count(MAX_PER_REQUEST)?;
                        if count == 0 {
                            return invalid();
                        }
                        Self::RegisterAffixes {
                            dispatcher,
                            expected_manager,
                            affixes: (0..count).map(|_| r.get()).collect::<Result<_, _>>()?,
                        }
                    }
                    op::READ_FIELDS => Self::ReadFields {
                        dispatcher,
                        expected_manager: manager(&mut r)?,
                        set: crate::field_edits::FieldSet::decode(r.byte()?)?,
                    },
                    op::WRITE_FIELDS => Self::WriteFields {
                        dispatcher,
                        edit: Box::new(r.get()?),
                    },
                    op::DESCRIBE_IMPLICITS => Self::DescribeImplicits(ForgeDescriptionRequest::read(dispatcher, &mut r)?),
                    op::DESCRIBE_UNIQUE => Self::DescribeUnique(ForgeDescriptionRequest::read(dispatcher, &mut r)?),
                    op::PREVIEW_UNIQUE => Self::PreviewUnique(UniquePreviewRequest::read(dispatcher, &mut r)?),
                    op::MAP_ROLL => Self::MapRoll(AffixMappingQuery::read(dispatcher, &mut r)?),
                    op::MAP_MODIFIER_ROLL => Self::MapModifierRoll(ModifierMappingQuery::read(dispatcher, &mut r)?),
                    _ => Self::EditForge(ForgeWrite::read(dispatcher, operation, &mut r)?),
                }
            }
        };
        r.finish()?;
        Ok(request)
    }
}

/// The character manager a request is for: never none.
fn manager(r: &mut Reader<'_>) -> Result<u64, ProtocolError> {
    match r.u64()? {
        0 => invalid(),
        manager => Ok(manager),
    }
}
