// The C# suite's stand-in mods (tests/EterrasGift.Companion.Testing/ModFixtures.cs and CompanionPageTests' CompilingRunner),
// one per -p:Sample. A mod's category is a string literal, as MelonLoader mods create theirs.
using MelonLoader;
#if FLICKER_DREAMS
[assembly: MelonInfo(typeof(Sample.Entry), "Flicker Dreams", "0.12.1", "sengokudaikon")]
[assembly: System.Reflection.AssemblyDescription("Dreamslash dashes from enemy to enemy while Dream stacks last.")]
#elif ABILITY_RANGES
[assembly: MelonInfo(typeof(Sample.Entry), "Ability Ranges", "0.2.0", "sengokudaikon")]
#elif STASH_SORTER
[assembly: MelonInfo(typeof(Sample.Entry), "Stash Sorter", "0.4.1", "kestrel")]
[assembly: System.Reflection.AssemblyDescription("Sorts a stash tab by item type and level.")]
#elif WARD_METER
[assembly: MelonInfo(typeof(Sample.Entry), "Ward Meter", "2.0.0", "Mirelle")]
#elif SAMPLE_MOD
[assembly: MelonInfo(typeof(Sample.Entry), "Sample Mod", "1.0.0", "Sample author")]
#elif LONG_NAME
[assembly: MelonInfo(typeof(Sample.Entry), "An Extremely Long Mod Name That Somebody Thought Was A Good Idea For Their Loot Filter Helper", "1.0.0", "Sample author")]
// Builds from a sources project: MelonInfo's version, and the project's <Version> as the SDK writes it.
#elif FLICKER_0_13_0_FROM_0_12_1
[assembly: MelonInfo(typeof(Sample.Entry), "Flicker Dreams", "0.13.0", "sengokudaikon")]
[assembly: System.Reflection.AssemblyInformationalVersion("0.12.1+0c1d2e3")]
#elif FLICKER_0_12_1_FROM_0_12_1
[assembly: MelonInfo(typeof(Sample.Entry), "Flicker Dreams", "0.12.1", "sengokudaikon")]
[assembly: System.Reflection.AssemblyInformationalVersion("0.12.1+0c1d2e3")]
#elif FLICKER_0_13_0_FROM_0_13_0
[assembly: MelonInfo(typeof(Sample.Entry), "Flicker Dreams", "0.13.0", "sengokudaikon")]
[assembly: System.Reflection.AssemblyInformationalVersion("0.13.0+0c1d2e3")]
#elif LOOM
[assembly: MelonInfo(typeof(Sample.Entry), "Loom", "0.2.0", "sengokudaikon")]
[assembly: System.Reflection.AssemblyInformationalVersion("1.0.0+0c1d2e3")]
#endif
[assembly: MelonGame("Eleventh Hour Games", "Last Epoch")]
namespace Sample {
    public sealed class Entry : MelonMod {
#if FLICKER_DREAMS
        public static readonly string Category = "FlickerDreams";
#elif ABILITY_RANGES
        public static readonly string Category = "AbilityRanges";
#endif
    }
}
