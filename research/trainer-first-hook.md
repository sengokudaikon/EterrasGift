# First gameplay hooks: mysterious rift and offhand

The [feature implementation plan](trainer-feature-plan.md) inventories all 68 hook features,
separates shared support from gameplay code, and proposes implementation and extraction order.

The first implemented target is `TimeBeastTracker.NextEncounterShouldBeEggs()` from the original
`native/hooks/world.h`. It is an instance Boolean method with a receiver and hidden MethodInfo:

```rust
type RiftOriginal = unsafe extern "C-unwind" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> bool;
```

The row now selects **Game decides / Portal / Boss**. Portal returns true; Boss returns false,
without calling the original. Game decides restores the entry and forwards both arguments,
preserving the original result and exceptions. One retained atomic answer extends the existing
leaf; it needs no receiver dereference, player selection or gameplay-field restoration. Both
the enabled flag and selected answer are independently read back with actual patch bytes.
Profiles with the old switch enabled retain Portal; a choice change updates wanted state and
value together, so selecting Boss cannot briefly submit Portal first.

Static tracing of the installed GameAssembly (SHA-256
`323e9183f3bee600b6c9e1c469218b9de926e9044f2ca3d44fae8a0ec9a91464`) found
`RandomTimeBeastSpawner.ExecuteSpawn` and `RandomEncounterPlacer.PlacePlayerDependentEncounters`
branching on this predicate: true calls `CaveManager.CreateCave`, false calls
`TimeBeastRiftEncounter.PlaceEncounter` / `TimeBeastData.SpawnRegularRift`. `TimeBeastRift.Open`
starts its enemy spawner. This selects newly eligible encounters; cave availability, spawn
rules and special scripted paths still apply. In particular, a specific woven echo can bypass
the predicate in `MonolithZoneManager.SetupTimeBeastObjective`.

Selection verification (2026-10-27): release trainer/controller tests and UI tests passed;
workspace lint, formatting, tokens and fake-host captures passed. Standard and 680px captures
were inspected. The native fixture passed 100 reloads with Portal/Boss switches and blocked
callback retention; the Unity fixture passed six loads, mode switches with original-call
counts, travel suspension and departure restoration. The desktop debug build was refreshed.
These are owned fixtures, not live Last Epoch encounter validation.
The user separately confirmed the selector works in the live game on 2026-10-27.

Offhand uses that same lifecycle for four concrete targets:

- The alternate-base-type predicate on `OneSlotItemContainerWithCharacterRequirements`
  returns true only for an exact `OffhandItemContainer` receiver. Placement and equipped
  weapon eligibility therefore agree without requiring a dual-wield unlock.
- `OffhandItemContainer.IncompatibleDueTo2hWeapon` returns false, allowing two-handed pairings.
- `LinkedOneSlotItemContainer.incompatibleWithItemInLinkedSlot` returns false for an offhand
  receiver or a receiver whose `linkedSlot` is an offhand. This handles both weapon-slot
  directions without bypassing restrictions on unrelated linked slots.
- `OffhandItemContainer.EnforceWeaponLimitations` returns without work. Its inlined two-handed
  check would otherwise move one of the accepted weapons into the inventory or cursor.

The original `CanReceiveItem` still checks the item's level, class, faction, primordial limit,
cocoon state and quantity. Turning the feature off does not unequip accepted items or force
cached equipment stats to refresh; re-equip to observe the changed policy immediately.

Both features use process-owned constant-return leaf gateways in `eg-game-access::hooks::leaf`.
Each offhand leaf reads the same retained atomic enable switch. A leaf returns its Boolean
constant (or void) when enabled and otherwise tail-jumps its retained trampoline. The offhand
eligibility guard reads the receiver's class header; the linked-slot guard also reads the
metadata-validated `linkedSlot` reference and its class header. These bounded reads match the
original method's live-receiver/reference contract, with no calls or allocation between them.
This is the specific justified exception to the draft's proposed ban on gameplay-object reads.
The leaves call no helper code and change no argument registers or stack. Native exceptions
follow the original caller's unwind path; only the dispatcher needs a callback counter/drain.

