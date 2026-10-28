# Trainer feature implementation plan

Build working feature families on a small common lifecycle, then extract repeated behavior as
those families establish it. Do not build the complete generic handler system before the first
feature, and do not give each feature its own installation, transport or unloading machinery.

This plan covers the 68 hook features in the original application at
`e1c1ed4e9e0ab6404574113f2dcf1179b2113629`, checked against the adjacent checkout on 2026-10-23.
Mysterious rift and offhand now share the first production Boolean-override lifecycle (see
[implementation](trainer-first-hook.md)). Incoming damage now combines God mode and Damage taken in a single 0%-100% control on that lifecycle; outgoing damage combines the multiplier and one-hit kills in a second control; the other 62 original features remain unsupported. The seven direct field sets and
Forge item editor use another path; their boundary is recorded below.

## What the feature inventory establishes

The original has 74 hook owners, 54 detour installations, 31 `CodePatch` declarations and a
separate echo-completion switch-table edit. An owner is an installation unit, not necessarily
one method: `experimentalUniquesHook` installs both a detour and a byte patch. A feature can
depend on several owners, and several features can share one owner.

| Feature implementation | Features | Consequence |
|---|---:|---|
| Detours only | 47 | Need a retained callback route, typed handlers and target-specific behavior. |
| Byte patches only | 17 | Need checked patch ranges and restoration; no gameplay handler gateway. |
| Detours plus byte or table patches | 4 | Need coordinated installation and truthful partial-failure reporting. |
| Total | 68 | A method-to-handler table alone does not cover the port. |

The four mixed features are `echo-cycle`, `eternity-cache-anywhere`, `waypoints` and
`experimental-uniques`. The existing [owner inventory](../tools/trainer-eval/hooks-fields-audit/hook-inventory.md)
and [declared ABI inventory](../tools/trainer-eval/hooks-fields-audit/abi-inventory.md) provide the
source declarations. Those declarations erase some value types as pointers or integers; they
are starting evidence, not validated Rust signatures for the installed game.

## Existing support to use

- [Feature descriptions and intents](../crates/trainer/src/features.rs) already own IDs, ranges,
  choices, arity and validated desired values. Keep those user-facing meanings.
- [Worker profile handling](../crates/trainer/src/backend/profiles.rs) already queues changes,
  tracks `HookRun`, saves intent and handles character profiles. Extend its production
  `supports_hook`/`set_hook` connection and readback; do not replace it with another scheduler.
- [RetainedHook](../crates/game-access/src/hooks.rs) already owns detour transactions, retained
  trampolines, byte/state checks and recovery. Keep its accepted best-effort thread-suspension
  boundary. It is not an arbitrary interior-byte patch implementation.
- [CallbackGate](../crates/game-access/src/hooks/gateway.rs) proves the retained-code pattern
  for the dispatcher, but only implements original-first `void(MethodInfo*)`. Gameplay needs
  additional exact signatures and different call order.
- [Helper lifecycle](../crates/trainer-helper/src/engine.rs) already closes the dispatcher,
  restores its entry and drains callbacks. Extend this ownership to gameplay routes, pending
  changes and necessary effect cleanup.
- Existing wire framing, session identity, write certainty, fake connections, capture routes
  and native reload fixtures remain in use. Add hook requests and independent observation to
  them rather than inventing another transport or proof framework.

Native targets, handler pointers and field layouts belong in the helper/game-access tier.
The desktop catalog should reference IDs and capabilities without importing native handlers
or making the helper depend on `eg-trainer`. A catalog row is supported only after its complete
native path exists and its required targets are admitted for that connection.

## How to read each feature entry

Every entry specifies the original target, the behavior to implement, the useful shared code,
and a distinguishing fixture case. All features additionally need the common on/change/off,
independent-readback, owner-loss and unload checks described later; these are not repeated in
every row. Value arity and ranges come from the existing catalog, not a second table here.

The reusable pieces named below are deliberately small:

- **Detour lifecycle:** retain, install, observe actual state, share ownership, close and drain.
- **Patch groups:** resolve bounded patterns before writing, own non-overlapping ranges, check
  bytes and recover/report partial failure. Include table data when its first consumer arrives.
- **Settings:** validate on the command path and publish a bounded coherent snapshot to handlers.
- **Object access:** reuse checked field descriptors and narrow bounded reads where the layouts
  really match. Each target still owns its object lifetime and threading assumptions.
- **Scoped context:** small per-thread state and unwind cleanup for nested related calls; build
  with the first consumer and extract only after a second demonstrates the same semantics.

Restoring a hook stops future intervention. It does not reverse experience, items, revealed map
areas, or other gameplay results already produced. Temporary changes and explicit restoration
on disabling are identified separately below.

## Character

Source: [character.h](../../EterrasGift/native/hooks/character.h).

