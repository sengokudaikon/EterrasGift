# Retained Rust control helper

`eg-trainer-helper` is the explicitly started, in-process side of the trainer. It builds a Windows
x64 DLL and has no loader-lock initialization or app/UI wiring. The authored Unity player calls
`eg_helper_start(legacy_tid)` after initialization; that legacy argument is ignored. The evaluation controller
also tests the explicit `eg_helper_bootstrap` native-thread entry after standard DLL loading.
The caller must satisfy the unsafe startup contract; this is not a generic Unity-version adapter.

The dedicated control OS thread owns its IL2CPP attachment and a current-thread Tokio runtime.
Discovery resolves public exports, validates all 37 legacy layout keys, field types, static fields,
selected method signatures and modern x64 header adapters. It allocates rooted byte/string samples
to cross-check the adapters. It does not invoke arbitrary game methods. The exact Unity 6000.4.8
`MethodInfo.methodPointer` position remains an explicit unsafe layout contract. Unsupported metadata
fails the entire profile; no partial discovery is published.
These are legacy discovery checks, including legacy generic-name matching. They do not authorize
future field writes or invocations; those need exact per-operation type, storage and lifetime checks.

The first successful discovery retains pristine dispatcher bytes. Subsequent status/empty-clear
requests must match this profile, even while the scheduler entry is patched. The existing bounded
first-party scheduler installs from the control thread. Its callback calls Unity's original first,
with no Rust lock or catch around that call. After a normal return, the first accepted callback
checks the current UnitySynchronizationContext main managed ID against Thread.CurrentThread.ManagedThreadId.
Only a matching callback establishes the Windows thread ID used for subsequent dispatch; other threads
forward the original without taking a command. There is no pre-bootstrap main-thread nomination. Rust panics in this post-call work close admission;
native/managed unwind recovery is not promised. Independently, the callback observes runtime status
at most once per second while an owner is present.

Status roots the manager wrapper, checks assignability and reads its metadata-validated Unity
`m_CachedPtr`. A retained managed wrapper whose native object was destroyed reports no character.
It also reads the independent runtime online flag. No native Unity-object pointer is dereferenced.
Returned manager addresses are ephemeral observations, never managed-object ownership tokens.

Protocol28 supports ReadForge (18), ReadItemLimits (19), ReadLocale (20), ReadCatalogPage (21) and the narrow
SetLevelRequirement (22), ReadAttackLeech (23) and SetAttackLeechRate (24), alongside Discover,
Status and an empty ClearHooks. The forge wire model
is in `eg-game-access::forge`. The read traverses rooted containers on the engine thread, validates
exact scalar/reference classes, enum storage and array/list element types, and copies every value
into owned Rust storage. Enum identities come from their actual field metadata, supporting nested
and namespaced declarations. Strings use bounded strict UTF-16/UTF-8. Affixes are limited to 32,
implicit rolls to three, unique/set rolls to eight; List capacity may exceed count, bounded to 4096.
The main-thread copy requires no concurrent mutation of the graph or native-manager destruction.
Runtime offline/native-manager checks precede item-graph access. No game methods are invoked.

`ReadItemLimits` reads the eight exact `System.Int32` static literal fields on `ItemData` and
`ExperienceTracker.maxLevel`, using the public static getter's metadata-default branch. It checks
declaring ownership, type, non-byref storage and literal/static flags before copying each constant.
All five ranges must satisfy `0 <= minimum <= maximum <= 255`; level minimum zero follows the
original helper. Runtime offline/native-manager admission runs first, independently of Session.
The returned owned ranges omit localization, rarity names and the rest of the definitions catalog.
Their exact 40-byte codec is `eg-game-access::definitions`. Older protocol versions are rejected.

`ReadLocale` observes `LocalizationSettings.s_Instance` and its already completed selected-locale operation.
The generic handle is boxed through the public field API and rooted; its operation must have matching versions,
Succeeded status and a positive reference count. The declared result field must be exactly Locale. Settings and
Locale wrappers must both have live native pointers. Uninitialized, pending, stale, failed and empty selections
return NotReady, without invoking settings/selection/initialization/waiting methods. Metadata/layout failures fail
the whole read. All objects, including the handle and returned identifier box, stay rooted through copying.

