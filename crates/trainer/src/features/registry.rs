//! The trainer's pages and features: what each row is, says and takes. Data only; whether a connected game can run a
//! feature is the connection's to say.
use super::*;
use spiderland_phrases::Phrase;
pub static PAGES: &[TrainerPage] = &[
    TrainerPage {
        area: TrainerArea::Character,
        title: Phrase::new("Character"),
        icon: "IconCharacter",
        sections: &[Phrase::new("Progression"), Phrase::new("Trees"), Phrase::new("Equipment rules")],
        stacked: true,
    },
    TrainerPage {
        area: TrainerArea::Stash,
        title: Phrase::new("Stash"),
        icon: "IconStash",
        sections: &[Phrase::new("Currency"), Phrase::new("Keys"), Phrase::new("Woven echoes"), Phrase::new("Duplicate")],
        stacked: true,
    },
    TrainerPage {
        area: TrainerArea::Combat,
        title: Phrase::new("Combat"),
        icon: "IconCombat",
        sections: &[Phrase::new("Survival"), Phrase::new("Offense"), Phrase::new("Resources"), Phrase::new("Movement")],
        stacked: true,
    },
    TrainerPage {
        area: TrainerArea::Forge,
        title: Phrase::new("Forge"),
        icon: "IconForge",
        sections: &[Phrase::new("Item"), Phrase::new("Crafting rules")],
        stacked: false,
    },
    TrainerPage {
        area: TrainerArea::Loot,
        title: Phrase::new("Loot"),
        icon: "IconLoot",
        sections: &[Phrase::new("Drops"), Phrase::new("Quality"), Phrase::new("Pickup")],
        stacked: true,
    },
    TrainerPage {
        area: TrainerArea::Monolith,
        title: Phrase::new("Monolith"),
        icon: "IconMonolith",
        sections: &[Phrase::new("Run"), Phrase::new("Echo web")],
        stacked: true,
    },
    TrainerPage {
        area: TrainerArea::Factions,
        title: Phrase::new("Factions"),
        icon: "IconFactions",
        sections: &[Phrase::new("Standing"), Phrase::new("Prophecies"), Phrase::new("Vendors")],
        stacked: true,
    },
    TrainerPage {
        area: TrainerArea::World,
        title: Phrase::new("World"),
        icon: "IconWorld",
        sections: &[Phrase::new("Monsters"), Phrase::new("Encounters"), Phrase::new("Dungeons and map")],
        stacked: true,
    },
];
impl FeatureDescriptor {
    /// A hook that takes values, shown as `kind`.
    const fn with_values(self, kind: FeatureKind, values: &'static [FeatureValue]) -> Self {
        Self { kind, values, ..self }
    }
    /// A hook that works only when the game shows `needs`.
    const fn needing(self, needs: Phrase) -> Self {
        Self { needs: Some(needs), ..self }
    }
}

/// A hook's row: a switch, unless `with_values` gives it values.
const fn switch(id: &'static str, area: TrainerArea, section: Phrase, title: Phrase, description: Phrase) -> FeatureDescriptor {
    FeatureDescriptor {
        key: FeatureKey::Hook(HookId(id)),
        area,
        section,
        title,
        description,
        kind: FeatureKind::Switch,
        values: &[],
        fields: &[],
        needs: None,
    }
}

/// A number a hook takes.
const fn number(label: Phrase, start: f64, minimum: f64, maximum: f64, step: f64, unit: Option<&'static str>) -> FeatureValue {
    FeatureValue {
        label,
        start,
        choices: &[],
        hint: None,
        minimum,
        maximum,
        step,
        unit,
    }
}

/// One of `choices`, by index.
const fn choice(label: Phrase, start: f64, choices: &'static [Phrase]) -> FeatureValue {
    FeatureValue {
        label,
        start,
        choices,
        hint: None,
        minimum: 0.0,
        maximum: (choices.len() - 1) as f64,
        step: 1.0,
        unit: None,
    }
}