| Feature and original target | What must be coded | What can be shared | Distinguishing fixture case |
|---|---|---|---|
| `skill-slots` — `PlayerFinder.getPlayerActor` | Call original, raise the returned actor's `localTreeData.numberOfUnlockedSlots` to at least five. On disabling, recount through `LocalTreeData.updateUnlockedSlots` on the main thread when a character still exists. Avoid the folded byte getter. | Detour lifecycle; object descriptors. Keep recount and its timing feature-specific. | A level/load recount is raised again; off restores the natural count; missing character needs no stale-object write. |
| `offhand` — original `OffhandItemContainer.CanReceiveItem(item, amount)` | Corrected 2026-10-27: override `OneSlotItemContainerWithCharacterRequirements.alternateSufficientBaseTypeConditionFulfilled(int)` only for an OffhandItemContainer receiver. Placement and equipped weapon eligibility then agree while normal item requirements remain. The same feature also bypasses two-handed/linked-slot pairing checks and automatic conflict displacement. See trainer-first-hook.md for the installed-binary trace and refresh limits. | Same retained lifecycle, grouping four constant-return leaves under one enable switch; receiver/linked-slot guards keep other slots unchanged. | Qualifying offhand item is accepted and eligible for effects; unmet requirements refuse, other slots stay unchanged; original arguments and behavior return off. |
| `primordial-limit` — `ItemContainersManager.NumberOfEquippedPrimordialItems` | Return zero only for the local containers manager; forward other managers. Resolve the local identity outside metadata discovery in the hot path. | Local-identity publication/lookup where its freshness contract fits; integer-return detour. | Another manager keeps its count; replacement of the local manager invalidates the old identity. |
| `xp-multiplier` — `ExperienceTracker.GainExp` | Scale positive character and ability XP separately for the player's tracker; round and cap as the original does, preserving gains already above `INT32_MAX`. Forward favor XP unchanged. Share this handler with instant level. | Settings pair; shared-target ownership; numeric conversion helpers only where semantics match. | Independent multipliers, nonpositive gains, large gains, nonplayer tracker and simultaneous instant level. |
| `instant-level` — same `GainExp` | Read current and next-level XP on the player's tracker. A positive character gain becomes at least the remaining requirement. This wins over the character XP multiplier; the skill multiplier still applies. | Same target and owner lookup as XP multiplier. Keep precedence in this handler. | Zero gain stays zero; character multiplier loses precedence; skill XP remains scaled. |
| `max-skill-level` — `LocalTreeData.ApplyAbilityXp` | Patch the conditional jump after adding skill XP so the existing loop uses the level-20 XP cap. Disabling restores bytes, not earned skill XP. | Patch groups. | Positive gain follows the cap path on and the original conditional off; other loop logic remains intact. |
| `item-requirements` — `OneSlotItemContainerWithCharacterRequirements.CanReceiveItem` | Remove the class, faction and level rejection branches as one group. Preserve slot compatibility and primordial checks. | Three-range patch group. | Each intended restriction bypasses; incompatible slot and primordial limit still refuse. |
| `set-bonus` — `ItemEquipManager.UpdateStats` | Remove the branch skipping a set bonus whose piece requirement exceeds the number worn. Applies during the game's next stats recalculation; no immediate recalculation is performed by the original toggle. | Patch groups. | One worn set piece enables its higher bonuses during recalculation; an unworn set gains nothing. |
| `free-skill-points` — `LocalTreeData.tryToSpendSkillPoint` | Remove the total-spent versus max-points refusal. Keep per-node maximums, connections and other requirements. Allocated points are not undone by disabling. | Patch groups. | Exhausted total points can spend; maxed or otherwise invalid nodes still refuse. |
| `free-passive-points` — `LocalTreeData.tryToSpendPassivePoint` | Replace the virtual unspent-points call at its checked site with the original app's constant 113 result. Do not hook the general getter. | Patch ranges and shared-method non-overlap checks with `passive-chain`. | Only this spend path sees the constant; either feature can be removed while the other remains. |
| `passive-chain` — same `tryToSpendPassivePoint` | Remove the chosen-mastery mismatch branch for deeper nodes. Preserve having a mastery, its point requirement and the node connection. | Patch groups; separate range ownership in a shared method. | Wrong mastery unlocks; absent mastery and insufficient prerequisite investment still fail. |
| `free-weaver-points` — preview coroutine `MoveNext` and `LocalTreeData.TryToSpendWeaverPoint` | Resolve `TreeUIManager.TrySimulatePreviewPointSpentWeaverTree`'s state machine; remove its preview refusal/decrement and replace the spent-point sum in the real spend path. At least one earned Weaver point is still required. | Multi-method patch group; state-machine resolver only if reusable. | Preview and actual spend agree; zero earned points still refuses; one missing pattern changes no bytes. |

## Combat

Implementation update (2026-10-27): the first two rows below are the original feature inventory.
Their replacement is one `incoming-damage` percentage control, using a concrete retained numeric
leaf and the existing lifecycle. The original is always called once. No helper handler, panic
boundary or arbitrary-ABI generator is needed for this slice. Damage dealt and one-hit kill
remain unsupported and will extend ownership of this same target when implemented.


Source: [combat.h](../../EterrasGift/native/hooks/combat.h).

