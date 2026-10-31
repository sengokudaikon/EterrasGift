//! Concrete supported gameplay effects; arbitrary signatures never cross the pipe.
use crate::protocol::ProtocolError;

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
    /// One of the desktop's choices, by index, each sent as its value here.
    Choice(&'static [u64]),
}

impl ValueKind {
    /// The value it rests at while off.
    pub const fn neutral(self) -> u64 {
        match self {
            Self::Switch => 1,
            Self::Percent => 100,
            Self::Real { neutral, .. } => neutral.to_bits(),
            Self::RealPair { neutral, .. } => neutral.to_bits() as u64 | ((neutral.to_bits() as u64) << 32),
            Self::Choice(values) => values[0],
        }
    }

    pub fn accepts(self, value: u64) -> bool {
        match self {
            Self::Switch => value == 1,
            Self::Percent => value <= 100,
            Self::Real { .. } => self.takes(f64::from_bits(value)),
            Self::RealPair { .. } => [value as u32, (value >> 32) as u32].into_iter().all(|bits| self.takes(f64::from(f32::from_bits(bits)))),
            Self::Choice(values) => values.contains(&value),
        }
    }

    /// Whether the desktop's number is one this value takes.
    pub fn takes(self, number: f64) -> bool {
        number.is_finite()
            && match self {
                Self::Switch => false,
                Self::Percent => number.fract() == 0.0 && (0.0..=100.0).contains(&number),
                Self::Real { min, max, infinity, .. } => (min..=max).contains(&number) || Some(number) == infinity,
                Self::RealPair { min, max, .. } => (min..=max).contains(&number),
                Self::Choice(values) => number.fract() == 0.0 && number >= 0.0 && number < values.len() as f64,
            }
    }

    /// A saved number as this value takes it now: rounded and within its range, or `None` when nothing fits.
    pub fn fit(self, number: f64) -> Option<f64> {
        let fitted = match self {
            Self::Percent => number.round().clamp(0.0, 100.0),
            Self::Real { min, max, infinity, .. } if Some(number) != infinity => number.clamp(min, max),
            Self::RealPair { min, max, .. } => number.clamp(min, max),
            _ => number,
        };
        self.takes(fitted).then_some(fitted)
    }

    /// The value sent for the desktop's numbers: none for a switch, one otherwise.
    pub fn encode(self, numbers: &[f64]) -> Option<u64> {
        match (self, numbers) {
            (Self::Switch, []) => Some(1),
            (Self::Percent, [number]) if self.takes(*number) => Some(*number as u64),
            (Self::Real { .. }, [number]) if self.takes(*number) => Some(number.to_bits()),
            (Self::RealPair { .. }, [first, second]) if self.takes(*first) && self.takes(*second) => {
                let packed = u64::from((*first as f32).to_bits()) | (u64::from((*second as f32).to_bits()) << 32);
                self.accepts(packed).then_some(packed)
            }
            (Self::Choice(values), [number]) if self.takes(*number) => Some(values[*number as usize]),
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
    ]
};

impl GameplayHook {
    pub const COUNT: usize = 39;
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
    pub fn default_value(self) -> u64 {
        self.value_kind().neutral()
    }
    pub fn validate_value(self, value: u64) -> Result<u64, ProtocolError> {
        if self.value_kind().accepts(value) { Ok(value) } else { Err(ProtocolError::InvalidData) }
    }
    pub(crate) fn decode(value: u8) -> Result<Self, ProtocolError> {
        Self::ALL.get(value as usize).copied().ok_or(ProtocolError::InvalidData)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchState {
    Original,
    Patched,
    Unknown,
    /// Entry bytes are patched and another feature still owns the patch group.
    Shared,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HookObservation {
    pub patch: PatchState,
    pub enabled: bool,
    pub value: u64,
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
    pub fn encode(self) -> Vec<u8> {
        let mut bytes = vec![
            match self.patch {
                PatchState::Original => 0,
                PatchState::Patched => 1,
                PatchState::Unknown => 2,
                PatchState::Shared => 3,
            },
            u8::from(self.enabled),
        ];
        bytes.extend(self.value.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let [patch, enabled @ 0..=1, ref value @ ..] = *bytes else {
            return Err(ProtocolError::InvalidData);
        };
        Ok(Self {
            patch: match patch {
                0 => PatchState::Original,
                1 => PatchState::Patched,
                2 => PatchState::Unknown,
                3 => PatchState::Shared,
                _ => return Err(ProtocolError::InvalidData),
            },
            enabled: enabled != 0,
            value: u64::from_le_bytes(value.try_into().map_err(|_| ProtocolError::InvalidData)?),
        })
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
        assert_eq!(enemy.encode(&[2.5]), Some(2.5f64.to_bits()));
        assert_eq!(enemy.encode(&[101.0]), Some(101.0f64.to_bits()));
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
                assert!(GameplayHook::ExperienceMultiplier.validate_value(packed).is_err());
            }
        }
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
}
