# Sample mods for the Mods captures

Metadata-only stand-ins for a sample mod library: Flicker Dreams, Ability Ranges (with its embedded settings
description), Stash Sorter, Ward Meter, the many-mods samples, and the builds the sources scenarios install. They are
never loaded or executed; `tests/captures.rs` (`mods_captures`) reads them through the real mod library, so running the
captures needs no .NET SDK.

To regenerate, build `Samples/Samples.csproj` with the .NET 10 SDK once per sample, with the build output outside the
repository:

```
dotnet build Samples/Samples.csproj -c Release -p:Sample=<NAME> -o <scratch>/<NAME> -p:BaseIntermediateOutputPath=<scratch>/obj/<NAME>/
```

and copy `<scratch>/<NAME>/<NAME>.dll` to the `.bin` named after it in lower case with dashes (`FLICKER_DREAMS` →
`flicker-dreams.bin`). The names are `FLICKER_DREAMS`, `ABILITY_RANGES`, `STASH_SORTER`, `WARD_METER`, `SAMPLE_MOD`,
`LONG_NAME`, `FLICKER_0_13_0_FROM_0_12_1`, `FLICKER_0_12_1_FROM_0_12_1`, `FLICKER_0_13_0_FROM_0_13_0` and `LOOM`. They
reference the stand-in MelonLoader in `libs/melon-loader/tests/fixtures/Loader`.
