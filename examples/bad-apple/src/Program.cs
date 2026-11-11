// Bad Apple, played by a window manager.
//
// This program is a window manager for Solium, written in C#. Solium starts
// it with a socket as its standard input, and everything it does, it does
// by sending messages over that socket: it draws nothing itself. It opens a
// few dozen plain white windows, and for each frame of a black and white
// film it tells the compositor where to draw each one, so that together
// they are the frame's lit parts. The compositor moves them there.
//
// What it shows of the protocol:
//   - the handshake, and what a window manager says before the first frame;
//   - `listen`: it hears three events, and is never woken for another;
//   - `declare`: its keys are its own, and the compositor knows without
//     asking;
//   - `submit`: commands sent on its own clock, with no event asking;
//   - `present`: a window drawn somewhere other than where it lives, moved
//     there by the compositor in the time the window manager says.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;

namespace Solium.Wm;

public static class Program
{
    public static int Main(string[] arguments)
    {
        var glide = arguments.Length > 0 && arguments[0] == "--glide";
        if (glide)
        {
            arguments = arguments[1..];
        }
        if (arguments.Length < 2)
        {
            Console.Error.WriteLine(
                "usage: bad-apple [--glide] <film.frames> <tiles program> [its arguments…]\n" +
                "Started by Solium: name it in ~/.config/solium/wm.toml.");
            return 2;
        }
        var film = Film.Load(arguments[0]);
        // The program that opens the windows, told how many: `tiles 48`.
        var open = arguments.Skip(1).Append(film.Most.ToString()).ToList();
        try
        {
            using var solium = Connection.FromStandardInput();
            return new Player(solium, film, open, glide).Run();
        }
        catch (Exception error) when (error is System.Net.Sockets.SocketException or System.IO.IOException)
        {
            Console.Error.WriteLine($"bad-apple: {error.Message}");
            return 1;
        }
    }
}

/// <summary>One window the film is drawn with, and the cells it is lighting.</summary>
internal sealed class Tile
{
    public ulong Id;
    public Cells? Lit;
}

internal sealed class Player
{
    /// <summary>The <c>app_id</c> of the windows the film is drawn with.</summary>
    private const string Tiles = "solium-tiles";

    /// <summary>
    /// How far, in cells, a window may be from a rectangle and still glide
    /// to it: the distance between their centres and the difference of
    /// their sizes, added up.
    /// </summary>
    private const double Near = 4;

    private readonly Connection _solium;
    private readonly Film _film;
    private readonly List<string> _open;

    /// <summary>
    /// Whether a window that is lit a little way from where it is wanted
    /// glides there, moved by the compositor, or is there at once. At once
    /// is the film as it is, frame for frame; gliding is smoother, and
    /// leaves a seam for a frame where a window that glides meets one that
    /// did not have to.
    /// </summary>
    private readonly bool _glide;

    // What it has been told of the desktop: the monitors, and its windows.
    private List<Monitor> _monitors = new();
    private readonly List<Tile> _tiles = new();

    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private TimeSpan? _began;
    private TimeSpan? _paused;
    private int _shown = -1;

    public Player(Connection solium, Film film, List<string> open, bool glide)
    {
        _solium = solium;
        _film = film;
        _open = open;
        _glide = glide;
    }

    /// <summary>
    /// What it tells the compositor before the first frame, and again after
    /// a reload: which keys are its own, which events it wants to hear, and
    /// that windows are drawn bare, with no frame.
    /// </summary>
    private static List<Command> Setup()
    {
        Binding Key(string combo, string note) => new() { Combo = combo, Note = note };
        return new List<Command>
        {
            new Command.Declare
            {
                Value = new Declarations
                {
                    Bindings =
                    {
                        Key("escape", "end the session"),
                        Key("space", "pause, and go on"),
                        Key("r", "from the beginning"),
                    },
                },
            },
            new Command.Listen { Value = { "opened", "closed", "monitors-changed" } },
            new Command.Decoration { Value = "none" },
        };
    }