| Feature and original target | What must be coded | What can be shared | Distinguishing fixture case |
|---|---|---|---|
| `god-mode` — `BaseHealth.HealthDamage` | Resolve health's actor and compare with the local player; pass zero damage for that player. Still call original once. Wins over damage taken. | One damage handler/target for all four damage features; object identity and settings. | Player versus enemy versus ally; enabling/disabling one of four features keeps the required shared patch. |
| `damage-taken` — same `HealthDamage` | Divide player damage by the configured divisor when at least one, unless god mode is active. | Damage target; settings and player lookup. | Divisor boundary and god-mode precedence; original receives the adjusted amount once. |
| `one-hit-kill` — same `HealthDamage` | For a foe and a positive incoming hit, make adjusted damage at least current health. Preserve the alignment/`isFoe` lookup and its fallback before the foe table exists. Allies and minions must remain excluded. | Damage target and enemy classification with damage dealt. Current health is a call-time read. | Allies/minions, missing/same alignment, absent foe table, zero/negative hit and current health greater than scaled damage. |
| `damage-dealt` — same `HealthDamage` | Multiply damage only to player foes, then let one-hit kill impose its health floor where applicable. | Damage target; same classification as one-hit kill. | Multiplier and health floor compose; friendly damage remains unchanged. |
| `infinite-mana` — `BaseMana.OnUpdateTick` | Match the player's mana component, store its current mana from its maximum before the original tick. Preserve the original tick; this is not a promise that every later instant has full mana. | Player/component lookup; field descriptors; mixed float/void gateway. | Player starts tick full, enemy unchanged, maximum changes followed; original tick still runs. |
| `area` — `Stats.GetIncreasedAreaForAreaSkill` | Return the configured increased-area value for the player's stats, skipping original for that case. Other stats keep original calculation. | Settings and player stats identity; float return. | Player versus minion/monster stats, original call count, changed player identity. |
| `no-cooldowns` — `ChargeManager.useCharges` | For the player, keep up to 64 charges, call original with zero amount, then raise lowered charges back if the backing array is still the same. Keep the original shared-cooldown behavior around that adjustment. | Bounded list access. Extract snapshot mechanics only after another matching consumer; policy remains local. | Shared cooldown decreases are restored; increased charges and replaced arrays are preserved; define the original-throw path explicitly. |
| `zero-mana-cost` — `BaseMana.getManaCost(Ability, ...)` | Select the exact five-parameter overload. Call original first, then replace cost with zero only for the player's mana component. | Exact method query; player lookup; mixed scalar/stack ABI support. | Original side effects remain; wrong overload/nonplayer stays unchanged; stack bools/pointers are forwarded. |
| `cast-speed` — `UsingAbility.getSpeedMultiplierStat` | Call original, identify player through state/controller/actor, multiply positive configured speed. Preserve abilities whose mutator fixes speed downstream. | Owner-read helpers and float result adjustment. | Enemy state untouched; valid player multiplier; fixed-speed ability still follows its mutator. |
| `projectiles` — `ExtraProjectiles.onCreation` and `GetTotalExtraProjectiles` | Mark the locally created extras object per thread while original creation runs; restore the outer mark on return/unwind. Scale the marked object's positive returned count, rounded and capped at one million. | Multi-target ownership; small scoped TLS pattern, later shared with duplicate where appropriate. | Nested extras restore the outer object; unrelated/thread-concurrent queries unchanged; exception clears context. |
| `endless-buffs` — `StatBuffs.OnUpdateTick` and `AilmentReceiver.OnUpdateTick` | Pass zero time for player buffs. For positive player ailments, keep up to 64 durations, compensate their calculated tick loss before original, then restore durations of surviving entries. Negative ailments and damage/healing must keep ticking. | Bounded collection access and two-target lifecycle. No generic 'skip tick' handler. | Positive/negative mix, local/global speed modifiers, removed entries, more than 64 ailments and original throwing. |
| `move-speed` — `SpeedManager.updateSpeed` and `Stats.GetStatValue` | Mark player stats during speed update. Multiply only the first matching Movespeed query, consuming the mark before calling original. Resolve enum value and forward all nine managed arguments. Preserve the original source's clear-on-exit semantics; review nested updates explicitly. | Scoped context mechanics; long mixed ABI forwarding; multi-target ownership. | Other stats/properties and subsequent reads unaffected; nested update/exception leaves no stale mark. |
| `auto-target` — base and Fireball `getProjectileAutoAim`, `getNewTargetPositionWithAutoTarget`, `Alignment.nearestActorInRange` | Implement player ownership and exclusions for minion, movement, traversal and transform abilities; return VeryHigh aim, pass 360-degree angle, and maximum range in the respective paths. Fireball needs its own override. The constructor method returns Vector3 through hidden storage. | Four-target ownership, enum/field discovery and proven aggregate ABI support. Keep targeting decisions explicit. | All exclusions, Fireball override, creator fallback, nonplayer target and exact Vector3 return-buffer behavior. |

## Stash

Source: [stash.h](../../EterrasGift/native/hooks/stash.h).

| Feature and original target | What must be coded | What can be shared | Distinguishing fixture case |
|---|---|---|---|
| `duplicate` — `ItemContainersManager.AttemptItemDropInWorld` and `DropItem` | Maintain a per-thread nesting count during local-player drop attempts, with unwind cleanup. Inside that scope, double ordinary positive item amounts; keys and woven echoes use the floored configured factor, at least one. Cap at `INT32_MAX`. Existing duplicates remain after off. | Two-target ownership; scoped TLS mechanics from projectiles; item-type descriptors. | Nested and failed attempts, another thread, ordinary/key/echo types, zero amount and overflow; no duplication outside player-drop scope. |

## Forge rules

Source: [forge.h](../../EterrasGift/native/hooks/forge.h). These hooks are separate from the item editor.

| Feature and original target | What must be coded | What can be shared | Distinguishing fixture case |
|---|---|---|---|
| `forge-level` — `CraftingManager.CheckForgeCapability` | Remove only the character-versus-item-level rejection branch. Preserve the rest of forging admission and side effects. | First simple consumer of patch groups. | Low level passes that check; unrelated forging restrictions still refuse; off restores the exact branch. |
| `shards` — `GlobalDataTracker.GetShardCount` | Walk saved shard entries, raising quantities below 99 before original counts them. Null entries are skipped. This changes inventory; disabling does not restore prior quantities. | Bounded list/array access and scalar stores, later compared with runes/glyphs. | Quantities below/above 99, empty/null entries, count agrees with actual stored quantities. |
| `runes-glyphs` — `OneSlotItemContainer.GetQuantity` | Follow content/data and raise quantity to 99 only for rune/glyph item types, then call original. The hook applies to this container class wherever used, not just the visible Forge slot. Off stops replenishment without removing material. | Item-entry descriptors and floor operation if also useful for shards. | Other item types untouched; already-higher counts preserved; stored quantity and returned quantity agree. |
| `woven-enchanter` — both `ZoneRestrictedCraftingManager.OnUse` overloads | Remove the offline local-use increment in both bodies; preserve other station work and server-account branch behavior. Previous uses are not reset by toggling. | Multi-method patch group and overload validation. | Both overloads stop decrementing remaining uses; failure finding either site leaves neither installed. |

## Loot

Source: [loot.h](../../EterrasGift/native/hooks/loot.h).

