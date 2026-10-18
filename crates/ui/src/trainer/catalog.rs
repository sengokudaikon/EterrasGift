//! Everything the trainer has or will have, by page and section: the C# app's `TrainerCatalog`
//! (`src/Trainer/EterrasGift.Trainer.Application/Features/TrainerCatalog.cs`), ported verbatim. Pages, sections and rows
//! are built from this list. TODO(trainer): it belongs to the trainer's application tier once that crate takes it.

/// The trainer's pages, in navigation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Area {
    Character,
    Stash,
    Combat,
    Forge,
    Loot,
    Monolith,
    Factions,
    World,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureKind {
    /// A game hook that is on or off.
    Switch,
    /// A game hook with values of its own (a multiplier, a count, a choice).
    SwitchWithValues,
    /// Fields the game holds, read and written directly.
    Editor,
    /// A field for each thing of a kind the character holds (a count each), as the game lists them.
    HeldItems,
}

/// A value of a game hook. `default` is the value it starts at, as shown: a number, or one of the choices. `choices` makes
/// it a pick from a list (the helper gets the choice's index). `min`, `max` and `step` are what its number box allows;
/// `unit` is what the number is, shown inside its box after it ("×" for a multiplier).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureValue {
    pub label: &'static str,
    pub default: &'static str,
    pub choices: &'static [&'static str],
    pub hint: Option<&'static str>,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub unit: Option<&'static str>,
}

impl FeatureValue {
    const fn new(label: &'static str, default: &'static str) -> FeatureValue {
        FeatureValue {
            label,
            default,
            choices: &[],
            hint: None,
            min: 0.0,
            max: 100.0,
            step: 0.5,
            unit: None,
        }
    }

    const fn choices(self, choices: &'static [&'static str]) -> FeatureValue {
        FeatureValue { choices, ..self }
    }

    const fn hint(self, hint: &'static str) -> FeatureValue {
        FeatureValue { hint: Some(hint), ..self }
    }

    const fn min(self, min: f64) -> FeatureValue {
        FeatureValue { min, ..self }
    }

    const fn max(self, max: f64) -> FeatureValue {
        FeatureValue { max, ..self }
    }

    const fn step(self, step: f64) -> FeatureValue {
        FeatureValue { step, ..self }
    }

    const fn unit(self, unit: &'static str) -> FeatureValue {
        FeatureValue { unit: Some(unit), ..self }
    }
}

/// A field an editor reads from the game and writes back; a whole number from 0 unless it says otherwise. `group` is what
/// it belongs to, when an editor holds the same fields for several things (each faction).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeatureField {
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub group: Option<&'static str>,
}

impl FeatureField {
    const fn new(label: &'static str) -> FeatureField {
        FeatureField {
            label,
            min: 0.0,
            max: i32::MAX as f64,
            step: 1.0,
            group: None,
        }
    }

    const fn min(self, min: f64) -> FeatureField {
        FeatureField { min, ..self }
    }

    const fn max(self, max: f64) -> FeatureField {
        FeatureField { max, ..self }
    }

    const fn step(self, step: f64) -> FeatureField {
        FeatureField { step, ..self }
    }

    const fn group(self, group: &'static str) -> FeatureField {
        FeatureField { group: Some(group), ..self }
    }
}

/// `id` is the stable key a page binds a ported feature by. `ported`: whether the app has it; a feature that is not ported
/// is listed only in a source checkout. `fills_section`: it is the whole section (the Item Forge), not one row in it.
/// `needs`: what the game has to be showing for it to work, when that is more than an offline character.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feature {
    pub id: &'static str,
    pub area: Area,
    pub section: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub kind: FeatureKind,
    pub ported: bool,
    pub values: &'static [FeatureValue],
    pub fields: &'static [FeatureField],
    pub needs: Option<&'static str>,
    pub fills_section: bool,
}

