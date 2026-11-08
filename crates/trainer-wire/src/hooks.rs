//! Concrete supported gameplay effects; arbitrary signatures never cross the pipe.
use crate::{
    codec::{Reader, Writer, wire_enum},
    protocol::ProtocolError,
};

/// A bounded settings snapshot: existing scalar/pair encodings use the low word; up to four game floats use both.
pub type HookValue = u128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum GameplayHook {
    MysteriousRift = 0,
    AnyOffhand = 1,
    IncomingDamage = 2,
    EnemyDamage = 3,
    FixedAttackRate = 4,
    FixedLeechRate = 5,
    NoManaCost = 6,
    SkillsToLevel20 = 7,
    EquipWithoutRequirements = 8,
    FullSetBonus = 9,
    UnlimitedSkillPoints = 10,
    UnlimitedPassivePoints = 11,
    AnyMasteryPassives = 12,
    UnlimitedWeaverPoints = 13,
    ForgeAboveLevel = 14,
    UnlimitedWovenEnchanter = 15,
    PickupAnywhere = 16,
    FreeHunterVendor = 17,
    EndgameCache = 18,
    FavorCache = 19,
    Nemesis = 20,
    OmenWindows = 21,
    ClassExperimentals = 22,
    RunePrisonComplex = 23,
    ShardsTo99 = 24,
    RunesGlyphsTo99 = 25,
    MaxRandomRolls = 26,
    BossLootChance = 27,
    Tier7Affixes = 28,
    MaxBonusStability = 29,
    AbilitySpeed = 30,
    OneEchoType = 31,
    ExperienceMultiplier = 32,
    LevelUpPerGain = 33,
    FactionGains = 34,
    RankUpPerGain = 35,
    SeveralPrimordials = 36,
    FixedArea = 37,
    RefillMana = 38,
    MovementSpeed = 39,
    AnyWaypoint = 40,
    ExperimentalUniques = 41,
    EternityCacheAnywhere = 42,
    LootLizards = 43,
    RuneWardens = 44,
    Champions = 45,
    RevealMap = 46,
    RevealObjective = 47,
    ProphecyAnyDistance = 48,
    NewItemRarity = 49,
    DropRates = 50,
    ExtraRare = 51,
    DuplicateOnDrop = 52,
    ProphecyUses = 53,
    SkipQuestEchoes = 54,
    MorditasTimer = 55,
    CameraDistance = 56,
    PackCount = 57,
    PackSize = 58,
    MonsterRarity = 59,
}

