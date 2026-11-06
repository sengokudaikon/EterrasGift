# eg-game-files

Last Epoch's data, read from the files it ships, with no game process. `mise run game-files:install` reads a real install
(`EG_LAST_EPOCH`) and checks the rebuilt layouts against every type tree the bundles ship. The readers themselves are
[`unity-files`](../../libs/unity-files/README.md); this crate knows what Last Epoch keeps where.

## The build

- Unity **6000.4.8f1**, IL2CPP, `global-metadata.dat` magic `FAB11BAF`, **metadata version 39**, not encrypted.
- `Last Epoch_Data/build_hash.txt` holds a build hash (`8dfd6929…` on 2026-10-20): a free update marker before any
  hashing.
- The selected language is Unity Localization's PlayerPrefs value, in the registry:
  `HKCU\Software\Eleventh Hour Games\Last Epoch`, `selected-locale_h3890535593` = `en`.

## Where each part of the catalog lives

| Capture reads (live) | File | Type trees | Content |
|---|---|---|---|
| `GlobalAssets._storage_MasterAffixesList` | `StreamingAssets/LEAssetBundles/PermaLoad.bundle`, MonoBehaviour `MasterAffixesList` (class `AffixList`) | in the bundle | 616 single + 540 multi affixes (ids 0–1156): name, title, type, property, tags, level requirement, weighting, `canRollOn`, 8 tiers with min/max and extra rolls, affix properties |
| `ItemList.get` | same bundle, `MasterItemsList` (class `ItemList`) | in the bundle | 42 equippable + 9 non-equippable base types, each with subtypes (level/class requirement, cannot-drop, corrupted, implicits) |
| `UniqueList.get` | `Last Epoch_Data/resources.assets`, `UniqueList` | **stripped** | 489 uniques: ids, base and subtypes, set membership, legendary data, mods (value, max value, roll id, property, tags), tooltip descriptions, lore |
| `PropertyList.get` | `resources.assets`, `MasterPropertyList` (class `PropertyList`) | **stripped** | 113 properties: rounding, percentage/tenth/less-is-better display flags, text keys |
| (set bonuses) | `resources.assets`, `SetBonusesList` | **stripped** | 25 entries |
| `LocalizationSettings` + `Localization.Items` names | `StreamingAssets/aa/StandaloneWindows64/localization-string-tables-<language>_assets_all.bundle` + `localization-assets-shared_assets_all.bundle` (keys) | in the bundles | 23 tables per language, e.g. `Item_Affixes` (`Item_Affix_25_DisplayName` = "Health", `_Title` = "of Life"), `Item_Names` (`Item_BaseType_Name_0` = "Helmet"), `Properties`, `Descriptors`, `Ailments`, `Skills` |

