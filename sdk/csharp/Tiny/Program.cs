// A whole window manager for Solium, in C#: every window a column of the
// work area, with keys to open a terminal, close a window, move the
// keyboard between windows and end the session.
//
// Name it in ~/.config/solium/wm.toml:
//
//     command = ["dotnet", "/path/to/tiny-wm.dll"]
//
// It is the same window manager as crates/wm-protocol/examples/tiny_wm.rs,
// which is the one in Rust.

using System;
using System.Collections.Generic;
using System.Linq;
using Solium.Wm;

namespace Tiny;

public static class Program
{
    public static int Main() => new Tiling().Run();
}

internal sealed class Tiling : WindowManager
{
    /// <summary>The windows it arranges, in the order they opened.</summary>
    private readonly List<ulong> _tiled = new();

    protected override IEnumerable<Command> Setup() => new[]
    {
        // Its keys are its own: the compositor knows without asking.
        Commands.Bind(
            ("super+return", "open a terminal"),
            ("super+q", "close the focused window"),
            ("super+j", "focus the next window"),
            ("super+k", "focus the window before"),
            ("super+shift+e", "end the session")),
        // And these are all it wants to hear, beyond its keys.
        Commands.Listen("opened", "closed", "monitors-changed"),
    };

    protected override void Handle(Batch batch, Answer answer)
    {
        foreach (var each in batch.Events)
        {
            switch (each.KindCase)
            {
                case Event.KindOneofCase.Opened:
                    // Placed in the answer to its opening, so it opens in
                    // its column and the others make room in the same frame.
                    _tiled.Add(each.Opened);
                    answer.Handled = true;
                    answer.Commands.Add(Arrange());
                    break;
                case Event.KindOneofCase.Closed:
                    _tiled.Remove(each.Closed);
                    answer.Commands.Add(Arrange());
                    break;
                case Event.KindOneofCase.MonitorsChanged:
                    answer.Commands.Add(Arrange());
                    break;
                case Event.KindOneofCase.Key:
                    answer.Handled = true;
                    answer.Commands.Add(Pressed(each.Key));
                    break;
            }
        }
    }

    /// <summary>Every window an equal column of the work area.</summary>
    private IEnumerable<Command> Arrange()
    {
        var area = Desktop.WorkArea ?? Desktop.Primary?.Area;
        if (area is null || _tiled.Count == 0)
        {
            return Array.Empty<Command>();
        }
        var width = area.W / _tiled.Count;
        return _tiled.Select((id, column) => Commands.Place(
            id,
            new Rect { X = area.X + width * column, Y = area.Y, W = width, H = area.H },
            Commands.Motion(180, Ease.OutCubic)));
    }

    private IEnumerable<Command> Pressed(string combo)
    {
        switch (combo)
        {
            case "super+return":
                var terminal = Environment.GetEnvironmentVariable("TERMINAL") ?? "foot";
                return new[] { Commands.Spawn(new[] { terminal }) };
            case "super+q" when Desktop.Focused is { } focused:
                return new[] { Commands.Close(focused.Id) };
            case "super+j" when Neighbour(1) is { } next:
                return new[] { Commands.Focus(next, FocusCause.Binding) };
            case "super+k" when Neighbour(-1) is { } before:
                return new[] { Commands.Focus(before, FocusCause.Binding) };
            case "super+shift+e":
                return new[] { Commands.Quit() };
            default:
                return Array.Empty<Command>();
        }
    }

    /// <summary>The window so many columns along from the focused one, round the ends.</summary>
    private ulong? Neighbour(int step)
    {
        if (Desktop.Focused is not { } focused)
        {
            return null;
        }
        var at = _tiled.IndexOf(focused.Id);
        if (at < 0)
        {
            return null;
        }
        return _tiled[((at + step) % _tiled.Count + _tiled.Count) % _tiled.Count];
    }
}