Only the audited Locale.Identifier, LocaleIdentifier.Code and Application.version getters run. The real identifier
has **two references, 16-byte payload and 8-byte alignment**; the earlier one-field invocation fixture was not its
layout. The version getter may initialize Application's framework CancellationTokenSource, cache an internal call,
allocate/convert strings and dispose a native buffer. These are explicitly authorized effects; read-only means no
gameplay mutation. Body permissions and immutable runtime/graph/native-object lifetimes remain unsafe obligations.
The owned language/version payload uses two bounded strict UTF-8 texts (`eg-game-access::locale`). There is no
fallback to the project default locale and no locale/catalog disk cache yet.

SetLevelRequirement accepts a full bounded expected snapshot and opaque manager token. It roots the current
graph, compares every owned field and manager identity, validates the exact mutable Int32 backing field,
checks literal bounds and independently gates offline/native liveness. One public field API store is followed
by exact full readback. Every failure after store entry is uncertain; no RebuildID, game setter or rollback
runs. Only owned authored Unity validation is claimed. Later game recalculation can replace the level again.

The complete hook feature set is currently empty:
`features_on = 0` and clear replies contain count zero. No gameplay hook can be enabled; other item,
stat or gameplay field writes remain unavailable. Before adding effects, their
runtime-offline/owner-loss cleanup must run and acknowledge completion on the engine thread.

The local pipe is `eterras-gift-<PID>`, one strict 336-byte request per connection. The first valid
Discover claims ownership using the kernel-reported client PID and a retained process handle/start
time. Normal per-request EOF preserves ownership. Other living processes receive Refused without
reaching the backend; process death relinquishes ownership. This strengthens the original helper's
ownership model. Protocol 25 still has no nonce or authentication against code in the same process.

Admission and request waits observe shutdown/process death. A five-second main-thread deadline or
disconnect cancels pending work when possible. Once started, the request retains its dispatcher
slot and failures report uncertain execution. Nothing is replayed automatically. Malformed requests
never reach discovery or dispatch. Slow clients have a separate bounded read/write deadline.

`eg_helper_shutdown()` accepts an asynchronous stop request. The worker closes admission, attempts
entry restoration, releases its attachment and stops the server. Metrics 9=1 and 10=0 confirm entry
restoration and server stop; they do not prove all callbacks have drained. Published trampoline,
unwind table, helper module and GameAssembly remain retained until process exit. No unload is allowed,
including after failure. A worker panic may leave the entry installed with a stop-only forwarding
callback; cleanup then remains unconfirmed. The public server future must be driven to completion:
dropping it cannot run asynchronous shutdown/restoration.

The evaluation bootstrap uses an exact 112-byte config with process/main-thread/source identity and
start/stop/observation operations. OS thread ownership/name only corroborates the caller's established
Unity main-thread identity. A stop returns addresses of three retained atomic counters so the external
controller can poll with process reads, without creating threads during patch restoration. Counter
values and original entry/unwind bytes are checked independently. The source-ID DLL filename allows
a distinct generation after a sole older generation is explicitly proven stopped and queried again;
all older modules remain retained. Repeated startup of the same generation is refused. This is an
experimental evaluator, not desktop-app injection or a general reload/unload mechanism.

`eg_helper_build_id()` returns 64 lowercase ASCII hex characters plus NUL. The build script hashes
relative paths and contents of the manifests, lockfile and Rust sources of core, game access, trainer
and helper. This is source compatibility/provenance, not authentication or a compiled-binary hash.
Evaluation separately records the DLL SHA256 and compiler identity.

Validation uses real local pipes with authored child processes plus the disposable Unity 6000.4.8
IL2CPP player in [persistent-player](../../tools/trainer-eval/persistent-player/README.md). All artifacts,
logs and phase observations stay separate from the installed game. The desktop application remains
unwired to this helper.
