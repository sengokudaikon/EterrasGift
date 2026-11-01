# unity-files

A Unity player's data files, read without running it: UnityFS bundles (format 6-8, LZ4 or stored), serialized files (format
17-22), type trees, Unity Localization string tables, and layouts rebuilt from an IL2CPP build (metadata format 39, Unity
6000.3-6000.4) for classes stored without type trees. Each module's `//!` docs say what it reads.

A file outside those formats is refused at its header, and every object must decode to its last byte, so a wrong layout is
caught rather than read as nonsense. The metadata's layouts follow Cpp2IL's LibCpp2IL (MIT), and the common string table
UnityPy's type tree package.

## Rebuilt layouts

Type trees are stripped from player data (`resources.assets`). The field names and order come from `global-metadata.dat`,
their types from the type tables `GameAssembly.dll` registers, and `[SerializeField]`/`[NonSerialized]` from the metadata's
attribute data.

The metadata has variable-width indices: type definition, generic container, type and parameter indices are sized to their
tables, and every record size is checked against its section. The binary's metadata registration has no symbol, so it is
found by its shape: its 13th word is the type definition count, its 11th the field offset count.

`layout.rs` applies Unity's serialization rules (`Unity.SerializationLogic`, as AssetStudio vendors it) with the
  changes Unity 6 made, all found by comparing against the trees the bundles ship:
- `bool` is written `UInt8`, `char` `UInt16`; enums take their underlying type.
- Generic types serialize; abstract types and interfaces do not (unless `[SerializeReference]`).
- Only 1- and 2-byte primitives (and collections of them) are aligned after; struct fields are not, and no
    collection element is.
- A collection's `Array` is aligned after primitives, strings and Unity's value types, not after other classes.
- Unity's native types use their own names (`LayerMask.m_Bits`, `Rect` and `RectInt` as `x/y/width/height`,
    `Vector2Int`/`Vector3Int`, `BoundsInt`; `Gradient` with `UInt8 m_Mode` and `SInt8 m_ColorSpace`).
- Past ten levels of nested classes only primitive fields are written (the recursive
    `WeightedCorruptionOutcome.extraCorruptionOutcomes` stops nine levels in).
- **The check:** `generated_layouts_match_the_shipped_type_trees`, over `layout::difference`, rebuilds the layout of every class `PermaLoad`
  ships a tree for and compares structure, names, primitive types and alignment. 1,170 of 1,182 match exactly; the
  other 12 hold `[SerializeReference]` fields and are refused by name (`conditionalEffects` on ailments, PCG rules).
  It runs in `mise run game-files:install`, in `eg-game-files`, against a real install.
- The decoder treats any node with an `Array` child as a collection, whatever its type name: the rebuilt trees (like
  Unity's own `SingleAffix singleAffixes`) name collection nodes after their element, and `List<byte>` became `UInt8`.

A changed serialization rule shows as a failed exact decode, and as mismatches against shipped trees.

## String tables

- **They use `[SerializeReference]`.** Each `StringTable` has a managed reference registry (format 2) holding `SmartFormatTag`
  metadata; the shared tables hold `DistributedUIDGenerator` and `Comment`. Their layouts are the serialized file's reference
  types, which the reader parses and decodes against.
- **Entries with no key** carry an ID the collection's shared data does not name. Unity looks text up by key, so they are
  unreachable; they are left out and counted (`Strings::unkeyed`).
- **Keys named twice:** the later ID wins, as a dictionary filled in order would. The earlier entries are counted
  (`Strings::shadowed`).