/// What a hook's value is: how the desktop's number for it is sent, which values the helper takes, and the one it
/// rests at while off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValueKind {
    /// On or off; its value is always 1.
    Switch,
    /// A whole percentage from 0 to 100; 100 changes nothing.
    Percent,
    /// An `f64` (sent as its bits) from `min` to `max`, or exactly `infinity`, a number past `max` that means "without
    /// limit"; `neutral` changes nothing.
    Real { min: f64, max: f64, infinity: Option<f64>, neutral: f64 },
    /// Two finite single-precision numbers, low word first, published together in one aligned 64-bit store.
    RealPair { min: f64, max: f64, neutral: f32 },
    /// Up to four single-precision values, each with its own bounds and neutral value, low lane first.
    Reals(&'static [(f64, f64, f32)]),
    /// One of the desktop's choices, by index, each sent as its value here.
    Choice(&'static [u64]),
}

impl ValueKind {
    /// The value it rests at while off.
    pub const fn neutral(self) -> HookValue {
        match self {
            Self::Switch => 1,
            Self::Percent => 100,
            Self::Real { neutral, .. } => neutral.to_bits() as HookValue,
            Self::RealPair { neutral, .. } => neutral.to_bits() as HookValue | ((neutral.to_bits() as HookValue) << 32),
            Self::Reals(lanes) => {
                assert!(!lanes.is_empty() && lanes.len() <= 4);
                let mut value = 0;
                let mut index = 0;
                while index < lanes.len() {
                    value |= (lanes[index].2.to_bits() as HookValue) << (32 * index);
                    index += 1;
                }
                value
            }
            Self::Choice(values) => values[0] as HookValue,
        }
    }

    pub fn accepts(self, value: HookValue) -> bool {
        if !self.wide() && value > u64::MAX as HookValue {
            return false;
        }
        match self {
            Self::Switch => value == 1,
            Self::Percent => value <= 100,
            Self::Real { .. } => self.takes(f64::from_bits(value as u64)),
            Self::RealPair { .. } => [value as u32, (value >> 32) as u32].into_iter().all(|bits| self.takes(f64::from(f32::from_bits(bits)))),
            Self::Reals(lanes) => {
                (1..=4).contains(&lanes.len())
                    && lanes
                        .iter()
                        .enumerate()
                        .all(|(index, _)| self.takes_at(index, f64::from(f32::from_bits((value >> (32 * index)) as u32))))
                    && (lanes.len() == 4 || value >> (32 * lanes.len()) == 0)
            }
            Self::Choice(values) => values.contains(&(value as u64)),
        }
    }

    /// Whether the desktop's number is one this value takes.
    pub fn takes(self, number: f64) -> bool {
        self.takes_at(0, number)
    }

    pub fn wide(self) -> bool {
        matches!(self, Self::Reals(_))
    }

    pub fn takes_at(self, index: usize, number: f64) -> bool {
        number.is_finite()
            && match self {
                Self::Switch => false,
                Self::Percent => number.fract() == 0.0 && (0.0..=100.0).contains(&number),
                Self::Real { min, max, infinity, .. } => (min..=max).contains(&number) || Some(number) == infinity,
                Self::RealPair { min, max, .. } => (min..=max).contains(&number),
                Self::Reals(lanes) => lanes.get(index).is_some_and(|(min, max, _)| (*min..=*max).contains(&number)),
                Self::Choice(values) => number.fract() == 0.0 && number >= 0.0 && number < values.len() as f64,
            }
    }

    /// A saved number as this value takes it now: rounded and within its range, or `None` when nothing fits.
    pub fn fit(self, number: f64) -> Option<f64> {
        self.fit_at(0, number)
    }

    pub fn fit_at(self, index: usize, number: f64) -> Option<f64> {
        let fitted = match self {
            Self::Percent => number.round().clamp(0.0, 100.0),
            Self::Real { min, max, infinity, .. } if Some(number) != infinity => number.clamp(min, max),
            Self::RealPair { min, max, .. } => number.clamp(min, max),
            Self::Reals(lanes) => {
                let (min, max, _) = lanes.get(index)?;
                number.clamp(*min, *max)
            }
            _ => number,
        };
        self.takes_at(index, fitted).then_some(fitted)
    }

    /// Encode the desktop's numbers, preserving each game-float lane for multi-value settings.
    pub fn encode(self, numbers: &[f64]) -> Option<HookValue> {
        match (self, numbers) {
            (Self::Switch, []) => Some(1),
            (Self::Percent, [number]) if self.takes(*number) => Some(*number as HookValue),
            (Self::Real { .. }, [number]) if self.takes(*number) => Some(number.to_bits().into()),
            (Self::RealPair { .. }, [first, second]) if self.takes(*first) && self.takes(*second) => {
                let packed = HookValue::from((*first as f32).to_bits()) | (HookValue::from((*second as f32).to_bits()) << 32);
                self.accepts(packed).then_some(packed)
            }
            (Self::Reals(lanes), numbers)
                if (1..=4).contains(&lanes.len()) && numbers.len() == lanes.len() && numbers.iter().enumerate().all(|(index, value)| self.takes_at(index, *value)) =>
            {
                let packed = numbers
                    .iter()
                    .enumerate()
                    .fold(0, |packed, (index, value)| packed | (HookValue::from((*value as f32).to_bits()) << (32 * index)));
                self.accepts(packed).then_some(packed)
            }
            (Self::Choice(values), [number]) if self.takes(*number) => Some(values[*number as usize].into()),
            _ => None,
        }
    }
}

/// A hook as the pipe names it: its id (the desktop's feature id) and its value.
pub struct HookSpec {
    pub hook: GameplayHook,
    pub id: &'static str,
    pub value: ValueKind,
}

const fn spec(hook: GameplayHook, id: &'static str, value: ValueKind) -> HookSpec {
    HookSpec { hook, id, value }
}

const fn switch(hook: GameplayHook, id: &'static str) -> HookSpec {
    spec(hook, id, ValueKind::Switch)
}

const fn real(hook: GameplayHook, id: &'static str, min: f64, max: f64, neutral: f64) -> HookSpec {
    spec(
        hook,
        id,
        ValueKind::Real {
            min,
            max,
            infinity: None,
            neutral,
        },
    )
}

/// Every hook, in the enum's order: a hook's place is its number on the pipe, so a new one goes last.
pub const SPECS: [HookSpec; GameplayHook::COUNT] = {
    use GameplayHook::*;
    [
        // NextEncounterShouldBeEggs's result for "Game decides" (off), Portal and Boss.
        spec(MysteriousRift, "mysterious-rift", ValueKind::Choice(&[1, 1, 0])),
        switch(AnyOffhand, "any-offhand"),
        spec(IncomingDamage, "incoming-damage", ValueKind::Percent),
        // 101 is one-hit kills.
        spec(
            EnemyDamage,
            "enemy-damage",
            ValueKind::Real {
                min: 0.0,
                max: 100.0,
                infinity: Some(101.0),
                neutral: 1.0,
            },
        ),
        real(FixedAttackRate, "fixed-attack-rate", 0.01, 20.0, 1.0),
        real(FixedLeechRate, "fixed-leech-rate", -1.0, 100.0, 0.0),
        switch(NoManaCost, "no-mana-cost"),
        switch(SkillsToLevel20, "skills-to-level-20"),
        switch(EquipWithoutRequirements, "equip-without-requirements"),
        switch(FullSetBonus, "full-set-bonus"),
        switch(UnlimitedSkillPoints, "unlimited-skill-points"),
        switch(UnlimitedPassivePoints, "unlimited-passive-points"),
        switch(AnyMasteryPassives, "any-mastery-passives"),
        switch(UnlimitedWeaverPoints, "unlimited-weaver-points"),
        switch(ForgeAboveLevel, "forge-above-level"),
        switch(UnlimitedWovenEnchanter, "unlimited-woven-enchanter"),
        switch(PickupAnywhere, "pickup-anywhere"),
        switch(FreeHunterVendor, "free-hunter-vendor"),
        switch(EndgameCache, "endgame-cache"),
        switch(FavorCache, "favor-cache"),
        switch(Nemesis, "nemesis"),
        switch(OmenWindows, "omen-windows"),
        switch(ClassExperimentals, "class-experimentals"),
        switch(RunePrisonComplex, "rune-prison-complex"),
        switch(ShardsTo99, "shards-to-99"),
        switch(RunesGlyphsTo99, "runes-glyphs-to-99"),
        switch(MaxRandomRolls, "max-random-rolls"),
        switch(BossLootChance, "boss-loot-chance"),
        switch(Tier7Affixes, "tier-7-affixes"),
        switch(MaxBonusStability, "max-bonus-stability"),
        real(AbilitySpeed, "ability-speed", 0.0, 100.0, 1.0),
        spec(OneEchoType, "one-echo-type", ValueKind::Choice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9])),
        spec(
            ExperienceMultiplier,
            "experience-multiplier",
            ValueKind::RealPair {
                min: 1.0,
                max: 100.0,
                neutral: 1.0,
            },
        ),
        switch(LevelUpPerGain, "level-up-per-gain"),
        real(FactionGains, "faction-gains", 0.0, 100.0, 1.0),
        switch(RankUpPerGain, "rank-up-per-gain"),
        switch(SeveralPrimordials, "several-primordials"),
        real(FixedArea, "fixed-area", 0.0, 50.0, 0.0),
        switch(RefillMana, "refill-mana"),
        real(MovementSpeed, "movement-speed", 0.0, 100.0, 1.0),
        switch(AnyWaypoint, "any-waypoint"),
        switch(ExperimentalUniques, "experimental-uniques"),
        switch(EternityCacheAnywhere, "eternity-cache-anywhere"),
        real(LootLizards, "loot-lizards", 0.0, 20.0, 1.0),
        real(RuneWardens, "rune-wardens", 0.0, 20.0, 1.0),
        real(Champions, "champions", 0.0, 3.0, 1.0),
        switch(RevealMap, "reveal-map"),
        switch(RevealObjective, "reveal-objective"),
        switch(ProphecyAnyDistance, "prophecy-any-distance"),
        spec(NewItemRarity, "new-item-rarity", ValueKind::Choice(&[0, 1, 3, 7, 8])),
        spec(
            DropRates,
            "drop-rates",
            ValueKind::Reals(&[(0.0, 1.0, 0.1), (0.0, 100.0, 1.0), (0.0, 1.0, 0.1), (0.0, 100.0, 2.0)]),
        ),
        switch(ExtraRare, "extra-rare"),
        real(DuplicateOnDrop, "duplicate-on-drop", 1.0, 20.0, 2.0),
        switch(ProphecyUses, "prophecy-uses"),
        switch(SkipQuestEchoes, "skip-quest-echoes"),
        spec(MorditasTimer, "morditas-timer", ValueKind::Reals(&[(1.0, 10.0, 1.0), (0.0, 1.0, 1.0), (1.0, 10.0, 1.0)])),
        real(CameraDistance, "camera-distance", -100.0, 0.0, -17.5),
        real(PackCount, "pack-count", 0.0, 100.0, 1.0),
        real(PackSize, "pack-size", 0.0, 10.0, 1.0),
        spec(MonsterRarity, "monster-rarity", ValueKind::Reals(&[(0.0, 20.0, 1.0), (0.0, 20.0, 1.0), (0.0, 20.0, 1.0)])),
    ]
};

