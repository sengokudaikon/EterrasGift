# Native trainer composition

`eg-trainer-host` implements the concrete Windows x64 `HelperPreparer` used by
`eg-trainer::connection::SessionConnector`. It belongs to the trainer tier; the
domain/session crate keeps `forbid(unsafe_code)`. Construction performs no target
access. The caller runs preparation and session work on its background worker.

`NativePreparer` owns the exact helper artifact, a runtime authority, independent
log readers and retained process generations. Before a first load, its authority
must return an identity/profile-bound `RuntimeAttestation`. Constructing that
attestation is unsafe: the authority establishes the exact IL2CPP ABI, metadata,
GC, main-thread, method-body, exclusive patch and lifetime contracts required by
the lower bootstrap API. Requested writes additionally require permission for the
narrow metadata-validated Int32 level store and exclusive graph/readback ownership.
File hashes and a named thread are corroborating checks.
The authored player controller has an authority limited to its newly spawned,
manifest-validated Unity 6000.4.8f1 child. A production Last Epoch authority still
needs implementation and validation.

Preparation repeats the connector's admission callback before startup. The returned
`VerifiedTransport` independently uses fresh Release-policy log admission, preserves
the exact target and verifies the complete file-derived dispatcher body, exception
table record and unwind mapping before the first Status. Subsequent requests must
carry that exact dispatcher. Reusing a retained helper reuses its proof; an installed
scheduler is expected to have changed the entry since the original check.

The preparer survives session failures and disconnects. It reuses an admitted loaded
generation, retains uncertain calls and refuses another load after uncertainty or a
caught startup panic. A reservation precedes the unsafe startup boundary. A factual
NotSent result releases that reservation; successful and uncertain results retain it.
The caller's ownership guard is pinned after a potentially submitted start, including
when the final preparer/transport drops. Confirmed-exit reclamation is not implemented.
There are eight retained target slots per preparer and no automatic eviction.

Dropping this composition submits no RPC, stop or unload. The lower pending-call
guard may release its configuration allocation after proving its thread has exited.
Explicit session cleanup remains the session's responsibility. Helper code stays
resident; a stopped or missing server never causes reinjection into that process.

The offline host tests use authored inert file bytes and scripted authority failures.
They check admission ordering, source/profile binding, replay fences, capacity, guard
retention, caught panic and exact dispatcher proof. Successful preparation is tested
by the authored player controller's `--rust-preparer` route.
