// The protocol this package speaks.

namespace Solium.Wm;

public static class Protocol
{
    /// <summary>
    /// The protocol's version, as `hello` and its answer say it: the one
    /// `solium_wm.proto` was generated for. A compositor of the same major
    /// version (before 1.0, the same minor one) speaks to this package.
    /// </summary>
    public const string Version = "0.2.0";

    /// <summary>Whether a compositor that says `theirs` speaks to this package.</summary>
    public static bool Speaks(string theirs)
    {
        var ours = Version.Split('.');
        var other = theirs.Split('.');
        if (other.Length < 2 || ours[0] != other[0])
        {
            return false;
        }
        return ours[0] != "0" || ours[1] == other[1];
    }
}
