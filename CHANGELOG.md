# Changelog

What the rewrite can do, newest first, one line per feature. Add a line when a feature lands; details live in the
code and its tests, and what is still to build in [research/trainer-features.md](research/trainer-features.md).

## 2026-11-01

- Mods: a build tool left running no longer stalls reading or building mod projects, or closing the app.

## 2026-10-31

- Trainer: reveal the map through the game's normal fog update, preserving larger reveal radii and the position argument.
- Trainer: reveal Monolith objectives on their next score event, preserving the game's event and kill handling.
- Trainer: combat prophecies count at any distance without bypassing their other completion requirements.
- Trainer: choose the rarity of newly rolled items while preserving explicit rarities, generation inputs and output references.
- Forge: spent Weaver counters remain editable below their generated minimum; the override allows Weaver's Touch and Weaver's Will from 0 to 255.
- Trainer: Loot Lizard count applies to new zone calculations without refilling encounters already consumed.
- Trainer: Rune Warden count is an independent control on the same verified encounter lifecycle.
- Trainer: Champions count overrides only the champion spawner's roll and preserves the game's placement limit.
- Trainer: Encounter forcing hooks resolve the game's nested placement-data type correctly.
- Forge: affix lines in the game's own words ("Chance to apply Damned on Hit While Channelling"), in every language it ships; the picker's tip adds each line's value and explanation, and an affix named only for its source ("Ucenui's") is listed by what it does.
- Forge: affix values read as they will on this item, using the game's per-base modifier (two-handed 3.2×, body armor 1.5×). A unique's modifier now keeps its value on its unique's base; it read half on most items.
- Trainer: Travel to locked waypoints, preserving the original excluded-location rule and restoring both travel checks when off.
- Trainer: Always an experimental unique from eligible Rune Prisons, with both chance checks bypassed and item-level requirements preserved.
- Trainer: Eternity Cache anywhere and repeatedly for the local player, using the game's highest unique-level cap.
- Forge: the affix picker filters by slot, level, stat tags and source as well, keeps unique modifiers out until asked or searched for by name, and its rows show pills with the rest in a tip.
- Forge: a unique's modifiers as affixes: with "I know what I'm doing" on, the picker lists them under Unique modifier (search by the unique's name), with permanent IDs, and the helper adds those in use to the game's list.
- Trainer: Movement speed multiplier, scoped to the local player's movement update without changing unrelated stat queries.
- Logs: a bug button beside Settings opens connection diagnostics, fault reports and the game log, with Refresh and Copy.
- Trainer: connection failures explain missing or unrecognized game-log evidence and how to recover.
- Trainer: Several primordial items, bypassing the local equip limit and the matching stat-suppression checks.
- Trainer: Fixed increased area for the current player's area skills, preserving other actors' calculations.
- Trainer: Refill mana before each player tick; normal regeneration and drains still run.

## 2026-10-30

- Trainer: independent character and skill experience multipliers, published together and applied to the current player.
- Trainer: Level up on every experience gain; takes precedence over character multiplication and keeps skill multiplication.
- Trainer: Favor and reputation multiplier, applied before the game derives reputation from favor.
- Trainer: Rank up on every reputation gain, using the remainder to the next rank and the game's normal rank-up path.
- UI: ready for translations: a `lang/<language>/LC_MESSAGES/eg-ui.po` reaches the views and the Rust phrases alike,
  with contexts for words that mean two things, each language's plural forms, and lists joined in its own words.
- Trainer: hook values are value cells, as affix values are: the value over its range bar, dragged (sent once, on
  release), stepped with the arrow keys, Home/End, or typed in a popover; 0–100× multipliers have a logarithmic bar.

## 2026-10-29

- UI: text and tooltips rewritten to be short and plain; facts once joined with dots are separate texts.
- Trainer: six more hooks: Every random roll at its maximum, Boss loot at full chance, Tier 7 affixes on new items,
  Ability speed multiplier, Maximum bonus stability, One echo type.
- Trainer: 17 code-patch hooks: Skills reach level 20, Unlimited skill/passive/Weaver points, Deeper passives of any
  mastery, Equip without requirements, Every set bonus from one piece, Forge above your level, Unlimited Woven
  Enchanter uses, Pick up from anywhere, Hunter vendor for free, Always an endgame cache/Favor Cache/Nemesis/Omen
  Window, Always a Rune Prison Complex, Experimentals for your class.
- Trainer: Top up shards, and runes and glyphs, to 99.
- Trainer: each feature has one name, in the UI, the settings and the code; settings saved under the C# names still
  load. Champions goes up to three, as the game places at most three.
- Trainer: every page but Forge is one list with section headings.
- Trainer: feature and item names in the game's language.
- Forge: item templates; the affix picker filters by kind and class and shows values; Max rolls; Undo restores
  legendary potential; the game's values for unique and implicit rolls; a unique's upgrade spends legendary potential.

## 2026-10-28

- Trainer: Fixed attack rate and Fixed leech rate, kept through the game's recalculations; No mana cost.
- Trainer: affixes named from the game's string tables.
- Forge: write unique, set and implicit rolls; an affix makes a unique legendary.

## 2026-10-27

- Trainer: first gameplay hooks: Mysterious Rifts (portal or boss) and Any item in the offhand, both confirmed in the
  game; Damage you take (0–100%, 0 is God mode) and Damage enemies take (multiplier up to one-hit kills).
- Forge: items with idol and unique-modifier affixes show; the affix picker shows tiers; re-roll and re-tier held
  affixes; edit rolls by position; an idol can change type with the override.

## 2026-10-21 to 2026-10-23

- Trainer: per-character hook profiles, kept when the user chooses and editable while no game runs.
- Trainer: the game data (affixes, item bases, uniques, sets, strings) is read from the shipped files, re-read when
  the game updates; the helper is embedded in the desktop and loads only into a game its data describes.
- Forge: affix add and remove are drafts until applied.
- App: one app per settings file; every companion screen at parity with the C# app.

## 2026-10-17 to 2026-10-20

- App: the Slint UI: Home, Settings, Mods (library, builds, management), the trainer pages and the shell, ported
  from the C# app.
- Trainer: connection to an offline character through the retained helper and its pipe.
- Forge: the Item Forge reads the held item and edits status, base and unique identity, affixes (replace, tier, seal,
  add, remove) and rolls, each with conditional Undo; game-built implicit descriptions and unique previews.
