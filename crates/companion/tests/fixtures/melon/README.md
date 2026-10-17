# Managed metadata fixtures

These are synthetic assemblies, never loaded or executed. Rust tests inspect the checked-in `.bin` files, so running
the tests needs no .NET SDK or installed game. `Loader` supplies small fake MelonLoader attribute/base-type definitions;
`Mod` supplies normal, plugin, unresolved-base, and invalid-base examples plus an embedded settings description.

To regenerate, copy `Loader/` and `Mod/` into a scratch directory, then build `Mod/Mod.csproj` with the .NET 10 SDK in
Release mode. Copy `Mod/bin/Release/net10.0/FixtureMod.dll` to `clean.bin`, and
`Loader/bin/Release/net10.0/MelonLoader.dll` to `loader.bin`. Rebuild the mod project with each
`-p:DefineConstants=NUMERIC`, `EXTERNAL`, and `WRONG`, copying its output to the corresponding lowercase `.bin`.
Build output stays in the scratch directory. No third-party assembly or game file is included.
