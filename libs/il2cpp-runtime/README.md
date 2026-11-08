# il2cpp-runtime

A live IL2CPP runtime through its public exports. Modules: `raw` (declarations, export tables, type codes), `thread` (attachments,
strong roots), `metadata` (lookups, typed reads of objects and collections, method and field contracts, each checked against what
its caller states), `invoke` (bounded invocation), `module` (the loaded module, its exports and size; Windows), `test_fakes` (the
fake thread for tests, behind `test-stubs`). In-game code only: nothing else may link it.

## What it assumes

Windows x64, an initialized runtime, and Unity 6000.4's exports (authored native fixtures use 6000.4.8f1). Export signatures,
metadata storage, method ABI and the small object, array and `MethodInfo` layout adapters must match the build, with GC handles
pointer-valued. Loaded runtime code and metadata must stay valid while used. The crate owns neither the engine's publication and
lifetime rules nor other native patchers, and types and file hashes cannot establish those facts. Metadata it does not support
refuses the operation.

## Attachment and roots

- Raw unsafe table constructors exist for authored runtimes; their caller must supply valid ABI and export provenance.
- A thread-local attachment registry shares ownership across contexts for the same export table, so dropping one context's last
  lease cannot detach another context's live leases or roots. Engine-owned attachments are never detached. External runtime
  shutdown or detach is an unsafe caller obligation.
- Roots preserve managed reachability. They do not prevent unrelated field writes or native-object destruction.
- ABI declarations, receiver and type checks and foreign exception boundaries still need correct use. Rust panics must not
  escape a native callback.

## Stores and invocation

- Reference field stores keep the raw export private and check both roots' runtime origin, reference class and type code,
  mutability and declaring-class compatibility before the barrier.
- A reference `Field::SetValue` takes the direct object pointer, unlike scalar setters, which take the address of the scalar slot
  (the runtime's `Field.cpp`, `deref_pointer=false`).
- Closed `List<T>` calls use `CheckedMethod` with an explicit allowlist of concrete reference classes, keeping the receiver,
  argument and result checks. Ordinary binding refuses generics.
- Virtual dispatch must use the exact resolved override. Direct lookups reject virtual, abstract and native replacements.
- Every method lookup is a `MethodQuery` for `find_method`: its name and parameters choose the candidates, and the rest is a
  contract the one chosen must meet. What another method of the name does is the query's: it fails the lookup, or one of
  another count is passed over, or any overload is. `nearest_declaring` finds the parent that declares a method, and
  `InvocationApi::dispatched` the override a receiver runs.