| Feature and original target | What must be coded | What can be shared | Distinguishing fixture case |
|---|---|---|---|
| `drop-rates` — `DeathItemDrop.OnDeath` | Before original, write item/gold override flags, chances clamped to 0..1 and nonnegative multipliers on the passed drop component. Keep those writes because delayed drop work reads them later. | Shared target with extra rares; coherent four-value settings; field descriptors. | Item/gold settings remain coherent; a delayed original continuation sees the changed component; off does not retroactively undo drops. |
| `extra-rares` — same `OnDeath` | Set `guaranteedAdditionalRare` before original on the passed component. Leave it on that dying enemy as the original app does. | Death-drop target and descriptors. | Works independently of drop rates; disabling one keeps the other's target installed; original executes once. |
| `boss-drops` — nested `BossLoot.LootGroup.GetDropChance` | Call original with all five managed arguments, then return 1 when active. Preserve original execution; this is the chance before downstream modifiers, not a universal final drop-count override. | Nested-class query; mixed float/integer stack forwarding; result adjustment. | Original called once; rarity/corruption and stack tier arguments forwarded; final result overridden only on. |
| `perfect-rolls` — `LE.Core.RngElement.RandomByte` | Call original to advance RNG, then return 255. Preserve the original app's broad scope: other uses of RandomByte are affected too. | Byte-return detour and result adjustment pattern. | RNG state advances identically on/off; return value is a byte; non-item RandomByte calls retain the same broad effect. |
| `force-rarity` — `GenerateItems.RollBaseItem` | Replace only a caller-supplied rarity of -1 with the selected catalog mapping 0/1/3/7/8. Forward 21 managed parameters, receiver and MethodInfo, including reference/aggregate payloads with validated ABI. Explicitly requested rarity wins. | Long-stack forwarding once proven, settings choice validation. | All 23 native slots survive; explicit unique/blessing/gambler rarity unchanged; only -1 is replaced. |
| `always-t7` — `GenerateItems.RollTier` | Let original consume its RNG, then return internal tier 6. Apply to newly generated affixes, not existing items or Forge edits; this differs from the older cheat-table RebuildID patch. | Long mixed-argument forwarding; result adjustment. | Original RNG advances; returned tier is 6; editing an existing item does not silently change all its tiers. |
| `pickup-range` — `pickupItem`, `ClickedItem`, `vacuumNearbyStackableItems`, `pickupGold`, `PickupGroundObjectInteraction.initialise` | Patch four distance-refusal paths and set initial `canPickup` true for ground-object interaction. Resolve GroundItemList overloads. Preserve pickup delay and other admission rules. Previously collected items stay collected. | Five-site patch group and overload query. | Item click, materials, gold and delayed ground objects each exercise their own path; one missing site prevents a partial feature. |

## Monolith

Source: [monolith.h](../../EterrasGift/native/hooks/monolith.h).

| Feature and original target | What must be coded | What can be shared | Distinguishing fixture case |
|---|---|---|---|
| `skip-quest-echoes` — `MonolithTimelinePanelManager.SetRunInfo` | Raise run quest completion to at least two and stability to at least the selected difficulty's maximum before original. Validate difficulty indexing. Preserve the original trigger on showing run info; changes belong to the saved run and are not undone on off. | Object/list reads and field stores. | Lower values raised, higher values preserved, invalid difficulty safe fallback, opening the panel triggers original once. |
| `max-bonus-stability` — `MonolithZoneManager.getBonusStabilityInRegularEcho` | Call original, then return at least `maxBonusStablity` when active. Preserve the game's first-kill/update trigger rather than adding an immediate write. | Integer result adjustment; one field descriptor. | Original higher result is preserved; first qualifying update reaches maximum; off resumes the game's calculation. |
| `echo-cycle` — `getIslandTypeForNewIsland`, `CalculateModAndZoneForIsland`, `CanHaveSubsequentConnectionsRevealed`, `OnIslandCompleted` table | Cycle the seven original type values; pass tier at least 19 for the related generation calls using per-thread web/tier context; permit subsequent connections; redirect normal/arena completion table entries to Super Beacon. Share type target with echo-type. Restore the table on off. | Shared/multiple target ownership, scoped context, atomic cycle index and checked data patching. Keep the cycle and table decoding specific. | Sequence/wrap, unrelated web, explicit-type precedence, normal/arena reveal behavior, table restoration and partial installation failure. |
| `echo-type` — same `getIslandTypeForNewIsland` | Map the selected catalog index to the original ten IslandType values and override the original result. Wins over cycling type, while the active cycle's tier and other effects remain. Original still runs. | Shared target with echo-cycle; settings choice validation. | Each index maps correctly; explicit type plus cycle retains tier behavior; removing either feature preserves the other. |
| `reveal-objective` — `LE.Gameplay.Monolith.Frontend.MonolithPulse.OnScoreChanged` | Raise incoming score to at least the pulse's threshold before original. Preserve its existing score-change/first-kill trigger; do not add a global objective scan. | Float argument adjustment and object field read. | Below/above threshold, original last-kill argument, next score event after activation; previously revealed objective stays revealed. |

## Factions

Source: [factions.h](../../EterrasGift/native/hooks/factions.h).

