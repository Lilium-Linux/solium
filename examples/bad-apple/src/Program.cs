// Bad Apple, played by a window manager.
//
// This program is a window manager for Solium, written in C#. Solium starts
// it with a socket as its standard input, and everything it does, it does
// by sending messages over that socket: it draws nothing itself.
//
// The film is a grid of cells, each light or dark. For every cell it opens
// two real terminals, one with a light theme and one with a dark one, each
// with a shell and its prompt, and stands them both in the cell. Then, for
// each frame, it has the compositor show the one whose colour the cell
// needs. No terminal is ever resized or redrawn for it: which windows are
// drawn, and where, is the whole of the picture.
//
// What it shows of the protocol:
//   - the handshake, and what a window manager says before the first frame;
//   - `listen`: it hears three events, and beyond them only its own keys
//     and timer;
//   - `declare`: its keys are its own, and the compositor knows without
//     asking;
//   - `configure`: a setting of the compositor's, as a document;
//   - `place`: where a window lives, said in the answer to its opening;
//   - `present`: how a window is drawn, apart from where it lives;
//   - `timer`: woken on the compositor's clock, to hear what changed;
//   - `submit`: commands sent on its own clock, with no event asking;
//   - `surface`: a scene of its own on the desktop, for what it measures.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Text.Json;

namespace Solium.Wm;

public static class Program
{
    public static int Main(string[] arguments)
    {
        if (arguments.Length < 2)
        {
            Console.Error.WriteLine(
                "usage: bad-apple <film.frames> <terminals program> [its arguments…]\n" +
                "Started by Solium: name it in ~/.config/solium/wm.toml.");
            return 2;
        }
        var film = Film.Load(arguments[0]);
        try
        {
            using var solium = Connection.FromStandardInput();
            return new Player(solium, film, arguments.Skip(1).ToList()).Run();
        }
        catch (Exception error) when (error is System.Net.Sockets.SocketException or IOException)
        {
            Console.Error.WriteLine($"bad-apple: {error.Message}");
            return 1;
        }
    }
}

/// <summary>
/// One terminal: its window, whether it is drawn, and whether it is still
/// open. One that somebody closed keeps its cell, which stays empty.
/// </summary>
internal sealed class Terminal
{
    public ulong Id;
    public bool Shown = true;
    public bool Open = true;
}

internal sealed class Player
{
    /// <summary>The application ids of the terminals of each theme.</summary>
    private const string Light = "bad-apple-light";
    private const string Dark = "bad-apple-dark";

    /// <summary>
    /// The title of a terminal whose shell has printed its prompt: the
    /// prompt itself names the window so (`terminals.sh`).
    /// </summary>
    private const string Ready = "ready";

    /// <summary>The timer that wakes this program while the terminals start, and how often.</summary>
    private const ulong Watch = 1;
    private const double WatchedMs = 250;

    /// <summary>How long the terminals stand in their grid, all ready, before the film.</summary>
    private static readonly TimeSpan Gathered = TimeSpan.FromSeconds(5);

    /// <summary>How often the panel's numbers are worked out again.</summary>
    private static readonly TimeSpan Measured = TimeSpan.FromMilliseconds(500);

    private readonly Connection _solium;
    private readonly Film _film;
    private readonly List<string> _open;

    // What it has been told of the desktop: the monitors and the windows,
    // and of those, the terminals of each theme, in the order they opened:
    // the terminal that opened nth of its theme stands in the nth cell.
    private List<Monitor> _monitors = new();
    private readonly Dictionary<ulong, Window> _windows = new();
    private readonly List<Terminal> _light = new();
    private readonly List<Terminal> _dark = new();

    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private bool _watching = true;
    private TimeSpan? _gathered;
    private TimeSpan? _began;
    private TimeSpan? _paused;
    private int _shown = -1;

    // What the panel says, counted since it last said it.
    private TimeSpan _measuredAt;
    private int _frames;
    private int _changed;
    private readonly Queue<(TimeSpan At, TimeSpan Used)> _cpu = new();

    public Player(Connection solium, Film film, List<string> open)
    {
        _solium = solium;
        _film = film;
        _open = open;
    }