Metadata resolution checks each method's instance/static status, Boolean/void return and exact
parameter types, rejecting ambiguous overloads, generic/shared bodies and invalid code ranges.
The linked reference's type, instance storage and aligned offset are checked from metadata.
RetainedHook validates each copied prefix. No audited RVA is a startup prerequisite.

One shared owner prepares all targets before publishing, closes the feature's switch before
any transaction, patches/restores the group together, and opens the switch only after verified
success. Readback aggregates every actual entry: mixed or unknown bytes report Unknown, never
a confirmed on/off. A failed transaction retains ownership with the effects closed. The shared
status/watchdog path closes/restores all entries on loading, online state, failed status or
manager change. All gameplay entries restore before the dispatcher drains; failed restoration
keeps the helper resident. No new helper callback runs on gameplay threads.

The production session sends one set request and requires a separate manager-bound entry/flag
readback before reporting on or off. Its monitor rechecks hooks it has used. The catalog and
worker capabilities also include the combined incoming-damage row described below.

Validation uses the authored native reload test, the authored Unity lifecycle player, session
certainty tests and fake-worker UI captures. Unity checks each original's side-effect counter,
independent toggles, unchanged offhand arguments, other equipment slots, item requirements,
placement versus equipped-item eligibility, two-handed/linked pairing in both directions,
automatic displacement, unrelated linked slots, travel suspension and original behavior after
unload across six loads. Authored fixtures do not establish live Last Epoch behavior.

## Offhand correction, 2026-10-27

The user reported a level-appropriate one-handed axe accepted into offhand but marked with
an X. The original C++ feature and first Rust implementation only forced `CanReceiveItem`.
Placement alone was insufficient, and bypassed more item requirements than intended.

Static trace of the installed Steam build 26087601 (no game process opened):

- GameAssembly.dll SHA-256 `323e9183f3bee600b6c9e1c469218b9de926e9044f2ca3d44fae8a0ec9a91464`;
  metadata SHA-256 `2a4e8440606c420345934158b6784d8f5d8b0184f32fd91af0e9e0903ba94f83`.
- `InventoryItemUI.get_IsEquippedItemNotApplyingStats` calls
  `ItemEquipManager.ItemIsEquippedWithStatsApplied` at VA `0x1813f0af2` and inverts its result.
  The warning therefore reflects the game's applied-stats record, not merely placement.
- `Offhand_ItemAdded` calls `EquipWeapon`; its offhand branch calls
  `ItemContainersManager.CanEquipInOffhand` at `0x181437c76`. A false result skips equipping
  the weapon through the actor's equipment path.
- `CanEquipInOffhand` (`0x1813f80e0`) forwards the item base type to the offhand slot's
  `OneSlotItemContainer.CanAddItemType` (`0x18144d360`). That method and the inlined type
  check in `OneSlotItemContainer.CanReceiveItem` call virtual slot 51, the alternate
  sufficient-base-type predicate (`0x18144d140`), before consulting the normal allowed list.
- `UpdateOffHandWeaponIfInvalid` likewise uses `CanAddItemType` at `0x1814135f3` and feeds
  its result into `UpdateOffHandInfo` and `UpdateStats`. It runs on character load and
  character-mutator updates; toggling a hook alone does not call it.
- `OneSlotItemContainerWithCharacterRequirements.CanReceiveItem` (`0x18144cc20`) retains
  class/faction checks, primordial/cocoon restrictions and the level comparison. It still
  runs because the offhand entry is no longer replaced.