| Feature and original target | What must be coded | What can be shared | Distinguishing fixture case |
|---|---|---|---|
| `hunter-cost` — `HunterVendorContainer.CanRemoveItemPreCheck` | Remove the failure branch after the base purchase precheck. Preserve the original precheck execution and its other effects; do not replace the entire method with true. | Simple patch group. | The failed-payment branch is bypassed while the rest of the method still executes; original bytes return on off. |
| `faction-gains` — `LE.Factions.Faction.GainFavor` | Multiply positive favor only when `ignoreMultiplier` is false; round/clamp to nonnegative `Int32`. Let original handle joined faction, rank scaling, favor cap and resulting reputation. | Settings, integer argument transformation, proven rounding helper. | Ignore flags, nonpositive gains, max bound and resulting reputation; no duplicate reputation multiplication. |
| `instant-rank` — `LE.Factions.Faction.GainReputation` | For positive gain, read rank requirements and current reputation; replace gain with the positive remainder to the next rank. Forward unchanged when unavailable or not positive. This can reduce a gain that would otherwise cover multiple ranks. | Bounded array/object reads; same arithmetic helpers only if their semantics fit. | Invalid/top rank, missing requirement, remainder <=0, large original gain; exactly the source's replacement behavior. |
| `prophecy-any-zone` — `LE.Factions.CircleOfFortune.IsPlayerInRangeOf` | Return true when active; otherwise call original. Despite the legacy ID, current meaning is combat prophecy distance, not removal of a zone requirement. Position is an aggregate passed indirectly and must be validated. | Boolean-return lifecycle once the indirect value argument is checked. | Out-of-range completion succeeds, original skipped only on, position forwarded correctly off. |
| `prophecy-uses` — `LE.Factions.ProphecySlot.TryTriggerReward` | Save completed charges; call original with the actual out parameter; after a successful trigger restore only charges that decreased. Keep original return and `chargesConsumed` output. | Small snapshot mechanics if they match another feature. Success-dependent restoration remains specific. | Failed trigger, increased charges, multiple consumed charges, exact out value; explicitly decide the original-throw case. |
| `prophecy-max-lp` — `ProphecySlotReward.SpawnRewardForPlayer` | Temporarily raise generic reward minimum LP to at least four, call original, restore the previous asset value even on native unwind. Share handler with reward multiplier; prove the eight-byte Nullable lens ABI. | Shared target; scoped restoration mechanism established with this family. | Shared asset remains pristine after normal/throwing calls; existing LP above four preserved; absent generic item. |
| `prophecy-rewards` — same `SpawnRewardForPlayer` | Temporarily scale `itemsDropped`, rounded and clamped to 0..1000, then restore. Preserve downstream lucky-lens doubling and composition with max LP. Shared reward assets must not retain repeated multiplication. | Same target and restoration record as max LP; settings. | Repeated calls do not compound; both features together; nested/concurrent calls require an explicit target-threading contract. |

## World

Source: [world.h](../../EterrasGift/native/hooks/world.h).

| Feature and original target | What must be coded | What can be shared | Distinguishing fixture case |
|---|---|---|---|
| `mysterious-rift` — `TimeBeastTracker.NextEncounterShouldBeEggs` | Return true on; otherwise forward receiver and MethodInfo and preserve original result/exception. No object reads or effect cleanup. | First complete consumer of retained gameplay gateway, settings flag, native readback and production worker integration. | True on, original answer off, original side-effect count, restored entry and absent helper after unload/reload. |
| `morditas-timer` — `RageOfMorditasLogic.OnEnter_HuntState`, `FixedUpdate_Hunt`, `OnActorDiedDuringHunt` | Snapshot times, call original, scale only that call's changed initial duration, positive elapsed delta while still hunting, or positive kill bonus. Do not revive a hunt ended by original. Three settings and targets compose. | Coherent settings triple and multi-target ownership; field descriptors. | Secondary hunt with unchanged length, ending tick, zero/negative deltas, kill popup timing and off mid-hunt without rewinding elapsed time. |
| `eternity-cache-anywhere` — `ActorCanUseEternityCacheInThisZone`, `Item.getMaxUniqueLevelRequirementForDungeonTier`, `ActorUsedEternityCache` patch | Permit the local actor, pass `INT32_MAX` tier into original level-cap calculation, suppress the final local-used store. The actor-check method's source ABI is static actor plus MethodInfo, not an instance receiver plus actor. | Mixed detour/patch ownership; identity lookup; static integer signature. | Other actor unchanged; deepest tier not lowered; used store suppressed; removing the patch and both detours is reported together. |
| `zoom` — `CameraManager.ApplyZoom` | Write configured distance before original. Keep the game's distance per camera, adopt external distance changes as the new baseline, and restore only if current value still equals our last write. Apply immediately on toggle through the main thread where available. | Lifecycle cleanup entry point; conditional restoration idea from existing writes. Keep camera state/refresh local. | Camera replacement, user changing game setting while on, off after foreign change, menu departure and cleanup before dispatcher removal. |
| `reveal-map` — `LE.UI.Minimap.Minimap.UpdateFoW_Singular` | Raise radius to at least 1000 and call original. Forward indirect float3 position exactly. Disabling does not hide already revealed map. | Mixed scalar forwarding and argument adjustment after aggregate validation. | Larger radius preserved; exact position forwarded; existing reveal persists after off. |
| `waypoints` — `MapPanel.SelectLocation` and `UIWaypointStandard.LoadWaypointScene` | For an inactive eligible waypoint, pass both availability flags true; retain the original seven-character/no-waypoint exclusion. Patch both travel rejection branches. Do not blindly unlock every scene. | Mixed target/patch ownership; object/string-length reads and long scalar ABI. | Inactive/active, missing waypoint and excluded scene; selection and travel agree; one travel-site failure cannot report full support. |
| `eternity-cache` — `RandomOneShotCacheSpawner.ExecuteSpawn` | Remove the failed chance-roll branch for endgame cache spawn. Keep eligibility and placement checks and the RNG call. This is separate from unlimited cache use. | Patch groups; identical lifecycle to other encounter-roll patches. | Eligible versus ineligible zone, no free placement, original RNG call and restored failure branch. |
| `favor-cache` — `RandomFavourCacheSpawner.CanSpawn` | Replace the final tail-call roll with true after earlier eligibility checks. This intentionally skips that RNG call; do not generalize it as the same semantics as a post-roll branch patch. | Patch groups. | Weaver/zone prerequisites still apply; final RNG call skipped on and executed off. |
| `nemesis` — `RandomNemesisSpawner.ExecuteSpawn` | Remove the chance-roll rejection branch while retaining zone/placement rules and original roll execution. | Encounter patch lifecycle. | Failed roll proceeds only in eligible placement context; off resumes normal rejection. |
| `omen-windows` — both branches of `RandomObserverWindowSpawner.ExecuteSpawn` | Remove campaign and Monolith failed-roll jumps as one feature. Both original rolls still execute. | Multi-range patch group. | Campaign and Monolith cases; one missing pattern leaves both original; original RNG advancement preserved. |
| `champions` — `RngElement.RollRangeLucky` call resolved from `RandomChampionSpawner.ExecuteSpawn` | Return configured count only for the checked champion call site; other RNG users call original. Preserve the caller-return-address distinction across the new gateway, or prove a different scoped implementation. Original game placement caps still apply. | Pattern/call-target validation and detour lifecycle. Caller context is a special adapter, not a requirement for every handler. | Two callers of the same RNG method, one champion and one unrelated; gateway must not turn the caller address into its own return site. |
| `loot-lizards` — `RandomEncounterPlacer.HandleEncountersCalculations` | Keep context and pre-call done flag; after original, set total/remaining/count-to-rate multiplier only when this call completed lizard calculations on the same context. Use 0.0001 as quotient for zero spawn rate. | Shared encounter target and typed tally helper with Rune Wardens. | Already-calculated context not refilled; replacement context untouched; zero rate; per-feature independence. |
| `rune-wardens` — same `HandleEncountersCalculations` | Apply the equivalent logic to the Warden fields, preserving encounters already consumed. | Same target and tally implementation, parameterized by the checked field set. | Lizards off/Wardens on and the reverse; completed state and existing remaining count are preserved. |
| `class-experimentals` — `RunePrisonItemDrops.SpawnExperimentalItem` | Replace the class-compatibility flag load with true at its checked instruction site. Keep the rest of item generation unchanged. | Patch-range ownership within a method also used by two other features. | Class-compatible generation on; independently remove this patch with Rune Prison/unique patches still active. |
| `rune-prison` — same `SpawnExperimentalItem` | Remove the Rune Prison Complex chance-roll refusal, preserving its level/echo eligibility and original roll. | Separate patch range in the shared method. | Low-level/ineligible echo still does not drop it; other active patches survive off. |
| `experimental-uniques` — `GetExperimentalUniqueDropChance` plus `SpawnExperimentalItem` fail-to-drop patch | Return 1 from the chance method while on and remove the later failure branch. Preserve item-level eligibility. Install/remove as a mixed unit even though the original packs both into one owner. | Mixed target/patch coordination, shared-method range checks. | Chance detour alone is insufficient; injected failure restores prior state or reports uncertainty; above-level unique still does not drop. |
| `monster-density` — `SpawnerPlacementRoom.rollSpawners` | Temporarily set density modifier to `(1 + old) * multiplier - 1`, invoke original, restore even on unwind. Validate the small by-value two-bool configuration struct. Retain the game's placement limit. | Scoped restoration after the prophecy family proves it; scalar settings. | Original modifier restored, not compounded; exception, nested call and configuration bytes; multiplier does not bypass room cap. |
| `pack-size` — `Spawner.GenerateEntitiesInternal` | Temporarily multiply `numberToSpawn` only when its original value exceeds one; preserve single boss/named spawns. Restore after original including unwind. Share with monster rarity. | Shared spawner target and restoration record. | One remains one; multi-pack scales; repeated/throwing calls do not retain modified count. |
| `monster-rarity` — same `GenerateEntitiesInternal` | Temporarily multiply common/magic/rare weightings by three coherent settings, call original, restore all weights. Compose with pack size in one handler. | Same target/restoration record; settings triple. | Each weighting, both features together, zero weights, repeated calls and foreign unwind restoration. |