impl GameplayHook {
    pub const COUNT: usize = 60;
    /// Every hook by its number on the pipe, checked against [`SPECS`] when the crate builds.
    pub const ALL: [Self; Self::COUNT] = {
        let mut all = [Self::MysteriousRift; Self::COUNT];
        let mut index = 0;
        while index < Self::COUNT {
            assert!(SPECS[index].hook as usize == index, "SPECS follows the enum's order");
            all[index] = SPECS[index].hook;
            index += 1;
        }
        all
    };
    pub const fn spec(self) -> &'static HookSpec {
        &SPECS[self as usize]
    }
    pub const fn id(self) -> &'static str {
        self.spec().id
    }
    pub const fn value_kind(self) -> ValueKind {
        self.spec().value
    }
    pub fn from_id(id: &str) -> Option<Self> {
        SPECS.iter().find(|spec| spec.id == id).map(|spec| spec.hook)
    }
    /// The value it rests at while off.
    pub fn default_value(self) -> HookValue {
        self.value_kind().neutral()
    }
    pub fn validate_value(self, value: HookValue) -> Result<HookValue, ProtocolError> {
        if self.value_kind().accepts(value) { Ok(value) } else { Err(ProtocolError::InvalidData) }
    }
    pub(crate) fn decode(value: u8) -> Result<Self, ProtocolError> {
        Self::ALL.get(value as usize).copied().ok_or(ProtocolError::InvalidData)
    }
}