    /// <summary>
    /// What it tells the compositor before the first frame, and again after
    /// a reload: which keys are its own, which events it wants to hear, and
    /// that every window is drawn with a plain border.
    /// </summary>
    private List<Command> Setup()
    {
        Binding Key(string combo, string note) => new() { Combo = combo, Note = note };
        var commands = new List<Command>
        {
            new Command.Declare
            {
                Value = new Declarations
                {
                    Bindings =
                    {
                        Key("escape", "end the session"),
                        Key("space", "pause, and go on"),
                        Key("r", "the chequerboard, and the film from the beginning"),
                    },
                },
            },
            new Command.Listen { Value = { "opened", "closed", "monitors-changed" } },
            // The frame every window wears from the moment it opens. Not
            // changed after: a frame of another size is another size for
            // the application in it, and a terminal takes its time over
            // that.
            new Command.Decoration { Value = "border" },
            // A setting, as any window manager sets one: a section's name
            // and a document of its keys. Nobody touches a key while a
            // film plays: the session is not locked, and the screens are
            // not turned off, for want of one. Nought is never.
            new Command.Configure { Value = ("idle", Section(("lock_after", 0L), ("screens_off_after", 0L))) },
        };
        if (_watching)
        {
            // What changed on the desktop comes only with a batch, and a
            // batch only with an event. A shell printing its prompt is no
            // event, but it is a change: the window's title. So until the
            // film begins a timer is the event, four times a second, and
            // each brings the titles that changed.
            commands.Add(new Command.Timer
            {
                Value = new Timer { Id = Watch, AfterMs = WatchedMs, PeriodMs = WatchedMs },
            });
        }
        return commands;
    }