The pairing extension also traces `IncompatibleDueTo2hWeapon` (`0x18144bb80`) from both
`CannotPutItemInMainHandBecauseOfOffhand` and `CannotPutItemInOffhandBecauseOfMainhand`.
`UpdateOffHandWeaponIfInvalid` uses that check and the main-hand linked-slot check before
applying offhand stats. `EnforceWeaponLimitations` (`0x18144b630`) inlines the two-handed
check, calls the linked-slot predicate, then moves conflicting equipment: replacing only
the Boolean predicates would miss this path. Its MSVC prologue starts with a redundant
REX-prefixed push; the backend now preserves that byte after stock relocation so registered
unwind offsets still match. The authored fixture exposed the same case. `OffhandItemContainerUI.UpdateOffhandState`
has a separate two-handed ghost/placeholder display; this correction does not replace that
UI method. Live verification must distinguish this visual from the equipped-stat X.

Addresses were resolved from the current metadata method tokens and current code-generation
module table, then traced in native instructions. MelonLoader's older generated assemblies
were used only for signature context, not current addresses. REA identified the native file;
its full Ghidra analysis timed out, so the focused instruction trace used Capstone instead.
Ignored evidence and analysis scripts are under `artifacts/trainer-eval/offhand-trace/`.
The corrected target deliberately departs from the original C++ implementation. On 2026-10-27,
the user confirmed that the correction works in the live game. The exact combinations tested
were not recorded; the authored fixtures separately cover unmet requirements and on/off/unload.

See the [original inventory](../tools/trainer-eval/hooks-fields-audit/hook-inventory.md),
the hook evaluation (archived 2026-10-28 in `../EterrasGiftRS-archive/2026-10-28-trainer-eval/`; `crates/trainer-helper/tests/hooks.rs` replaced it) and
[interop notes](trainer-interop.md). The former unpatched/bridge experiment narrative and
artifacts are preserved by `archive/trainer-evaluations-2026-10-19`.

## Incoming damage, 2026-10-27

The user chose one **Damage taken** control: 0%-100% in whole percentages, default 50%.
0% replaces God mode, 100% passes the amount unchanged, and disabling restores the entry.
The stable ID is `incoming-damage`; profiles migrate old `damage-taken` divisors to the nearest
whole percentage. An enabled old `god-mode` wins at zero. Already migrated values take precedence.
The original two features become one row (67 hook rows, 3 supported, 64 unsupported).

Original behavior comes from `native/hooks/combat.h`: `float BaseHealth.HealthDamage(float)`
reads `BaseHealth.actor`, compares with `PlayerFinder.playerActorComponent`, changes incoming
player damage, and calls the original exactly once. We preserve that call even at zero; skipping
it would suppress unrelated game bookkeeping. Enemies and other actors remain unchanged.

The shared lifecycle is now named `gameplay_hooks`, with a third concrete target and bounded
setting byte (Boolean selection for rift, fixed one for offhand, 0..100 for damage). Both enabled
state and setting must match independent byte/setting readback. Values publish atomically;
changing them follows the existing close/transaction/open ordering. Profile, worker, UI,
helper-side status suspension and unload all use the same path as the first two features.

