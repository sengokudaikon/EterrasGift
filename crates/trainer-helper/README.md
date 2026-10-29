# Retained Rust control helper

`eg-trainer-helper` is the explicitly started, in-process side of the trainer. It builds a Windows
x64 DLL with explicit startup outside loader lock. The desktop connects through the trainer worker.
`eg_helper_bootstrap` starts the helper once using a 96-byte configuration containing game identity,
helper build ID, returned status and controller PID/birth. Ownership is bound before the first
pipe request, so controller exit before Discover also stops the helper. Both the native preparer and the authored player use it.
The supported runtime assumptions are listed in [trainer-interop.md](../../research/trainer-interop.md).

The control OS thread owns its IL2CPP attachment and current-thread Tokio runtime. Discovery
checks the dispatcher method and modern x64 header adapters with rooted byte/string samples.
Field and invocation contracts are checked by the operations that use them. No offset table is
sent to the controller. `MethodInfo.methodPointer` at offset zero remains a supported-runtime
layout assumption.

The first successful discovery retains pristine dispatcher bytes. Subsequent status/empty-clear
requests must match this profile, even while the scheduler entry is patched. Stock detour 0.10
installs from the control thread through this helper's retained transaction wrapper in `hooks`.
Owners and registered trampoline unwind tables remain for the process lifetime; copied calls are
refused. Threads::All uses best-effort suspension, with the limits recorded in the interop notes.
Its callback calls Unity's original first,
with no Rust lock or catch around that call. After a normal return, the first accepted callback
asks Unity's native UnityEngine.Object.CurrentThreadIsMainThread predicate whether it runs on the main thread.
Only a matching callback establishes the Windows thread ID used for subsequent dispatch; other threads
forward the original without taking a command. Rust panics in this post-call work close admission;
native/managed exceptions continue to their caller. The authored Unity throwing-task route checks
that later queued tasks still run. Game graph reads serve requested operations and the bounded app-state watchdog.

Status roots the manager wrapper, checks assignability and reads its metadata-validated Unity
`m_CachedPtr`. A retained managed wrapper whose native object was destroyed reports no character.
It also reads the independent runtime online flag. No native Unity-object pointer is dereferenced.
Returned manager addresses are ephemeral observations, never managed-object ownership tokens.

The helper supports owned Forge edits, completed locale reads and persistent gameplay controls. Attack rate and
leech rate use independent character accessors and the shared hook lifecycle. It reads no definitions catalog: the app
reads the definitions from the game's files (`eg-trainer`'s game data), and the catalog and item-limits operations (21
and 19) are retired.
EditForge carries status, base/identity and regular affix-roll changes through the shared
submission/readback path. The helper validates the complete expected state and every destination
before the first store. It roots the selected graph through managed refreshes. A failure after
mutation begins is Uncertain; there is no automatic rollback or replay. Unsupported operations
remain refused. See [current scope](../../research/trainer-readiness.md).

Forge reads copy bounded rooted graphs into owned snapshots: exact storage/element types,
nested enum metadata and strict string decoding. Roots do not exclude concurrent game writes
or native-manager destruction; the synchronous supported-game boundary remains explicit.

The level-cache write still reads the item limits in the game, to check its destination: the eight exact
`System.Int32` static literal fields on `ItemData` and `ExperienceTracker.maxLevel`, through the public static
getter's metadata-default branch. It checks declaring ownership, type, non-byref storage and literal/static flags
before copying each constant, and all five ranges must satisfy `0 <= minimum <= maximum <= 255`.

`ReadLocale` observes `LocalizationSettings.s_Instance` and its already completed selected-locale operation.
The generic handle is boxed through the public field API and rooted; its operation must have matching versions,
Succeeded status and a positive reference count. The declared result field must be exactly Locale. Settings and
Locale wrappers must both have live native pointers. Uninitialized, pending, stale, failed and empty selections
return NotReady, without invoking settings/selection/initialization/waiting methods. Metadata/layout failures fail
the whole read. All objects, including the handle and returned identifier box, stay rooted through copying.

The Locale.Identifier, LocaleIdentifier.Code, Application.version and cached
ClientStateManager.CurrentClientAppStateType getters run. The real identifier
has **two references, 16-byte payload and 8-byte alignment**; the earlier one-field invocation fixture was not its
layout. The version getter may initialize Application's framework CancellationTokenSource, cache an internal call,
allocate/convert strings and dispose a native buffer. These are explicitly authorized effects; read-only means no
gameplay mutation. Body permissions and immutable runtime/graph/native-object lifetimes remain unsafe obligations.
The owned language/version payload uses two bounded strict UTF-8 texts (`eg-game-access::locale`). There is no
fallback to the project default locale.

The EditForge level-cache operation accepts a full bounded expected snapshot and opaque manager token. It roots the current
graph, compares every owned field and manager identity, validates the exact mutable Int32 backing field,
checks literal bounds and independently gates offline/native liveness. One public field API store is followed
by exact full readback. Every failure after store entry is uncertain; no RebuildID, game setter or rollback
runs. Only owned authored Unity validation is claimed. Later game recalculation can replace the level again.

Gameplay controls cover mysterious-rift selection, offhand compatibility, incoming/outgoing damage, independent
attack/leech rates and No mana cost. Each module in `game/features` owns its targets, explicit ABI contracts, effect and receiver rules.
`game/features/hooks` holds the helper's patch-group identities and common method validator; incoming/outgoing damage
share one group, while offhand and attack rate each own multiple methods. The wire carries feature IDs, settings and
observations, without knowing patch ownership.