impl Feature {
    const fn new(id: &'static str, area: Area, section: &'static str, title: &'static str, description: &'static str, kind: FeatureKind) -> Feature {
        Feature {
            id,
            area,
            section,
            title,
            description,
            kind,
            ported: false,
            values: &[],
            fields: &[],
            needs: None,
            fills_section: false,
        }
    }

    const fn ported(self) -> Feature {
        Feature { ported: true, ..self }
    }

    const fn values(self, values: &'static [FeatureValue]) -> Feature {
        Feature { values, ..self }
    }

    const fn fields(self, fields: &'static [FeatureField]) -> Feature {
        Feature { fields, ..self }
    }

    const fn needs(self, needs: &'static str) -> Feature {
        Feature { needs: Some(needs), ..self }
    }

    const fn fills_section(self) -> Feature {
        Feature { fills_section: true, ..self }
    }

    /// What its (?) says: what it does, what the game has to be showing for it, and what its values mean.
    pub fn help(&self) -> String {
        let mut lines = vec![self.description.to_owned()];
        lines.extend(self.needs.map(str::to_owned));
        lines.extend(self.values.iter().filter_map(|value| value.hint.map(|hint| format!("{}: {hint}", value.label))));
        lines.join("\n")
    }
}

/// `sections` in the order the page shows them. `stacked`: its sections sit one under another on one screen, each under its
/// heading, with no strip to choose from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrainerPage {
    pub area: Area,
    pub title: &'static str,
    pub icon: &'static str,
    pub sections: &'static [&'static str],
    pub stacked: bool,
}

pub const PAGES: [TrainerPage; 8] = [
    TrainerPage {
        area: Area::Character,
        title: "Character",
        icon: "IconCharacter",
        sections: &["Progression", "Trees", "Equipment rules"],
        stacked: true,
    },
    TrainerPage {
        area: Area::Stash,
        title: "Stash",
        icon: "IconStash",
        sections: &["Currency", "Keys", "Woven echoes", "Duplicate"],
        stacked: false,
    },
    TrainerPage {
        area: Area::Combat,
        title: "Combat",
        icon: "IconCombat",
        sections: &["Survival", "Offense", "Resources", "Movement"],
        stacked: false,
    },
    TrainerPage {
        area: Area::Forge,
        title: "Forge",
        icon: "IconForge",
        sections: &["Item", "Crafting rules"],
        stacked: false,
    },
    TrainerPage {
        area: Area::Loot,
        title: "Loot",
        icon: "IconLoot",
        sections: &["Drops", "Quality", "Pickup"],
        stacked: false,
    },
    TrainerPage {
        area: Area::Monolith,
        title: "Monolith",
        icon: "IconMonolith",
        sections: &["Run", "Echo web"],
        stacked: false,
    },
    TrainerPage {
        area: Area::Factions,
        title: "Factions",
        icon: "IconFactions",
        sections: &["Standing", "Prophecies", "Vendors"],
        stacked: false,
    },
    TrainerPage {
        area: Area::World,
        title: "World",
        icon: "IconWorld",
        sections: &["Encounters", "Monsters", "Dungeons", "Map"],
        stacked: false,
    },
];

const X2: &str = "2";
const TIMES: &str = "×";

const fn multiplier(label: &'static str, start: &'static str) -> FeatureValue {
    FeatureValue::new(label, start).unit(TIMES)
}

const MULTIPLIER: FeatureValue = multiplier("Multiplier", X2);

const fn count(label: &'static str, start: &'static str) -> FeatureValue {
    FeatureValue::new(label, start).max(20.0).step(1.0)
}

const fn rarity(label: &'static str, start: &'static str) -> FeatureValue {
    FeatureValue::new(label, start).max(20.0).unit(TIMES)
}

const fn standing(faction: &'static str) -> [FeatureField; 3] {
    [
        FeatureField::new("Rank").group(faction),
        FeatureField::new("Favor").max(999_999.0).group(faction),
        FeatureField::new("Reputation").group(faction),
    ]
}

