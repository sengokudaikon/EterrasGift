# Changelog

What the rewrite can do, newest first, one line per feature. Add a line when a feature lands; details live in the
code and its tests, and what is still to build in [the helper's backlog](crates/trainer-helper/BACKLOG.md).

## 2026-11-09

- Item Forge: template saves and deletes run one at a time in the order asked, and a save that finishes late leaves a newly typed name alone.
- Item Forge: closing the window during Apply or Undo waits until every step is sent and read back; a step that fails keeps the window open on the Forge.
- Mods: closing during a build stops the compiler and closes at once; a build already installing finishes and isn't started again if the close is called off.
- Build: a dev build composites Eterra's band as a shipped one does (1.3–1.6 ms a frame, from 11–12): it is eg-ui-band, optimised in every profile.
- UI: work a page hands off that never gets to run now ends as a fault would, not with the page loading for good.
- Build: an edit to the UI's Rust no longer compiles the Slint views again (about 1 s, from 7): they are the new eg-ui-view, so a translation is now `lang/<language>/LC_MESSAGES/eg-ui-view.po`.
- Build: an edit to Settings, Logs or a trainer page no longer rebuilds the trainer's worker: they are in eg-companion and the new eg-trainer-screens.
- Settings: Restart still works after the game data service stopped on a fault, as after the mod library stopped.
- Mods: a fault while reading mod projects now fails the app's exit code.

## 2026-11-08

- Trainer: a session's game catalog is checked without being encoded, and is saved whole; a catalog saved by an older version is read from the game files again once.
- Forge: Home and End go to the first and last entry of its menus.
- Mods: a build the window's close stopped starts again when the window stays after all.
- Logs: a fault report lists what the window did last, each page's intents by name.
- Forge: a drafted unique roll always shows what it was, and an affix's tier menu is rescaled by a drafted base.
- Forge: a write's, read's or preview's answer is shown in one redraw with the worker's state it was judged on.
- Build: the words and numbers the UI shows come from `libs/phrases`, no longer from spiderland.
- Build: `mise run phrases` leaves a current template alone, so it no longer forces a UI rebuild, and a translation whose plural rule can't be read fails the build.

## 2026-11-06

- Trainer: a hook group that can't restore no longer keeps the others in the game; the rest are restored and the helper retries it before giving up.
- Trainer: only the installed game, at the app's integrity level or above, is prepared; a process this user can't open is passed over.
- Trainer: the helper's pipe refuses lower-integrity processes.
- Mods: project evaluation ignores response files and the project's global.json, and keeps no network paths.
- Mods: a mod DLL over 64 MiB is refused, and parsing grows with what is read rather than with the DLL's tables.
- Logs: fault reports and the Logs page write the profile folder as %USERPROFILE%; Forge reports leave out memory addresses.
- App: release builds carry no build-machine paths.
- Settings: an About section with the version, the licence (GPL-3.0-only), the source and third-party licences.
- Build: spiderland comes from its repository at a pinned revision.

## 2026-11-02

- World: set camera distance persistently, preserving game-setting changes and restoring each camera's baseline on off.
- World: multiply pack count through the game's room density without bypassing its placement cap.
- World: multiply pack size while leaving bosses and other lone monsters alone.
- World: independently scale Normal, Magic and Rare pack weights, sharing one restored spawner call with pack size.
- Character: read and apply earned passive and Weaver points, with UInt16 limits and an allocated-point floor.
- Monolith: edit the character's current or last selected saved run, recalculate corruption, save and refresh a matching open panel.
- Factions: edit joined factions' rank, favor and reputation with the game's rank rules and save/sync methods.
- World: tune Morditas hunt starting time, countdown speed and time per kill through one persistent control.
- Trainer: keep wide hook settings separate from Windows unwind metadata, and publish every target's settings before reopening a shared group.
- Factions: successful prophecy rewards keep completed charges without changing the game's reported consumption.
- Monolith: skip quest echoes when opening a timeline, raising quest progress and stability without lowering either.
- Trainer: item and gold drop chances and multipliers apply together to new enemy deaths.
- Trainer: an extra rare item per enemy uses its own control on the shared death-drop hook.
- Stash: edit gold, soul embers and ancient bones through the game's currency methods.
- Stash: read and edit held keys and consumables, including removing a stack by setting its quantity to zero.
- Stash: read and edit held woven echoes through the same checked quantity workflow.
- Stash: duplicate player-dropped items; keys, consumables and woven echoes use the stack multiplier.

- Forge: a drag that stays on one roll redraws nothing, and the base and identity menus are made again only when the item, catalog or override changes.
- Trainer: the monitor reads every active hook in one request, one turn of the game's main thread rather than one per hook.
- Trainer: the helper resolves its status and assembly lookups once per game run rather than on every request.
- Trainer: an edit made just before an area load is refused and the session stays, instead of stopping at Blocked.
- Trainer: a session that connects between areas reads its catalog when the character arrives, checked against the game's build.
- Forge: Undo is offered only while it can still be done, and an area load ends it.
- App: a lock file nothing can open no longer reads as another running app.
- Forge: Weaver's Touch stops at the game's maximum again, with the override too; an idol already above it can only come down.
- Trainer: connecting while the game freezes for a few seconds during a load waits it out, instead of stopping at Blocked.
- Mods: once the mod library has stopped, the page is read-only, a change that never reached it is shown undone, and the app can close and restart.

## 2026-11-01

- Mods: a build tool left running no longer stalls reading or building mod projects, or closing the app.
- Mods: a DLL that breaks the parser reads as a DLL that cannot be read, instead of closing the app.
- Mods: a fault in the mod library stops the library and says so on the page, instead of closing the app.

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
