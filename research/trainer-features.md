# Trainer features

Current implementation checklist, updated 2026-10-29. Maintain feature status and current behavior here when a
feature changes. The C# implementation in `../EterrasGift` is the behavior reference; intentional departures are
recorded below. Runtime assumptions stay in [trainer-interop.md](trainer-interop.md), and broader Forge/bootstrap
shipping scope stays in [trainer-readiness.md](trainer-readiness.md).

- **Done:** the native feature, shared lifecycle, worker/profile commands, independent readback and UI control exist.
- **Todo:** the complete feature is unavailable; existing shared support does not mark it done.
- **Live validation:** separate from implementation. The user confirmed mysterious-rift selection and offhand in
  Last Epoch on 2026-10-27. The other implemented hooks still need live checks; fixtures/static inspection are not
  live proof. Exact combinations covered by those two user checks were not recorded.

**Hooks: 26 Done / 42 Todo (68 current controls). Direct field editors: 0 Done / 6 Todo.** God mode and Damage taken
are combined as `incoming-damage`; one-hit kills are the ∞ endpoint of `damage-dealt`. The former `attack-leech`
Read/Apply editor is retired in favor of independent persistent `attack-rate` and `leech-rate` hooks.

The former feature-plan, first-hook and code-patch notes are preserved in
`../EterrasGiftRS-archive/2026-10-29-trainer-features/`; this file replaces their active tracking.

## Page layout

Character, Stash, Combat, Loot, Monolith, Factions and World use one combined list with section headings, using the
existing Character page layout and responsive column splitting. Forge keeps its Item / Crafting rules tabs.
World's groups are Monsters, Encounters, then Dungeons and map. Champions belongs to Monsters. This is presentation
only; feature IDs/profile settings and the original gameplay behavior remain independent of where rows appear.

## Shared foundation

| Status | Support | Current scope / next consumer |
|---|---|---|
| Done | Helper-owned feature/target ownership | Each feature owns its targets/contracts/semantics; damage controls share one group. The wire carries IDs and settings, not native topology. |
| Done | Retained leaves and lifecycle | Prepare before publication, close before changes, verify actual entries, restore on manager loss/departure and before unload. Generated gameplay paths do not enter helper code. |
| Done | Persistent scalar overrides | Selected independent accessors, pinned receiver, recalculation baseline and conditional restoration; attack and leech are the consumers. |
| Done | Float-result postfix gateway | No mana cost preserves the exact original call and its stack arguments, then adjusts the returned float. |
| Done | Interior code-patch groups | 17 features / 27 owned ranges; bounded unique patterns, grouped transaction/recovery and actual-byte readback. |
| Done | Material replenishment leaves | Shards and runes/glyphs mutate stored quantities before the original query; shared online/closed guard and field contracts. |
| Done | Worker/profile/UI integration | Generic Boolean controls, typed values, separate readback and desired-versus-observed state. |
| Todo | Mixed activation as one complete feature | Experimental uniques, cache use, waypoints and echo cycle still need their feature-specific detours/table behavior and partial-failure policy. |
| Todo | New coherent multi-value settings | First consumers: XP multiplier/instant level, faction gains or drop rates. Existing publication covers only the implemented shapes. |
| Todo | Scoped context | Projectiles, then duplicate; movement consumes one matching read and needs its own policy. |
| Todo | Temporary asset mutation/restoration | Prophecy rewards/max LP, then density and pack size/rarity; settle threading and native-unwind behavior with those consumers. |
| Todo | Additional ABI/caller adapters | Validate indirect aggregates, long signatures, hidden result storage and champion caller identity when their features arrive. |

## Hook checklist

Each row retains the concrete behavior, useful shared mechanism and distinguishing targeted check. Common on/off,
readback, manager-loss and unload requirements apply to every implemented feature. Value ranges/arity remain owned
by [the catalog](../crates/trainer/src/features/registry.rs); native support is declared by
[GameplayHook](../crates/trainer-wire/src/hooks.rs) and resolved in
[helper features](../crates/trainer-helper/src/game/features/hooks.rs).

### Character

