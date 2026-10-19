# First gameplay hook

The source candidate is `TimeBeastTracker.NextEncounterShouldBeEggs()` from the original
`native/hooks/world.h`. It is an instance Boolean method with a receiver and hidden MethodInfo:

```rust
type RiftOriginal = unsafe extern "C-unwind" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> bool;
```

Enabled behavior returns true without calling the original, including its side effects.
Disabled behavior forwards both arguments and preserves the result and exceptions. It needs
no receiver dereference, player selection or gameplay-field restoration.

The general hook backend must pass the retained regressions before this is installed. Keep
callback ownership, threading, offline auto-off and owner-loss cleanup with the feature. A
failed removal cannot be reported as off. A candidate RVA from an old audit is not a portable
method identity or a startup prerequisite.

See the [original inventory](../tools/trainer-eval/hooks-fields-audit/hook-inventory.md),
[hook evaluation](../tools/trainer-eval/hook-eval/README.md) and
[interop notes](trainer-interop.md). The former unpatched/bridge experiment narrative and
artifacts are preserved by `archive/trainer-evaluations-2026-10-19`.