## Direct edits stay outside the hook system

The remaining catalog rows are seven field sets and the Forge item editor. They need
main-thread requests, fresh observations, conditional writes and independent readback, not a
gateway per edited field. Some resulting values are recalculated by the game; that does not
automatically turn the editor into a continuous hook.

| Field set | Remaining feature work and useful reuse |
|---|---|
| `points-earned` | Read/write passive and Weaver tree points through their getters/setters; validate UInt16 range and allocated-point floor. Reuse conditional field-write transport. Values are recalculated by progression and load. |
| `currency` | Edit gold, soul embers and ancient bones by deltas through `modifyGold`, `modifySoulEmberCountOnSingleplayerOrServer` and `modifyAncientBones`. Preserve refusals, display updates and saves; share conditional write transport rather than a generic raw numeric store. |
| `keys` | Read dynamic stash entries and change quantities using the game's remove/set paths, respecting removal permission. Share the held-item request machinery with woven echoes. |
| `woven-echoes` | Same held-item traversal and quantity workflow with its own container getter and labels. Preserve entry identity and deletion behavior. |
| `attack-leech` | Extend the existing observation and conditional rate path to the remaining supported edits as needed; preserve float conversion and game recalculation behavior. No hook allocation. |
| `monolith-run` | Current/saved run lookup, corruption/Gaze of Orobyss/options-rerolled mutation, corruption recalculation, save and optional matching-panel refresh. Share write certainty, not the hook registry. |
| `faction-standing` | Rank/favor/reputation validation, joined-faction rules and required save/sync calls. Keep separate from GainFavor/GainReputation detours. |

Unique/implicit Forge roll work stays on the existing Forge planner and private-preview path.
This plan does not change its priority or add these editors to the 68-hook count.

## Build shared support at the point it becomes necessary