Other masters in `PermaLoad` (not read today): `MasterColorList` (the game's own colours),
`MasterItemTooltipAssetList`, `MasterGearSpriteList`, `AilmentList`, `Character Class List`, `FactionList`, `ShrineList`…
`resources.assets` also has `AbilityPropertyList`, `PlayerPropertyList`, `GlyphOfInsightReplacementAffixList`,
`ShardDropTable`. A `Localization` text asset there is the platform SDK's UI strings (`Gpp`), not game text.

## Formats in this game

- **Bundles:** UnityFS format 8, LZ4HC blocks, block info at the start after 16-byte padding (flags `0x243`). Inner
  serialized files are version 22. `LEAssetBundles` holds 26,667 bundles (21 GB) named by hash, plus `PermaLoad`,
  `monoscripts` (the `MonoScript`s every LE bundle references: `archive:/CAB-monoscripts/CAB-monoscripts`),
  `unitybuiltinassets` and per-scene bundles. A directory index of all of them (block info only) takes ~10 s.
- **`LEAssetBundles/Catalog.bin`:** a zip holding one `data` file, a custom format (`LABC` v4, a dictionary of path
  segments). Not needed so far: the masters are in `PermaLoad`. `DevOnly_GUIDToAssetPathLUT.bin` is not shipped.
- **`aa/`:** standard Unity Addressables (`catalog.bin`, `settings.json`), used only for localisation.
- **`resources.assets`:** a loose serialized file whose MonoBehaviours reference scripts in
  `globalgamemanagers.assets`; type trees are stripped, as usual for player data.

## What the string tables hold

- **A sixth of the entries have no key.** 12,069 of 71,628 English entries carry an ID the collection's shared data
  does not name (deleted keys, kept in the tables). Unity looks text up by key, so they are unreachable; they are
  left out and counted (`Strings::unkeyed`).
- **Some keys are named twice.** 176 keys in the shared data have two IDs (39 with different English text). The
  later ID wins, as a dictionary filled in order would; where the texts differ the later one is the newer
  ("PH: Chasing Apophis" → "Chasing Apophis"). The earlier entries are counted (`Strings::shadowed`). **Not yet
  checked against the game's own lookup**: compare with names from a live capture when one is next run.
- Locale codes are the tables' own (`en`, `de`, `es-ES`, `jp`, plus the `dev` and `key` debug locales), matched
  exactly; file names are not used to pick one.

## The catalog from the files

`eg_trainer::definitions::files` builds the trainer's `GameCatalog` from `eg-game-files` with the live capture's rules: stored names
(`displayName`, else `name`), the same `Unavailable` reasons (generated affix names, rarity names, implicit texts),
and the presentation selection ported from `catalog_numeric_property`. Item limits come from the `ItemData` and
`ExperienceTracker.maxLevel` `const int`s in the metadata (format 29+ stores them as compressed signed integers). On the
installed build: 42 bases, 1,156 affixes and 489 uniques, every numeric presentation available, ~0.3–0.9 s with hashing.

- **Identity:** the SHA-256 of `GameAssembly.dll` and `global-metadata.dat`, as the session's build profile has them.
  The locale is the session's own (`ReadLocale`), since nothing in the catalog is localised; no registry is read.
- **The backend** reads the files on a blocking thread when it starts (`BackendOptions::game_files`, the folder of the
  found game) and publishes `GameFiles` in its snapshot. On connect, and when the game's language changes, a session
  whose hashes match uses it. Another build or an unreadable folder: nothing is loaded into that game, as its data
  cannot describe it.
- **Settings** says "Reading game files", then "Game files · 1156 affixes" before a session.
- `AilmentList` lists Ignite three times; repeats are accepted while their effect agrees, refused if they ever differ.
- **A Steam update while the app runs** (`ff0352a`): the catalog keeps the size and write time of every file it
  read; at connect, if any changed, the backend reads the files again before comparing hashes.
- **Not checked:** the file catalog against a live read of the game (which would also settle the shadowed string
  keys above).

## A stat line's `tags`: the `AT` flags

`AT` (`LE.dll`, no namespace, read from `global-metadata.dat` 2026-10-31) is an `Int32` flags enum. The affix picker's stat
tags (`crates/ui/src/pages/forge/picker.rs`) name these.

| Flag | Name | Flag | Name | Flag | Name |
|---|---|---|---|---|---|
| 1 | Physical | 256 | Spell | 65536 | Potion |
| 2 | Lightning | 512 | Melee | 131072 | Buff |
| 4 | Cold | 1024 | Throwing | 262144 | Channelling |
| 8 | Fire | 2048 | Bow | 524288 | Transform |
| 16 | Void | 4096 | DoT | 1048576 | LowLife |
| 32 | Necrotic | 8192 | Minion | 2097152 | HighLife |
| 64 | Poison | 16384 | Totem | 4194304 | FullLife |
| 128 | Elemental | 32768 | PetResisted | 8388608 | Hit |
| | | | | 16777216 | Curse |
| | | | | 33554432 | Ailment |

Three more are named `Crit_deprecated`, `Kill_deprecated` and `Die_deprecated` (1 << 26 to 28). There is no Attack flag:
attacks are Melee, Throwing and Bow.

`tags` holds these flags only where the property reads it so. Ability (58) reads it as the ability's ID; player (98),
tracker (123) and idol altar (130) as an index into their registries. Increased ailment effect (43) is the one left
open: one shipped affix has flags on it (8192), which isn't enough to say what they mean there, so the picker doesn't read them.

`SP`, the properties' enum, is a `byte` enum, so its metadata defaults are raw bytes rather than the compressed integers
`int_default` reads; the property list's `id`s and names are the same numbering and read straight.

## A stat line's words, 2026-10-31

Read from `GameAssembly.dll`'s disassembly, with names from `global-metadata.dat`. `eg_trainer::definitions::stat_text` is the port.

**The path.** `ModFormatting.FormatProperty` chooses a `PropertyPrefix` and calls `ModFormatting.GetPropertyDescriptor(property, tags, special, extra, modType, prefix)`:

- `Localization.IsOriginalLocale` (the locale is `English (en)`) or a miss in `LocalizationDescriptors.TryGetText`: compose, with `Tags.TaggedPropertyStringWithoutLimit`, then `Tags.ApplyCooldownOrLimitToTaggedPropertyString` for properties 38 to 40 (added only), 50, 126 and 127, then `ModFormatting.AddModifierPrefix`.
- Otherwise the `Descriptors` text. With prefix AddedTo the code puts `ModFormat_AddedToPrefix_To` in front, as those entries leave it out; AddedAsPercentageOf entries carry their "of".

So **English never reads `Descriptors`**. It is the translators' source, written when the line was exported, and some of it is stale: "more more Damage per stack of Shock", "weilding", `98,557` is another stat entirely. A real-install test composes every 4- and 5-part key and compares; beyond drop rates and lens effects (not composed here, below), the differences are of that kind or a doubled word on a property that names its own modifier ("increased Increased Healing Effectiveness").

**`PropertyPrefix`:** 0 None, 1 AddedAsPercentageOf, 2 AddedTo, 3 Increased, 4 Reduced, 5 More, 6 Less. By `FormatProperty`, from `BasePropertyInfo`'s flags and the value's sign:

| Type | Prefix |
|---|---|
| 0 added | AddedAsPercentageOf if `displayAsPercentageOf`, else AddedTo if `displayAsAddedTo`, else None |
| 1 increased | None if `nameIncludesModifier` and not negative; Reduced if negative; else Increased |
| 2 more | Less if negative, else More |
| 3 and up | None |

**`Descriptors` keys** (`LocalizationDescriptors.CacheTable` splits on commas, parsing with `Enum.TryParse`, `Byte.TryParse` and `Int32.TryParse`):

| Parts | Fields | Dictionary |
|---|---|---|
| 5 | property, tags, special, extra, prefix | `_propertyTableCache`, `DescriptorKey` |
| 4 | property, tags, special, prefix (extra 0) | the same |
| 2 | affix ID, prefix | `_affixTableCache`, `AffixDescriptorKey` |

The capitalisation differs because the 4-part keys are older ("Increased Mana", "reduced Mana"). 41 lines have both shapes, 15 differing in English and 13 in German, and the 5-part text is the newer each time; `stat_text` prefers it. Not settled: the game's dictionary is filled in table order and which of the two it keeps there was not read. The 2-part lookup (`TryGetText(affixId, modType, out text)`) has no direct call or tail jump in the build, so nothing was found to use it.

**The composition** (`Tags.TaggedPropertyStringWithoutAbilitySpecificity`) is words from `Common` (`AT_{flag}`, `Tags_AT_{flag}` first; `HitEventTag_{name}`; `Tags_*`), `Properties` (`Property_Master_{name}_Name`, `Property_Damage_{condition}`), `Ailments` (`Ailment_{AilmentID}_Name`, `_InstanceName`), `Abilities` (`Ability_{object name}_Name`) and `UI` (`Tooltip_Stat_ForAbility`, `_WithAbility`, `_UpToXPerY`, `_EffectCooldown`). Each `GetText(key, fallback)` carries an English fallback in code. Every shipped language has all the keys, so the port holds no English and a missing key gives no name. Text is `String.Format`, positional only.

- Registries: 58 `Property_Ability_{key}_{n}_Name`, 98 `Property_Player_{tags}_Name` and 130 `Property_IdolAltar_{tags}_Name`, each entry's `textKey` + `_Name`, falling back to its `propertyName`. The registry holds the key, so nothing maps ability IDs to names for these.
- `Tracker` (123): `TrackerPropertyInfo` has no text; the game prints `TrackerPropertyID`'s name. No shipped line uses it.
- Drop rates (104) and lens effects (122) compose from item and faction data. No unique mod or shipped affix uses them; `stat_text` gives no words for them.
- Ability-specific lines (`extra`): `AbilityManager.abilities[id]`, in `PermaLoad`, named by the object's `m_Name`. Property 126 reads the ability from `tags`.
- Proc limits: `PropertyList.extraStatData`, keyed property, special, tags, extra, then with tags 0.
- Class-specific "level of skills" (specials 2 to 6): `CharacterClass.specialTagForClassSpecificLevelOfSkillsStats`; 7 to 11 are the attributes.
- Word order per language is `ILanguageModel.DescriptorPreferences`. Only Korean sets the ones used here: `StatModifiersAsSuffix`, `ValuesPrependModifiers`, `OmitAddedToPrefix`, `OmitPercentageOfPrefix`.

**The explanation** (`LocalizationProperties.TryGetAltTextPropertyString`): `GetText(propertyInfo.getAltTextKey(...), propertyInfo.getAltText(...))`. A stat property's key is its `textKey` (already ending `_AltText`), or an `altTextOverride`'s key when one matches type, tags and special, or then special alone. Registry entries use `textKey_AltText`, blank where the entry's `altText` is blank, except 98 which always asks. `getAltText` was taken to choose the same override as `getAltTextKey`, which was read; its own body was not.

**Coverage, 2026-10-31 build.** 4,144 lines (2,448 unique mods, 1,696 affix properties), 1,829 distinct. Composition names all of them in all ten languages, and in English none falls through to a stored name (other languages were not checked for that). `Descriptors` alone would have 915 of 1,310 distinct unique lines and 813 of 923 affix lines (counted per source): 395 and 110 miss, 325 and 78 of those in registries 58, 98 and 130. Those lines are composed in every language.

## An affix's value on an item, 2026-10-31

`AffixList.GetSingleAffixValue` multiplies a tier's roll by `1 + Affix.getModifier(m)`, and `getModifier(m)` is 0 when `m` equals the affix's `standardAffixEffectModifier` (`s`), else `(m + 1) / (s + 1) - 1`. So a value is `tier * (1 + m) / (1 + s)`. `m` comes from `ItemList.getAffixEffectModifier(baseType, subType)`: the base type's `affixEffectModifier`, or `ItemList.omenIdolAffixEffectModifier` for a subtype whose `affixEffectiveness` is 1. Stats, tooltips and skills all read it.

`m` by base: armor pieces, boots, belts, rings, quivers, relics 0; body armor 0.5; shield, catalyst, amulet 0.17; one-handed weapons and bows 0.75; crossbows 0.6; two-handed 2.2; idols -0.83 to -0.05. Shipped affixes set `s` to the base they mostly roll on, 0 for most.
