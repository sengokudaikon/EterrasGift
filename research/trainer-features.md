# Trainer: what is left to build

The backlog of trainer features not built yet, and what each needs. A feature leaves this file when it lands: its
row is deleted, never marked done. What landed, in brief, is in [CHANGELOG.md](../CHANGELOG.md); how a built feature
works is in its module's `//!` docs under `crates/trainer-helper/src/game/features/`. The C# implementation in
`../EterrasGift` is the behaviour reference; runtime assumptions stay in [trainer-interop.md](trainer-interop.md).

**Confirmed in the live game:** Mysterious Rifts selection and Any item in the offhand (the user, 2026-10-27). Other
built hooks are covered by fixtures and static inspection only. Record a new live check here, with the exact
combinations checked.

**Names (2026-10-29):** each feature has one name. The ID uses its title's words, and the wire variant, helper group
and module are spelled from the ID. The C# IDs (`cast-speed`, `offhand`, …) are read through `RENAMED` in
[profiles](../crates/trainer/src/features/profiles.rs) and never written back. Game nouns follow the game's own text
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
| Todo | Mixed activation as one complete feature | Experimental uniques, cache use, waypoints and echo cycle still need their feature-specific detours/table behavior and partial-failure policy. |
| Todo | New coherent multi-value settings | First consumers: experience multiplier/level up on every gain, faction gains or drop rates. Existing publication covers only the implemented shapes. |
| Todo | Scoped context | Projectiles, then duplicate; movement consumes one matching read and needs its own policy. |
| Todo | Temporary asset mutation/restoration | Prophecy rewards/max LP, then density and pack size/rarity; settle threading and native-unwind behavior with those consumers. |
| Todo | Additional ABI/caller adapters | Validate indirect aggregates, long signatures, hidden result storage and champion caller identity when their features arrive. |

## Hooks to build

Each row retains the concrete behavior, useful shared mechanism and distinguishing targeted check. Common on/off,
readback, manager-loss and unload requirements apply to every implemented feature. Value ranges/arity remain owned
by [the catalog](../crates/trainer/src/features/registry.rs); native support is declared by
[GameplayHook](../crates/trainer-wire/src/hooks.rs) and resolved in
[helper features](../crates/trainer-helper/src/game/features/registry.rs).

### Character