    /// <summary>
    /// A settings document of plain values. A document is a tree,
    /// flattened: the first node is the root, and a table names its
    /// children by where they are in the list.
    /// </summary>
    private static List<DocNode> Section(params (string Key, object Value)[] keys)
    {
        var root = new DocTable();
        var nodes = new List<DocNode> { new DocNode.Table { Value = root } };
        foreach (var (key, value) in keys)
        {
            root.Fields.Add((key, (uint)nodes.Count));
            nodes.Add(value switch
            {
                string text => new DocNode.Text { Value = text },
                long number => new DocNode.Integer { Value = number },
                bool flag => new DocNode.Boolean { Value = flag },
                _ => throw new ArgumentException($"{key} is not a value a document carries here"),
            });
        }
        return nodes;
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
            switch (_solium.Receive(UntilSomethingIsDue()))
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
            // What follows nobody asked for: it is sent as a `submit`, on
            // this program's own clock.
            var commands = new List<Command>();
            Gather(commands);
            Play(commands);
            Measure(commands);
            if (commands.Count > 0)
            {
                _solium.Send(new Message.Submit { Value = commands });
            }
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
                    // Every cell is somewhere else on another screen.
                    foreach (var (terminal, cell) in Everyone().Where(each => each.Terminal.Open))
                    {
                        answer.Commands.Add(Put(terminal.Id, Cell(cell), 0, new Easing.Linear()));
                    }
                    break;
                case Change.Window window:
                    // Kept, and no more: a window's row can come before the
                    // window has opened, and it is not this program's to
                    // place until it has. A shell that has just printed its
                    // prompt is something to see: the wait starts again.
                    if (window.Value.Title == Ready
                        && !(_windows.TryGetValue(window.Value.Id, out var before) && before.Title == Ready))
                    {
                        _gathered = _clock.Elapsed;
                    }
                    _windows[window.Value.Id] = window.Value;
                    break;
                case Change.WindowGone gone:
                    _windows.Remove(gone.Value);
                    foreach (var (terminal, _) in Everyone().Where(each => each.Terminal.Id == gone.Value))
                    {
                        terminal.Open = false;
                    }
                    break;
            }
        }
        foreach (var each in batch.Events)
        {
            switch (each)
            {
                case Event.Startup:
                    // The terminals of each theme, one for every cell. Run
                    // and left to themselves: a window manager starts
                    // nothing itself, and each window they open is told as
                    // an `opened`.
                    foreach (var which in new[] { "light", "dark" })
                    {
                        answer.Commands.Add(new Command.RunDetached
                        {
                            Value = _open.Append(which).Append(_film.Cells.ToString()).ToList(),
                        });
                    }
                    break;
                case Event.Opened opened when _windows.TryGetValue(opened.Value, out var window):
                    // Placed in the answer to its opening, which is where a
                    // window manager places a window. The compositor glides
                    // it to its cell from where it opened.
                    var theme = window.AppId == Light ? _light : window.AppId == Dark ? _dark : null;
                    if (theme is null || theme.Count >= _film.Cells)
                    {
                        break;
                    }
                    var cell = theme.Count;
                    var terminal = new Terminal { Id = opened.Value };
                    theme.Add(terminal);
                    answer.Handled = true;
                    answer.Commands.Add(Put(terminal.Id, Cell(cell), 500, new Easing.OutCubic()));
                    // Before the film, a chequerboard: both themes seen.
                    Show(terminal, (theme == _light) == Chequered(cell), answer.Commands);
                    _gathered = _clock.Elapsed;
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
            case "space" when _began is not null:
                _paused = now;
                break;
            case "r" when _began is not null:
                Stand(now, commands);
                break;
        }
    }

    /// <summary>Whether the shell in a terminal has printed its prompt, which its title says.</summary>
    private bool IsReady(Terminal terminal) =>
        _windows.TryGetValue(terminal.Id, out var window) && window.Title == Ready;

    /// <summary>Every terminal, with the cell it stands in.</summary>
    private IEnumerable<(Terminal Terminal, int Cell)> Everyone() =>
        _light.Select((terminal, cell) => (terminal, cell))
            .Concat(_dark.Select((terminal, cell) => (terminal, cell)));

    /// <summary>Whether a cell is a light square of the chequerboard the terminals stand in before the film.</summary>
    private bool Chequered(int cell) => (cell % _film.Columns + cell / _film.Columns) % 2 == 0;

    /// <summary>
    /// `present`: how a window is drawn, apart from where it lives. Here
    /// only whether it is drawn at all: a terminal whose colour its cell
    /// does not need stays a window, open and in its place, with no
    /// opacity. Nothing is said of one that is already as it is wanted.
    /// </summary>
    private static void Show(Terminal terminal, bool shown, List<Command> commands)
    {
        if (terminal.Shown == shown || !terminal.Open)
        {
            return;
        }
        terminal.Shown = shown;
        commands.Add(new Command.Present
        {
            Value = new Present
            {
                Id = terminal.Id,
                Opacity = shown ? 1f : 0f,
                Pivot = (0.5f, 0.5f),
                Motion = new Motion { DurationMs = 0, Easing = new Easing.Linear() },
            },
        });
    }

    /// <summary>How long to wait for the compositor before something of this program's own is due.</summary>
    private TimeSpan UntilSomethingIsDue()
    {
        var now = _clock.Elapsed;
        var due = _measuredAt + Measured;
        if (_began is { } began && _paused is null)
        {
            var frame = began + TimeSpan.FromSeconds((_shown + 1) / _film.FramesASecond);
            due = frame < due ? frame : due;
        }
        return due > now ? due - now : TimeSpan.Zero;
    }

    /// <summary>Where a cell of the film is on the screen.</summary>
    private Rect Cell(int cell)
    {
        var stage = Stage();
        var (w, h) = (stage.W / _film.Columns, stage.H / _film.Rows);
        return new Rect
        {
            X = stage.X + cell % _film.Columns * w,
            Y = stage.Y + cell / _film.Columns * h,
            W = w,
            H = h,
        };
    }

    /// <summary>
    /// Once every terminal has opened, the shell in each has printed its
    /// prompt, and they have stood so a while, begin.
    /// </summary>
    private void Gather(List<Command> commands)
    {
        var now = _clock.Elapsed;
        if (_began is not null || _gathered is not { } gathered
            || _light.Count < _film.Cells || _dark.Count < _film.Cells
            || !Everyone().All(each => !each.Terminal.Open || IsReady(each.Terminal)))
        {
            return;
        }
        if (now - gathered >= Gathered)
        {
            if (_watching)
            {
                // Nothing more to wait for: the film is on this program's
                // own clock.
                _watching = false;
                commands.Add(new Command.TimerCancel { Value = Watch });
            }
            _began = now;
            _shown = -1;
            // For whoever puts the film's sound to a recording of this.
            Console.Error.WriteLine($"bad-apple: the film began at {DateTime.UtcNow:O}");
        }
    }

    /// <summary>
    /// The chequerboard the terminals stand in while no film plays: as they
    /// stood before it, when it ends, and when it is asked for from the
    /// beginning. The film begins from here (`Gather`).
    /// </summary>
    private void Stand(TimeSpan now, List<Command> commands)
    {
        _began = null;
        _paused = null;
        _gathered = now;
        for (var cell = 0; cell < _film.Cells; cell++)
        {
            Show(_light[cell], Chequered(cell), commands);
            Show(_dark[cell], !Chequered(cell), commands);
        }
    }

    /// <summary>Show the frame that is due, if it is not the one showing.</summary>
    private void Play(List<Command> commands)
    {
        var now = _clock.Elapsed;
        if (_began is not { } began || _paused is not null || now < began)
        {
            return;
        }
        var index = (int)((now - began).TotalSeconds * _film.FramesASecond);
        if (index >= _film.Frames.Count)
        {
            // The end: the chequerboard again, and after a while the film
            // again.
            Stand(now, commands);
            return;
        }
        if (index == _shown)
        {
            return;
        }
        // The frame: in each cell, the terminal whose theme is the colour
        // the cell has now. Only the cells that changed are spoken of.
        var before = commands.Count;
        var frame = _film.Frames[index];
        for (var cell = 0; cell < frame.Length; cell++)
        {
            Show(_light[cell], frame[cell], commands);
            Show(_dark[cell], !frame[cell], commands);
        }
        _changed += (commands.Count - before) / 2;
        _frames++;
        _shown = index;
    }

    /// <summary>
    /// The panel: a few numbers, said twice a second in a scene of this
    /// program's own that the compositor draws.
    /// </summary>
    private void Measure(List<Command> commands)
    {
        var now = _clock.Elapsed;
        if (now - _measuredAt < Measured || _monitors.Count == 0)
        {
            return;
        }
        var seconds = (now - _measuredAt).TotalSeconds;
        // What this program has used of a processor over the last two
        // seconds: the system counts it in steps too coarse for half of one.
        _cpu.Enqueue((now, Process.GetCurrentProcess().TotalProcessorTime));
        while (_cpu.Count > 5)
        {
            _cpu.Dequeue();
        }
        var (since, used) = _cpu.Peek();
        var cpu = now > since
            ? (Process.GetCurrentProcess().TotalProcessorTime - used).TotalSeconds / (now - since).TotalSeconds * 100
            : 0;
        // The film's numbers are said only of a half second it played
        // right through.
        var played = _began is { } began && _paused is null && began <= _measuredAt;
        var rows = new[]
        {
            new { name = "FPS", value = played ? $"{_frames / seconds:0}" : "–" },
            new { name = "Terminals", value = $"{Everyone().Count(each => each.Terminal.Open)}" },
            new { name = "Changes", value = played ? $"{_changed / seconds:N0} / s" : "–" },
            // Of one processor, all this program used: the film, the
            // protocol and the runtime under them.
            new { name = "WM CPU", value = $"{cpu:0.0} %" },
        };
        _measuredAt = now;
        _frames = _changed = 0;

        var whole = Whole();
        commands.Add(new Command.Surface
        {
            Value = new Surface
            {
                Name = "bad-apple",
                Scene = Path.Combine(AppContext.BaseDirectory, "Stats.qml"),
                Layer = Layer.Overlay,
                On = new Placement.Rect { Value = ((int)whole.X + 10, (int)whole.Y + 10, 180, 128) },
                Properties = JsonSerializer.Serialize(new { rows }),
                OutsideClick = new OutsideClick { Default = Outside.Pass },
                Keyboard = KeyPolicy.ExceptClaimed,
                Reserve = new Insets(),
            },
        });
    }

    /// <summary>The whole of the primary monitor.</summary>
    private Rect Whole()
    {
        var monitor = _monitors.FirstOrDefault(monitor => monitor.Primary) ?? _monitors.FirstOrDefault();
        return monitor?.Whole ?? new Rect { W = 1280, H = 720 };
    }

    /// <summary>The part of it the film is shown in: as large as fits, in the film's own shape, in the middle.</summary>
    private Rect Stage()
    {
        var whole = Whole();
        var scale = Math.Min(whole.W / _film.Columns, whole.H / _film.Rows);
        var (w, h) = (_film.Columns * scale, _film.Rows * scale);
        return new Rect { X = whole.X + (whole.W - w) / 2, Y = whole.Y + (whole.H - h) / 2, W = w, H = h };
    }

    /// <summary>
    /// `place`: where a window lives from now on. Its application is told
    /// the size, and the compositor takes it there over `ms`.
    /// </summary>
    private static Command Put(ulong id, Rect at, double ms, Easing easing) => new Command.Place
    {
        Value = new Place
        {
            Id = id,
            Rect = at,
            Motion = new Motion { DurationMs = ms, Easing = easing },
            Tile = true,
        },
    };
}
