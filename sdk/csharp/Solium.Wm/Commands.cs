// The commands a window manager writes most, each in one call.

using System.Collections.Generic;
using System.Linq;

namespace Solium.Wm;

/// <summary>
/// Shorthands for commands. Every command can be written out as the
/// generated types have it (<c>new Command { Place = new Place { … } }</c>);
/// these are the ones written often enough to want a name.
/// </summary>
public static class Commands
{
    /// <summary>The events the window manager hears, by their names in the protocol.</summary>
    public static Command Listen(params string[] events) =>
        new() { Listen = new StringList { Items = { events } } };

    /// <summary>The key combinations that are the window manager's own, each with a note of what it does.</summary>
    public static Command Bind(params (string Combo, string Note)[] keys) => new()
    {
        Declare = new Declarations
        {
            Bindings = { keys.Select(key => new Binding { Combo = key.Combo, Note = key.Note }) },
        },
    };

    /// <summary>
    /// A settings section of the compositor's, from any object that reads
    /// as a document: <c>Configure("idle", new { lock_after = 0 })</c>.
    /// </summary>
    public static Command Configure(string section, object settings) => new()
    {
        Configure = new Configure { Section = section, Settings = { Document.From(settings) } },
    };

    /// <summary>A timer that falls due every so many milliseconds, on the compositor's clock.</summary>
    public static Command Every(ulong id, double milliseconds) => new()
    {
        Timer = new Timer { Id = id, AfterMs = milliseconds, PeriodMs = milliseconds },
    };

    /// <summary>A timer that falls due once.</summary>
    public static Command After(ulong id, double milliseconds) => new()
    {
        Timer = new Timer { Id = id, AfterMs = milliseconds },
    };

    public static Command CancelTimer(ulong id) => new() { TimerCancel = id };

    /// <summary>
    /// Where a window lives and how large: a tile it is held inside unless
    /// <paramref name="tile"/> is false, reached by <paramref name="motion"/>
    /// or at once with none.
    /// </summary>
    public static Command Place(ulong id, Rect rect, Motion? motion = null, bool tile = true) => new()
    {
        Place = new Place { Id = id, Rect = rect, Motion = motion ?? new Motion(), Tile = tile },
    };

    /// <summary>Give a window the keyboard.</summary>
    public static Command Focus(ulong id, FocusCause cause = FocusCause.Script) => new()
    {
        Focus = new Focus { Id = id, Cause = cause },
    };

    /// <summary>Ask a window to close.</summary>
    public static Command Close(ulong id) => new() { Close = id };

    /// <summary>Start a program, by its name and arguments.</summary>
    public static Command Spawn(IEnumerable<string> program) =>
        new() { Spawn = new StringList { Items = { program } } };

    /// <summary>End the session.</summary>
    public static Command Quit() => new() { Quit = new Unit() };

    /// <summary>
    /// Name a group of windows, and of scene surfaces by their names; a
    /// member is carried by the group's shift, then by its
    /// <paramref name="parent"/>'s and so on up. With
    /// <paramref name="windows"/> null the name is taken away.
    /// </summary>
    public static Command Group(
        string name,
        IEnumerable<ulong>? windows,
        string? parent = null,
        Motion? motion = null,
        IEnumerable<string>? surfaces = null)
    {
        var group = new Group { Name = name, Motion = motion ?? new Motion() };
        if (windows is not null)
        {
            group.Selection = new Selection { Windows = { windows } };
            if (surfaces is not null)
            {
                group.Selection.Surfaces.AddRange(surfaces);
            }
        }
        if (parent is not null)
        {
            group.Parent = parent;
        }
        return new Command { Group = group };
    }

    /// <summary>
    /// Carry a group to a pose in three dimensions, by a motion: each number
    /// of the pose is what the motion moves, and the members'
    /// <paramref name="opacity"/> with them. With <paramref name="backface"/>
    /// a member seen from behind is hidden.
    /// </summary>
    public static Command Carry(string name, Pose pose, Motion motion, bool backface = false, float opacity = 1) => new()
    {
        PresentGroup = new PresentGroup
        {
            Name = name,
            Shift = new Shift
            {
                Opacity = opacity,
                Matrix = { Identity },
                Pose = pose,
                Backface = backface,
            },
            Motion = motion,
        },
    };

    /// <summary>Let a group go back to where its members are, by a motion.</summary>
    public static Command Uncarry(string name, Motion motion) => new()
    {
        ClearGroup = new ClearGroup { Name = name, Motion = motion },
    };

    private static readonly float[] Identity =
    {
        1, 0, 0, 0,
        0, 1, 0, 0,
        0, 0, 1, 0,
        0, 0, 0, 1,
    };

    /// <summary>A motion of so many milliseconds, by an easing: <see cref="Ease"/> has them.</summary>
    public static Motion Motion(double milliseconds, Easing? easing = null) =>
        new() { DurationMs = milliseconds, Easing = easing ?? Ease.Linear };
}

/// <summary>The easings that carry no numbers of their own.</summary>
public static class Ease
{
    public static Easing Linear => new() { Linear = new Unit() };
    public static Easing OutCubic => new() { OutCubic = new Unit() };
    public static Easing OutBack => new() { OutBack = new Unit() };
    public static Easing InOutQuad => new() { InOutQuad = new Unit() };
    public static Easing InOutCubic => new() { InOutCubic = new Unit() };

    /// <summary>
    /// A spring, which takes as long as it takes and carries the speed of
    /// the motion it interrupts.
    /// </summary>
    public static Easing Spring(double stiffness, double damping, double mass = 1) => new()
    {
        Spring = new Spring { Stiffness = stiffness, Damping = damping, Mass = mass, Epsilon = 0.001 },
    };

    /// <summary>A cubic bezier through (x1, y1) and (x2, y2), as CSS has it.</summary>
    public static Easing Bezier(double x1, double y1, double x2, double y2) => new()
    {
        Bezier = new Bezier { X1 = x1, Y1 = y1, X2 = x2, Y2 = y2 },
    };
}