const STANDING: [FeatureField; 12] = {
    let [a, b, c] = standing("Circle of Fortune");
    let [d, e, f] = standing("Merchant's Guild");
    let [g, h, i] = standing("Forgotten Knights");
    let [j, k, l] = standing("Weaver");
    [a, b, c, d, e, f, g, h, i, j, k, l]
};

const HELD: &[FeatureField] = &[FeatureField::new("A count for each one you hold")];

use Area::*;
use FeatureKind::*;

pub const FEATURES: &[Feature] = &[
    // Character
    Feature::new("xp-multiplier", Character, "Progression", "Experience multiplier", "Multiplies the experience your character and your skills gain.", SwitchWithValues)
        .ported()
        .values(&[
            FeatureValue::new("Character experience", X2).min(1.0).unit(TIMES),
            FeatureValue::new("Skill experience", X2).min(1.0).unit(TIMES),
        ]),
    Feature::new("instant-level", Character, "Progression", "A level for every kill", "Every experience gain completes your current level.", Switch).ported(),
    Feature::new(
        "max-skill-level",
        Character,
        "Progression",
        "Skills at their maximum level",
        "Your specialised skills reach their maximum level the next time they gain experience.",
        Switch,
    )
    .ported(),
    Feature::new(
        "points-earned",
        Character,
        "Trees",
        "Points earned",
        "The passive and Weaver points your character has earned, until the game works them out again on a level, a Weaver rank or a load.",
        Editor,
    )
    .ported()
    .fields(&[FeatureField::new("Passive points").max(u16::MAX as f64), FeatureField::new("Weaver points").max(u16::MAX as f64)]),
    Feature::new("free-skill-points", Character, "Trees", "Skill trees without points", "Learn skill tree nodes without skill points left to spend.", Switch).ported(),
    Feature::new("free-passive-points", Character, "Trees", "Passive trees without points", "Allocate passives without points left to spend.", Switch).ported(),
    Feature::new("free-weaver-points", Character, "Trees", "Weaver tree without points", "Allocate Weaver tree nodes without points left to spend.", Switch)
        .ported()
        .needs("At least one Weaver point earned."),
    Feature::new("passive-chain", Character, "Trees", "Any mastery's deeper passives", "Allocate the deeper passives of a mastery you did not choose.", Switch).ported(),
    Feature::new("skill-slots", Character, "Trees", "All five skill slots", "Unlocks every skill bar slot, whatever your level.", Switch).ported(),
    Feature::new(
        "item-requirements",
        Character,
        "Equipment rules",
        "Equip without requirements",
        "Wear items above your level, meant for another class, or needing a faction rank.",
        Switch,
    )
    .ported(),
    Feature::new("set-bonus", Character, "Equipment rules", "Set bonuses without the pieces", "Every bonus of a set applies once you wear one of its pieces.", Switch).ported(),
    Feature::new("offhand", Character, "Equipment rules", "Anything in the offhand", "Any item fits the offhand slot.", Switch).ported(),
    Feature::new("primordial-limit", Character, "Equipment rules", "Several primordial items", "Equip more than one primordial item at a time.", Switch).ported(),
    // Stash
    Feature::new("currency", Stash, "Currency", "Currency", "Your gold, soul embers and ancient bones, changed through the game's own counters.", Editor)
        .ported()
        .fields(&[FeatureField::new("Gold"), FeatureField::new("Soul embers"), FeatureField::new("Ancient bones")]),
    Feature::new("keys", Stash, "Keys", "Keys and consumables", "How many you hold of each key, lizard tail, primordial item, charm and token.", HeldItems)
        .ported()
        .fields(HELD)
        .needs("At least one of the item in your stash."),
    Feature::new("woven-echoes", Stash, "Woven echoes", "Woven echoes", "How many you hold of each woven echo.", HeldItems)
        .ported()
        .fields(HELD)
        .needs("At least one of the echo in your stash."),
    Feature::new(
        "duplicate",
        Stash,
        "Duplicate",
        "Duplicate dropped items",
        "An item you drop lands as two. A stack of keys, consumables or woven echoes is multiplied instead.",
        SwitchWithValues,
    )
    .ported()
    .values(&[FeatureValue::new("Stack multiplier", X2).min(1.0).max(20.0).step(1.0).unit(TIMES)])
    .needs("Drop the item while this is on."),
    // Combat
    Feature::new("god-mode", Combat, "Survival", "God mode", "Your character takes no damage to health.", Switch).ported(),
    Feature::new("damage-taken", Combat, "Survival", "Less damage taken", "Damage you take is divided by this value.", SwitchWithValues)
        .ported()
        .values(&[FeatureValue::new("Divide by", X2).min(1.0)]),
    Feature::new("one-hit-kill", Combat, "Offense", "One-hit kills", "Every hit on an enemy kills it.", Switch).ported(),
    Feature::new("damage-dealt", Combat, "Offense", "Damage multiplier", "Damage you deal to enemies is multiplied.", SwitchWithValues)
        .ported()
        .values(&[MULTIPLIER]),
    Feature::new("cast-speed", Combat, "Offense", "Cast speed multiplier", "Your abilities are used faster.", SwitchWithValues)
        .ported()
        .values(&[MULTIPLIER]),
    Feature::new("projectiles", Combat, "Offense", "Extra projectiles", "Multiplies the projectiles your abilities fire.", SwitchWithValues)
        .ported()
        .values(&[MULTIPLIER]),
    Feature::new("area", Combat, "Offense", "Fixed area of effect", "Area skills use this increased area instead of your own.", SwitchWithValues)
        .ported()
        .values(&[FeatureValue::new("Increased area", "3").hint("1 is 100% increased area.").max(50.0)]),
    Feature::new(
        "attack-leech",
        Combat,
        "Offense",
        "Attack and leech rates",
        "Your character's attack rate and increased leech rate, until the game works them out again: the attack rate when you change weapons, the leech rate when your stats change.",
        Editor,
    )
    .ported()
    .fields(&[
        FeatureField::new("Attack rate").min(0.01).max(20.0).step(0.1),
        FeatureField::new("Increased leech rate").min(-1.0).max(100.0).step(0.1),
    ]),
    Feature::new("zero-mana-cost", Combat, "Resources", "No mana cost", "Your abilities cost no mana.", Switch).ported(),
    Feature::new("infinite-mana", Combat, "Resources", "Mana always full", "Mana refills to its maximum all the time.", Switch).ported(),
    Feature::new("no-cooldowns", Combat, "Resources", "No cooldowns", "Your abilities are ready again at once and use no charges.", Switch).ported(),
    Feature::new("endless-buffs", Combat, "Resources", "Buffs never expire", "Your buffs and positive ailments stop counting down.", Switch).ported(),
    Feature::new("move-speed", Combat, "Movement", "Movement speed multiplier", "You move faster.", SwitchWithValues)
        .ported()
        .values(&[MULTIPLIER])
        .needs("Re-equip your boots after changing it."),
    Feature::new(
        "auto-target",
        Combat,
        "Movement",
        "Abilities aim for you",
        "Abilities aim at enemies by themselves, except minion, movement and transform skills.",
        Switch,
    )
    .ported(),
    // Forge
    Feature::new("forge-item", Forge, "Item", "Item Forge", "Edit the item in the in-game forge.", Editor).ported().fills_section(),
    Feature::new(
        "forge-level",
        Forge,
        "Crafting rules",
        "Forge above your level",
        "The forge refuses a craft that would raise the item's level requirement above your character's level. This lets it through.",
        Switch,
    )
    .ported(),
    Feature::new(
        "shards",
        Forge,
        "Crafting rules",
        "Top up shards to 99",
        "Every shard in your stash below 99 is raised to 99 when the game counts them. They stay in your save.",
        Switch,
    )
    .ported(),
    Feature::new(
        "runes-glyphs",
        Forge,
        "Crafting rules",
        "Top up runes and glyphs to 99",
        "A stack in the forge's rune or glyph slot is raised to 99 whenever the game reads it. It stays in your save.",
        Switch,
    )
    .ported(),
    Feature::new("woven-enchanter", Forge, "Crafting rules", "Unlimited Woven Enchanter uses", "The Woven Enchanter does not count down its uses.", Switch).ported(),
    // Loot
    Feature::new("drop-rates", Loot, "Drops", "Item and gold drops", "Changes how often enemies drop items and gold, and how much.", SwitchWithValues)
        .values(&[
            FeatureValue::new("Item drop chance", "0.1").max(1.0).step(0.05),
            multiplier("Item drop multiplier", "1"),
            FeatureValue::new("Gold drop chance", "0.1").max(1.0).step(0.05),
            multiplier("Gold drop multiplier", X2),
        ])
        .ported(),
    Feature::new("boss-drops", Loot, "Drops", "Bosses always drop", "A boss's drop roll always succeeds.", Switch).ported(),
    Feature::new("extra-rares", Loot, "Drops", "An extra rare item", "Enemies drop an additional rare item when they die.", Switch).ported(),
    Feature::new(
        "force-rarity",
        Loot,
        "Quality",
        "Rarity of new items",
        "New items have this rarity when the game would roll it: drops, and shop and gambling stock.",
        SwitchWithValues,
    )
    .ported()
    .values(&[FeatureValue::new("Rarity", "Unique").choices(&["Normal", "Magic", "Rare", "Unique", "Set"])]),
    Feature::new("always-t7", Loot, "Quality", "Tier 7 affixes", "Every affix on a new item rolls at tier 7.", Switch).ported(),
    Feature::new(
        "perfect-rolls",
        Loot,
        "Quality",
        "Perfect rolls",
        "Every random roll lands at its maximum: affixes and unique modifiers, and other random rolls in the game too.",
        Switch,
    )
    .ported(),
    Feature::new("pickup-range", Loot, "Pickup", "Pick up from anywhere", "Removes the range limit on picking up items and collecting crafting materials.", Switch).ported(),
    // Monolith
    Feature::new(
        "monolith-run",
        Monolith,
        "Run",
        "This run",
        "The current run's corruption and Gaze of Orobyss, and whether its echo options were rerolled (1) or can still be (0). Uses the last requested timeline when no run is current; the map can be closed.",
        Editor,
    )
    .ported()
    .fields(&[FeatureField::new("Corruption"), FeatureField::new("Gaze of Orobyss").max(10000.0), FeatureField::new("Options rerolled").max(1.0)])
    .needs("A Monolith run selected. Open a timeline and choose its difficulty first."),
    Feature::new("skip-quest-echoes", Monolith, "Run", "Skip the quest echoes", "The timeline opens with its boss available and stability full.", Switch).ported(),
    Feature::new("max-bonus-stability", Monolith, "Echo web", "Most bonus stability", "Every echo grants the most bonus stability it can.", Switch).ported(),
    Feature::new(
        "echo-cycle",
        Monolith,
        "Echo web",
        "Cycle echo types",
        "New echoes rotate through arena, vessel, cemetery and beacon types with the modifiers of a far echo, and more of the web is revealed.",
        Switch,
    )
    .ported(),
    Feature::new("echo-type", Monolith, "Echo web", "One echo type", "New echoes are all this type.", SwitchWithValues)
        .ported()
        .values(&[FeatureValue::new("Echo type", "Arena").choices(&[
            "Normal",
            "Arena",
            "Shade",
            "Beacon",
            "Vessel of Chaos",
            "Vessel of Memory",
            "Sanctuary of Eterra",
            "Cemetery of the Erased",
            "Super Arena",
            "Super Beacon",
        ])]),
    Feature::new("reveal-objective", Monolith, "Echo web", "Reveal the objective", "An echo's objective shows at the first kill.", Switch).ported(),
    // Factions
    Feature::new(
        "faction-standing",
        Factions,
        "Standing",
        "Rank, favor and reputation",
        "Your rank, favor and reputation with each faction, changed through the game's own methods. Only a faction you have joined changes.",
        Editor,
    )
    .ported()
    .fields(&STANDING),
    Feature::new("faction-gains", Factions, "Standing", "Favor and reputation multiplier", "Multiplies the favor and reputation you gain.", SwitchWithValues)
        .ported()
        .values(&[MULTIPLIER]),
    Feature::new("instant-rank", Factions, "Standing", "A rank for every gain", "Every reputation gain is exactly what the next rank needs.", Switch).ported(),
    Feature::new(
        "prophecy-any-zone",
        Factions,
        "Prophecies",
        "Combat prophecies at any distance",
        "A combat target counts for your prophecies however far from you it falls.",
        Switch,
    )
    .ported(),
    Feature::new(
        "prophecy-max-lp",
        Factions,
        "Prophecies",
        "Prophecy uniques with full legendary potential",
        "Uniques from prophecies drop with 4 legendary potential.",
        Switch,
    )
    .ported(),
    Feature::new("prophecy-rewards", Factions, "Prophecies", "Prophecy reward multiplier", "Multiplies how many items a prophecy reward drops.", SwitchWithValues)
        .ported()
        .values(&[MULTIPLIER]),
    Feature::new("prophecy-uses", Factions, "Prophecies", "Prophecies are not used up", "A prophecy keeps the charges its reward would use, so it pays again.", Switch).ported(),
    Feature::new("hunter-cost", Factions, "Vendors", "Hunter vendor for free", "Buy from the Hunter vendor without paying its cost.", Switch).ported(),
    // World
    Feature::new("rune-wardens", World, "Encounters", "Rune Wardens", "New zones hold this many Rune Wardens, which appear as you fight.", SwitchWithValues)
        .ported()
        .values(&[count("Rune Wardens", "2")]),
    Feature::new("rune-prison", World, "Encounters", "Always a Rune Prison complex", "Opening a Rune Prison always drops a Rune Prison Complex.", Switch).ported(),
    Feature::new("experimental-uniques", World, "Encounters", "Always an experimental unique", "Opening a Rune Prison always drops an experimental unique.", Switch).ported(),
    Feature::new(
        "class-experimentals",
        World,
        "Encounters",
        "Experimentals for your class",
        "The experimental items a Rune Prison drops are ones your class can use.",
        Switch,
    )
    .ported(),
    Feature::new("loot-lizards", World, "Encounters", "Loot lizards", "New zones hold this many loot lizards, which appear as you fight.", SwitchWithValues)
        .ported()
        .values(&[count("Loot lizards", "3")]),
    Feature::new("eternity-cache", World, "Encounters", "Eternity caches", "New zones include an Eternity cache where the game can place one.", Switch).ported(),
    Feature::new(
        "favor-cache",
        World,
        "Encounters",
        "Favor caches",
        "New zones include a favor cache where the game can place one (with the Weaver's faction).",
        Switch,
    )
    .ported(),
    Feature::new("nemesis", World, "Encounters", "Nemesis", "New zones include a Nemesis encounter where the game can place one.", Switch).ported(),
    Feature::new("mysterious-rift", World, "Encounters", "Mysterious rifts", "The next time-beast encounter is a Mysterious rift.", Switch).ported(),
    Feature::new("omen-windows", World, "Encounters", "Omen windows", "New zones include an Omen window where the game can place one.", Switch).ported(),
    Feature::new(
        "morditas-timer",
        World,
        "Encounters",
        "Rage of Morditas timer",
        "Gives a Rage of Morditas hunt more time: a longer start, a slower countdown, more time for each kill.",
        SwitchWithValues,
    )
    .ported()
    .values(&[
        FeatureValue::new("Starting time", X2).hint("From the next hunt you start.").min(1.0).max(10.0).unit(TIMES),
        FeatureValue::new("Timer speed", "0.5")
            .hint("1 is the game's own; 0 stops the countdown, so the hunt ends when no foes are left.")
            .max(1.0)
            .step(0.05)
            .unit(TIMES),
        FeatureValue::new("Time per kill", X2)
            .hint("As in the game, a kill adds less the longer the hunt has grown.")
            .min(1.0)
            .max(10.0)
            .unit(TIMES),
    ]),
    Feature::new(
        "monster-density",
        World,
        "Monsters",
        "Monster packs",
        "Multiplies how many monster packs a randomly populated zone places (Monolith echoes, dungeons).",
        SwitchWithValues,
    )
    .ported()
    .values(&[MULTIPLIER]),
    Feature::new(
        "pack-size",
        World,
        "Monsters",
        "Pack size",
        "Multiplies how many monsters each pack spawns. Monsters that spawn alone, such as bosses, stay alone.",
        SwitchWithValues,
    )
    .ported()
    .values(&[FeatureValue::new("Multiplier", X2).max(10.0).unit(TIMES)]),
    Feature::new(
        "monster-rarity",
        World,
        "Monsters",
        "Monster rarity",
        "Multiplies how often a pack rolls each rarity, on top of the game's own odds. Raising one makes the others rarer, and a pack that never rolls a rarity still does not.",
        SwitchWithValues,
    )
    .ported()
    .values(&[rarity("Normal", "1"), rarity("Magic", X2), rarity("Rare", "3")]),
    Feature::new("champions", World, "Encounters", "Champions", "New zones spawn this many champions.", SwitchWithValues)
        .ported()
        .values(&[count("Champions", "2")]),
    Feature::new(
        "eternity-cache-anywhere",
        World,
        "Dungeons",
        "Unlimited Eternity Cache",
        "Use the Temporal Sanctum's Eternity Cache anywhere and as often as you like, with no level cap on its uniques.",
        Switch,
    )
    .ported(),
    Feature::new("reveal-map", World, "Map", "Reveal the zone map", "The whole map of a zone shows from the start.", Switch).ported(),
    Feature::new("waypoints", World, "Map", "Travel anywhere", "Travel to any place on the map, not only waypoints you have unlocked.", Switch).ported(),
    Feature::new("zoom", World, "Map", "Camera distance", "Sets how far the camera is from your character.", SwitchWithValues)
        .ported()
        .values(&[FeatureValue::new("Zoom distance", "-30").hint("The game's own is -17.5.").min(-100.0).max(0.0)]),
];

