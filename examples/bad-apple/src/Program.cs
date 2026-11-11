// Bad Apple, played by a window manager.
//
// This program is a window manager for Solium, written in C#. Solium starts
// it with a socket as its standard input, and everything it does, it does
// by sending messages over that socket: it draws nothing itself.
//
// The film is a black and white one, each frame cut into rectangles: the
// light parts into some, the dark parts into others. Every rectangle is a
// real terminal with a shell and its prompt, light-themed or dark-themed,
// and a rectangle lives from frame to frame as a window does, so playing
// the film is managing windows: each frame, the terminals whose rectangles
// changed are placed where they are now, which moves them and resizes
// them, and a terminal no rectangle needs is not drawn.
//
// A terminal does not redraw itself at a new size the instant it is asked
// to, and it does not have to. The compositor draws a window in the
// rectangle it is on its way to at once, from the picture its application
// last drew, and the application catches up when it can.
//
// It keeps no clock of its own. The compositor's timers tick on the clock
// its animations run on, and the film is played from one: a tick, a frame.
// Every batch says what that clock read, which is what the panel's rates
// are worked out by.
//
// What it shows of the protocol:
//   - the handshake, and what a window manager says before the first frame;
//   - `listen`: it hears four events, and beyond them only its own keys
//     and timers;
//   - `declare`: its keys are its own, and the compositor knows without
//     asking;
//   - `configure`: a setting of the compositor's, as a document;
//   - `timer`: woken on the compositor's clock, for each frame of the film
//     and for what it measures;
//   - `place`: where a window lives and how large, and how it gets there;
//   - `present` and `clear`: how a window is drawn, apart from where it
//     lives, and that taken away again;
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
/// One terminal: its window, where it was last placed, whether it is
/// drawn, and whether it is still open. One that somebody closed is left
/// out of every frame after, and its rectangles stay empty.
/// </summary>
internal sealed class Terminal
{
    public ulong Id;
    public (int X, int Y, int W, int H) At;
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

    /// <summary>
    /// The two timers. One falls due twice a second, always: that is how
    /// often the panel's numbers are worked out, and what the terminals'
    /// standing ready is counted in. The other falls due once for each
    /// frame of the film while it plays.
    /// </summary>
    private const ulong Measure = 1;
    private const ulong Frame = 2;
    private const double MeasuredMs = 500;

    /// <summary>How many times the first timer falls due with the terminals in their rows, all ready, before the film.</summary>
    private const int Gathered = 10;

    /// <summary>
    /// How long, in milliseconds, a terminal takes into its row and out of
    /// it into the film's first frame, and how long one the first frame has
    /// no use for takes to fade.
    /// </summary>
    private const double Arranged = 500;
    private const double Faded = 300;

    private readonly Connection _solium;
    private readonly Film _film;
    private readonly List<string> _open;

    // What it has been told of the desktop: the monitors and the windows,
    // and of those, the terminals of each theme, in the order they opened:
    // the one that opened nth of its theme is the film's nth of that colour.
    private List<Monitor> _monitors = new();
    private readonly Dictionary<ulong, Window> _windows = new();
    private readonly List<Terminal> _light = new();
    private readonly List<Terminal> _dark = new();

    // Where the film is: whether it plays, whether it is held, and the
    // frame last shown. And how many times the first timer has fallen due
    // since a terminal last opened or printed its prompt.
    private bool _playing;
    private bool _paused;
    private int _shown = -1;
    private int _settled;

    // The compositor's clock, in milliseconds, as the batch being answered
    // read it. And what the panel says, counted since it last said it:
    // when that was, whether the film played right through since, and
    // what this program had used of a processor at each of the last few.
    private double _now;
    private double _said;
    private int _frames;
    private int _moved;
    private bool _played;
    private readonly Queue<(double At, TimeSpan Used)> _used = new();

    public Player(Connection solium, Film film, List<string> open)
    {
        _solium = solium;
        _film = film;
        _open = open;
    }