    public int Run()
    {
        // The compositor speaks first. A protocol of another version is
        // another protocol: say so, and go.
        if (_solium.Receive(TimeSpan.FromSeconds(10)) is not Message.Hello hello)
        {
            return 1;
        }
        if (!Speaks(hello.Value.Version))
        {
            _solium.Send(new Message.Goodbye
            {
                Value = $"this window manager speaks the protocol {Protocol.Version}, " +
                        $"the compositor {hello.Value.Version}",
            });
            return 1;
        }
        _solium.Send(new Message.HelloAnswer
        {
            Value = new HelloAnswer { Version = Protocol.Version, Commands = Setup() },
        });

        while (true)
        {
            switch (_solium.Receive(UntilTheNextFrame()))
            {
                case Message.Batch batch:
                    _solium.Send(new Message.Answer { Value = Handle(batch.Value) });
                    break;
                case Message.Reload:
                    // Nothing of its own to read again: it says what it
                    // said at the start.
                    _solium.Send(new Message.Reloaded
                    {
                        Value = new Reloaded.Loaded { Value = new Answer { Commands = Setup() } },
                    });
                    break;
                case Message.Goodbye:
                    return 0;
            }
            Play();
        }
    }

    /// <summary>The same major version, and before 1.0 the same minor one.</summary>
    private static bool Speaks(string theirs)
    {
        var ours = Protocol.Version.Split('.');
        var other = theirs.Split('.');
        if (other.Length < 2 || ours[0] != other[0])
        {
            return false;
        }
        return ours[0] != "0" || ours[1] == other[1];
    }

    /// <summary>One batch: what changed first, then what happened.</summary>
    private Answer Handle(Batch batch)
    {
        var answer = new Answer { Serial = batch.Serial };
        foreach (var change in batch.Changes)
        {
            switch (change)
            {
                case Change.Monitors monitors:
                    _monitors = monitors.Value;
                    // Everything is somewhere else on another screen.
                    _shown = -1;
                    break;
                case Change.Window window when window.Value.AppId == Tiles:
                    if (_tiles.All(tile => tile.Id != window.Value.Id))
                    {
                        _tiles.Add(new Tile { Id = window.Value.Id });
                        // Out of sight until the film wants it.
                        answer.Commands.Add(Draw(window.Value.Id, null, 0, Stage()));
                    }
                    break;
                case Change.WindowGone gone:
                    _tiles.RemoveAll(tile => tile.Id == gone.Value);
                    break;
            }
        }
        foreach (var each in batch.Events)
        {
            switch (each)
            {
                case Event.Startup:
                    // Run and left to itself: it opens its windows when it likes.
                    answer.Commands.Add(new Command.RunDetached { Value = _open });
                    break;
                case Event.Key key:
                    answer.Handled = true;
                    Pressed(key.Value, answer.Commands);
                    break;
            }
        }
        return answer;
    }

    private void Pressed(string combo, List<Command> commands)
    {
        var now = _clock.Elapsed;
        switch (combo)
        {
            case "escape":
                commands.Add(new Command.Quit());
                break;
            case "space" when _paused is { } since && _began is { } began:
                // On from where it stopped.
                _began = began + (now - since);
                _paused = null;
                break;
            case "space":
                _paused = now;
                break;
            case "r":
                _began = now;
                _paused = null;
                _shown = -1;
                break;
        }
    }

    /// <summary>How long to wait for the compositor before the film needs a frame.</summary>
    private TimeSpan UntilTheNextFrame()
    {
        if (_began is not { } began || _paused is not null)
        {
            return TimeSpan.FromMilliseconds(50);
        }
        var due = began + TimeSpan.FromSeconds((_shown + 1) / _film.FramesASecond);
        var left = due - _clock.Elapsed;
        return left > TimeSpan.Zero ? left : TimeSpan.Zero;
    }

    /// <summary>
    /// Show the frame that is due, if it is not the one showing. Nothing
    /// asked for this: the commands go as a `submit`, on this program's
    /// own clock.
    /// </summary>
    private void Play()
    {
        var now = _clock.Elapsed;
        if (_began is null)
        {
            // Begin once every window has opened and had a moment to draw.
            if (_tiles.Count < _film.Most || _monitors.Count == 0)
            {
                return;
            }
            _began = now + TimeSpan.FromSeconds(1);
        }
        if (_paused is not null || now < _began)
        {
            return;
        }
        var index = (int)((now - _began.Value).TotalSeconds * _film.FramesASecond);
        if (index >= _film.Frames.Count)
        {
            // Again, from the beginning.
            _began = now;
            index = 0;
        }
        if (index == _shown)
        {
            return;
        }
        var commands = Arrange(_film.Frames[index]);
        _shown = index;
        if (commands.Count > 0)
        {
            _solium.Send(new Message.Submit { Value = commands });
        }
    }

