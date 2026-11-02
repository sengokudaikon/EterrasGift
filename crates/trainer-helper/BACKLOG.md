# Trainer: what is left to build

The backlog of trainer features not built yet, and what each needs. A feature leaves this file when it lands: its
row is deleted, never marked done. What landed, in brief, is in [CHANGELOG.md](../../CHANGELOG.md); how a built feature
works is in its module's `//!` docs under `crates/trainer-helper/src/game/features/`. The C# implementation in
`../EterrasGift` is the behaviour reference; the helper's runtime assumptions are in [its README](README.md).

**Confirmed in the live game:** Mysterious Rifts selection and Any item in the offhand (the user, 2026-10-27). Other
built hooks are covered by fixtures and static inspection only. Record a new live check here, with the exact
combinations checked.

**Names (2026-10-29):** each feature has one name. The ID uses its title's words, and the wire variant, helper group
and module are spelled from the ID. The C# IDs (`cast-speed`, `offhand`, …) are read through `RENAMED` in
[profiles](../trainer/src/features/profiles.rs) and never written back. Game nouns follow the game's own text
(Skill Specialization Slots, Favor Cache, Omen Window). `endgame-cache` is the zone cache the old cheat table called
"Eternity cache"; the game's Eternity Cache is only the Temporal Sanctum's (`eternity-cache-anywhere`).

## Page layout

Character, Stash, Combat, Loot, Monolith, Factions and World use one combined list with section headings, using the
existing Character page layout and responsive column splitting. Forge keeps its Item / Crafting rules tabs.
World's groups are Monsters, Encounters, then Dungeons and map. Champions belongs to Monsters. This is presentation
only; feature IDs/profile settings and the original gameplay behavior remain independent of where rows appear.

## Shared support still to build

| Status | Support | Current scope / next consumer |
|---|---|---|
| Todo | Scoped context | Projectiles and echo cycling. Duplicate uses its checked direct caller and needs no retained mark. Movement uses its checked, single native call site and needs no retained mark. |
| Todo | Temporary asset mutation/restoration | Prophecy rewards/max LP, then density and pack size/rarity; settle threading and native-unwind behavior with those consumers. |
| Todo | Additional ABI/caller adapters | Validate auto-aim hidden result storage and the placement configuration struct when their features arrive. |

## Hooks to build

Each row retains the concrete behavior, useful shared mechanism and distinguishing targeted check. Common on/off,
readback, manager-loss and unload requirements apply to every implemented feature. Value ranges/arity remain owned
by [the catalog](../trainer/src/features/registry.rs); native support is declared by
[GameplayHook](../trainer-wire/src/hooks.rs) and resolved in
[helper features](src/game/features/registry.rs).

### Character