    /// <summary>
    /// What it tells the compositor before the first frame, and again after
    /// a reload: which keys are its own, which events it wants to hear,
    /// that every window is drawn with a plain border, and its timers.
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
                        Key("r", "the rows, and the film from the beginning"),
                    },
                },
            },
            // `renamed` for the terminals' titles: a shell names its window
            // as it prints its prompt, and no other event says so.
            new Command.Listen { Value = { "opened", "closed", "monitors-changed", "renamed" } },
            // The frame every window wears: a border and no titlebar, so
            // a terminal a cell wide still has room for its prompt.
            new Command.Decoration { Value = "border" },
            // A setting, as any window manager sets one: a section's name
            // and a document of its keys. Nobody touches a key while a
            // film plays: the session is not locked, and the screens are
            // not turned off, for want of one. Nought is never.
            new Command.Configure { Value = ("idle", Section(("lock_after", 0L), ("screens_off_after", 0L))) },
            Every(Measure, MeasuredMs),
        };
        if (_playing && !_paused)
        {
            commands.Add(Every(Frame, 1000 / _film.FramesASecond));
        }
        return commands;
    }

    /// <summary>`timer`: woken every so often, by the clock the compositor's animations run on.</summary>
    private static Command Every(ulong id, double ms) => new Command.Timer
    {
        Value = new Timer { Id = id, AfterMs = ms, PeriodMs = ms },
    };

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

        // From here it only answers: everything it does is its answer to
        // something the compositor told it.
        while (true)
        {
            switch (_solium.Receive(TimeSpan.FromMinutes(1)))
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
        _now = batch.At;
        foreach (var change in batch.Changes)
        {
            switch (change)
            {
                case Change.Monitors monitors:
                    _monitors = monitors.Value;
                    // Everything is somewhere else on another screen: the
                    // rows now, and the film's frame when the next is
                    // shown.
                    if (!_playing)
                    {
                        Arrange(0, answer.Commands);
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
                        _settled = 0;
                    }
                    _windows[window.Value.Id] = window.Value;
                    break;
                case Change.WindowGone gone:
                    _windows.Remove(gone.Value);
                    foreach (var (terminal, _, _) in Everyone().Where(each => each.Terminal.Id == gone.Value))
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
                    // The terminals of each theme, as many as the film
                    // needs. Run and left to themselves: a window manager
                    // starts nothing itself, and each window they open is
                    // told as an `opened`.
                    foreach (var (which, count) in new[] { ("light", _film.Light), ("dark", _film.Dark) })
                    {
                        answer.Commands.Add(new Command.RunDetached
                        {
                            Value = _open.Append(which).Append(count.ToString()).ToList(),
                        });
                    }
                    break;
                case Event.Opened opened when _windows.TryGetValue(opened.Value, out var window):
                    // Placed in the answer to its opening, which is where a
                    // window manager places a window. The compositor glides
                    // it to its place in the rows from where it opened.
                    var light = window.AppId == Light;
                    var theme = light ? _light : window.AppId == Dark ? _dark : null;
                    if (theme is null || theme.Count >= (light ? _film.Light : _film.Dark))
                    {
                        break;
                    }
                    var terminal = new Terminal { Id = opened.Value };
                    theme.Add(terminal);
                    answer.Handled = true;
                    Put(terminal, Standing(light, theme.Count - 1), Arranged, new Easing.OutCubic(), answer.Commands);
                    _settled = 0;
                    break;
                case Event.Timers timers:
                    foreach (var (id, _) in timers.Value)
                    {
                        if (id == Frame)
                        {
                            Play(answer.Commands);
                        }
                        else if (id == Measure)
                        {
                            Say(answer.Commands);
                            Gather(answer.Commands);
                        }
                    }
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
        switch (combo)
        {
            case "escape":
                commands.Add(new Command.Quit());
                break;
            case "space" when _playing && _paused:
                // On from where it stopped.
                _paused = false;
                _played = false;
                commands.Add(Every(Frame, 1000 / _film.FramesASecond));
                break;
            case "space" when _playing:
                _paused = true;
                commands.Add(new Command.TimerCancel { Value = Frame });
                break;
            case "r" when _playing:
                Stand(commands);
                break;
        }
    }

    /// <summary>Whether the shell in a terminal has printed its prompt, which its title says.</summary>
    private bool IsReady(Terminal terminal) =>
        _windows.TryGetValue(terminal.Id, out var window) && window.Title == Ready;

    /// <summary>Every terminal, with its theme and which of that theme it is.</summary>
    private IEnumerable<(Terminal Terminal, bool Light, int Nth)> Everyone() =>
        _light.Select((terminal, nth) => (terminal, true, nth))
            .Concat(_dark.Select((terminal, nth) => (terminal, false, nth)));

    /// <summary>
    /// `place`: where a window lives from now on, and how large. Its
    /// application is told the size, and the compositor takes it there
    /// over `ms`, drawing it in its rectangle from the picture the
    /// application has drawn so far until it has drawn the size. Nothing
    /// is said of a terminal that is there already.
    /// </summary>
    private void Put(Terminal terminal, (int X, int Y, int W, int H) at, double ms, Easing easing,
        List<Command> commands)
    {
        if (terminal.At == at || !terminal.Open)
        {
            return;
        }
        terminal.At = at;
        _moved++;
        commands.Add(new Command.Place
        {
            Value = new Place
            {
                Id = terminal.Id,
                Rect = new Rect { X = at.X, Y = at.Y, W = at.W, H = at.H },
                Motion = new Motion { DurationMs = ms, Easing = easing },
                // Cut to its rectangle: a terminal still drawn larger
                // does not lie over its neighbours.
                Tile = true,
            },
        });
    }

    /// <summary>
    /// `present`: how a window is drawn, apart from where it lives. Here
    /// only whether it is drawn at all: a terminal no rectangle needs stays
    /// a window, open and where it was, with no opacity, and `clear` takes
    /// that away again. Nothing is said of one that is already as wanted.
    /// </summary>
    private static void Show(Terminal terminal, bool shown, double ms, List<Command> commands)
    {
        if (terminal.Shown == shown || !terminal.Open)
        {
            return;
        }
        terminal.Shown = shown;
        var motion = new Motion { DurationMs = ms, Easing = new Easing.Linear() };
        commands.Add(shown
            ? new Command.Clear { Value = (terminal.Id, motion) }
            : new Command.Present
            {
                Value = new Present { Id = terminal.Id, Opacity = 0f, Pivot = (0.5f, 0.5f), Motion = motion },
            });
    }

    /// <summary>
    /// Where a terminal stands while no film plays: all of them in rows,
    /// light and dark by turns.
    /// </summary>
    private (int X, int Y, int W, int H) Standing(bool light, int nth)
    {
        var paired = Math.Min(_film.Light, _film.Dark);
        var place = nth < paired ? 2 * nth + (light ? 0 : 1) : paired + nth;
        var all = _film.Light + _film.Dark;
        var stage = Stage();
        var columns = (int)Math.Ceiling(Math.Sqrt(all * stage.W / stage.H));
        // An odd number across, so the colours take turns down a column as
        // well as along a row.
        columns += 1 - columns % 2;
        var rows = (all + columns - 1) / columns;
        return Span(place % columns, place / columns, 1, 1, columns, rows);
    }

    /// <summary>
    /// Where a rectangle of a grid is on the screen, in whole pixels. Each
    /// edge is rounded by itself, so two rectangles that share an edge in
    /// the grid share it on the screen.
    /// </summary>
    private (int X, int Y, int W, int H) Span(int x, int y, int w, int h, int columns, int rows)
    {
        var stage = Stage();
        int Across(int at) => (int)Math.Round(stage.X + stage.W * at / columns);
        int Down(int at) => (int)Math.Round(stage.Y + stage.H * at / rows);
        return (Across(x), Down(y), Across(x + w) - Across(x), Down(y + h) - Down(y));
    }

    /// <summary>
    /// Once every terminal has opened, the shell in each has printed its
    /// prompt, and they have stood so a while, begin: the film's timer is
    /// set, and each time it falls due is a frame.
    /// </summary>
    private void Gather(List<Command> commands)
    {
        if (_playing || _light.Count < _film.Light || _dark.Count < _film.Dark
            || !Everyone().All(each => !each.Terminal.Open || IsReady(each.Terminal)))
        {
            return;
        }
        if (++_settled < Gathered)
        {
            return;
        }
        _playing = true;
        _paused = false;
        _played = false;
        _shown = -1;
        commands.Add(Every(Frame, 1000 / _film.FramesASecond));
        // For whoever puts the film's sound to a recording of this.
        Console.Error.WriteLine($"bad-apple: the film began at {DateTime.UtcNow:O}");
    }

    /// <summary>
    /// No film: every terminal back in the rows, as they stood before it.
    /// So it is when the film ends and when it is asked for from the
    /// beginning, and the film begins from here (`Gather`).
    /// </summary>
    private void Stand(List<Command> commands)
    {
        if (!_paused)
        {
            commands.Add(new Command.TimerCancel { Value = Frame });
        }
        _playing = false;
        _paused = false;
        _settled = 0;
        Arrange(Arranged, commands);
    }

    /// <summary>Every terminal to its place in the rows, and drawn.</summary>
    private void Arrange(double ms, List<Command> commands)
    {
        foreach (var (terminal, light, nth) in Everyone())
        {
            Put(terminal, Standing(light, nth), ms, new Easing.OutCubic(), commands);
            Show(terminal, true, Math.Min(ms, Faded), commands);
        }
    }

    /// <summary>The film's timer fell due: the next frame.</summary>
    private void Play(List<Command> commands)
    {
        if (!_playing || _paused)
        {
            return;
        }
        var index = _shown + 1;
        if (index >= _film.Frames.Count)
        {
            // The end: the rows again, and after a while the film again.
            Stand(commands);
            return;
        }
        var frame = _film.Frames[index];
        var first = _shown < 0;
        Lay(_light, frame.Light, first, commands);
        Lay(_dark, frame.Dark, first, commands);
        _frames++;
        _shown = index;
    }

    /// <summary>
    /// One colour of a frame: each of its rectangles is the terminal it
    /// names, placed where the rectangle is, and a terminal the frame does
    /// not name is no longer drawn.
    ///
    /// A terminal is placed with no motion: it is in its rectangle on the
    /// next frame the compositor draws, whole, from the picture it last
    /// drew, and stays so while it draws the new size. Out of the rows,
    /// into the film's `first` frame, the terminals take their time
    /// instead, and those it has no use for fade.
    /// </summary>
    private void Lay(List<Terminal> terminals, Box[] boxes, bool first, List<Command> commands)
    {
        var wanted = new Box?[terminals.Count];
        foreach (var box in boxes)
        {
            wanted[box.Terminal] = box;
        }
        for (var nth = 0; nth < terminals.Count; nth++)
        {
            var terminal = terminals[nth];
            if (wanted[nth] is not { } rectangle)
            {
                Show(terminal, false, first ? Faded : 0, commands);
                continue;
            }
            var at = Span(rectangle.X, rectangle.Y, rectangle.W, rectangle.H, _film.Columns, _film.Rows);
            if (first)
            {
                Put(terminal, at, Arranged, new Easing.OutCubic(), commands);
                continue;
            }
            Show(terminal, true, 0, commands);
            Put(terminal, at, 0, new Easing.Linear(), commands);
        }
    }

    /// <summary>
    /// The panel: a few numbers, said each time the first timer falls due,
    /// in a scene of this program's own that the compositor draws. How
    /// long it has been since the last is read off the compositor's
    /// clock, which every batch carries: a timer that fell due late is
    /// not taken for half a second.
    /// </summary>
    private void Say(List<Command> commands)
    {
        if (_monitors.Count == 0)
        {
            return;
        }
        var seconds = _said > 0 && _now > _said ? (_now - _said) / 1000 : MeasuredMs / 1000;
        _said = _now;
        // What this program has used of a processor over the last two
        // seconds: the system counts it in steps too coarse for half of one.
        _used.Enqueue((_now, Process.GetCurrentProcess().TotalProcessorTime));
        while (_used.Count > 5)
        {
            _used.Dequeue();
        }
        var (since, used) = _used.Peek();
        var cpu = _now > since
            ? (_used.Last().Used - used).TotalSeconds / ((_now - since) / 1000) * 100
            : 0;
        var rows = new[]
        {
            // How many frames of the film were shown: fewer than the film
            // has when the compositor took longer than a frame over one.
            new { name = "FPS", value = _played ? $"{_frames / seconds:0}" : "–" },
            new { name = "Terminals", value = $"{Everyone().Count(each => each.Terminal.Open)}" },
            // Terminals placed anew: moved, resized, or both.
            new { name = "Moves", value = _played ? $"{_moved / seconds:N0} / s" : "–" },
            // Of one processor, all this program used: the film, the
            // protocol and the runtime under them.
            new { name = "WM CPU", value = $"{cpu:0.0} %" },
        };
        // The film's numbers are said only of a time it played right
        // through.
        _played = _playing && !_paused;
        _frames = _moved = 0;

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
}
