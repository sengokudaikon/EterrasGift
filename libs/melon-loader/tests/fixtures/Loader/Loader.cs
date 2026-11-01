using System;
namespace MelonLoader;
public abstract class MelonMod { }
public abstract class MelonPlugin { }
[AttributeUsage(AttributeTargets.Assembly)]
public sealed class MelonInfoAttribute : Attribute {
    public MelonInfoAttribute(Type type, string name, string version, string author, string? link = null) { }
    public MelonInfoAttribute(Type type, string name, int major, int minor, int patch, string identifier, string author, string? link) { }
}
[AttributeUsage(AttributeTargets.Assembly, AllowMultiple = true)]
public sealed class MelonGameAttribute(string? developer, string? name) : Attribute { }
[AttributeUsage(AttributeTargets.Assembly)]
public sealed class MelonPlatformDomainAttribute(MelonPlatformDomainAttribute.CompatibleDomains domain) : Attribute {
    public enum CompatibleDomains { Universal, Mono, Il2Cpp }
}
[AttributeUsage(AttributeTargets.Assembly)]
public sealed class MelonPlatformAttribute(params MelonPlatformAttribute.CompatiblePlatforms[] platforms) : Attribute {
    public enum CompatiblePlatforms { Universal, WindowsX86, WindowsX64, Android, Linux, Mac }
}
[AttributeUsage(AttributeTargets.Assembly)]
public sealed class VerifyLoaderVersionAttribute : Attribute {
    public VerifyLoaderVersionAttribute(string version, bool minimum) { }
    public VerifyLoaderVersionAttribute(int major, int minor, int patch, bool minimum = false) { }
}