wire_enum! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PatchState: u8 {
        Original = 0,
        Patched = 1,
        Unknown = 2,
        /// Entry bytes are patched and another feature still owns the patch group.
        Shared = 3,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookObservation {
    pub patch: PatchState,
    pub enabled: bool,
    pub value: HookValue,
}
impl HookObservation {
    pub fn confirmed(self, wanted: bool) -> bool {
        self.enabled == wanted
            && if wanted {
                self.patch == PatchState::Patched
            } else {
                matches!(self.patch, PatchState::Original | PatchState::Shared)
            }
    }
    /// Its state, whether it is on, then its value: eight bytes, or sixteen for a value that needs them.
    pub fn encode(self) -> Vec<u8> {
        let mut w = Writer::default();
        w.byte(self.patch as u8);
        w.flag(self.enabled);
        w.u64(self.value as u64);
        if self.value > u64::MAX as HookValue {
            w.u64((self.value >> 64) as u64);
        }
        w.0
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut r = Reader::new(bytes);
        let patch = PatchState::try_from(r.byte()?)?;
        let enabled = r.flag()?;
        let mut value = HookValue::from(r.u64()?);
        if !r.is_empty() {
            value |= HookValue::from(r.u64()?) << 64;
        }
        r.finish()?;
        Ok(Self { patch, enabled, value })
    }
    /// The reply to `ReadGameplayHooks`: each observation behind its length, in the order asked.
    pub fn encode_all(observations: &[Self]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(observations.len() * 19);
        for observation in observations {
            let encoded = observation.encode();
            bytes.push(encoded.len() as u8);
            bytes.extend(encoded);
        }
        bytes
    }
    /// Exactly `count` observations and nothing after them.
    pub fn decode_all(bytes: &[u8], count: usize) -> Result<Vec<Self>, ProtocolError> {
        let mut r = Reader::new(bytes);
        let mut observations = Vec::with_capacity(count);
        for _ in 0..count {
            let length = usize::from(r.byte()?);
            observations.push(Self::decode(r.take(length)?)?);
        }
        r.finish()?;
        Ok(observations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DispatcherSpec, Request};
    #[test]
    fn ids_are_unique_and_each_rest_value_is_taken() {
        for (index, spec) in SPECS.iter().enumerate() {
            assert!(SPECS[..index].iter().all(|other| other.id != spec.id), "{}", spec.id);
            assert!(spec.value.accepts(spec.value.neutral()), "{}", spec.id);
        }
    }

    #[test]
    fn numbers_encode_as_their_kind() {
        let enemy = GameplayHook::EnemyDamage.value_kind();
        assert_eq!(enemy.encode(&[2.5]), Some(2.5f64.to_bits().into()));
        assert_eq!(enemy.encode(&[101.0]), Some(101.0f64.to_bits().into()));
        assert_eq!(enemy.encode(&[100.5]), None);
        assert_eq!((enemy.fit(150.0), enemy.fit(101.0)), (Some(100.0), Some(101.0)));
        let damage = GameplayHook::IncomingDamage.value_kind();
        assert_eq!((damage.encode(&[50.0]), damage.encode(&[50.5])), (Some(50), None));
        assert_eq!(damage.fit(49.6), Some(50.0));
        let rift = GameplayHook::MysteriousRift.value_kind();
        assert_eq!([0.0, 1.0, 2.0, 3.0].map(|choice| rift.encode(&[choice])), [Some(1), Some(1), Some(0), None]);
        assert_eq!((ValueKind::Switch.encode(&[]), ValueKind::Switch.encode(&[1.0])), (Some(1), None));
    }

    #[test]
    fn new_item_rarity_choices_map_to_game_rarities_without_accepting_arbitrary_numbers() {
        let kind = GameplayHook::NewItemRarity.value_kind();
        for (index, rarity) in [0, 1, 3, 7, 8].into_iter().enumerate() {
            assert_eq!(kind.encode(&[index as f64]), Some(rarity));
            assert!(GameplayHook::NewItemRarity.validate_value(rarity).is_ok());
        }
        for values in [&[][..], &[1.0, 2.0], &[-1.0], &[2.5], &[5.0], &[f64::NAN]] {
            assert_eq!(kind.encode(values), None);
        }
        for rarity in [2, 4, 6, 9, u64::MAX] {
            assert!(GameplayHook::NewItemRarity.validate_value(rarity.into()).is_err());
        }
    }

    #[test]
    fn real_pairs_encode_both_lanes_and_reject_invalid_values_or_arity() {
        let kind = GameplayHook::ExperienceMultiplier.value_kind();
        let packed = kind.encode(&[2.5, 7.25]).unwrap();
        assert_eq!([packed as u32, (packed >> 32) as u32].map(f32::from_bits), [2.5, 7.25]);
        assert!(kind.accepts(packed));
        for values in [&[][..], &[2.0], &[2.0, 3.0, 4.0], &[0.0, 2.0], &[2.0, 101.0], &[f64::NAN, 2.0], &[2.0, f64::INFINITY]] {
            assert!(kind.encode(values).is_none(), "{values:?}");
        }
        for bad in [f32::NAN, f32::INFINITY, 0.0, 101.0] {
            for packed in [
                u64::from(bad.to_bits()) | (u64::from(2.0f32.to_bits()) << 32),
                u64::from(2.0f32.to_bits()) | (u64::from(bad.to_bits()) << 32),
            ] {
                assert!(GameplayHook::ExperienceMultiplier.validate_value(packed.into()).is_err());
            }
        }
    }

    #[test]
    fn a_batched_read_names_each_hook_once_in_order_and_its_reply_holds_one_observation_each() {
        let dispatcher = DispatcherSpec { rva: 32, bytes: [0x90; 16] };
        let all = Request::ReadGameplayHooks {
            dispatcher,
            expected_manager: 1,
            hooks: GameplayHook::ALL.to_vec(),
        };
        assert_eq!(Request::decode(&all.encode()).unwrap(), all);
        let read = |hooks: Vec<GameplayHook>| {
            Request::ReadGameplayHooks {
                dispatcher,
                expected_manager: 1,
                hooks,
            }
            .encode()
        };
        let (first, second) = (GameplayHook::ALL[0], GameplayHook::ALL[1]);
        assert!(Request::decode(&read(vec![])).is_err(), "nothing to read");
        assert!(Request::decode(&read(vec![second, first])).is_err(), "out of order");
        assert!(Request::decode(&read(vec![first, first])).is_err(), "twice");
        let mut unknown = read(vec![first]);
        *unknown.last_mut().unwrap() = 255;
        assert!(Request::decode(&unknown).is_err());

        let observations = [
            HookObservation {
                patch: PatchState::Patched,
                enabled: true,
                value: 3,
            },
            HookObservation {
                patch: PatchState::Original,
                enabled: false,
                value: HookValue::MAX,
            },
        ];
        let reply = HookObservation::encode_all(&observations);
        assert_eq!(HookObservation::decode_all(&reply, 2).unwrap(), observations);
        assert!(HookObservation::decode_all(&reply, 1).is_err(), "an observation left over");
        assert!(HookObservation::decode_all(&reply, 3).is_err(), "one missing");
        assert!(HookObservation::decode_all(&reply[..reply.len() - 1], 2).is_err(), "cut short");
    }

    #[test]
    fn requests_reject_unknown_hooks_flags_and_values() {
        let dispatcher = DispatcherSpec { rva: 32, bytes: [0x90; 16] };
        for hook in GameplayHook::ALL {
            for request in [
                Request::SetGameplayHook {
                    dispatcher,
                    expected_manager: 1,
                    hook,
                    on: true,
                    value: hook.default_value(),
                },
                Request::ReadGameplayHook {
                    dispatcher,
                    expected_manager: 1,
                    hook,
                },
            ] {
                let bytes = request.encode();
                let mut invalid = bytes.clone();
                invalid[44] = 255;
                assert!(Request::decode(&invalid).is_err());
                if matches!(request, Request::SetGameplayHook { .. }) {
                    for offset in [45, 46] {
                        let mut invalid = bytes.clone();
                        if offset == 46 {
                            invalid[offset..offset + 8].fill(255);
                        } else {
                            invalid[offset] = 255;
                        }
                        assert!(Request::decode(&invalid).is_err());
                    }
                }
            }
        }
    }

    #[test]
    fn four_float_settings_validate_each_lane_and_roundtrip_both_words() {
        let hook = GameplayHook::DropRates;
        let kind = hook.value_kind();
        let numbers = [0.123, 1.234, 0.456, 7.89];
        let value = kind.encode(&numbers).unwrap();
        for (index, number) in numbers.into_iter().enumerate() {
            assert_eq!(f32::from_bits((value >> (index * 32)) as u32), number as f32);
        }
        for count in [0, 1, 2, 3, 5] {
            assert!(kind.encode(&vec![0.0; count]).is_none());
        }
        for (index, maximum) in [1.0, 100.0, 1.0, 100.0].into_iter().enumerate() {
            for bad in [-0.1, maximum + 0.1, f64::NAN, f64::INFINITY] {
                let mut invalid = numbers;
                invalid[index] = bad;
                assert!(kind.encode(&invalid).is_none());
                let invalid_bits = (value & !(HookValue::from(u32::MAX) << (32 * index))) | (HookValue::from((bad as f32).to_bits()) << (32 * index));
                assert!(hook.validate_value(invalid_bits).is_err());
            }
        }
        let dispatcher = DispatcherSpec { rva: 32, bytes: [0x90; 16] };
        let request = Request::SetGameplayHook {
            dispatcher,
            expected_manager: 7,
            hook,
            on: true,
            value,
        };
        let bytes = request.encode();
        assert_eq!(Request::decode(&bytes).unwrap(), request);
        assert!(Request::decode(&bytes[..bytes.len() - 8]).is_err());
        for value in [value, 0] {
            let observation = HookObservation {
                patch: PatchState::Patched,
                enabled: true,
                value,
            };
            assert_eq!(HookObservation::decode(&observation.encode()).unwrap(), observation);
        }
        assert!(GameplayHook::ExtraRare.validate_value((1 << 64) | 1).is_err());
    }
}