| Support | Build before which consumer | Why it deserves common code | What to defer |
|---|---|---|---|
| Retained gameplay entry and complete drain | First rift hook | Required for every helper callback to unload safely. Reuse current detour ownership and Windows unwind support. | Arbitrary ABI generation, caller-context adapters, counter repair during native unwind. |
| Native command, capability and independent readback | First rift hook | Every feature must integrate with existing profiles/worker/UI and distinguish requested from observed state. | A new catalog framework or second worker. |
| Target ownership and dependencies | Small form with rift; shared use by god mode/damage taken | Sharing already exists in the source. Losing another feature's target on disable is a correctness error. | A dependency solver; explicit static lists and one serialized owner are sufficient initially. |
| Settings publication | Flag with rift; scalar with damage taken; coherent group with XP/drop rates | Game-thread access cannot borrow the worker's mutable collections or block on it. | Universal snapshot/reclamation library; use the smallest proven atomic protocol. |
| Interior code/data patch groups | Forge level, then equipment requirements | Seventeen features are patch-only and four are mixed. Resolution, overlap checks, byte verification and failure recovery repeat. | A disassembler-driven generic patch language; retain explicit target-specific patterns and transformations. |
| Object descriptors and reads | Damage and player-owned return effects | Resolving field names once and reusing existing layout checks is useful. | A generic safe Unity object graph or a universal player predicate; callback lifetime/thread assumptions remain target-specific. |
| Multi-target preparation/cleanup | First patch group and first multi-hook feature | All required parts must be ready before reporting full feature support. Shared targets must survive unrelated failures. | Automatic rollback of gameplay effects already produced. |
| Scoped per-thread context | Projectiles, then duplicate | Both have nesting and unwind cleanup. Extract their actual common mechanics after both work. | Global context tracking across all features; owning TLS destructors tied to an unloadable DLL. |
| Temporary mutation and restoration | Prophecy rewards/max LP, then monster density | The shared-asset family makes the required lifetime, partial preparation and unwind behavior concrete. | Generic interception of arbitrary game memory faults or guaranteed rollback after every possible exception. |
| Unusual ABI and original-caller context | Auto-target and champions respectively | Each has a concrete consumer and a decisive fixture. | Making either a prerequisite for simple rift/scalar hooks. |

The registry should therefore describe features and their required installation units. Each
detoured native entry has one typed handler; each patch group owns its actual modified ranges.
Several ranges can live inside the same method. A mixed feature names both kinds. Resolve
addresses before publication and reject accidental entry aliases or overlapping ranges unless
they are deliberately the same shared owner.

Do not promise an all-or-nothing instantaneous gameplay transition across arbitrary methods.
Detour handlers can remain inactive until their whole feature is ready. Interior patches take
effect when written, so grouped mutation requires an explicit thread/installation policy and
recovery when part fails. A patch feature cannot be suspended by a handler's effects flag.
Any replacement of a byte patch with a detour must first show equivalent game behavior; it is
not an automatic simplification.

## Contracts to settle with the first relevant feature

### Calls and exceptions

Keep original-call placement explicit: replacement before original, argument adjustment,
result adjustment, or temporary mutation around original. Perfect rolls and boss drops execute
original work; rift and offhand skip it while active. One generic return-constant helper must
not erase that distinction.

