using MelonLoader;
using System;
#if NUMERIC
[assembly: MelonInfo(typeof(Fixture.Early), "Early plugin", 1, 2, 3, "beta", "fixture", null)]
[assembly: VerifyLoaderVersion(0, 7, 3)]
#elif EXTERNAL
[assembly: MelonInfo(typeof(Fixture.External), "External", "1.0", "fixture")]
#elif WRONG
[assembly: MelonInfo(typeof(Fixture.Wrong), "Wrong", "1.0", "fixture")]
#else
[assembly: MelonInfo(typeof(Fixture.Outer.Entry), "Fixture Mod", "1.2.3", "fixture", "https://example.invalid/mod")]
[assembly: VerifyLoaderVersion("0.7.0", true)]
#endif
[assembly: MelonGame("Eleventh Hour Games", "Last Epoch")]
[assembly: MelonPlatformDomain(MelonPlatformDomainAttribute.CompatibleDomains.Il2Cpp)]
[assembly: MelonPlatform(MelonPlatformAttribute.CompatiblePlatforms.WindowsX64, MelonPlatformAttribute.CompatiblePlatforms.Linux)]
namespace Fixture {
    public abstract class Shared<T> : MelonMod where T : Shared<T> { }
    public static class Outer {
        public sealed class Entry : Shared<Entry> {
            public static string Category() => "FixtureCategory";
            public static int Payload(ref int number) {
                try { return number + 42; }
                finally { number++; }
            }
        }
        public sealed class Cache { public int Value; }
    }
    public static class Other { public sealed class Cache { public string? Value; } }
    public sealed class Early : MelonPlugin { }
    public sealed class External : FixtureFramework.Base { }
    public sealed class Wrong { }
    public sealed class DuplicateA { public static int Value() => 1; }
    public sealed class DuplicateB { public static int Value() => 2; }
    public sealed class DuplicateA_2 { public static int Value() => 3; }
}