Behavior reference: [character.h](../../EterrasGift/native/hooks/character.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `skill-slots` — `PlayerFinder.getPlayerActor` | Call original, raise the returned actor's `localTreeData.numberOfUnlockedSlots` to at least five. On disabling, recount through `LocalTreeData.updateUnlockedSlots` on the main thread when a character still exists. Avoid the folded byte getter. | Detour lifecycle; object descriptors. Keep recount and its timing feature-specific. | A level/load recount is raised again; off restores the natural count; missing character needs no stale-object write. |
| Done | `offhand` — four offhand type/pairing targets | Accept any offhand category and bypass two-handed/linked-slot pairings and automatic displacement. Keep the item's own requirements; off does not force unequip or a stats refresh. See [offhand details](#offhand). | Four retained Boolean/void leaves sharing one switch; exact receiver/linked-slot guards. | Placement and equipped eligibility agree; both pairing directions work; unmet requirements and unrelated slots remain unchanged. |
| Todo | `primordial-limit` — `ItemContainersManager.NumberOfEquippedPrimordialItems` | Return zero only for the local containers manager; forward other managers. Resolve the local identity outside metadata discovery in the hot path. | Local-identity publication/lookup where its freshness contract fits; integer-return detour. | Another manager keeps its count; replacement of the local manager invalidates the old identity. |
| Todo | `xp-multiplier` — `ExperienceTracker.GainExp` | Scale positive character and ability XP separately for the player's tracker; round and cap as the original does, preserving gains already above `INT32_MAX`. Forward favor XP unchanged. Share this handler with instant level. | Settings pair; shared-target ownership; numeric conversion helpers only where semantics match. | Independent multipliers, nonpositive gains, large gains, nonplayer tracker and simultaneous instant level. |
| Todo | `instant-level` — same `GainExp` | Read current and next-level XP on the player's tracker. A positive character gain becomes at least the remaining requirement. This wins over the character XP multiplier; the skill multiplier still applies. | Same target and owner lookup as XP multiplier. Keep precedence in this handler. | Zero gain stays zero; character multiplier loses precedence; skill XP remains scaled. |
| Done | `max-skill-level` — `LocalTreeData.ApplyAbilityXp` | Patch the conditional jump after adding skill XP so the existing loop uses the level-20 XP cap. Disabling restores bytes, not earned skill XP. | Patch groups. | Positive gain follows the cap path on and the original conditional off; other loop logic remains intact. |
| Done | `item-requirements` — `OneSlotItemContainerWithCharacterRequirements.CanReceiveItem` | Remove the class, faction and level rejection branches as one group. Preserve slot compatibility and primordial checks. | Three-range patch group. | Each intended restriction bypasses; incompatible slot and primordial limit still refuse. |
| Done | `set-bonus` — `ItemEquipManager.UpdateStats` | Remove the branch skipping a set bonus whose piece requirement exceeds the number worn. Applies during the game's next stats recalculation; no immediate recalculation is performed by the original toggle. | Patch groups. | One worn set piece enables its higher bonuses during recalculation; an unworn set gains nothing. |
| Done | `free-skill-points` — `LocalTreeData.tryToSpendSkillPoint` | Remove the total-spent versus max-points refusal. Keep per-node maximums, connections and other requirements. Allocated points are not undone by disabling. | Patch groups. | Exhausted total points can spend; maxed or otherwise invalid nodes still refuse. |
| Done | `free-passive-points` — `LocalTreeData.tryToSpendPassivePoint` | Replace the virtual unspent-points call at its checked site with the original app's constant 113 result. Do not hook the general getter. | Patch ranges and shared-method non-overlap checks with `passive-chain`. | Only this spend path sees the constant; either feature can be removed while the other remains. |
| Done | `passive-chain` — same `tryToSpendPassivePoint` | Remove the chosen-mastery mismatch branch for deeper nodes. Preserve having a mastery, its point requirement and the node connection. | Patch groups; separate range ownership in a shared method. | Wrong mastery unlocks; absent mastery and insufficient prerequisite investment still fail. |
| Done | `free-weaver-points` — preview coroutine `MoveNext` and `LocalTreeData.TryToSpendWeaverPoint` | Resolve `TreeUIManager.TrySimulatePreviewPointSpentWeaverTree`'s state machine; remove its preview refusal/decrement and replace the spent-point sum in the real spend path. At least one earned Weaver point is still required. | Multi-method patch group; state-machine resolver only if reusable. | Preview and actual spend agree; zero earned points still refuses; one missing pattern changes no bytes. |

### Combat

Behavior reference: [combat.h](../../EterrasGift/native/hooks/combat.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Done | `incoming-damage` — `BaseHealth.HealthDamage` | Player damage taken at 0%–100%, default 50%; 0 replaces God mode. Call original once. Migrate legacy divisor/God-mode profiles. See [damage details](#damage-controls). | One owned damage target and independent settings shared with `damage-dealt`. | Player/enemy/ally filtering; 0/50/100%; changing or disabling either control preserves the sibling; both off restores the entry. |
| Done | `damage-dealt` — same `HealthDamage` | Multiply damage to player foes by 0–100×, or use the finite 101 endpoint displayed as ∞ to raise positive hits to current health. Call original once. See [damage details](#damage-controls). | Same damage target, alignment classification and per-feature settings. | Friendly/missing alignment, absent foe table, zero/negative hits; multiplier/one-hit transitions and last-owner disable. |
| Done | `attack-rate` — `CharacterStats.calculateAttackRate`, main/offhand setters | Persistent absolute attack-rate override (0.01–20) through its selected character attack accessor. Reapply after all three recalculation paths. See [scalar details](#persistent-attack-and-leech). | Selected stat reader/binder; pinned receiver; original-first scalar gateway and conditional baseline restoration. | Both setter paths, replacement receiver, online filtering, fifth native argument, value changes and delayed original crossing close. |
| Done | `leech-rate` — `BaseStats.ApplyExternalStats` | Independent persistent increased-leech-rate override (-1–100) through its own accessor, reapplied after external stats rebuild. See [scalar details](#persistent-attack-and-leech). | Same scalar lifecycle with a separate selector, receiver and setting. | Recalculation updates the baseline; attack/leech toggle independently; later game writes win during conditional restoration. |
| Todo | `infinite-mana` — `BaseMana.OnUpdateTick` | Match the player's mana component, store its current mana from its maximum before the original tick. Preserve the original tick; this is not a promise that every later instant has full mana. | Player/component lookup; field descriptors; mixed float/void gateway. | Player starts tick full, enemy unchanged, maximum changes followed; original tick still runs. |
| Todo | `area` — `Stats.GetIncreasedAreaForAreaSkill` | Return the configured increased-area value for the player's stats, skipping original for that case. Other stats keep original calculation. | Settings and player stats identity; float return. | Player versus minion/monster stats, original call count, changed player identity. |
| Todo | `no-cooldowns` — `ChargeManager.useCharges` | For the player, keep up to 64 charges, call original with zero amount, then raise lowered charges back if the backing array is still the same. Keep the original shared-cooldown behavior around that adjustment. | Bounded list access. Extract snapshot mechanics only after another matching consumer; policy remains local. | Shared cooldown decreases are restored; increased charges and replaced arrays are preserved; define the original-throw path explicitly. |
| Done | `zero-mana-cost` — exact Ability overload of `BaseMana.getManaCost` | Run the original cost calculation once, then answer zero only for the current player's mana component. Preserve natural mana regeneration/drain and cooldowns. See [mana details](#no-mana-cost). | Full-signature overload query; retained float-result postfix gateway; current player/online predicate. | All seven native argument positions, original side effects and exception, nonplayer/null/online filtering, late return after close. |
| Todo | `cast-speed` — `UsingAbility.getSpeedMultiplierStat` | Call original, identify player through state/controller/actor, multiply positive configured speed. Preserve abilities whose mutator fixes speed downstream. | Owner-read helpers and float result adjustment. | Enemy state untouched; valid player multiplier; fixed-speed ability still follows its mutator. |
| Todo | `projectiles` — `ExtraProjectiles.onCreation` and `GetTotalExtraProjectiles` | Mark the locally created extras object per thread while original creation runs; restore the outer mark on return/unwind. Scale the marked object's positive returned count, rounded and capped at one million. | Multi-target ownership; small scoped TLS pattern, later shared with duplicate where appropriate. | Nested extras restore the outer object; unrelated/thread-concurrent queries unchanged; exception clears context. |
| Todo | `endless-buffs` — `StatBuffs.OnUpdateTick` and `AilmentReceiver.OnUpdateTick` | Pass zero time for player buffs. For positive player ailments, keep up to 64 durations, compensate their calculated tick loss before original, then restore durations of surviving entries. Negative ailments and damage/healing must keep ticking. | Bounded collection access and two-target lifecycle. No generic 'skip tick' handler. | Positive/negative mix, local/global speed modifiers, removed entries, more than 64 ailments and original throwing. |
| Todo | `move-speed` — `SpeedManager.updateSpeed` and `Stats.GetStatValue` | Mark player stats during speed update. Multiply only the first matching Movespeed query, consuming the mark before calling original. Resolve enum value and forward all nine managed arguments. Preserve the original source's clear-on-exit semantics; review nested updates explicitly. | Scoped context mechanics; long mixed ABI forwarding; multi-target ownership. | Other stats/properties and subsequent reads unaffected; nested update/exception leaves no stale mark. |
| Todo | `auto-target` — base and Fireball `getProjectileAutoAim`, `getNewTargetPositionWithAutoTarget`, `Alignment.nearestActorInRange` | Implement player ownership and exclusions for minion, movement, traversal and transform abilities; return VeryHigh aim, pass 360-degree angle, and maximum range in the respective paths. Fireball needs its own override. The constructor method returns Vector3 through hidden storage. | Four-target ownership, enum/field discovery and proven aggregate ABI support. Keep targeting decisions explicit. | All exclusions, Fireball override, creator fallback, nonplayer target and exact Vector3 return-buffer behavior. |

### Stash

Behavior reference: [stash.h](../../EterrasGift/native/hooks/stash.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `duplicate` — `ItemContainersManager.AttemptItemDropInWorld` and `DropItem` | Maintain a per-thread nesting count during local-player drop attempts, with unwind cleanup. Inside that scope, double ordinary positive item amounts; keys and woven echoes use the floored configured factor, at least one. Cap at `INT32_MAX`. Existing duplicates remain after off. | Two-target ownership; scoped TLS mechanics from projectiles; item-type descriptors. | Nested and failed attempts, another thread, ordinary/key/echo types, zero amount and overflow; no duplication outside player-drop scope. |

### Forge

Behavior reference: [forge.h](../../EterrasGift/native/hooks/forge.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Done | `forge-level` — `CraftingManager.CheckForgeCapability` | Remove only the character-versus-item-level rejection branch. Preserve the rest of forging admission and side effects. | First simple consumer of patch groups. | Low level passes that check; unrelated forging restrictions still refuse; off restores the exact branch. |
| Done | `shards` — `GlobalDataTracker.GetShardCount` | Walk existing saved shard entries, raising quantities below 99 before original counts them. Null entries are skipped; traversal uses min(active size, array capacity), with capacity at most 65,536. Inventory is not undone on off. See [material details](#crafting-materials). | Bounded list/array access and scalar stores, later compared with runes/glyphs. | Quantities below/above 99, empty/null entries, count agrees with actual stored quantities. |
| Done | `runes-glyphs` — `OneSlotItemContainer.GetQuantity` | Follow content/data and raise quantity to 99 only for rune/glyph item types, then call original. Applies to one-slot containers wherever queried, including the Forge. Off stops replenishment without removing material. See [material details](#crafting-materials). | Item-entry descriptors and floor operation if also useful for shards. | Other item types untouched; already-higher counts preserved; stored quantity and returned quantity agree. |
| Done | `woven-enchanter` — both `ZoneRestrictedCraftingManager.OnUse` overloads | Remove the offline local-use increment in both bodies; preserve other station work and server-account branch behavior. Previous uses are not reset by toggling. | Multi-method patch group and overload validation. | Both overloads stop decrementing remaining uses; failure finding either site leaves neither installed. |

### Loot

Behavior reference: [loot.h](../../EterrasGift/native/hooks/loot.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `drop-rates` — `DeathItemDrop.OnDeath` | Before original, write item/gold override flags, chances clamped to 0..1 and nonnegative multipliers on the passed drop component. Keep those writes because delayed drop work reads them later. | Shared target with extra rares; coherent four-value settings; field descriptors. | Item/gold settings remain coherent; a delayed original continuation sees the changed component; off does not retroactively undo drops. |
| Todo | `extra-rares` — same `OnDeath` | Set `guaranteedAdditionalRare` before original on the passed component. Leave it on that dying enemy as the original app does. | Death-drop target and descriptors. | Works independently of drop rates; disabling one keeps the other's target installed; original executes once. |
| Todo | `boss-drops` — nested `BossLoot.LootGroup.GetDropChance` | Call original with all five managed arguments, then return 1 when active. Preserve original execution; this is the chance before downstream modifiers, not a universal final drop-count override. | Nested-class query; mixed float/integer stack forwarding; result adjustment. | Original called once; rarity/corruption and stack tier arguments forwarded; final result overridden only on. |
| Todo | `perfect-rolls` — `LE.Core.RngElement.RandomByte` | Call original to advance RNG, then return 255. Preserve the original app's broad scope: other uses of RandomByte are affected too. | Byte-return detour and result adjustment pattern. | RNG state advances identically on/off; return value is a byte; non-item RandomByte calls retain the same broad effect. |
| Todo | `force-rarity` — `GenerateItems.RollBaseItem` | Replace only a caller-supplied rarity of -1 with the selected catalog mapping 0/1/3/7/8. Forward 21 managed parameters, receiver and MethodInfo, including reference/aggregate payloads with validated ABI. Explicitly requested rarity wins. | Long-stack forwarding once proven, settings choice validation. | All 23 native slots survive; explicit unique/blessing/gambler rarity unchanged; only -1 is replaced. |
| Todo | `always-t7` — `GenerateItems.RollTier` | Let original consume its RNG, then return internal tier 6. Apply to newly generated affixes, not existing items or Forge edits; this differs from the older cheat-table RebuildID patch. | Long mixed-argument forwarding; result adjustment. | Original RNG advances; returned tier is 6; editing an existing item does not silently change all its tiers. |
| Done | `pickup-range` — `pickupItem`, `ClickedItem`, `vacuumNearbyStackableItems`, `pickupGold`, `PickupGroundObjectInteraction.initialise` | Patch four distance-refusal paths and set initial `canPickup` true for ground-object interaction. Resolve GroundItemList overloads. Preserve pickup delay and other admission rules. Previously collected items stay collected. | Five-site patch group and overload query. | Item click, materials, gold and delayed ground objects each exercise their own path; one missing site prevents a partial feature. |

### Monolith

Behavior reference: [monolith.h](../../EterrasGift/native/hooks/monolith.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Todo | `skip-quest-echoes` — `MonolithTimelinePanelManager.SetRunInfo` | Raise run quest completion to at least two and stability to at least the selected difficulty's maximum before original. Validate difficulty indexing. Preserve the original trigger on showing run info; changes belong to the saved run and are not undone on off. | Object/list reads and field stores. | Lower values raised, higher values preserved, invalid difficulty safe fallback, opening the panel triggers original once. |
| Todo | `max-bonus-stability` — `MonolithZoneManager.getBonusStabilityInRegularEcho` | Call original, then return at least `maxBonusStablity` when active. Preserve the game's first-kill/update trigger rather than adding an immediate write. | Integer result adjustment; one field descriptor. | Original higher result is preserved; first qualifying update reaches maximum; off resumes the game's calculation. |
| Todo | `echo-cycle` — `getIslandTypeForNewIsland`, `CalculateModAndZoneForIsland`, `CanHaveSubsequentConnectionsRevealed`, `OnIslandCompleted` table | Cycle the seven original type values; pass tier at least 19 for the related generation calls using per-thread web/tier context; permit subsequent connections; redirect normal/arena completion table entries to Super Beacon. Share type target with echo-type. Restore the table on off. | Shared/multiple target ownership, scoped context, atomic cycle index and checked data patching. Keep the cycle and table decoding specific. | Sequence/wrap, unrelated web, explicit-type precedence, normal/arena reveal behavior, table restoration and partial installation failure. |
| Todo | `echo-type` — same `getIslandTypeForNewIsland` | Map the selected catalog index to the original ten IslandType values and override the original result. Wins over cycling type, while the active cycle's tier and other effects remain. Original still runs. | Shared target with echo-cycle; settings choice validation. | Each index maps correctly; explicit type plus cycle retains tier behavior; removing either feature preserves the other. |
| Todo | `reveal-objective` — `LE.Gameplay.Monolith.Frontend.MonolithPulse.OnScoreChanged` | Raise incoming score to at least the pulse's threshold before original. Preserve its existing score-change/first-kill trigger; do not add a global objective scan. | Float argument adjustment and object field read. | Below/above threshold, original last-kill argument, next score event after activation; previously revealed objective stays revealed. |

### Factions

Behavior reference: [factions.h](../../EterrasGift/native/hooks/factions.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Done | `hunter-cost` — `HunterVendorContainer.CanRemoveItemPreCheck` | Remove the failure branch after the base purchase precheck. Preserve the original precheck execution and its other effects; do not replace the entire method with true. | Simple patch group. | The failed-payment branch is bypassed while the rest of the method still executes; original bytes return on off. |
| Todo | `faction-gains` — `LE.Factions.Faction.GainFavor` | Multiply positive favor only when `ignoreMultiplier` is false; round/clamp to nonnegative `Int32`. Let original handle joined faction, rank scaling, favor cap and resulting reputation. | Settings, integer argument transformation, proven rounding helper. | Ignore flags, nonpositive gains, max bound and resulting reputation; no duplicate reputation multiplication. |
| Todo | `instant-rank` — `LE.Factions.Faction.GainReputation` | For positive gain, read rank requirements and current reputation; replace gain with the positive remainder to the next rank. Forward unchanged when unavailable or not positive. This can reduce a gain that would otherwise cover multiple ranks. | Bounded array/object reads; same arithmetic helpers only if their semantics fit. | Invalid/top rank, missing requirement, remainder <=0, large original gain; exactly the source's replacement behavior. |
| Todo | `prophecy-any-zone` — `LE.Factions.CircleOfFortune.IsPlayerInRangeOf` | Return true when active; otherwise call original. Despite the legacy ID, current meaning is combat prophecy distance, not removal of a zone requirement. Position is an aggregate passed indirectly and must be validated. | Boolean-return lifecycle once the indirect value argument is checked. | Out-of-range completion succeeds, original skipped only on, position forwarded correctly off. |
| Todo | `prophecy-uses` — `LE.Factions.ProphecySlot.TryTriggerReward` | Save completed charges; call original with the actual out parameter; after a successful trigger restore only charges that decreased. Keep original return and `chargesConsumed` output. | Small snapshot mechanics if they match another feature. Success-dependent restoration remains specific. | Failed trigger, increased charges, multiple consumed charges, exact out value; explicitly decide the original-throw case. |
| Todo | `prophecy-max-lp` — `ProphecySlotReward.SpawnRewardForPlayer` | Temporarily raise generic reward minimum LP to at least four, call original, restore the previous asset value even on native unwind. Share handler with reward multiplier; prove the eight-byte Nullable lens ABI. | Shared target; scoped restoration mechanism established with this family. | Shared asset remains pristine after normal/throwing calls; existing LP above four preserved; absent generic item. |
| Todo | `prophecy-rewards` — same `SpawnRewardForPlayer` | Temporarily scale `itemsDropped`, rounded and clamped to 0..1000, then restore. Preserve downstream lucky-lens doubling and composition with max LP. Shared reward assets must not retain repeated multiplication. | Same target and restoration record as max LP; settings. | Repeated calls do not compound; both features together; nested/concurrent calls require an explicit target-threading contract. |

### World

Behavior reference: [world.h](../../EterrasGift/native/hooks/world.h).

| Status | Feature and target | Behavior | Shared mechanism | Distinguishing targeted check |
|---|---|---|---|---|
| Done | `mysterious-rift` — `TimeBeastTracker.NextEncounterShouldBeEggs` | Game decides restores original behavior; Portal returns true; Boss returns false. Existing eligibility, placement and special scripted paths remain. See [rift details](#mysterious-rift). | Retained selectable-Boolean leaf; atomic answer and enabled state; shared lifecycle/profile/readback. | Both selected answers, original call count off, mode migration, restoration and helper departure. |
| Todo | `morditas-timer` — `RageOfMorditasLogic.OnEnter_HuntState`, `FixedUpdate_Hunt`, `OnActorDiedDuringHunt` | Snapshot times, call original, scale only that call's changed initial duration, positive elapsed delta while still hunting, or positive kill bonus. Do not revive a hunt ended by original. Three settings and targets compose. | Coherent settings triple and multi-target ownership; field descriptors. | Secondary hunt with unchanged length, ending tick, zero/negative deltas, kill popup timing and off mid-hunt without rewinding elapsed time. |
| Todo | `eternity-cache-anywhere` — `ActorCanUseEternityCacheInThisZone`, `Item.getMaxUniqueLevelRequirementForDungeonTier`, `ActorUsedEternityCache` patch | Permit the local actor, pass `INT32_MAX` tier into original level-cap calculation, suppress the final local-used store. The actor-check method's source ABI is static actor plus MethodInfo, not an instance receiver plus actor. | Mixed detour/patch ownership; identity lookup; static integer signature. | Other actor unchanged; deepest tier not lowered; used store suppressed; removing the patch and both detours is reported together. |
| Todo | `zoom` — `CameraManager.ApplyZoom` | Write configured distance before original. Keep the game's distance per camera, adopt external distance changes as the new baseline, and restore only if current value still equals our last write. Apply immediately on toggle through the main thread where available. | Lifecycle cleanup entry point; conditional restoration idea from existing writes. Keep camera state/refresh local. | Camera replacement, user changing game setting while on, off after foreign change, menu departure and cleanup before dispatcher removal. |
| Todo | `reveal-map` — `LE.UI.Minimap.Minimap.UpdateFoW_Singular` | Raise radius to at least 1000 and call original. Forward indirect float3 position exactly. Disabling does not hide already revealed map. | Mixed scalar forwarding and argument adjustment after aggregate validation. | Larger radius preserved; exact position forwarded; existing reveal persists after off. |
| Todo | `waypoints` — `MapPanel.SelectLocation` and `UIWaypointStandard.LoadWaypointScene` | For an inactive eligible waypoint, pass both availability flags true; retain the original seven-character/no-waypoint exclusion. Patch both travel rejection branches. Do not blindly unlock every scene. | Mixed target/patch ownership; object/string-length reads and long scalar ABI. | Inactive/active, missing waypoint and excluded scene; selection and travel agree; one travel-site failure cannot report full support. |
| Done | `eternity-cache` — `RandomOneShotCacheSpawner.ExecuteSpawn` | Remove the failed chance-roll branch for endgame cache spawn. Keep eligibility and placement checks and the RNG call. This is separate from unlimited cache use. | Patch groups; identical lifecycle to other encounter-roll patches. | Eligible versus ineligible zone, no free placement, original RNG call and restored failure branch. |
| Done | `favor-cache` — `RandomFavourCacheSpawner.CanSpawn` | Replace the final tail-call roll with true after earlier eligibility checks. This intentionally skips that RNG call; do not generalize it as the same semantics as a post-roll branch patch. | Patch groups. | Weaver/zone prerequisites still apply; final RNG call skipped on and executed off. |
| Done | `nemesis` — `RandomNemesisSpawner.ExecuteSpawn` | Remove the chance-roll rejection branch while retaining zone/placement rules and original roll execution. | Encounter patch lifecycle. | Failed roll proceeds only in eligible placement context; off resumes normal rejection. |
| Done | `omen-windows` — both branches of `RandomObserverWindowSpawner.ExecuteSpawn` | Remove campaign and Monolith failed-roll jumps as one feature. Both original rolls still execute. | Multi-range patch group. | Campaign and Monolith cases; one missing pattern leaves both original; original RNG advancement preserved. |
| Todo | `champions` — `RngElement.RollRangeLucky` call resolved from `RandomChampionSpawner.ExecuteSpawn` | Return configured count only for the checked champion call site; other RNG users call original. Preserve the caller-return-address distinction across the new gateway, or prove a different scoped implementation. Original game placement caps still apply. | Pattern/call-target validation and detour lifecycle. Caller context is a special adapter, not a requirement for every handler. | Two callers of the same RNG method, one champion and one unrelated; gateway must not turn the caller address into its own return site. |
| Todo | `loot-lizards` — `RandomEncounterPlacer.HandleEncountersCalculations` | Keep context and pre-call done flag; after original, set total/remaining/count-to-rate multiplier only when this call completed lizard calculations on the same context. Use 0.0001 as quotient for zero spawn rate. | Shared encounter target and typed tally helper with Rune Wardens. | Already-calculated context not refilled; replacement context untouched; zero rate; per-feature independence. |
| Todo | `rune-wardens` — same `HandleEncountersCalculations` | Apply the equivalent logic to the Warden fields, preserving encounters already consumed. | Same target and tally implementation, parameterized by the checked field set. | Lizards off/Wardens on and the reverse; completed state and existing remaining count are preserved. |
| Done | `class-experimentals` — `RunePrisonItemDrops.SpawnExperimentalItem` | Replace the class-compatibility flag load with true at its checked instruction site. Keep the rest of item generation unchanged. | Patch-range ownership within a method also used by two other features. | Class-compatible generation on; independently remove this patch with Rune Prison/unique patches still active. |
| Done | `rune-prison` — same `SpawnExperimentalItem` | Remove the Rune Prison Complex chance-roll refusal, preserving its level/echo eligibility and original roll. | Separate patch range in the shared method. | Low-level/ineligible echo still does not drop it; other active patches survive off. |
| Todo | `experimental-uniques` — `GetExperimentalUniqueDropChance` plus `SpawnExperimentalItem` fail-to-drop patch | Return 1 from the chance method while on and remove the later failure branch. Preserve item-level eligibility. Install/remove as a mixed unit even though the original packs both into one owner. | Mixed target/patch coordination, shared-method range checks. | Chance detour alone is insufficient; injected failure restores prior state or reports uncertainty; above-level unique still does not drop. |
| Todo | `monster-density` — `SpawnerPlacementRoom.rollSpawners` | Temporarily set density modifier to `(1 + old) * multiplier - 1`, invoke original, restore even on unwind. Validate the small by-value two-bool configuration struct. Retain the game's placement limit. | Scoped restoration after the prophecy family proves it; scalar settings. | Original modifier restored, not compounded; exception, nested call and configuration bytes; multiplier does not bypass room cap. |
| Todo | `pack-size` — `Spawner.GenerateEntitiesInternal` | Temporarily multiply `numberToSpawn` only when its original value exceeds one; preserve single boss/named spawns. Restore after original including unwind. Share with monster rarity. | Shared spawner target and restoration record. | One remains one; multi-pack scales; repeated/throwing calls do not retain modified count. |
| Todo | `monster-rarity` — same `GenerateEntitiesInternal` | Temporarily multiply common/magic/rare weightings by three coherent settings, call original, restore all weights. Compose with pack size in one handler. | Same target/restoration record; settings triple. | Each weighting, both features together, zero weights, repeated calls and foreign unwind restoration. |

## Direct field editors

These six current catalog rows remain Todo. They use main-thread requests, fresh expected observations,
conditional writes and independent readback. Keep read/apply primitives generic and operation semantics local.
The retired attack/leech editor is covered by the two persistent hooks above.

| Status | Field set | Remaining feature work and useful reuse |
|---|---|---|
| Todo | `points-earned` | Read/write passive and Weaver tree points through their getters/setters; validate UInt16 range and allocated-point floor. Reuse conditional field-write transport. Values are recalculated by progression and load. |
| Todo | `currency` | Edit gold, soul embers and ancient bones by deltas through `modifyGold`, `modifySoulEmberCountOnSingleplayerOrServer` and `modifyAncientBones`. Preserve refusals, display updates and saves; share conditional write transport rather than a generic raw numeric store. |
| Todo | `keys` | Read dynamic stash entries and change quantities using the game's remove/set paths, respecting removal permission. Share the held-item request machinery with woven echoes. |
| Todo | `woven-echoes` | Same held-item traversal and quantity workflow with its own container getter and labels. Preserve entry identity and deletion behavior. |
| Todo | `monolith-run` | Current/saved run lookup, corruption/Gaze of Orobyss/options-rerolled mutation, corruption recalculation, save and optional matching-panel refresh. Share write certainty, not the hook registry. |
| Todo | `faction-standing` | Rank/favor/reputation validation, joined-faction rules and required save/sync calls. Keep separate from GainFavor/GainReputation detours. |

The Forge item editor follows its own planner/private-preview path and is outside the hook/field counts here.
See [shipping scope](trainer-readiness.md) and [PORT.md](../PORT.md) for its connected operations and remaining work.

## Implementation details

### Shared ownership and cleanup

Feature modules in `crates/trainer-helper/src/game/features/` own complete metadata signatures, target selection and
behavior. `features/hooks` provides helper-local group IDs and the common validator. Discovery resolves metadata;
selected stat readers/binders and generic read/apply operations own object access and mutation separately.

`engine/gameplay_hooks` owns admission, retained installation units and ordering. Its effect adapter handles
settings and receiver retention through close, restore, publish and observation operations. All effects close
before restoration; failures keep owners/pins and the helper loaded. Pure patch and detour groups use their native
transactions; global cleanup retains all owners even if restoration of one family succeeded before another failed.
Mixed feature activation has not yet been implemented.

The worker confirms settings/values and independent actual-byte observations. Shared means a damage control is off
while its sibling keeps their common entry patched. Other partially restored or unknown groups cannot confirm on
or off. Manager loss/loading/departure use the same restore path. A direct code patch stops intervening only when
its bytes are restored. Disabling never reverses already-earned XP, allocated points, items, spawns or other
completed gameplay results.

### Mysterious rift

Portal/Boss map to true/false from instance `TimeBeastTracker.NextEncounterShouldBeEggs(receiver, MethodInfo)`;
Game decides restores the entry. A retained atomic answer and enable switch are independently read back. The old
enabled Boolean profile migrates to Portal; mode and wanted state change together.

The installed callers use true for `CaveManager.CreateCave` and false for regular rift encounter/boss spawning.
Cave availability and placement rules still apply. Special woven-echo objectives can bypass this predicate in
`MonolithZoneManager.SetupTimeBeastObjective`. No receiver read, player selection or field restoration is needed.
The user confirmed the selector works live on 2026-10-27.

### Offhand

The feature owns four targets under one switch:

1. Return true from `OneSlotItemContainerWithCharacterRequirements.alternateSufficientBaseTypeConditionFulfilled`
   for an exact `OffhandItemContainer` receiver, making placement and equipped eligibility agree.
2. Return false from `OffhandItemContainer.IncompatibleDueTo2hWeapon`.
3. Return false from `LinkedOneSlotItemContainer.incompatibleWithItemInLinkedSlot` for an offhand receiver or a
   receiver linked to an offhand, covering both weapon-slot directions.
4. Skip `OffhandItemContainer.EnforceWeaponLimitations`, whose inlined pairing check otherwise displaces equipment.

The normal `CanReceiveItem` path retains level/class/faction, primordial/cocoon and quantity checks. The separate
Ignore item requirements feature can deliberately bypass its own three checks. This corrects the original C++
unconditional placement-only replacement, which could equip an item while leaving its stats unusable.

The guards read only metadata-validated receiver/linked-slot class references under the called method's object
lifetime contract. They allocate/call nothing. Off does not unequip items or recalculate cached equipment stats;
re-equip to observe the changed policy immediately. The UI's two-handed ghost placeholder is a separate visual.
The backend preserves redundant REX-prefixed pushes when validating trampoline unwind offsets. Current trace
artifacts are under `artifacts/trainer-eval/offhand-trace/`; fixed addresses are evidence, never runtime constants.
The user confirmed the corrected usability and pairing behavior works live on 2026-10-27.

### Damage controls

`incoming-damage` accepts whole 0–100 percentages, default 50. Zero replaces God mode; 100 passes the original amount.
Legacy `damage-taken` divisors migrate to the nearest percentage, with enabled old God mode winning at zero;
already-migrated values take precedence. The target is the current player's health; enemies/allies remain unchanged.
Intermediate arithmetic uses double precision before conversion to float; zero replaces even NaN/infinity, while
100/off preserve the amount.

`damage-dealt` accepts 0–100× and the finite 101 endpoint displayed as ∞; 101 is a mode marker, never a multiplier.
Enabled old one-hit profiles migrate to it. Foes are classified by target/player alignments and the bounded foe
table; a missing table preserves the original fallback that different alignments are foes. It identifies the
recipient rather than the source of damage, so allied/minion attacks against foes are included. Numeric mode scales
the amount; one-hit mode imposes the current-health floor only on positive hits. Same/missing alignments pass through.

Both controls share one retained `BaseHealth.HealthDamage` leaf, preserve its float ABI and call the original once.
Each setting publishes in one aligned 64-bit store. Static player/online storage and metadata-validated object fields
are read on the call path, without a helper/runtime call; the supported managed graph must remain readable. There
is no SEH recovery for an invalid receiver. Off/100%, 50%, 0%, 1×/2×/∞, both sibling-disable orders and disconnect
remain the focused live checks.

### Persistent attack and leech

Attack and leech have separate selectors, desired values and profile switches. `features/stat` reads only the chosen
accessor; `persistent_stat` validates writable scalar storage and pins that receiver. Discovery does not implement
feature writes. The old paired snapshots/planner and Read/Apply RPCs are retired; operations 23 and 24 remain rejected.

Attack rate covers `calculateAttackRate`, `setMainHandAttackRate` and `setOffHandAttackRate`; the setters inline the
calculation, so one hook is insufficient. Leech covers `BaseStats.ApplyExternalStats`, before subsequent
notifications; consumers read its field directly. Each original-first retained gateway forwards the original once,
records its newly calculated baseline and reapplies the selected bits for the pinned receiver while offline.
The main-hand setter requires a fifth native stack argument. The game serializes these recalculations on its main
thread; generated code, return paths and unwind tables are process-owned.

Value changes, off, manager loss and unload close/drain scalar stores, conditionally restore the latest baseline,
restore entries and release pins. A later game write wins over conditional restoration. A pending original may
return after close, but cannot reapply the override. Independent readback checks the current selected accessor and
identity as well as bytes/settings, preventing confirmation through an obsolete pinned object.

### No mana cost

The exact instance signature is `Single(Ability, Single, Boolean, AbilityMutator, Boolean)`, plus receiver and
MethodInfo. The same-name AbilityID overload is different. The shared validator selects full parameter types before
checking the selected method's instance/return/uniqueness contract.

The retained postfix gateway forwards four register positions and three stack arguments, runs original once,
restores the receiver and adjusts XMM0 only for the current player's mana component while offline. Its 72-byte
frame and unwind metadata remain process-owned. The player/actor/mana graph must stay readable across this
post-call span. Closing before a delayed original returns suppresses adjustment; no object pin or field restoration
is needed. Live checks: ordinary and channeled skill off/on/off, with Mana always full and No cooldowns off.

### Crafting materials

`shards` and `runes-glyphs` are independent switches under Forge > Crafting rules, using the same retained leaf,
entry/readback and profile lifecycle as other supported hooks. Each feature owns its metadata contracts and emitted
behavior; `features/materials` shares the closed/online guard, checked field offsets, writable Int32 contract and
small branch emitter. Discovery and generic read/apply do not gain feature-specific operations.

Shards follow `GlobalDataTracker.<stash>` → `LE.Data.Stash.<SavedShards>` → closed framework
`List<LE.Data.SavedShard>` → its exact reference array. Each existing active non-null shard whose quantity is below
99 is raised before `GetShardCount` runs. Active population is clamped to capacity as in the original trainer;
negative/empty size, absent references and backing capacities above the shared collection bound of 65,536 forward
without replenishment. Unused array slots and quantities already at least 99 remain unchanged. No missing shard
kinds are created, and the list/version itself is not modified.

Runes/glyphs follow `OneSlotItemContainer.content` → `ItemContainerEntry.data`, check byte item categories 102/103,
and raise only that entry's stored quantity. Other item types, absent content/data and higher quantities are unchanged.
The feature covers any one-slot container using this method; the UI description now records that original scope.

Both generated leaves preserve receiver/MethodInfo, use volatile registers without a stack frame, and tail-call the
original once. Their process-owned code/data survive helper departure. They check enabled/online before following
the inventory graph, allocate/call no helper/runtime code, and require the called method's retained readable graph,
aligned writable primitive quantities and game-serialized inventory mutation. Null checks do not provide the
original C++ guard's access-violation recovery. An already-admitted call can finish across close; the shared entry
restoration prevents new admissions. Disabling never removes materials already granted, including grants before
an original call that subsequently throws.

Static inspection of the GameAssembly identified below confirms the count path reads tracker stash, saved shards
and each shard quantity, and the slot query returns the content entry's stored quantity. Runtime enable resolves
field types/flags/alignment, the exact List/array element and method signatures; observed addresses/offsets are not
compiled constants. Targeted authored cases cover active/capacity bounds, null links, signed quantities, item types,
online/off behavior, original-call count/arguments/failure, independent toggles, actual entry readback, manager loss
and terminal clear. The session/wire tests cover both IDs through the existing generic switch path. Live gameplay
verification remains pending: compare stored quantities off/on/off, consume materials and query again, then disconnect.

### Interior code patches

The 17 patch-only controls own 27 ranges; recipes stay beside each feature under
`crates/trainer-helper/src/game/features/patches/`, sharing only resolution/ownership/transactions.

#### Resolution and ownership

The resolver checks the full method signature, including static/instance, return type and explicit by-reference
parameters. Forge capability has four by-reference parameters. Weaver preview resolves the unique nested state
machine whose name begins `<TrySimulatePreviewPointSpentWeaverTree>d__`, then its instance void `MoveNext`; its numeric
suffix is not fixed. Pickup gold selects the GroundItemList overload rather than the other three-parameter overload.

Each pattern is searched in its recipe's bounded code window, additionally clipped by the module end and next
declared method address. Windows unwind records alone cannot bound a managed method: Forge capability includes
multiple EH funclets. A unique complete pattern and contained replacement are required. Every target of a feature
is resolved before any code is changed. The native owner checks the captured original bytes and rejects overlapping
ranges; independent non-overlapping patches in one method retain separate ownership.

`hooks/patch` owns actual bytes, page protection and instruction-cache readback. All transaction records, thread
handles and aligned contexts are allocated before suspending peers. A peer stopped inside a changing range refuses
the write; the backend does not guess how to relocate that instruction. A failed store attempts restoration of every
attempted range to its pre-transaction state, preserving failures in the result. Pending protection/cache failures
make the range Unknown. Foreign bytes are never overwritten as normal cleanup.

The thread census is a Toolhelp snapshot, not a proof that every possible caller is quiescent. The supported-target
contract excludes execution of these ranges by threads created/missed after that snapshot, and excludes interior
return addresses and competing code writers. Live handles that cannot be opened/suspended refuse the operation;
already-departed threads may be skipped. This remains an explicit native execution assumption rather than a new
bootstrap prerequisite.

#### Shared lifecycle

The existing coordinator retains entry detours and interior-range owners behind one installation interface.
Feature activation remains homogeneous: pure patch groups use one native patch transaction; detour groups use the
existing stock transaction. Global clear closes every effect before restoring either family and releases receiver
bindings only after restoration succeeds. Terminal unload also releases the restored range-owner metadata;
process-owned gateway pages remain retained. Failure retains owners and keeps the helper loaded, including after
partial cleanup across families.

The independent request reads actual bytes and the feature setting. It reports off only after every owned range is
Original; a partially restored group cannot confirm on or off. Interior patches stop intervening when their bytes
are restored, rather than through a generated gateway's enable flag. Manager loss, departure and unload use this
same restore path. Already-earned skill levels, allocated points, items, spawns and completed station uses are not
reversed by disabling a feature.

The wire adds feature identities and Boolean settings only. Switches use the catalog's existing value arity in the
worker bridge; profiles and UI capabilities follow the same generic command/readback route as other supported hooks.
No new gameplay-thread Rust handler or helper return address is introduced by this family.

#### Preserved distinctions

- Equipment requirements still preserve slot compatibility and the primordial limit. Skill/passive nodes retain
  their other prerequisites; Weaver still requires at least one earned point. Set bonuses still require a worn piece.
- Pickup range owns all five sites, including the ground-object contact flag. The one-byte flag change is deliberately
  an immediate inside an instruction, rather than an instruction replacement.
- Encounter patches preserve eligibility and placement. Cache, Nemesis, Omen and Rune Prison roll calls still run.
  Favor cache instead replaces its final tail-call with true after earlier checks, as the original implementation does.
- The two passive-point features and the two Rune Prison features own disjoint ranges in their respective shared
  methods. Toggling one does not restore another's bytes.

The remaining mixed features (waypoint selection/travel, unlimited Eternity Cache use and experimental uniques)
also need their detour behavior. Echo cycling additionally has a dynamic switch-table operation. Those complete
features remain unsupported until their additional semantics are implemented.

#### Evidence and verification

Static inspection used GameAssembly SHA-256 `323e9183f3bee600b6c9e1c469218b9de926e9044f2ca3d44fae8a0ec9a91464`
and metadata SHA-256 `2a4e8440606c420345934158b6784d8f5d8b0184f32fd91af0e9e0903ba94f83`. All 27 recipes match exactly
once inside the bounded declaring-class method windows. Applying either independent shared-method feature leaves
the other's pattern resolvable. Observed addresses are evidence only; runtime resolution never uses a fixed RVA.

Targeted native tests cover pattern ambiguity/bounds, overlap and foreign-byte refusal, grouped on/off and exact
readback, injected second-write recovery, a peer inside the changing range, and mixed detour/patch manager-loss
cleanup. Metadata tests cover explicit by-reference contracts. Session/catalog and wire tests cover all supported
IDs through existing controls. App build, Clippy and the fixture build are checked separately. No captures or live
Last Epoch testing were requested; static matching and native fixtures do not establish live gameplay parity.

## Remaining work by reusable family

The checklist is the feature-level Todo list. These groups identify the next useful extraction points; they do not
require building a universal handler/ABI/object framework first.

- **Settings and numeric transforms:** XP multiplier/instant level share a target and precedence; faction gains,
  area and cast speed reuse only matching arithmetic/owner lookup. Preserve independent original-call placement.
- **Post-call results and argument forwarding:** perfect rolls, boss drops and always T7 preserve RNG/original work;
  force rarity needs its full long signature; map/prophecy distance need validated indirect aggregates.
- **Scoped context:** projectiles then duplicate establish nesting/unwind behavior; movement consumes a mark,
  while echo context spans calls. Do not give all three the same cleanup policy.
- **Temporary shared assets:** prophecy reward count/max LP, then density and pack size/rarity need explicit target
  serialization and restoration on native unwind. Cooldown/ailment/prophecy-charge throw behavior must be decided
  against the original's normal-return-only restoration rather than silently promising universal rollback.
- **Mixed features:** experimental uniques, unlimited Eternity Cache use and waypoints need their complete detour
  behavior alongside checked ranges; echo cycling also needs its dynamic completion table and shared echo-type target.
- **Feature-specific cleanup:** skill-slot recount and camera zoom restoration require main-thread work before the
  dispatcher leaves. Persistent inventory/run changes and temporary overrides have different off semantics.

## Maintenance and verification

When implementing a feature, update its row's status/behavior and any shared-support row it completes. Keep current
constraints here; archive completed experiment/checkpoint narratives rather than appending another feature report.
The [original owner inventory](../tools/trainer-eval/hooks-fields-audit/hook-inventory.md) and
[ABI inventory](../tools/trainer-eval/hooks-fields-audit/abi-inventory.md) are source evidence, not validated runtime
contracts. Source declarations sometimes erase value types and cannot substitute for metadata/ABI checks.

Every supported feature needs its behavior/exclusions, exact original-call count, on/change/off, independent settings
and byte readback, manager-loss and unload checks as relevant. Validate the largest actual signature; test original
exceptions without retry. A Rust panic catch is not a foreign-exception or memory-fault recovery mechanism. Future
coherent multi-value settings need an actual atomic protocol; a version counter does not make plain concurrent
payload accesses safe, and readers must not wait for a paused writer.

Use targeted tests for the affected behavior plus app build, Clippy and formatting, per the current user scope.
Captures and live game runs are requested separately. Native/Unity fixtures and shipped-file pattern matches prove
their own boundary only. Existing fixtures exercise rift/offhand mode, original counts, requirements/pairing,
damage filtering and shared ownership, scalar recalculation/restoration, mana forwarding/unwind, patch transactions
and lifecycle/readback. A new live confirmation should record the feature and exact combinations checked rather
than relabel all implemented hooks as verified.