Catch Rust panics only around helper-only work that cannot let a foreign exception cross the
catch. Invoke the original through its exact `unsafe extern "C-unwind" fn` outside those
catches. Before-original failure may forward unchanged arguments only if no game-side mutation
needs recovery. After-original failure must never invoke the original a second time. Rust's
[catch_unwind contract](https://doc.rust-lang.org/std/panic/fn.catch_unwind.html) does not promise
transparent foreign-exception propagation. The
native ABI fixture (`native-abi/src/lib.rs`, archived 2026-10-28 in `../EterrasGiftRS-archive/2026-10-28-trainer-eval/`)
deliberately has no catch around the foreign throw.

The current dispatcher executes original code before entering its counted callback. A new
gameplay gateway counting the entire handler can strand its count when an original throws
through it. Retaining the DLL forever for that generation is an acceptable conservative first
implementation only if explicitly represented as Stuck and tested; it is not evidence that
the exception itself was swallowed. Do not add an unwind finalizer merely to avoid this
limitation before a real feature needs it.

### Handler data and thread ownership

Default to bounded, allocation-free helper work without blocking locks, metadata lookup or
additional runtime calls. Permit the object reads, writes and thread-local state that each
feature actually requires. Resolve exact offsets and method signatures before enabling the
feature. Pointer equality can simplify identity checks, but cached component/actor identities
still require invalidation and lifetime rules; an address alone is not permanent identity.

The original's `guarded` uses Windows access-violation handling for some object operations;
other handlers perform direct reads. Its `staticGet` also performs runtime field access in
some hooks. Preserve useful behavior, but do not claim that a null check, atomic setting or
Rust panic catch provides equivalent memory-fault handling. Decide each unsafe target boundary
against its actual call lifetime. Shared-asset mutation needs a supported serialization/thread
assumption or another concrete implementation; a scope guard by itself does not prevent races
between two threads changing the same reward/spawner.

The original cooldown, positive-ailment and prophecy-charge handlers restore after a normal
return, not through the `OnExit` used by reward/spawner mutations. Record the chosen throw
behavior when porting them. Do not silently advertise universal exception restoration, and do
not impose rollback on a success-dependent feature without deciding what the original's
partial work means. Movement context clears instead of restoring an outer mark; echo context
persists between related calls. They cannot all use the projectile nesting policy unchanged.

### Settings and character changes

Publish settings with a concrete memory-ordering protocol. A version number does not make
ordinary concurrent payload reads safe. If a coherent multi-value snapshot uses retries, keep
them bounded and fall back to original behavior rather than waiting for a paused writer. Do
not allocate a new retained snapshot or gateway on every slider update or on/off toggle.

Separate desired profile state from effective state. A helper-owned observer must revoke
effects and identities on unknown presence, loading or an observed character change, and
refresh them before reactivation. The current watchdog polls on dispatcher callbacks at
250 ms intervals; it does not synchronously observe every transition. The original detour
`active()` checks the online flag each time. Choose and document any change to that freshness
contract with the first hook, without claiming a cached flag is equivalent.

Byte patches require actual removal when their admission disappears. Therefore loading/offline
policy must cover patch-only and mixed features before they become supported. Preserving a
connection during zone travel does not by itself authorize every old gameplay effect.

### Off and unloading

Disabled means no new application of that feature, subject to the documented in-flight-call
boundary. Shared targets may remain installed for other features. Distinguish effective-off,
still-installed, fully-removed and unknown removal instead of collapsing all into a success
Boolean. Independent readback needs settings/values, effective admission, required target
states and failure information; installed bytes alone do not prove gameplay behavior.

The current worker accepts `WriteOutcome<bool>` from `set_hook`. An enabled flag alone cannot
confirm a slider change: the production connection must compare the observed values as well
as every required target before returning Confirmed. Extend the wire observation and adapt
this boundary narrowly; do not rewrite profiles just to add richer native evidence. A
handler-side failure or helper-side auto-off also needs monitor readback so the UI cannot stay
On indefinitely after the original command succeeded.

Stop accepting new changes before shutdown cleanup. Existing admitted handlers and pending
commands must not republish effects behind restoration. Complete required feature cleanup
while the dispatcher and runtime access still exist. Camera restoration and skill recount are
specific cleanup work; draining a gateway is not their substitute. Avoid waiting on the main
thread for a callback that needs the main thread to finish. Close gameplay admission, restore
owned code/data ranges, coordinate handler drain and feature cleanup, then remove/drain the
dispatcher and release helper-owned state. The exact ordering of effect cleanup follows each
feature's thread contract. Uncertainty retains the helper and reports the actual failure.

## Implementation and extraction order

The order below exercises different requirements early. It does not rank the user's preferred
gameplay features, and it does not require finishing a whole row before shipping a completed
feature within it.

| Pass | Implement through the production worker and fixtures | Decision to make afterwards |
|---|---|---|
| 1 | Mysterious rift, then offhand using the same retained lifecycle. | Extract only the proven lifecycle and simple forwarding support. Confirm complete on/off/readback/unload, not merely generated code. |
| 2 | Incoming damage is one percentage control; outgoing multiplier includes the infinity one-hit endpoint. Both implemented on the same target. | Mixed float ABI, current-player/foe lookup and shared target composition use one retained leaf. Both controls off restores the method. |
| 3 | Forge level, then equipment requirements and a mixed feature such as experimental uniques. | Prove patch groups, non-overlap and mixed partial-failure reporting before freezing the registry shape. Extract patch mechanics here. |
| 4 | XP multiplier/instant level, faction gains, primordial limit, area, zero mana cost and cast speed. | Consolidate scalar settings and repeated owner reads. Add coherent multi-value publication with an actual consumer. |
| 5 | Perfect rolls, boss drops, always T7, force rarity, reveal map and prophecy distance; remaining simple patch features as their targets validate. | Extend only the ABI shapes exercised by these signatures. Keep original-call and RNG behavior explicit. |
| 6 | Projectiles, then duplicate, then movement speed. | Extract nested-context mechanics after two consumers; keep one-shot consumption and between-call context distinct. |
| 7 | Prophecy reward multiplier/max LP, then monster density and pack size/rarity. | Extract scoped restoration after shared assets, exceptions and target threading are understood. |
| 8 | Infinite mana, cooldowns, endless buffs, prophecy uses, shards/runes, instant rank, Monolith state effects, Morditas, encounter counts, skill slots and zoom. | Reuse established object/lifecycle support; add collection and disable-cleanup helpers only where repeated. Persistent and temporary effects remain distinct. |
| 9 | Auto-target, champions, echo cycle/type, and remaining mixed features. | Add hidden-result/caller-context/table support as demanded. Revisit APIs if these reveal a concrete mismatch, then do a bounded cleanup pass. |

For each family, complete the native behavior, helper request/readback, production connection
capability, worker outcomes and fake-host presentation before moving on. Existing worker fakes
are useful but need shared-target, actual-value mismatch, partial removal and gate-loss cases
as those become observable. Do not add another fake-only feature and call it native support.

Extract code when a second working consumer demonstrates the same mechanism and failure
semantics. Keep a short duplication temporarily when it prevents a speculative abstraction.
Make a broader cleanup pass after passes 3, 6 and 7: those are the points where patch groups,
context and restoration have enough evidence. There is no need to wait until all 68 are done.

## Verification and stopping conditions

Every supported feature must have a fixture case that distinguishes its behavior from a
no-op, plus the relevant row's exclusions and off behavior. Native gateway checks should cover
mixed integer/floating-point registers, bool/byte/integer/pointer/float returns, stack forwarding,
hidden result storage when added, original-call counts, native exceptions and close races.
Exercise the real largest supported signature instead of assuming six arguments is sufficient.

Patch tests need authored executable targets/ranges, exact pre/post bytes, missing/ambiguous
patterns, overlapping or externally changed ranges, multi-site failure recovery, and coexistence
with another feature in the same method. A pattern match alone is not a behavioral test.

Extend the native reload fixture with a continuously invoked gameplay target, including a
handler blocked across close. The Unity rift fixture checks on/off and original behavior after
helper departure; later families add only the managed objects needed for their behavior.
Reuse the existing fixture project and build output. Authored native/Unity fixtures establish
the implementation against those fixtures; they do not establish Last Epoch gameplay parity.
Live-game testing remains separately requested work.

Run `mise run trainer:verify` for implementation changes and inspect the captures. Use native
fixtures for the added unsafe paths. Stop a family when its behavior and lifecycle are proven
at that boundary; do not expand it into an unrelated general Unity/runtime framework.

## Recommendation

Use feature-first implementation with a common lifecycle from the first feature, followed by
small extraction passes between families. The source already justifies shared installation,
settings and readback. It does not justify one universal handler contract or implementing all
ABI, object, exception and cleanup machinery up front.

A generic-first effort would have to settle byte patches, shared assets, caller identity,
aggregate returns and multiple kinds of context before delivering a simple switch. Fully
independent feature implementations would duplicate precisely the ownership and unloading
logic that needs to remain consistent. The proposed sequence delivers useful features while
testing each shared design against the next materially different consumer.