Behavior reference: [character.h](../../../EterrasGift/native/hooks/character.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `specialization-slots` — `PlayerFinder.getPlayerActor` | Call original, raise the returned actor's `localTreeData.numberOfUnlockedSlots` to at least five. On disabling, recount through `LocalTreeData.updateUnlockedSlots` on the main thread when a character still exists. Avoid the folded byte getter. | Detour lifecycle; object descriptors. Keep recount and its timing feature-specific. | A level/load recount is raised again; off restores the natural count; missing character needs no stale-object write. |

### Combat

Behavior reference: [combat.h](../../../EterrasGift/native/hooks/combat.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `no-cooldowns` — `ChargeManager.useCharges` | For the player, keep up to 64 charges, call original with zero amount, then raise lowered charges back if the backing array is still the same. Keep the original shared-cooldown behavior around that adjustment. | Bounded list access. Extract snapshot mechanics only after another matching consumer; policy remains local. | Shared cooldown decreases are restored; increased charges and replaced arrays are preserved; define the original-throw path explicitly. |
| Todo | `extra-projectiles` — `ExtraProjectiles.onCreation` and `GetTotalExtraProjectiles` | Mark the locally created extras object per thread while original creation runs; restore the outer mark on return/unwind. Scale the marked object's positive returned count, rounded and capped at one million. | Multi-target ownership; small scoped TLS pattern, later shared where appropriate. | Nested extras restore the outer object; unrelated/thread-concurrent queries unchanged; exception clears context. |
| Todo | `buffs-never-expire` — `StatBuffs.OnUpdateTick` and `AilmentReceiver.OnUpdateTick` | Pass zero time for player buffs. For positive player ailments, keep up to 64 durations, compensate their calculated tick loss before original, then restore durations of surviving entries. Negative ailments and damage/healing must keep ticking. | Bounded collection access and two-target lifecycle. No generic 'skip tick' handler. | Positive/negative mix, local/global speed modifiers, removed entries, more than 64 ailments and original throwing. |
| Todo | `auto-aim` — base and Fireball `getProjectileAutoAim`, `getNewTargetPositionWithAutoTarget`, `Alignment.nearestActorInRange` | Implement player ownership and exclusions for minion, movement, traversal and transform abilities; return VeryHigh aim, pass 360-degree angle, and maximum range in the respective paths. Fireball needs its own override. The constructor method returns Vector3 through hidden storage. | Four-target ownership, enum/field discovery and proven aggregate ABI support. Keep targeting decisions explicit. | All exclusions, Fireball override, creator fallback, nonplayer target and exact Vector3 return-buffer behavior. |

### Monolith

Behavior reference: [monolith.h](../../../EterrasGift/native/hooks/monolith.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `skip-quest-echoes` — `MonolithTimelinePanelManager.SetRunInfo` | Raise run quest completion to at least two and stability to at least the selected difficulty's maximum before original. Validate difficulty indexing. Preserve the original trigger on showing run info; changes belong to the saved run and are not undone on off. | Object/list reads and field stores. | Lower values raised, higher values preserved, invalid difficulty safe fallback, opening the panel triggers original once. |
| Todo | `cycle-echo-types` — `getIslandTypeForNewIsland`, `CalculateModAndZoneForIsland`, `CanHaveSubsequentConnectionsRevealed`, `OnIslandCompleted` table | Cycle the seven original type values; pass tier at least 19 for the related generation calls using per-thread web/tier context; permit subsequent connections; redirect normal/arena completion table entries to Super Beacon. Share type target with echo-type. Restore the table on off. | Shared/multiple target ownership, scoped context, atomic cycle index and checked data patching. Keep the cycle and table decoding specific. | Sequence/wrap, unrelated web, explicit-type precedence, normal/arena reveal behavior, table restoration and partial installation failure. |

### Factions

Behavior reference: [factions.h](../../../EterrasGift/native/hooks/factions.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `prophecy-uses` — `LE.Factions.ProphecySlot.TryTriggerReward` | Save completed charges; call original with the actual out parameter; after a successful trigger restore only charges that decreased. Keep original return and `chargesConsumed` output. | Small snapshot mechanics if they match another feature. Success-dependent restoration remains specific. | Failed trigger, increased charges, multiple consumed charges, exact out value; explicitly decide the original-throw case. |
| Todo | `prophecy-max-lp` — `ProphecySlotReward.SpawnRewardForPlayer` | Temporarily raise generic reward minimum LP to at least four, call original, restore the previous asset value even on native unwind. Share handler with reward multiplier; prove the eight-byte Nullable lens ABI. | Shared target; scoped restoration mechanism established with this family. | Shared asset remains pristine after normal/throwing calls; existing LP above four preserved; absent generic item. |
| Todo | `prophecy-reward-multiplier` — same `SpawnRewardForPlayer` | Temporarily scale `itemsDropped`, rounded and clamped to 0..1000, then restore. Preserve downstream lucky-lens doubling and composition with max LP. Shared reward assets must not retain repeated multiplication. | Same target and restoration record as max LP; settings. | Repeated calls do not compound; both features together; nested/concurrent calls require an explicit target-threading contract. |

### World

Behavior reference: [world.h](../../../EterrasGift/native/hooks/world.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `morditas-timer` — `RageOfMorditasLogic.OnEnter_HuntState`, `FixedUpdate_Hunt`, `OnActorDiedDuringHunt` | Snapshot times, call original, scale only that call's changed initial duration, positive elapsed delta while still hunting, or positive kill bonus. Do not revive a hunt ended by original. Three settings and targets compose. | The existing coherent settings snapshot and multi-target ownership; field descriptors. | Secondary hunt with unchanged length, ending tick, zero/negative deltas, kill popup timing and off mid-hunt without rewinding elapsed time. |
| Todo | `camera-distance` — `CameraManager.ApplyZoom` | Write configured distance before original. Keep the game's distance per camera, adopt external distance changes as the new baseline, and restore only if current value still equals our last write. Apply immediately on toggle through the main thread where available. | Lifecycle cleanup entry point; conditional restoration idea from existing writes. Keep camera state/refresh local. | Camera replacement, user changing game setting while on, off after foreign change, menu departure and cleanup before dispatcher removal. |
| Todo | `pack-count` — `SpawnerPlacementRoom.rollSpawners` | Temporarily set density modifier to `(1 + old) * multiplier - 1`, invoke original, restore even on unwind. Validate the small by-value two-bool configuration struct. Retain the game's placement limit. | Scoped restoration after the prophecy family proves it; scalar settings. | Original modifier restored, not compounded; exception, nested call and configuration bytes; multiplier does not bypass room cap. |
| Todo | `pack-size` — `Spawner.GenerateEntitiesInternal` | Temporarily multiply `numberToSpawn` only when its original value exceeds one; preserve single boss/named spawns. Restore after original including unwind. Share with monster rarity. | Shared spawner target and restoration record. | One remains one; multi-pack scales; repeated/throwing calls do not retain modified count. |
| Todo | `monster-rarity` — same `GenerateEntitiesInternal` | Temporarily multiply common/magic/rare weightings by three coherent settings, call original, restore all weights. Compose with pack size in one handler. | Same target/restoration record; the existing coherent settings snapshot. | Each weighting, both features together, zero weights, repeated calls and foreign unwind restoration. |

## Direct field editors

The remaining editors use main-thread requests, fresh expected observations, conditional writes and independent
readback. Keep read/apply primitives generic and operation semantics local.
Attack rate and leech use their persistent hooks in `features/fixed_attack_rate.rs` and `features/fixed_leech_rate.rs`;
they do not need direct editors.

| Status | Field set | Remaining feature work and useful reuse |
|---|---|---|
| Todo | `points-earned` | Read/write passive and Weaver tree points through their getters/setters; validate UInt16 range and allocated-point floor. Reuse conditional field-write transport. Values are recalculated by progression and load. |
| Todo | `monolith-run` | Current/saved run lookup, corruption/Gaze of Orobyss/options-rerolled mutation, corruption recalculation, save and optional matching-panel refresh. Share write certainty, not the hook registry. |
| Todo | `faction-standing` | Rank/favor/reputation validation, joined-faction rules and required save/sync calls. Keep separate from GainFavor/GainReputation detours. |

The Forge item editor follows its own planner/private-preview path; its remaining work is in [PORT.md](../../PORT.md).

## Remaining work by reusable family

These groups identify the next useful extraction points; they do not
require building a universal handler/ABI/object framework first.

- **Existing gateway consumers:** prophecy uses fits the original-first gateway's invocation-local snapshot words;
  skip quest echoes uses before-original field adjustments. Neither needs scoped-context work.
- **Settings and numeric transforms:** Morditas/monster settings reuse the existing bounded settings snapshot and matching arithmetic,
  publication and owner lookup from implemented gains. Preserve each feature's original-call placement.
- **Further argument forwarding:** auto-aim needs its exact Vector3 hidden-result ABI. Existing argument hooks
  preserve their opaque aggregate inputs in place; they do not adapt hidden result storage.
- **Scoped context:** projectiles establish nesting/unwind behavior, while echo context spans calls.
  Keep each feature's cleanup policy explicit; the movement hook uses caller selection instead of a mark.
- **Temporary shared assets:** prophecy reward count/max LP, then density and pack size/rarity need explicit target
  serialization and restoration on native unwind. Cooldown/ailment/prophecy-charge throw behavior must be decided
  against the original's normal-return-only restoration rather than silently promising universal rollback.
- **Echo cycling:** still needs its dynamic completion table, scoped web/tier context and shared echo-type target,
  composing feature-specific behavior with the existing entry/patch lifecycle.
- **Feature-specific cleanup:** specialization-slot recount and camera distance restoration require main-thread work before the
  dispatcher leaves. Persistent inventory/run changes and temporary overrides have different off semantics.

## Rules for a new hook

Every supported feature needs its behavior/exclusions, exact original-call count, on/change/off, independent settings
and byte readback, manager-loss and unload checks as relevant. Validate the largest actual signature; test original
exceptions without retry. A Rust panic catch is not a foreign-exception or memory-fault recovery mechanism. The existing
coherent settings use atomic payload words and a version-checked snapshot; readers skip a changed or paused writer
instead of waiting.