Behavior reference: [character.h](../../EterrasGift/native/hooks/character.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `specialization-slots` — `PlayerFinder.getPlayerActor` | Call original, raise the returned actor's `localTreeData.numberOfUnlockedSlots` to at least five. On disabling, recount through `LocalTreeData.updateUnlockedSlots` on the main thread when a character still exists. Avoid the folded byte getter. | Detour lifecycle; object descriptors. Keep recount and its timing feature-specific. | A level/load recount is raised again; off restores the natural count; missing character needs no stale-object write. |
| Todo | `several-primordials` — `ItemContainersManager.NumberOfEquippedPrimordialItems` | Return zero only for the local containers manager; forward other managers. Resolve the local identity outside metadata discovery in the hot path. | Local-identity publication/lookup where its freshness contract fits; integer-return detour. | Another manager keeps its count; replacement of the local manager invalidates the old identity. |
| Todo | `experience-multiplier` — `ExperienceTracker.GainExp` | Scale positive character and ability XP separately for the player's tracker; round and cap as the original does, preserving gains already above `INT32_MAX`. Forward favor XP unchanged. Share this handler with instant level. | Settings pair; shared-target ownership; numeric conversion helpers only where semantics match. | Independent multipliers, nonpositive gains, large gains, nonplayer tracker and simultaneous instant level. |
| Todo | `level-up-per-gain` — same `GainExp` | Read current and next-level XP on the player's tracker. A positive character gain becomes at least the remaining requirement. This wins over the character XP multiplier; the skill multiplier still applies. | Same target and owner lookup as XP multiplier. Keep precedence in this handler. | Zero gain stays zero; character multiplier loses precedence; skill XP remains scaled. |

### Combat

Behavior reference: [combat.h](../../EterrasGift/native/hooks/combat.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `refill-mana` — `BaseMana.OnUpdateTick` | Match the player's mana component, store its current mana from its maximum before the original tick. Preserve the original tick; this is not a promise that every later instant has full mana. | Player/component lookup; field descriptors; mixed float/void gateway. | Player starts tick full, enemy unchanged, maximum changes followed; original tick still runs. |
| Todo | `fixed-area` — `Stats.GetIncreasedAreaForAreaSkill` | Return the configured increased-area value for the player's stats, skipping original for that case. Other stats keep original calculation. | Settings and player stats identity; float return. | Player versus minion/monster stats, original call count, changed player identity. |
| Todo | `no-cooldowns` — `ChargeManager.useCharges` | For the player, keep up to 64 charges, call original with zero amount, then raise lowered charges back if the backing array is still the same. Keep the original shared-cooldown behavior around that adjustment. | Bounded list access. Extract snapshot mechanics only after another matching consumer; policy remains local. | Shared cooldown decreases are restored; increased charges and replaced arrays are preserved; define the original-throw path explicitly. |
| Todo | `extra-projectiles` — `ExtraProjectiles.onCreation` and `GetTotalExtraProjectiles` | Mark the locally created extras object per thread while original creation runs; restore the outer mark on return/unwind. Scale the marked object's positive returned count, rounded and capped at one million. | Multi-target ownership; small scoped TLS pattern, later shared with duplicate where appropriate. | Nested extras restore the outer object; unrelated/thread-concurrent queries unchanged; exception clears context. |
| Todo | `buffs-never-expire` — `StatBuffs.OnUpdateTick` and `AilmentReceiver.OnUpdateTick` | Pass zero time for player buffs. For positive player ailments, keep up to 64 durations, compensate their calculated tick loss before original, then restore durations of surviving entries. Negative ailments and damage/healing must keep ticking. | Bounded collection access and two-target lifecycle. No generic 'skip tick' handler. | Positive/negative mix, local/global speed modifiers, removed entries, more than 64 ailments and original throwing. |
| Todo | `movement-speed` — `SpeedManager.updateSpeed` and `Stats.GetStatValue` | Mark player stats during speed update. Multiply only the first matching Movespeed query, consuming the mark before calling original. Resolve enum value and forward all nine managed arguments. Preserve the original source's clear-on-exit semantics; review nested updates explicitly. | Scoped context mechanics; long mixed ABI forwarding; multi-target ownership. | Other stats/properties and subsequent reads unaffected; nested update/exception leaves no stale mark. |
| Todo | `auto-aim` — base and Fireball `getProjectileAutoAim`, `getNewTargetPositionWithAutoTarget`, `Alignment.nearestActorInRange` | Implement player ownership and exclusions for minion, movement, traversal and transform abilities; return VeryHigh aim, pass 360-degree angle, and maximum range in the respective paths. Fireball needs its own override. The constructor method returns Vector3 through hidden storage. | Four-target ownership, enum/field discovery and proven aggregate ABI support. Keep targeting decisions explicit. | All exclusions, Fireball override, creator fallback, nonplayer target and exact Vector3 return-buffer behavior. |

### Stash

Behavior reference: [stash.h](../../EterrasGift/native/hooks/stash.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `duplicate-on-drop` — `ItemContainersManager.AttemptItemDropInWorld` and `DropItem` | Maintain a per-thread nesting count during local-player drop attempts, with unwind cleanup. Inside that scope, double ordinary positive item amounts; keys and woven echoes use the floored configured factor, at least one. Cap at `INT32_MAX`. Existing duplicates remain after off. | Two-target ownership; scoped TLS mechanics from projectiles; item-type descriptors. | Nested and failed attempts, another thread, ordinary/key/echo types, zero amount and overflow; no duplication outside player-drop scope. |

### Loot

Behavior reference: [loot.h](../../EterrasGift/native/hooks/loot.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `drop-rates` — `DeathItemDrop.OnDeath` | Before original, write item/gold override flags, chances clamped to 0..1 and nonnegative multipliers on the passed drop component. Keep those writes because delayed drop work reads them later. | Shared target with extra rares; coherent four-value settings; field descriptors. | Item/gold settings remain coherent; a delayed original continuation sees the changed component; off does not retroactively undo drops. |
| Todo | `extra-rare` — same `OnDeath` | Set `guaranteedAdditionalRare` before original on the passed component. Leave it on that dying enemy as the original app does. | Death-drop target and descriptors. | Works independently of drop rates; disabling one keeps the other's target installed; original executes once. |
| Todo | `new-item-rarity` — `GenerateItems.RollBaseItem` | Replace only a caller-supplied rarity of -1 with the selected catalog mapping 0/1/3/7/8. Forward 21 managed parameters, receiver and MethodInfo, including reference/aggregate payloads with validated ABI. Explicitly requested rarity wins. | Long-stack forwarding once proven, settings choice validation. | All 23 native slots survive; explicit unique/blessing/gambler rarity unchanged; only -1 is replaced. |

### Monolith

Behavior reference: [monolith.h](../../EterrasGift/native/hooks/monolith.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `skip-quest-echoes` — `MonolithTimelinePanelManager.SetRunInfo` | Raise run quest completion to at least two and stability to at least the selected difficulty's maximum before original. Validate difficulty indexing. Preserve the original trigger on showing run info; changes belong to the saved run and are not undone on off. | Object/list reads and field stores. | Lower values raised, higher values preserved, invalid difficulty safe fallback, opening the panel triggers original once. |
| Todo | `cycle-echo-types` — `getIslandTypeForNewIsland`, `CalculateModAndZoneForIsland`, `CanHaveSubsequentConnectionsRevealed`, `OnIslandCompleted` table | Cycle the seven original type values; pass tier at least 19 for the related generation calls using per-thread web/tier context; permit subsequent connections; redirect normal/arena completion table entries to Super Beacon. Share type target with echo-type. Restore the table on off. | Shared/multiple target ownership, scoped context, atomic cycle index and checked data patching. Keep the cycle and table decoding specific. | Sequence/wrap, unrelated web, explicit-type precedence, normal/arena reveal behavior, table restoration and partial installation failure. |
| Todo | `reveal-objective` — `LE.Gameplay.Monolith.Frontend.MonolithPulse.OnScoreChanged` | Raise incoming score to at least the pulse's threshold before original. Preserve its existing score-change/first-kill trigger; do not add a global objective scan. | Float argument adjustment and object field read. | Below/above threshold, original last-kill argument, next score event after activation; previously revealed objective stays revealed. |

### Factions

Behavior reference: [factions.h](../../EterrasGift/native/hooks/factions.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `faction-gains` — `LE.Factions.Faction.GainFavor` | Multiply positive favor only when `ignoreMultiplier` is false; round/clamp to nonnegative `Int32`. Let original handle joined faction, rank scaling, favor cap and resulting reputation. | Settings, integer argument transformation, proven rounding helper. | Ignore flags, nonpositive gains, max bound and resulting reputation; no duplicate reputation multiplication. |
| Todo | `rank-up-per-gain` — `LE.Factions.Faction.GainReputation` | For positive gain, read rank requirements and current reputation; replace gain with the positive remainder to the next rank. Forward unchanged when unavailable or not positive. This can reduce a gain that would otherwise cover multiple ranks. | Bounded array/object reads; same arithmetic helpers only if their semantics fit. | Invalid/top rank, missing requirement, remainder <=0, large original gain; exactly the source's replacement behavior. |
| Todo | `prophecy-any-distance` — `LE.Factions.CircleOfFortune.IsPlayerInRangeOf` | Return true when active; otherwise call original. This is combat prophecy distance, not removal of a zone requirement (the C# ID was `prophecy-any-zone`). Position is an aggregate passed indirectly and must be validated. | Boolean-return lifecycle once the indirect value argument is checked. | Out-of-range completion succeeds, original skipped only on, position forwarded correctly off. |
| Todo | `prophecy-uses` — `LE.Factions.ProphecySlot.TryTriggerReward` | Save completed charges; call original with the actual out parameter; after a successful trigger restore only charges that decreased. Keep original return and `chargesConsumed` output. | Small snapshot mechanics if they match another feature. Success-dependent restoration remains specific. | Failed trigger, increased charges, multiple consumed charges, exact out value; explicitly decide the original-throw case. |
| Todo | `prophecy-max-lp` — `ProphecySlotReward.SpawnRewardForPlayer` | Temporarily raise generic reward minimum LP to at least four, call original, restore the previous asset value even on native unwind. Share handler with reward multiplier; prove the eight-byte Nullable lens ABI. | Shared target; scoped restoration mechanism established with this family. | Shared asset remains pristine after normal/throwing calls; existing LP above four preserved; absent generic item. |
| Todo | `prophecy-reward-multiplier` — same `SpawnRewardForPlayer` | Temporarily scale `itemsDropped`, rounded and clamped to 0..1000, then restore. Preserve downstream lucky-lens doubling and composition with max LP. Shared reward assets must not retain repeated multiplication. | Same target and restoration record as max LP; settings. | Repeated calls do not compound; both features together; nested/concurrent calls require an explicit target-threading contract. |

### World

Behavior reference: [world.h](../../EterrasGift/native/hooks/world.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `morditas-timer` — `RageOfMorditasLogic.OnEnter_HuntState`, `FixedUpdate_Hunt`, `OnActorDiedDuringHunt` | Snapshot times, call original, scale only that call's changed initial duration, positive elapsed delta while still hunting, or positive kill bonus. Do not revive a hunt ended by original. Three settings and targets compose. | Coherent settings triple and multi-target ownership; field descriptors. | Secondary hunt with unchanged length, ending tick, zero/negative deltas, kill popup timing and off mid-hunt without rewinding elapsed time. |
| Todo | `eternity-cache-anywhere` — `ActorCanUseEternityCacheInThisZone`, `Item.getMaxUniqueLevelRequirementForDungeonTier`, `ActorUsedEternityCache` patch | Permit the local actor, pass `INT32_MAX` tier into original level-cap calculation, suppress the final local-used store. The actor-check method's source ABI is static actor plus MethodInfo, not an instance receiver plus actor. | Mixed detour/patch ownership; identity lookup; static integer signature. | Other actor unchanged; deepest tier not lowered; used store suppressed; removing the patch and both detours is reported together. |
| Todo | `camera-distance` — `CameraManager.ApplyZoom` | Write configured distance before original. Keep the game's distance per camera, adopt external distance changes as the new baseline, and restore only if current value still equals our last write. Apply immediately on toggle through the main thread where available. | Lifecycle cleanup entry point; conditional restoration idea from existing writes. Keep camera state/refresh local. | Camera replacement, user changing game setting while on, off after foreign change, menu departure and cleanup before dispatcher removal. |
| Todo | `reveal-map` — `LE.UI.Minimap.Minimap.UpdateFoW_Singular` | Raise radius to at least 1000 and call original. Forward indirect float3 position exactly. Disabling does not hide already revealed map. | Mixed scalar forwarding and argument adjustment after aggregate validation. | Larger radius preserved; exact position forwarded; existing reveal persists after off. |
| Todo | `any-waypoint` — `MapPanel.SelectLocation` and `UIWaypointStandard.LoadWaypointScene` | For an inactive eligible waypoint, pass both availability flags true; retain the original seven-character/no-waypoint exclusion. Patch both travel rejection branches. Do not blindly unlock every scene. | Mixed target/patch ownership; object/string-length reads and long scalar ABI. | Inactive/active, missing waypoint and excluded scene; selection and travel agree; one travel-site failure cannot report full support. |
| Todo | `champions` — `RngElement.RollRangeLucky` call resolved from `RandomChampionSpawner.ExecuteSpawn` | Return configured count only for the checked champion call site; other RNG users call original. Preserve the caller-return-address distinction across the new gateway, or prove a different scoped implementation. Original game placement caps still apply. | Pattern/call-target validation and detour lifecycle. Caller context is a special adapter, not a requirement for every handler. | Two callers of the same RNG method, one champion and one unrelated; gateway must not turn the caller address into its own return site. |
| Todo | `loot-lizards` — `RandomEncounterPlacer.HandleEncountersCalculations` | Keep context and pre-call done flag; after original, set total/remaining/count-to-rate multiplier only when this call completed lizard calculations on the same context. Use 0.0001 as quotient for zero spawn rate. | Shared encounter target and typed tally helper with Rune Wardens. | Already-calculated context not refilled; replacement context untouched; zero rate; per-feature independence. |
| Todo | `rune-wardens` — same `HandleEncountersCalculations` | Apply the equivalent logic to the Warden fields, preserving encounters already consumed. | Same target and tally implementation, parameterized by the checked field set. | Lizards off/Wardens on and the reverse; completed state and existing remaining count are preserved. |
| Todo | `experimental-uniques` — `GetExperimentalUniqueDropChance` plus `SpawnExperimentalItem` fail-to-drop patch | Return 1 from the chance method while on and remove the later failure branch. Preserve item-level eligibility. Install/remove as a mixed unit even though the original packs both into one owner. | Mixed target/patch coordination, shared-method range checks. | Chance detour alone is insufficient; injected failure restores prior state or reports uncertainty; above-level unique still does not drop. |
| Todo | `pack-count` — `SpawnerPlacementRoom.rollSpawners` | Temporarily set density modifier to `(1 + old) * multiplier - 1`, invoke original, restore even on unwind. Validate the small by-value two-bool configuration struct. Retain the game's placement limit. | Scoped restoration after the prophecy family proves it; scalar settings. | Original modifier restored, not compounded; exception, nested call and configuration bytes; multiplier does not bypass room cap. |
| Todo | `pack-size` — `Spawner.GenerateEntitiesInternal` | Temporarily multiply `numberToSpawn` only when its original value exceeds one; preserve single boss/named spawns. Restore after original including unwind. Share with monster rarity. | Shared spawner target and restoration record. | One remains one; multi-pack scales; repeated/throwing calls do not retain modified count. |
| Todo | `monster-rarity` — same `GenerateEntitiesInternal` | Temporarily multiply common/magic/rare weightings by three coherent settings, call original, restore all weights. Compose with pack size in one handler. | Same target/restoration record; settings triple. | Each weighting, both features together, zero weights, repeated calls and foreign unwind restoration. |

## Direct field editors

None is built. They use main-thread requests, fresh expected observations, conditional writes and independent
readback. Keep read/apply primitives generic and operation semantics local.
The retired attack/leech editor is covered by the two persistent hooks above.

| Status | Field set | Remaining feature work and useful reuse |
|---|---|---|
| Todo | `points-earned` | Read/write passive and Weaver tree points through their getters/setters; validate UInt16 range and allocated-point floor. Reuse conditional field-write transport. Values are recalculated by progression and load. |
| Todo | `currency` | Edit gold, soul embers and ancient bones by deltas through `modifyGold`, `modifySoulEmberCountOnSingleplayerOrServer` and `modifyAncientBones`. Preserve refusals, display updates and saves; share conditional write transport rather than a generic raw numeric store. |
| Todo | `keys` | Read dynamic stash entries and change quantities using the game's remove/set paths, respecting removal permission. Share the held-item request machinery with woven echoes. |
| Todo | `woven-echoes` | Same held-item traversal and quantity workflow with its own container getter and labels. Preserve entry identity and deletion behavior. |
| Todo | `monolith-run` | Current/saved run lookup, corruption/Gaze of Orobyss/options-rerolled mutation, corruption recalculation, save and optional matching-panel refresh. Share write certainty, not the hook registry. |
| Todo | `faction-standing` | Rank/favor/reputation validation, joined-faction rules and required save/sync calls. Keep separate from GainFavor/GainReputation detours. |

The Forge item editor follows its own planner/private-preview path; its remaining work is in [PORT.md](../PORT.md).

## Remaining work by reusable family

These groups identify the next useful extraction points; they do not
require building a universal handler/ABI/object framework first.

- **Settings and numeric transforms:** Experience multiplier/Level up on every gain share a target and precedence; faction
  gains and fixed area reuse only matching arithmetic/owner lookup. Preserve independent original-call placement.
- **Further argument forwarding:** new item rarity needs its full long signature; map/prophecy distance need validated
  indirect aggregates. The current scalar-result family is implemented; it does not imply support for hidden results.
- **Scoped context:** projectiles then duplicate establish nesting/unwind behavior; movement consumes a mark,
  while echo context spans calls. Do not give all three the same cleanup policy.
- **Temporary shared assets:** prophecy reward count/max LP, then density and pack size/rarity need explicit target
  serialization and restoration on native unwind. Cooldown/ailment/prophecy-charge throw behavior must be decided
  against the original's normal-return-only restoration rather than silently promising universal rollback.
- **Mixed features:** experimental uniques, Eternity Cache anywhere and any waypoint need their complete detour
  behavior alongside checked ranges; echo cycling also needs its dynamic completion table and shared echo-type target.
- **Feature-specific cleanup:** specialization-slot recount and camera distance restoration require main-thread work before the
  dispatcher leaves. Persistent inventory/run changes and temporary overrides have different off semantics.

## Rules for a new hook

Every supported feature needs its behavior/exclusions, exact original-call count, on/change/off, independent settings
and byte readback, manager-loss and unload checks as relevant. Validate the largest actual signature; test original
exceptions without retry. A Rust panic catch is not a foreign-exception or memory-fault recovery mechanism. Future
coherent multi-value settings need an actual atomic protocol; a version counter does not make plain concurrent
payload accesses safe, and readers must not wait for a paused writer.