/// A field an editor reads and writes.
const fn field(label: Phrase, minimum: f64, maximum: f64, step: f64) -> FeatureField {
    FeatureField {
        label,
        minimum,
        maximum,
        step,
        group: None,
    }
}

pub static DESCRIPTORS: &[FeatureDescriptor] = &[
    switch(
        "experience-multiplier",
        TrainerArea::Character,
        Phrase::new("Progression"),
        Phrase::new("Experience multiplier"),
        Phrase::new("Multiplies the experience your character and your skills gain."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[
            number(Phrase::new("Character experience"), 2.0, 1.0, 100.0, 0.5, Some("×")),
            number(Phrase::new("Skill experience"), 2.0, 1.0, 100.0, 0.5, Some("×")),
        ],
    ),
    switch(
        "level-up-per-gain",
        TrainerArea::Character,
        Phrase::new("Progression"),
        Phrase::new("Level up on every experience gain"),
        Phrase::new("Each experience gain is at least what your current level still needs. Skill experience is unchanged."),
    ),
    switch(
        "skills-to-level-20",
        TrainerArea::Character,
        Phrase::new("Progression"),
        Phrase::new("Skills to level 20"),
        Phrase::new("A specialized skill jumps to level 20 the next time it gains experience. Turning this off keeps the levels."),
    ),
    FeatureDescriptor {
        key: FeatureKey::Fields(FieldSetId("points-earned")),
        area: TrainerArea::Character,
        section: Phrase::new("Trees"),
        title: Phrase::new("Points earned"),
        description: Phrase::new("The passive and Weaver points your character has earned. The game counts them again on a level up, a Weaver rank or a load."),
        kind: FeatureKind::Editor,
        values: &[],
        fields: &[
            field(Phrase::new("Passive points"), 0.0, 65535.0, 1.0),
            field(Phrase::new("Weaver points"), 0.0, 65535.0, 1.0),
        ],
        needs: None,
    },
    switch(
        "unlimited-skill-points",
        TrainerArea::Character,
        Phrase::new("Trees"),
        Phrase::new("Unlimited skill points"),
        Phrase::new("Learn skill tree nodes without skill points. Node maximums and connections still apply."),
    ),
    switch(
        "unlimited-passive-points",
        TrainerArea::Character,
        Phrase::new("Trees"),
        Phrase::new("Unlimited passive points"),
        Phrase::new("Allocate passives without passive points."),
    ),
    switch(
        "unlimited-weaver-points",
        TrainerArea::Character,
        Phrase::new("Trees"),
        Phrase::new("Unlimited Weaver points"),
        Phrase::new("Allocate Weaver tree nodes without Weaver points."),
    )
    .needing(Phrase::new("At least one Weaver point earned.")),
    switch(
        "any-mastery-passives",
        TrainerArea::Character,
        Phrase::new("Trees"),
        Phrase::new("Deeper passives of any mastery"),
        Phrase::new("Allocate the deeper passives of masteries you didn't choose. You still need a mastery and the points."),
    ),
    switch(
        "specialization-slots",
        TrainerArea::Character,
        Phrase::new("Trees"),
        Phrase::new("All five specialization slots"),
        Phrase::new("All five skill specialization slots are open, whatever your level."),
    ),
    switch(
        "equip-without-requirements",
        TrainerArea::Character,
        Phrase::new("Equipment rules"),
        Phrase::new("Equip without requirements"),
        Phrase::new("Wear items above your level, meant for another class, or needing a faction rank."),
    ),
    switch(
        "full-set-bonus",
        TrainerArea::Character,
        Phrase::new("Equipment rules"),
        Phrase::new("Every set bonus from one piece"),
        Phrase::new("One piece of a set gives all its bonuses, from the next stats update."),
    ),
    switch(
        "any-offhand",
        TrainerArea::Character,
        Phrase::new("Equipment rules"),
        Phrase::new("Any item in the offhand"),
        Phrase::new("Use any item in the offhand, without type, dual-wield or pairing limits. The item's own requirements still apply."),
    ),
    switch(
        "several-primordials",
        TrainerArea::Character,
        Phrase::new("Equipment rules"),
        Phrase::new("Several primordial items"),
        Phrase::new("Equip more than one primordial item at a time."),
    ),
    FeatureDescriptor {
        key: FeatureKey::Fields(FieldSetId("currency")),
        area: TrainerArea::Stash,
        section: Phrase::new("Currency"),
        title: Phrase::new("Currency"),
        description: Phrase::new("Your gold, soul embers and ancient bones."),
        kind: FeatureKind::Editor,
        values: &[],
        fields: &[
            field(Phrase::new("Gold"), 0.0, 2147483647.0, 1.0),
            field(Phrase::new("Soul embers"), 0.0, 2147483647.0, 1.0),
            field(Phrase::new("Ancient bones"), 0.0, 2147483647.0, 1.0),
        ],
        needs: None,
    },
    FeatureDescriptor {
        key: FeatureKey::Fields(FieldSetId("keys")),
        area: TrainerArea::Stash,
        section: Phrase::new("Keys"),
        title: Phrase::new("Keys and consumables"),
        description: Phrase::new("How many keys, lizard tails, primordial items, charms and tokens you hold."),
        kind: FeatureKind::HeldItems,
        values: &[],
        fields: &[field(Phrase::new("A count for each one you hold"), 0.0, 2147483647.0, 1.0)],
        needs: Some(Phrase::new("At least one of the item in your stash.")),
    },
    FeatureDescriptor {
        key: FeatureKey::Fields(FieldSetId("woven-echoes")),
        area: TrainerArea::Stash,
        section: Phrase::new("Woven echoes"),
        title: Phrase::new("Woven echoes"),
        description: Phrase::new("How many of each woven echo you hold."),
        kind: FeatureKind::HeldItems,
        values: &[],
        fields: &[field(Phrase::new("A count for each one you hold"), 0.0, 2147483647.0, 1.0)],
        needs: Some(Phrase::new("At least one of the echo in your stash.")),
    },
    switch(
        "duplicate-on-drop",
        TrainerArea::Stash,
        Phrase::new("Duplicate"),
        Phrase::new("Duplicate items you drop"),
        Phrase::new("An item you drop lands as two. A stack of keys, consumables or woven echoes is multiplied instead."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Stack multiplier"), 2.0, 1.0, 20.0, 1.0, Some("×"))])
    .needing(Phrase::new("Drop the item while this is on.")),
    switch(
        "incoming-damage",
        TrainerArea::Combat,
        Phrase::new("Survival"),
        Phrase::new("Damage you take"),
        Phrase::new("Health damage your character takes. 0% is God mode, 100% is normal."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[FeatureValue {
            hint: Some(Phrase::new("Whole percentages, from 0 to 100.")),
            ..number(Phrase::new("Damage taken"), 50.0, 0.0, 100.0, 1.0, Some("%"))
        }],
    ),
    switch(
        "enemy-damage",
        TrainerArea::Combat,
        Phrase::new("Offense"),
        Phrase::new("Damage enemies take"),
        Phrase::new("Health damage enemies take, from you, your allies and your minions. 1× is normal, ∞ kills in one hit."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[FeatureValue {
            hint: Some(Phrase::new("0–100×. Step above 100 or type ∞ for one-hit kills.")),
            ..number(Phrase::new("Multiplier"), 2.0, 0.0, 101.0, 0.5, Some("×"))
        }],
    ),
    switch(
        "ability-speed",
        TrainerArea::Combat,
        Phrase::new("Offense"),
        Phrase::new("Ability speed multiplier"),
        Phrase::new("Multiplies the cast and attack speed of your abilities. Abilities with a fixed speed keep it."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Multiplier"), 2.0, 0.0, 100.0, 0.5, Some("×"))]),
    switch(
        "extra-projectiles",
        TrainerArea::Combat,
        Phrase::new("Offense"),
        Phrase::new("Extra projectile multiplier"),
        Phrase::new("Multiplies the extra projectiles your abilities fire. An ability with no extra projectiles gains none."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Multiplier"), 2.0, 0.0, 100.0, 0.5, Some("×"))]),
    switch(
        "fixed-area",
        TrainerArea::Combat,
        Phrase::new("Offense"),
        Phrase::new("Fixed increased area"),
        Phrase::new("Area skills use this increased area instead of your own."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[FeatureValue {
            hint: Some(Phrase::new("1 is 100% increased area.")),
            ..number(Phrase::new("Increased area"), 3.0, 0.0, 50.0, 0.5, None)
        }],
    ),
    switch(
        "fixed-attack-rate",
        TrainerArea::Combat,
        Phrase::new("Offense"),
        Phrase::new("Fixed attack rate"),
        Phrase::new("Holds your attack rate at this value, even through weapon changes. Turning it off restores the normal rate."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Rate"), 1.0, 0.01, 20.0, 0.1, None)]),
    switch(
        "fixed-leech-rate",
        TrainerArea::Combat,
        Phrase::new("Offense"),
        Phrase::new("Fixed leech rate"),
        Phrase::new("Holds your increased leech rate at this value. Turning it off restores the normal rate."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Increased rate"), 0.0, -1.0, 100.0, 0.1, None)]),
    switch(
        "no-mana-cost",
        TrainerArea::Combat,
        Phrase::new("Resources"),
        Phrase::new("No mana cost"),
        Phrase::new("Your abilities cost no mana."),
    ),
    switch(
        "refill-mana",
        TrainerArea::Combat,
        Phrase::new("Resources"),
        Phrase::new("Refill mana every tick"),
        Phrase::new("Refills your mana before each tick. Regeneration and drains still run."),
    ),
    switch(
        "no-cooldowns",
        TrainerArea::Combat,
        Phrase::new("Resources"),
        Phrase::new("No cooldowns"),
        Phrase::new("Your abilities are ready again at once and use no charges."),
    ),
    switch(
        "buffs-never-expire",
        TrainerArea::Combat,
        Phrase::new("Resources"),
        Phrase::new("Buffs never expire"),
        Phrase::new("Your buffs and positive ailments stop counting down."),
    ),
    switch(
        "movement-speed",
        TrainerArea::Combat,
        Phrase::new("Movement"),
        Phrase::new("Movement speed multiplier"),
        Phrase::new("Multiplies your movement speed. Zero keeps the normal speed."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Multiplier"), 2.0, 0.0, 100.0, 0.5, Some("×"))])
    .needing(Phrase::new("Re-equip your boots after changing it.")),
    switch(
        "auto-aim",
        TrainerArea::Combat,
        Phrase::new("Movement"),
        Phrase::new("Abilities aim for you"),
        Phrase::new("Abilities aim at enemies by themselves, except minion, movement and transform skills."),
    ),
    FeatureDescriptor {
        key: FeatureKey::Forge,
        area: TrainerArea::Forge,
        section: Phrase::new("Item"),
        title: Phrase::new("Item Forge"),
        description: Phrase::new("Edit the item in the in-game forge."),
        kind: FeatureKind::Editor,
        values: &[],
        fields: &[],
        needs: None,
    },
    switch(
        "forge-above-level",
        TrainerArea::Forge,
        Phrase::new("Crafting rules"),
        Phrase::new("Forge above your level"),
        Phrase::new("Allow crafts that raise an item's level requirement above your level."),
    ),
    switch(
        "shards-to-99",
        TrainerArea::Forge,
        Phrase::new("Crafting rules"),
        Phrase::new("Top up shards to 99"),
        Phrase::new("Shards in your stash below 99 are raised to 99. The change is saved."),
    ),
    switch(
        "runes-glyphs-to-99",
        TrainerArea::Forge,
        Phrase::new("Crafting rules"),
        Phrase::new("Top up runes and glyphs to 99"),
        Phrase::new("Runes and glyphs in single slots, the forge included, are raised to 99. The change is saved."),
    ),
    switch(
        "unlimited-woven-enchanter",
        TrainerArea::Forge,
        Phrase::new("Crafting rules"),
        Phrase::new("Unlimited Woven Enchanter uses"),
        Phrase::new("The Woven Enchanter's uses don't count down."),
    ),
    switch(
        "drop-rates",
        TrainerArea::Loot,
        Phrase::new("Drops"),
        Phrase::new("Item and gold drops"),
        Phrase::new("Changes how often enemies drop items and gold, and how much."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[
            number(Phrase::new("Item drop chance"), 0.1, 0.0, 1.0, 0.05, None),
            number(Phrase::new("Item drop multiplier"), 1.0, 0.0, 100.0, 0.5, Some("×")),
            number(Phrase::new("Gold drop chance"), 0.1, 0.0, 1.0, 0.05, None),
            number(Phrase::new("Gold drop multiplier"), 2.0, 0.0, 100.0, 0.5, Some("×")),
        ],
    ),
    switch(
        "boss-loot-chance",
        TrainerArea::Loot,
        Phrase::new("Drops"),
        Phrase::new("Boss loot at full chance"),
        Phrase::new("Boss and champion loot drops at 100% base chance. Other drop modifiers still apply."),
    ),
    switch(
        "extra-rare",
        TrainerArea::Loot,
        Phrase::new("Drops"),
        Phrase::new("An extra rare item"),
        Phrase::new("Every enemy drops an additional rare item when it dies."),
    ),
    switch(
        "new-item-rarity",
        TrainerArea::Loot,
        Phrase::new("Quality"),
        Phrase::new("Rarity of new items"),
        Phrase::new("New items roll this rarity: drops, shop stock and gambling."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[FeatureValue {
            label: Phrase::new("Rarity"),
            start: 3.0,
            choices: &[
                Phrase::ctx("rarity", "Normal"),
                Phrase::ctx("rarity", "Magic"),
                Phrase::ctx("rarity", "Rare"),
                Phrase::ctx("rarity", "Unique"),
                Phrase::ctx("rarity", "Set"),
            ],
            hint: None,
            minimum: 0.0,
            maximum: 100.0,
            step: 0.5,
            unit: None,
        }],
    ),
    switch(
        "tier-7-affixes",
        TrainerArea::Loot,
        Phrase::new("Quality"),
        Phrase::new("Tier 7 affixes on new items"),
        Phrase::new("Every affix on a new item rolls at tier 7."),
    ),
    switch(
        "max-random-rolls",
        TrainerArea::Loot,
        Phrase::new("Quality"),
        Phrase::new("Every random roll at its maximum"),
        Phrase::new("Every random roll lands at its maximum, item affixes and unique modifiers included."),
    ),
    switch(
        "pickup-anywhere",
        TrainerArea::Loot,
        Phrase::new("Pickup"),
        Phrase::new("Pick up from anywhere"),
        Phrase::new("Removes the range limit on picking up items and collecting crafting materials."),
    ),
    FeatureDescriptor {
        key: FeatureKey::Fields(FieldSetId("monolith-run")),
        area: TrainerArea::Monolith,
        section: Phrase::new("Run"),
        title: Phrase::new("This run"),
        description: Phrase::new(
            "The current run's corruption, Gaze of Orobyss and whether its echo options were rerolled (1) or not (0). With no run in progress, the last timeline you chose.",
        ),
        kind: FeatureKind::Editor,
        values: &[],
        fields: &[
            field(Phrase::new("Corruption"), 0.0, 2147483647.0, 1.0),
            field(Phrase::new("Gaze of Orobyss"), 0.0, 10000.0, 1.0),
            field(Phrase::new("Options rerolled"), 0.0, 1.0, 1.0),
        ],
        needs: Some(Phrase::new("A Monolith run selected. Open a timeline and choose its difficulty first.")),
    },
    switch(
        "skip-quest-echoes",
        TrainerArea::Monolith,
        Phrase::new("Run"),
        Phrase::new("Skip the quest echoes"),
        Phrase::new("The timeline opens with its boss available and stability full."),
    ),
    switch(
        "max-bonus-stability",
        TrainerArea::Monolith,
        Phrase::new("Echo web"),
        Phrase::new("Maximum bonus stability"),
        Phrase::new("Every echo grants its maximum bonus stability."),
    ),
    switch(
        "cycle-echo-types",
        TrainerArea::Monolith,
        Phrase::new("Echo web"),
        Phrase::new("Cycle echo types"),
        Phrase::new("New echoes rotate through arena, vessel, cemetery and beacon, with far-echo modifiers. More of the web is revealed."),
    ),
    switch(
        "one-echo-type",
        TrainerArea::Monolith,
        Phrase::new("Echo web"),
        Phrase::new("One echo type"),
        Phrase::new("New echoes are all this type."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[choice(
            Phrase::new("Echo type"),
            1.0,
            &[
                Phrase::ctx("echo type", "Normal"),
                Phrase::new("Arena"),
                Phrase::new("Shade"),
                Phrase::new("Beacon"),
                Phrase::new("Vessel of Chaos"),
                Phrase::new("Vessel of Memory"),
                Phrase::new("Sanctuary of Eterra"),
                Phrase::new("Cemetery of the Erased"),
                Phrase::new("Super Arena"),
                Phrase::new("Super Beacon"),
            ],
        )],
    ),
    switch(
        "reveal-objective",
        TrainerArea::Monolith,
        Phrase::new("Echo web"),
        Phrase::new("Reveal the objective"),
        Phrase::new("An echo's objective shows at the first kill."),
    ),
    FeatureDescriptor {
        key: FeatureKey::Fields(FieldSetId("faction-standing")),
        area: TrainerArea::Factions,
        section: Phrase::new("Standing"),
        title: Phrase::new("Rank, favor and reputation"),
        description: Phrase::new("Your rank, favor and reputation with each faction you've joined."),
        kind: FeatureKind::Editor,
        values: &[],
        fields: &[
            FeatureField {
                group: Some(Phrase::new("Circle of Fortune")),
                ..field(Phrase::new("Rank"), 0.0, 2147483647.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Circle of Fortune")),
                ..field(Phrase::new("Favor"), 0.0, 999999.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Circle of Fortune")),
                ..field(Phrase::new("Reputation"), 0.0, 2147483647.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Merchant's Guild")),
                ..field(Phrase::new("Rank"), 0.0, 2147483647.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Merchant's Guild")),
                ..field(Phrase::new("Favor"), 0.0, 999999.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Merchant's Guild")),
                ..field(Phrase::new("Reputation"), 0.0, 2147483647.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Forgotten Knights")),
                ..field(Phrase::new("Rank"), 0.0, 2147483647.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Forgotten Knights")),
                ..field(Phrase::new("Favor"), 0.0, 999999.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Forgotten Knights")),
                ..field(Phrase::new("Reputation"), 0.0, 2147483647.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Weaver")),
                ..field(Phrase::new("Rank"), 0.0, 2147483647.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Weaver")),
                ..field(Phrase::new("Favor"), 0.0, 999999.0, 1.0)
            },
            FeatureField {
                group: Some(Phrase::new("Weaver")),
                ..field(Phrase::new("Reputation"), 0.0, 2147483647.0, 1.0)
            },
        ],
        needs: None,
    },
    switch(
        "faction-gains",
        TrainerArea::Factions,
        Phrase::new("Standing"),
        Phrase::new("Favor and reputation multiplier"),
        Phrase::new("Multiplies the favor and reputation you gain."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Multiplier"), 2.0, 0.0, 100.0, 0.5, Some("×"))]),
    switch(
        "rank-up-per-gain",
        TrainerArea::Factions,
        Phrase::new("Standing"),
        Phrase::new("Rank up on every reputation gain"),
        Phrase::new("Every reputation gain is exactly what the next rank needs."),
    ),
    switch(
        "prophecy-any-distance",
        TrainerArea::Factions,
        Phrase::new("Prophecies"),
        Phrase::new("Combat prophecies at any distance"),
        Phrase::new("Combat targets count for your prophecies at any distance."),
    ),
    switch(
        "prophecy-max-lp",
        TrainerArea::Factions,
        Phrase::new("Prophecies"),
        Phrase::new("Prophecy uniques with full legendary potential"),
        Phrase::new("Uniques from prophecies drop with 4 legendary potential."),
    ),
    switch(
        "prophecy-reward-multiplier",
        TrainerArea::Factions,
        Phrase::new("Prophecies"),
        Phrase::new("Prophecy reward multiplier"),
        Phrase::new("Multiplies how many items a prophecy reward drops."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Multiplier"), 2.0, 0.0, 100.0, 0.5, Some("×"))]),
    switch(
        "prophecy-uses",
        TrainerArea::Factions,
        Phrase::new("Prophecies"),
        Phrase::new("Prophecies are not used up"),
        Phrase::new("A prophecy keeps its charges, so it pays out again."),
    ),
    switch(
        "free-hunter-vendor",
        TrainerArea::Factions,
        Phrase::new("Vendors"),
        Phrase::new("Hunter vendor for free"),
        Phrase::new("Buy from the Hunter vendor without paying its cost."),
    ),
    switch(
        "rune-wardens",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Rune Wardens"),
        Phrase::new("New zones hold this many Rune Wardens, which appear as you fight."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Rune Wardens"), 2.0, 0.0, 20.0, 1.0, None)]),
    switch(
        "rune-prison-complex",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Always a Rune Prison Complex"),
        Phrase::new("A Rune Prison drops a Rune Prison Complex whenever its level and echo allow."),
    ),
    switch(
        "experimental-uniques",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Always an experimental unique"),
        Phrase::new("A Rune Prison drops an experimental unique whenever its level allows."),
    ),
    switch(
        "class-experimentals",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Experimentals for your class"),
        Phrase::new("Rune Prisons drop experimental items your class can use."),
    ),
    switch(
        "loot-lizards",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Loot Lizards"),
        Phrase::new("New zones hold this many Loot Lizards, which appear as you fight."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Loot Lizards"), 3.0, 0.0, 20.0, 1.0, None)]),
    switch(
        "endgame-cache",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Always an endgame cache"),
        Phrase::new("New zones include an endgame cache where the game can place one. Not the Temporal Sanctum's Eternity Cache."),
    ),
    switch(
        "favor-cache",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Always a Favor Cache"),
        Phrase::new("New zones include a Favor Cache where the game can place one (with the Weaver's faction)."),
    ),
    switch(
        "nemesis",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Always a Nemesis"),
        Phrase::new("New zones include a Nemesis encounter where the game can place one."),
    ),
    // Portal is the default, so a profile saved while this was an on/off switch keeps its behaviour.
    switch(
        "mysterious-rift",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Mysterious Rifts"),
        Phrase::new("Choose the next time-beast encounter: a portal elsewhere or a boss rift in this zone."),
    )
    .with_values(
        FeatureKind::Choice,
        &[FeatureValue {
            hint: Some(Phrase::new("Spawn rules and scripted encounters still apply.")),
            ..choice(Phrase::new("Encounter"), 1.0, &[Phrase::new("Game decides"), Phrase::new("Portal"), Phrase::new("Boss")])
        }],
    ),
    switch(
        "omen-windows",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Always an Omen Window"),
        Phrase::new("New zones include an Omen Window where the game can place one."),
    ),
    switch(
        "morditas-timer",
        TrainerArea::World,
        Phrase::new("Encounters"),
        Phrase::new("Rage of Morditas timer"),
        Phrase::new("More time in Rage of Morditas hunts: a longer start, a slower countdown and more time per kill."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[
            FeatureValue {
                hint: Some(Phrase::new("From your next hunt.")),
                ..number(Phrase::new("Starting time"), 2.0, 1.0, 10.0, 0.5, Some("×"))
            },
            FeatureValue {
                hint: Some(Phrase::new("1 is normal. 0 stops the countdown, and the hunt ends when no foes are left.")),
                ..number(Phrase::new("Timer speed"), 0.5, 0.0, 1.0, 0.05, Some("×"))
            },
            FeatureValue {
                hint: Some(Phrase::new("Kills add less as the hunt goes on, as in the game.")),
                ..number(Phrase::new("Time per kill"), 2.0, 1.0, 10.0, 0.5, Some("×"))
            },
        ],
    ),
    switch(
        "pack-count",
        TrainerArea::World,
        Phrase::new("Monsters"),
        Phrase::new("Pack count multiplier"),
        Phrase::new("Multiplies the monster packs in randomly populated zones, such as echoes and dungeons."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Multiplier"), 2.0, 0.0, 100.0, 0.5, Some("×"))]),
    switch(
        "pack-size",
        TrainerArea::World,
        Phrase::new("Monsters"),
        Phrase::new("Pack size multiplier"),
        Phrase::new("Multiplies the monsters in each pack. Bosses and other lone monsters stay alone."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Multiplier"), 2.0, 0.0, 10.0, 0.5, Some("×"))]),
    switch(
        "monster-rarity",
        TrainerArea::World,
        Phrase::new("Monsters"),
        Phrase::new("Monster rarity"),
        Phrase::new("Multiplies how often packs roll each rarity. Raising one makes the others rarer."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[
            number(Phrase::ctx("monster rarity", "Normal"), 1.0, 0.0, 20.0, 0.5, Some("×")),
            number(Phrase::ctx("monster rarity", "Magic"), 2.0, 0.0, 20.0, 0.5, Some("×")),
            number(Phrase::ctx("monster rarity", "Rare"), 3.0, 0.0, 20.0, 0.5, Some("×")),
        ],
    ),
    switch(
        "champions",
        TrainerArea::World,
        Phrase::new("Monsters"),
        Phrase::new("Champions"),
        Phrase::new("New zones spawn up to this many champions, counting any already there. The game allows at most three."),
    )
    .with_values(FeatureKind::SwitchWithValues, &[number(Phrase::new("Champions"), 2.0, 0.0, 3.0, 1.0, None)]),
    switch(
        "eternity-cache-anywhere",
        TrainerArea::World,
        Phrase::new("Dungeons and map"),
        Phrase::new("Eternity Cache anywhere"),
        Phrase::new("Use the Temporal Sanctum's Eternity Cache anywhere and as often as you like, with no level cap on its uniques."),
    ),
    switch(
        "reveal-map",
        TrainerArea::World,
        Phrase::new("Dungeons and map"),
        Phrase::new("Reveal the zone map"),
        Phrase::new("The whole map of a zone shows from the start."),
    ),
    switch(
        "any-waypoint",
        TrainerArea::World,
        Phrase::new("Dungeons and map"),
        Phrase::new("Travel to any waypoint"),
        Phrase::new("Travel to any waypoint, unlocked or not. Places the game never allows stay closed."),
    ),
    switch(
        "camera-distance",
        TrainerArea::World,
        Phrase::new("Dungeons and map"),
        Phrase::new("Camera distance"),
        Phrase::new("Sets how far the camera is from your character."),
    )
    .with_values(
        FeatureKind::SwitchWithValues,
        &[FeatureValue {
            hint: Some(Phrase::new("The game's default is -17.5.")),
            ..number(Phrase::new("Distance"), -30.0, -100.0, 0.0, 0.5, None)
        }],
    ),
];
