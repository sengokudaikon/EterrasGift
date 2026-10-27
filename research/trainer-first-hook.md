# First gameplay hooks: mysterious rift and offhand

The [feature implementation plan](trainer-feature-plan.md) inventories all 68 hook features,
separates shared support from gameplay code, and proposes implementation and extraction order.

The first implemented target is `TimeBeastTracker.NextEncounterShouldBeEggs()` from the original
`native/hooks/world.h`. It is an instance Boolean method with a receiver and hidden MethodInfo:

```rust
type RiftOriginal = unsafe extern "C-unwind" fn(*mut core::ffi::c_void, *mut core::ffi::c_void) -> bool;
```

Enabled behavior returns true without calling the original, including its side effects.
Disabled behavior forwards both arguments and preserves the result and exceptions. It needs
no receiver dereference, player selection or gameplay-field restoration.

Offhand follows the same lifecycle for `OffhandItemContainer.CanReceiveItem(item, amount)`.
It returns true while enabled and otherwise preserves every argument and the original call.
Turning it off does not unequip items already accepted.

Both use a process-owned Boolean leaf gateway in `eg-game-access::hooks::boolean`.
The leaf reads one retained atomic flag, returns true or tail-jumps the retained trampoline.
It calls no helper code and changes no argument registers or stack. It needs neither a
gameplay in-flight counter nor a handler panic boundary; native exceptions follow the original
caller's unwind path. The existing dispatcher still uses its callback counter and drain.

The helper resolves each named non-generic instance Boolean method from live metadata,
rejects ambiguous overloads, checks the code range, and lets RetainedHook validate the copied
prefix. Offhand's virtual method is allowed. Argument layouts require no adaptation because
the gateway never reads them. No audited RVA is a startup prerequisite.

One shared owner closes flags before restoration and serializes installation against shutdown.
Its main-thread status/watchdog checks restore both entries on loading, online state, failed
status, or manager change. Re-enabling requires another admitted request; the worker reapplies
wanted hooks after successful independent reads. All gameplay entries restore before the
dispatcher drains. Unknown bytes or a failed restoration keep the module resident.

The production session sends one set request and requires a separate manager-bound entry/flag
readback before reporting on or off. Its monitor rechecks hooks it has used. The catalog and
worker capabilities enable only these two existing UI rows.

Validation uses the authored native reload test, the authored Unity lifecycle player, session
certainty tests and fake-worker UI captures. Unity checks each original's side-effect counter,
independent toggles, unchanged offhand arguments, travel suspension and original behavior after
unload across six loads. None of this establishes compatibility with Last Epoch's current
generated method prefixes; that still needs explicitly requested live validation.

See the [original inventory](../tools/trainer-eval/hooks-fields-audit/hook-inventory.md),
[hook evaluation](../tools/trainer-eval/hook-eval/README.md) and
[interop notes](trainer-interop.md). The former unpatched/bridge experiment narrative and
artifacts are preserved by `archive/trainer-evaluations-2026-10-19`.