`engine/gameplay_hooks` coordinates admission, entry transactions and cleanup ordering. Its prepared effects adapt
leaf, shared damage, pinned scalar and original-first float-result state to close/restore/publish/observe operations. Clear closes all effects,
restores receiver state and entry bytes, then releases receiver-bound groups. Every failure retains the affected
owners and pins. Generated gameplay code stays outside the DLL; the existing dispatcher drain controls helper unload.

No mana cost runs `BaseMana.getManaCost(Ability, float, bool, AbilityMutator, bool)` once before adjusting its result.
It answers zero only when the receiver matches the current player's `Actor.mana` and the online flag is clear. The
retained gateway forwards all seven ABI positions, including three stack arguments, and registers its own unwind
frame. The two five-parameter overloads are distinguished by their complete parameter signatures. No helper frame
or pinned player identity is retained across the original call; late returns after close keep the original result.

The 17 patch-only switches share owned interior ranges in `hooks/patch`, with feature-local recipes under
`game/features/patches`. They prepare complete groups before writing, check unique bounded patterns, reject range
overlap and verify original/replacement bytes. Transactions allocate before suspending peers and refuse a peer
inside a changing range. Off, manager loss and unload restore these ranges through the same lifecycle as entry
detours; failed cleanup retains owners and the helper. The thread-snapshot execution contract and all feature
distinctions are documented in [trainer features](../../research/trainer-features.md#interior-code-patches).

Shard and rune/glyph replenishment use generated before-original leaves on the same lifecycle. Each feature owns
its inventory layout and raises only existing stored quantities below 99; the original query then runs once.
Shared material helpers validate reference/primitive fields and emit the closed/online guard. The shard List/array
walk is bounded; runes/glyphs require their exact item categories. Off restores entries without removing already
granted materials. See [crafting materials](../../research/trainer-features.md#crafting-materials).

The local pipe is `eterras-gift-<PID>`, one bounded request per connection: a 12-byte header and a sequential operation body. Bootstrap binds the controller PID/birth before any request; the kernel-reported
client PID and retained process handle/start time verify each owner request. Fixture servers
without a bootstrap binding let the first valid Discover claim ownership. Normal per-request EOF preserves ownership. Other living processes receive Refused without
reaching the backend; owner death stops the helper and restores the dispatcher. A
restoration error retains the DLL rather than handing off an unconfirmed entry.
The protocol has no nonce or authentication against code in the same process.

Admission and request waits observe shutdown/process death. A five-second main-thread deadline or
disconnect cancels pending work when possible. Once started, the request retains its dispatcher
slot and failures report uncertain execution. Nothing is replayed automatically. Malformed requests
never reach discovery or dispatch. Slow clients have a separate bounded read/write deadline.

Owner-only `Unload` (operation 25, empty body) acknowledges shutdown without installing a
hook or queuing game work. Owner death also stops the server; an unclaimed server expires
after ten seconds. No module pins are taken. A retained gateway closes callback admission,
restores ExecuteTasks, and counts calls until they return out of the DLL. The control thread
waits at most five seconds for callbacks and verifies that its dedicated bootstrap thread
has exited, then drops its reactor/pipe/IL2CPP attachment before FreeLibraryAndExitThread.
Failure retains the DLL. An ACK never proves departure: the host checks module absence and
pristine dispatcher code before permitting another load.

Generated trampoline/gateway code, their unwind tables and small owners stay allocated until
process exit, so a delayed original call can return safely after the DLL has left. The
Copy-only attachment registry installs no game-thread TLS destructor. `mise run trainer:unload`
checks 100 unload/reloads under four callers and retention of a blocked callback. The server
future must still be driven to completion for restoration/drain; a panic leaves the DLL loaded.

`eg_helper_build_id()` returns 64 lowercase ASCII hex characters plus NUL. The build script hashes
relative paths and contents of the manifests, lockfile and Rust sources of core, game access, trainer wire
and helper. This is source compatibility/provenance, not authentication or a compiled-binary hash.
Evaluation separately records the DLL SHA256 and compiler identity.

Validation uses real local pipes with authored child processes plus the disposable Unity 6000.4.8
IL2CPP player in [persistent-player](../../tools/trainer-eval/persistent-player/README.md). All artifacts,
logs and phase observations stay separate from the installed game. The desktop uses this helper through the serialized trainer service. Historical
checkpoints are archived; current interop assumptions are in [trainer-interop.md](../../research/trainer-interop.md).

The game-side watchdog polls every 250 ms on the verified dispatcher. It caches the online
field, app-state getter and enum metadata. Login/CharacterSelect or online state stop the
helper immediately; InGame remains admitted across zone changes, even with a missing player.
None/unknown/failed state reads admit no work and unload after three continuous seconds.
The enum values come from literal metadata, never fixed numeric guesses.

A missing offline character during InGame returns typed CharacterLoading. The session clears
old manager/editor authority, retains the connection and binary lease, and resumes from a fresh
read without reloading. This response never authorizes a store or an automatic write retry.
`mise run trainer:player:lifecycle` checks six fresh external loads, menu/login/online/unknown
state departure, explicit Disconnect, and four seconds of InGame character absence.
