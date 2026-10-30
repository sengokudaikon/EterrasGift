# Changelog

What the rewrite can do, newest first, one line per feature. Add a line when a feature lands; details live in the
code and its tests, and what is still to build in [research/trainer-features.md](research/trainer-features.md).

## 2026-10-30

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