    /// <summary>
    /// The windows put where a frame's rectangles are. Each rectangle takes
    /// the lit window nearest to it, so a shape that moves is the same
    /// windows moving, and a window that is already where it is wanted is
    /// not spoken of at all. A window with no rectangle goes dark; a
    /// rectangle with no window near takes a dark one, which appears there.
    /// </summary>
    private List<Command> Arrange(Cells[] frame)
    {
        var stage = Stage();
        var commands = new List<Command>();
        var free = new List<Tile>(_tiles);
        var frameMs = 1000.0 / _film.FramesASecond;
        // The largest first: they are what the eye follows.
        foreach (var cells in frame.OrderByDescending(cells => cells.W * cells.H))
        {
            if (free.Count == 0)
            {
                break;
            }
            Tile? best = null;
            var nearest = double.MaxValue;
            foreach (var tile in free)
            {
                // A dark window is further than any lit one.
                var distance = tile.Lit is { } lit
                    ? Math.Abs(lit.CentreX - cells.CentreX) + Math.Abs(lit.CentreY - cells.CentreY)
                      + Math.Abs(lit.W - cells.W) + Math.Abs(lit.H - cells.H)
                    : 1e9;
                if (distance < nearest)
                {
                    nearest = distance;
                    best = tile;
                }
            }
            var chosen = best!;
            free.Remove(chosen);
            if (chosen.Lit != cells)
            {
                // With `--glide`, a window that is lit a little way off
                // glides there in the time a frame lasts: the compositor
                // moves it, at the display's own rate. One that is dark, or
                // far off, is there at once: a window crossing the picture
                // would be a streak in it.
                var glides = _glide && chosen.Lit is not null && nearest <= Near;
                commands.Add(Draw(chosen.Id, cells, glides ? frameMs : 0, stage));
                chosen.Lit = cells;
            }
        }
        foreach (var tile in free.Where(tile => tile.Lit is not null))
        {
            commands.Add(Draw(tile.Id, null, 0, stage));
            tile.Lit = null;
        }
        return commands;
    }

    /// <summary>
    /// The part of the primary monitor the film is shown in: as large as
    /// fits, in the film's own shape, in the middle.
    /// </summary>
    private Rect Stage()
    {
        var monitor = _monitors.FirstOrDefault(monitor => monitor.Primary) ?? _monitors.FirstOrDefault();
        var whole = monitor?.Whole ?? new Rect { W = 1280, H = 720 };
        var scale = Math.Min(whole.W / _film.Columns, whole.H / _film.Rows);
        var (w, h) = (_film.Columns * scale, _film.Rows * scale);
        return new Rect { X = whole.X + (whole.W - w) / 2, Y = whole.Y + (whole.H - h) / 2, W = w, H = h };
    }

    /// <summary>
    /// `present`: draw a window somewhere other than where it lives. The
    /// window keeps its place and its application is told nothing; the
    /// compositor stretches its picture to the rectangle and moves it there
    /// over `ms`. With no cells, it is drawn with no opacity at all.
    /// </summary>
    private Command Draw(ulong id, Cells? cells, double ms, Rect stage)
    {
        var present = new Present
        {
            Id = id,
            Opacity = cells is null ? 0f : 1f,
            Pivot = (0.5f, 0.5f),
            Motion = new Motion { DurationMs = ms, Easing = new Easing.Linear() },
        };
        if (cells is { } lit)
        {
            var cellW = stage.W / _film.Columns;
            var cellH = stage.H / _film.Rows;
            present.Rect = new Rect
            {
                X = stage.X + lit.X * cellW,
                Y = stage.Y + lit.Y * cellH,
                W = lit.W * cellW,
                H = lit.H * cellH,
            };
        }
        return new Command.Present { Value = present };
    }
}
