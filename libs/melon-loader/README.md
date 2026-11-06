# melon-loader

MelonLoader's files, read and edited without running it or any mod. Names no app or game.

- **`assembly`**: a DLL's MelonInfo and related attributes, its `#US` strings and one embedded resource, from CIL metadata
  only. No DLL is loaded or executed. A DLL is someone else's bytes: one over 64 MiB is refused unread, rows are looked up
  where they lie so the work grows with the attributes read rather than with the tables, and a panic in the parse is
  caught and read as a DLL that can't be read. An abort, or running out of stack, can't be caught.
- **`loader_state`**: the loader's version, whether it has prepared for a game build (the build's SHA-512 and Unity version
  against its `Config.cfg`, unless `Loader.cfg` forces regeneration), and the repair of its generated
  `UnityEngine.CoreModule.dll`, whose parses are bounded and contained the same way.
- **`log`**: what Latest.log observed. An earlier build's failure is no evidence against a new DLL, and an incomplete load or
  failed Unity support leaves each mod's status unknown.
- **`preferences`**: MelonPreferences.cfg. Edits replace only the original value's span, so comments, order, spacing, unknown
  entries and line endings stay.
- **`project`**: a mod's MSBuild project. It writes the arguments for MSBuild's evaluation and reads the answer, whether the
  project builds a DLL that references MelonLoader, what the evaluation depends on (`Stamp`), whether sources changed since a
  build, and the errors in a build's output. **`project::dotnet`** finds the SDK's `dotnet.exe`. It runs neither: starting
  the program is the caller's.
- **`switch`**: the proxy (`version.dll`) set aside beside itself or put back. Only a folder with MelonLoader in it is
  touched, so another tool's `version.dll` is left alone.

## What it refuses

- A DLL without exactly one assembly row, or a resource that lies outside the DLL's resources or over the caller's limit.
- A repair of a signed CoreModule, of orphan or cyclic nested types, or one whose output changes the type inventory: the
  DLL and its backup stay as they were. A clean CoreModule is left byte for byte.
- An edit to a preference that isn't in the file, that has a different type, or that is named twice.
- A switch while both `version.dll` and `version.dll.disabled` are there.

It has no words to show. `log::LogProblem` and `switch::SwitchError` are typed reasons for the caller to put in words; an
`io::Error` says what is wrong with the file in plain English.

The DLLs in `tests/fixtures` are synthetic; its `README.md` has their sources and how to rebuild them.
