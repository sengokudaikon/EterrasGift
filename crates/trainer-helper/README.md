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

Argument hooks use the same retained leaf lifecycle: they adjust only their declared scalar
argument and tail-call the original in place. Map/prophecy positions remain opaque indirect
vectors; new-item rarity preserves the full long signature, nullable payloads and byref outputs.
Map and objective revealing keep the game's existing update/event triggers and persist after off.

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

The helper supports owned Forge edits, completed locale reads and gameplay hooks. It also adds affix definitions the
desktop sends (a unique's modifiers, `game/features/new_affixes.rs`) and takes them out again before it leaves.
It reads no definitions catalog:
the app reads the definitions from the game's files (`eg-trainer`'s game data), and the catalog and item-limits
operations (21 and 19) are retired.
EditForge carries status, base/identity and regular affix-roll changes through the shared
submission/readback path. The helper validates the complete expected state and every destination
before the first store. It roots the selected graph through managed refreshes. A failure after
mutation begins is Uncertain; there is no automatic rollback or replay. Unsupported operations
remain refused.

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

Gameplay hooks: each module under `game/features` owns its targets, ABI contracts, effect and receiver rules, and
documents its behaviour in its `//!` docs. `game/features/registry` lists those features (`FEATURES`), each with the
controls it serves; a feature's place is its patch group (several controls can share one, as damage and experience
do). Shared settings map each control to its own atomic word, including independent readback and last-owner
removal; the coordinator has no feature-specific sibling handling. The wire carries feature
IDs, settings and observations, never patch ownership. `engine/gameplay_hooks` coordinates admission, entry
transactions and cleanup ordering: Clear closes all effects, restores receiver state and entry bytes, then releases
receiver-bound groups, and every failure retains the affected owners and pins. Generated gameplay code stays outside
the DLL; the existing dispatcher drain controls helper unload.

Mixed leaf/patch features use the same group and independent byte readback. Their gateways stay closed while the
detour transaction runs, then the interior-patch transaction; a later failure restores the previous detour states.
These are compensating transactions, not one simultaneous write: interior patches can briefly take effect before
the final settings publication. Cleanup attempts both kinds and keeps every owner if any restore fails. Waypoints,
experimental uniques and Eternity Cache anywhere each resolve their complete target set before activation; their
specific exclusions and checked code recipes stay in their feature modules.

`game/features/gains` holds before-original progression hooks: character/skill experience and level-up share one
target, while favor and rank gains retain their own targets. The XP pair is two finite f32 values in one aligned
64-bit publication, loaded once per call; arithmetic uses f64 with explicit integer caps. These leaves preserve all
other arguments and tail-call the original once, including on closed/online paths. Original exceptions pass straight
through, with no retry or helper return frame. The game owns level/rank changes, favor-derived reputation and saves.

`game/features/player_only` shares a fresh receiver check for local item-manager and actor-component leaves.
Several primordials returns zero from the local counter used by equip and stat checks; Fixed increased area returns
the configured value for the player's stats; Refill mana converts the player's current maximum before each normal
tick. Other receivers and closed/online paths forward unchanged. No helper callback or cached component is retained.

Movement speed selects updateSpeed's checked, single movement-stat call inside `Stats.GetStatValue`. The retained
postfix gateway preserves all eleven native positions, the original property register and caller return address;
no thread-local mark or helper frame spans the original. Unsupported call recipes refuse activation. Nested calls
are independent, and normal recalculation (such as re-equipping boots) applies an on/off or value change.

Loot Lizards and Rune Wardens share one original-first gateway and keep independent settings. Two words in each
call's incoming home area capture its context and done/enable flags before the original; these survive original
home writes without extra stack allocation or a helper callback. Only newly completed calculations on that same
context receive counts, so consumed encounters remain consumed. Nested calls and threads have separate snapshots;
exceptions run no post-call writes. Champions uses a leaf selected by the checked spawner call's original return
address; unrelated RNG calls and the game's placement limit remain intact.

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

The build ID is the SHA-256 of the helper's DLL. The desktop hashes the exact bytes it loads and gives the
ID to the helper in the bootstrap block, and the helper reports it back in discovery. A helper left in a game
by another build reports another ID. This is compatibility/provenance, not authentication.

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