The damage leaf (`discovery::damage`, generated code on the retained leaf's `FRAME`) keeps the exact Windows x64 ABI:
RCX is the health receiver, XMM1 the float amount, R8 the hidden MethodInfo. RAX/R11/XMM0 are
volatile scratch; the stack is untouched, and the original's XMM0 return is preserved by a
tail jump. Intermediate percentages calculate in double precision, then convert back to float.
Zero explicitly replaces even NaN/infinity; 100 and disabled paths leave the amount untouched.
Negative amounts scale as in the original C++ feature.

Static check of the installed GameAssembly with SHA-256
`323e9183f3bee600b6c9e1c469218b9de926e9044f2ca3d44fae8a0ec9a91464`
found `BaseHealth.HealthDamage` at RVA `0x10fbeb0`, consuming XMM1, and the exported
`il2cpp_class_get_static_field_data` at RVA `0x2170`. The retained disassembly is
`artifacts/trainer-eval/offhand-trace/BaseHealth.HealthDamage.asm.txt`; these addresses
are evidence only, never compiled into the hook.

Metadata validates the method signature and actor reference layout. Public
`il2cpp_class_get_static_field_data` supplies process-owned storage for the current player and
online flag, after the public field API initializes the class. Both addresses and offsets are
checked. The leaf reads those statics on every call, forwarding when online, missing a player,
or called on another actor. It reads just the receiver's actor field and compares the pointer;
it never follows the player object or enters the helper/runtime. This relies on the supported
non-moving IL2CPP heap, atomic aligned reference stores, valid method receiver and static
storage lifetime. Unlike the OG SEH guard, it does not recover from an invalid receiver pointer;
that is the original method's own readable-object precondition.

Authored coverage includes native percentage/identity/online/original-call tests, the native
100-reload fixture with concurrent damage calls, Unity percentage changes and C++ exception
propagation from the original, travel/manager departure, profile migration, independent wire
readback, and real worker UI controls on a fake host. These are not live-game proof.

Live checklist: with an offline character and repeatable incoming attacks, compare off/100%,
50%, and 0%; enemies should still take normal damage. Change the percentage while enabled,
turn it off, travel to another zone, reconnect, and disconnect the trainer. Confirm original
incoming damage returns on off/disconnect and the helper leaves when the character leaves play.

Validation evidence for this slice: the Unity lifecycle passed six loads with 20 percentage
probes (`artifacts/trainer-eval/lifecycle-run-c66fa738d5e442088621e992e55d64f1/report.json`).
The native reload test passed 100 cycles with running damage callers and blocked-callback
retention in 448.76 seconds. Release profile/catalog/session regressions passed, including
percentage readback mismatch. The 0%, 50% and 100% worker/UI captures are retained as
`artifacts/captures/backend-hook-damage-*.png`. The temporary Unity project and superseded
helper DLL/PDB were removed; the successful helper snapshot, reports and static disassembly
remain as validation evidence. Live Last Epoch behavior is still unverified.

Final workspace verification passed release trainer/controller tests, all 90 UI unit tests,
lint, formatting and token checks. The complete capture run passed all gameplay-hook cases
and then timed out waiting for the unrelated Forge "Unusual values" fixture item
(`crates/ui/tests/trainer_backend_fixture/mod.rs`). Full `trainer:verify` therefore remains
failed at that capture, amid concurrent Forge changes. The damage control was visually checked
at standard and narrow widths. `mise run build` refreshed `target/debug/eterras-gift.exe`.


## Outgoing damage and shared target, 2026-10-27

Damage multiplier now covers 0–100× and a separate one-hit mode displayed as ∞.
The saved finite endpoint is 101; it is never multiplied into damage. The number box
steps from 100 to ∞ and back, and accepts ∞/inf/infinity. The separate one-hit row
is removed; legacy wanted `one-hit-kill` migrates to an enabled multiplier at 101.
This leaves 66 hook rows: 4 supported controls, 62 unsupported.

The OG `native/hooks/combat.h` filters target actors through
`actor.alignmentManager.alignment`, compared with the current player's alignment.
Same or missing alignments pass through. Otherwise `ours.isFoe[theirs.alignmentID]`
selects enemies with a bounds check; a missing array follows the OG fallback that
different alignments are foes. This includes damage inflicted by allies or minions.
It does not establish who caused the hit. Numeric mode multiplies the argument;
one-hit mode raises a positive hit to current health when that is larger. Zero,
negative and NaN hits receive no one-hit floor. The original is called once.

Metadata validates the reference/scalar types and offsets before installation.
The leaf reads this managed graph directly, with no helper calls, allocation,
locks or runtime API calls. It relies on the supported game's managed graph
remaining readable for the call, as the OG hook does. A same-thread native access
fault is not caught by this leaf. This is an explicit supported-runtime assumption,
not something metadata validation can prove. The online static byte still bypasses
effects immediately, and helper status clears all settings on character transitions.

Two feature IDs map to one owned HealthDamage target. Each effect publishes its
value/enabled state in one aligned 64-bit store (disabled sentinel); changing one
preserves the other. Closing a target suppresses both, restores its bytes and leaves
only process-owned code/data behind. A subsequent enable resets stale sibling
settings. Independent readback distinguishes Original, Patched, Unknown and Shared:
Shared means this feature is off, actual bytes are patched, and the sibling effect
is active. Only damage controls can confirm this shared-off state.

Wire settings now occupy a u64: integer percentage/Boolean values for earlier hooks,
exact f64 bits for outgoing damage. Decoder and session reject invalid ranges,
nonfinite multipliers and values between 100 and the one-hit endpoint. The existing
helper artifact binding requires matching desktop/helper builds.

Focused verification covers emitted code (foes/allies, null fields, bounded arrays,
OG fallback, multiplier/one-hit transitions, original arguments/return/call count),
actual patch sharing and restoration, clear/re-enable, wire/readback and profile
migration. The Unity fixture's metadata is updated for the extra fields; its full
lifecycle/captures are not rerun for this change, per the requested scope.

Live verification pending: compare off/1×/2×/∞ against an enemy, confirm allied/minion
health is unaffected, then toggle each damage control off while the other remains on.
Finally disconnect and confirm ordinary damage in both directions.

Verified for this slice: `mise run build` and `mise run lint` pass. Focused release
checks pass for the native leaves (8), shared helper lifecycle (2), wire hook codec
(1), session hook readback (8), catalog/profile behavior (18), worker profile
commands (11), and the two affected UI model tests. No captures or live game run.
The normal debug desktop/helper build is retained for manual verification; no new
scratch workspace or Unity build was created.


## Independent persistent scalar controls (2026-10-28)

Attack rate and increased leech rate now have separate profile switches and absolute values (the previous editor's
0.01–20 and -1–100 ranges). The combined `attack-leech` field editor, paired snapshots, planner and read/write RPCs
are retired; operation numbers 23 and 24 remain rejected. Other direct field editors remain unsupported.

Each feature selects one character accessor. `features/stat` resolves and reads just that selected field over the
rooted current player, using the shared reader. `persistent_stat` validates mutable scalar storage through `apply`
and pins the managed receiver for the enabled lifetime. Discovery still resolves metadata only.

Shipped-code inspection of GameAssembly SHA-256 `323e9183f3bee600b6c9e1c469218b9de926e9044f2ca3d44fae8a0ec9a91464`:
- `CharacterStats.calculateAttackRate`, `setMainHandAttackRate(float,bool,bool)` and `setOffHandAttackRate(float,bool)`
  each write `attackRate`; the weapon setters inline the calculation, so all three entries are covered.
- `BaseStats.ApplyExternalStats` resets and rebuilds `increasedLeechRate`. Its caller `UpdateStatsInternal` runs
  notifications afterward, so the override is present before those notifications. Leech consumers read the field
  directly; replacing `getIncreasedLeechRate` would not cover them.

The shared lifecycle now supports both retained leaves and retained original-first scalar gateways. The latter
preserves register arguments and, where needed, the fifth stack argument, runs the original once, then records its
new field value and applies the selected bits only for the pinned receiver while offline. Its unwind table and code
remain process-owned; no generated path calls helper code. Recalculations are assumed serialized on the game's main
thread. A pending original may finish after close, but its closed post-call path cannot reapply the override.

Changing a value closes/drains the override, conditionally restores the latest baseline and applies the new value.
Off, manager loss and unload close admission, drain scalar stores, conditionally restore fields, restore entry bytes
and release pins. A later independent game write wins over conditional restoration. Failure retains installed owners
and pins. Independent readback checks patch state, settings and the current player's selected scalar/identity; a
replacement stats object cannot be confirmed using the old pinned object. These are runtime overrides, not saves.

Targeted authored tests cover independent accessor availability, graph/type/root checks, recalculation, receiver and
online filtering, the fifth argument, value changes, entry restoration and a delayed original crossing close.
Session tests cover separate controls, values and readback. Live Last Epoch verification remains pending; no captures
or real-game run were part of this change.

## Feature ownership cleanup (2026-10-28)

Each implemented hook now has a module that owns its method targets, explicit parameter/return contracts and behavior:
`mysterious_rift`, `offhand`, `damage`, `attack_rate` and `leech_rate`. The two damage controls share one implementation
because they transform the same call and must preserve each other's settings. Attack and leech keep independent stat
selectors beside their recalculation targets; the shared scalar reader/binder accepts a selector without feature IDs.

`features/hooks` retains only the helper-local group identities, feature lookup, resolved target types and common
metadata validator. A group can own several methods; damage controls select the same group. Patch topology and
damage/stat classifications have been removed from the wire. The session validates settings and observations and
accepts the helper's verified Shared state for an off feature without duplicating its ownership map.

`engine/gameplay_hooks` owns admission, retained patch groups, entry transactions and cleanup ordering. Its prepared
effect adapter owns gateway settings, shared damage semantics and pinned receiver state behind close, restore,
publish and observation operations. The coordinator no longer branches on damage or stat features. All effects close
before any restoration; receiver-bound groups are released only after restoration succeeds. Failure keeps the group
and its pins retained. The generated gateway code and read/apply primitives are unchanged.

Targeted regressions cover multi-entry leaf preparation/restoration, shared damage settings and last-owner disable,
independent scalar restoration, selected-accessor checks and topology-independent session readback. The fixture build,
app build and workspace Clippy pass. No captures or live-game run were requested.

## No mana cost (2026-10-28)

The next hook is the existing Combat > Resources switch `zero-mana-cost`. Behavior follows
`../EterrasGift/native/hooks/combat.h`: call the original cost calculation exactly once, then answer zero only for
the player's mana component. Other actors keep their costs; this neither fills mana nor changes cooldowns.

Static metadata for GameAssembly SHA-256 `323e9183f3bee600b6c9e1c469218b9de926e9044f2ca3d44fae8a0ec9a91464` and metadata
SHA-256 `2a4e8440606c420345934158b6784d8f5d8b0184f32fd91af0e9e0903ba94f83` confirms two five-parameter overloads:
AbilityID comes before Ability. The chosen contract is instance `Single(Ability, Single, Boolean, AbilityMutator,
Boolean)`, plus receiver and MethodInfo. The common validator now selects the entire parameter signature before
checking the selected overload's ABI/uniqueness. Runtime names/types and storage layouts remain checked on enable;
the observed addresses/offsets are evidence only.

`features/zero_mana_cost` owns the contract, Actor.mana layout and post-call predicate. A small retained gateway
forwards four register positions and three stack arguments, calls the original, restores the receiver and applies
that predicate to XMM0. It preserves original side effects and propagates unwinding through its registered 72-byte
frame. Code, return path, data and unwind metadata stay process-owned; no generated path calls into the helper.
The current player/online state is read after the original returns. The player/actor/mana graph must remain readable
over this supported-game post-call span, as in the original trainer's owner check. No extra object pin or restoration
is required because the effect changes only the returned value. Closing before a delayed original returns suppresses
the adjustment, and the retained path remains valid after the helper leaves.

The feature reuses patch ownership, independent flag/byte readback, character profiles, suspension and unload ordering.
The lifecycle coordinator has no new feature-specific branch. Targeted native tests cover argument forwarding,
original side effects, player/enemy/null and online filtering, replacement player identity, unwinding without retry,
late original return after owner drop and on/off/clear entry restoration. Session/catalog and wire tests cover the
new switch through existing commands. App build, Clippy and the fixture build pass; no captures or live-game run.

Live check when requested: on an offline character, compare an ordinary mana skill and a channeled skill off/on/off.
With No mana cost on, their mana costs should be zero while natural regeneration/drain behavior remains the game's.
Disable should restore normal costs; zone/character changes and reconnect should follow the saved profile and shared
lifecycle. Keep Mana always full and No cooldowns off while checking this feature.
