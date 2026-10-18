# Retained Rust control helper

`eg-trainer-helper` is the explicitly started, in-process side of the trainer. It builds a Windows
x64 DLL and has no loader-lock initialization, injection API or app/UI wiring. The authored Unity
player calls `eg_helper_start(main_tid)` from its established engine main thread after initialization.
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
with no Rust lock or catch around that call. After a normal return it checks the established main
thread and takes at most one owned command. Rust panics in this post-call work close admission;
native/managed unwind recovery is not promised. Independently, the callback observes runtime status
at most once per second while an owner is present.

Status roots the manager wrapper, checks assignability and reads its metadata-validated Unity
`m_CachedPtr`. A retained managed wrapper whose native object was destroyed reports no character.
It also reads the independent runtime online flag. No native Unity-object pointer is dereferenced.
Returned manager addresses are ephemeral observations, never managed-object ownership tokens.

Only Discover, Status and an empty ClearHooks exist. The complete feature set is currently empty:
`features_on = 0` and clear replies contain count zero. No gameplay hook can be enabled and no item,
stat or other gameplay field can be written through this helper. Before adding effects, their
runtime-offline/owner-loss cleanup must run and acknowledge completion on the engine thread.

The local pipe is `eterras-gift-<PID>`, one strict 336-byte request per connection. The first valid
Discover claims ownership using the kernel-reported client PID and a retained process handle/start
time. Normal per-request EOF preserves ownership. Other living processes receive Refused without
reaching the backend; process death relinquishes ownership. This strengthens the original helper's
ownership model. Protocol 21 still has no nonce or authentication against code in the same process.

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

`eg_helper_build_id()` returns 64 lowercase ASCII hex characters plus NUL. The build script hashes
relative paths and contents of the manifests, lockfile and Rust sources of core, game access, trainer
and helper. This is source compatibility/provenance, not authentication or a compiled-binary hash.
Evaluation separately records the DLL SHA256 and compiler identity.

Validation uses real local pipes with authored child processes plus the disposable Unity 6000.4.8
IL2CPP player in [persistent-player](../../tools/trainer-eval/persistent-player/README.md). All artifacts,
logs and phase observations stay separate from the installed game. The desktop application remains
unwired to this helper.