pub fn page(area: Area) -> &'static TrainerPage {
    PAGES.iter().find(|page| page.area == area).expect("every area has a page")
}

pub fn feature(id: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|feature| feature.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_every_section_is_a_page_section() {
        let mut ids: Vec<&str> = FEATURES.iter().map(|f| f.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), FEATURES.len());
        for feature in FEATURES {
            assert!(page(feature.area).sections.contains(&feature.section), "{} sits in a section of its page", feature.id);
        }
    }

    #[test]
    fn help_joins_description_needs_and_hints() {
        let timer = feature("morditas-timer").unwrap();
        assert_eq!(
            timer.help(),
            "Gives a Rage of Morditas hunt more time: a longer start, a slower countdown, more time for each kill.\n\
             Starting time: From the next hunt you start.\n\
             Timer speed: 1 is the game's own; 0 stops the countdown, so the hunt ends when no foes are left.\n\
             Time per kill: As in the game, a kill adds less the longer the hunt has grown."
        );
        assert_eq!(feature("move-speed").unwrap().help(), "You move faster.\nRe-equip your boots after changing it.");
    }

    #[test]
    fn defaults_follow_the_catalog_helpers() {
        let rarity = feature("monster-rarity").unwrap().values[1];
        assert_eq!((rarity.min, rarity.max, rarity.step, rarity.unit), (0.0, 20.0, 0.5, Some("×")));
        let wardens = feature("rune-wardens").unwrap().values[0];
        assert_eq!((wardens.max, wardens.step, wardens.unit), (20.0, 1.0, None));
        let favor = feature("faction-standing").unwrap().fields[4];
        assert_eq!((favor.label, favor.max, favor.group), ("Favor", 999_999.0, Some("Merchant's Guild")));
        assert_eq!(feature("currency").unwrap().fields[0].max, i32::MAX as f64);
    }
}
