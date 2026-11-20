// Space: a window manager for Solium, in C#, that puts the desktop in three
// dimensions. Three modes over the same windows:
//
//   super+1  cube      four desks on the faces of a cube, turned with
//                      super+left and super+right; super+shift+left and
//                      super+shift+right take the focused window along.
//   super+2  carousel  every window a card on a ring, the front one flat
//                      and in use; super+left and super+right turn the ring.
//   super+3  fly       the windows stand about in space and you fly among
//                      them: w a s d, r and f for up and down, the arrows to
//                      turn, return to dock in the window ahead (it lands flat
//                      and takes the keyboard), super+escape to take off.
//
// super+return opens a terminal ($TERMINAL, or foot), super+q closes the
// focused window, super+shift+e ends the session.
//
// Nothing here draws a frame or keeps time. Each mode says where the windows
// live and how each group of them is posed in three dimensions; the
// compositor turns the poses into pixels on every frame, with springs that
// keep their speed when a key changes the target. A key held down while
// flying retargets the camera at the key's repeat rate, and the compositor
// glides between.
//
// Name it in ~/.config/solium/wm.toml:
//
//     command = ["dotnet", "/path/to/space-wm.dll"]

using System;
using System.Collections.Generic;
using System.Linq;
using Solium.Wm;

namespace Space;

public static class Program
{
    public static int Main() => new Spaces().Run();
}

internal enum Mode
{
    Cube,
    Carousel,
    Fly,
}

internal sealed class Spaces : WindowManager
{
    /// <summary>The windows, in the order they opened.</summary>
    private readonly List<ulong> _windows = new();

    /// <summary>Each window's desk, a face of the cube.</summary>
    private readonly Dictionary<ulong, int> _desks = new();

    private Mode _mode = Mode.Cube;

    /// <summary>The cube's quarter turns to the right, and the ring's cards: never wrapped, so a turn always goes the near way.</summary>
    private int _turn;
    private int _card;

    /// <summary>The camera, in fly mode: where it is, and its heading in degrees.</summary>
    private double _cameraX;
    private double _cameraY;
    private double _cameraZ;
    private double _yaw;

    /// <summary>The window docked in, in fly mode: none while flying.</summary>
    private ulong? _docked;

    private const ulong ZoomBack = 1;
    private const ulong Docked = 2;

    private const string Backdrop = "space-backdrop";

    protected override IEnumerable<Command> Setup()
    {
        yield return Keys();
        yield return Commands.Listen("opened", "closed", "monitors-changed");
        yield return Surface();
    }

    // ---- The conversation ----

    protected override void Handle(Batch batch, Answer answer)
    {
        foreach (var each in batch.Events)
        {
            switch (each.KindCase)
            {
                case Event.KindOneofCase.Opened:
                    _windows.Add(each.Opened);
                    _desks[each.Opened] = Mod(_turn, 4);
                    // On the ring, a new card is turned to the front, the
                    // near way round.
                    var newest = _windows.Count - 1;
                    _card += Mod(newest - _card + _windows.Count / 2, _windows.Count) - _windows.Count / 2;
                    answer.Handled = true;
                    answer.Commands.Add(Arrange(Gentle()));
                    answer.Commands.Add(Commands.Focus(each.Opened));
                    break;
                case Event.KindOneofCase.Closed:
                    _windows.Remove(each.Closed);
                    _desks.Remove(each.Closed);
                    if (_docked == each.Closed)
                    {
                        _docked = null;
                    }
                    answer.Commands.Add(Arrange(Gentle()));
                    break;
                case Event.KindOneofCase.MonitorsChanged:
                    answer.Commands.Add(Arrange(Gentle()));
                    break;
                case Event.KindOneofCase.Key:
                    answer.Handled = true;
                    answer.Commands.Add(Pressed(each.Key));
                    break;
                case Event.KindOneofCase.Timers:
                    foreach (var due in each.Timers.Items)
                    {
                        answer.Commands.Add(Due(due.Id));
                    }
                    break;
            }
        }
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
            case "super+shift+e":
                return new[] { Commands.Quit() };
            case "super+1":
                return Switch(Mode.Cube);
            case "super+2":
                return Switch(Mode.Carousel);
            case "super+3":
                return Switch(Mode.Fly);
        }
        return _mode switch
        {
            Mode.Cube => CubeKey(combo),
            Mode.Carousel => CarouselKey(combo),
            _ => FlyKey(combo),
        };
    }

    private IEnumerable<Command> Due(ulong timer) => timer switch
    {
        ZoomBack when _mode == Mode.Cube => new[] { CubePose(0, Settle()) },
        Docked when _mode == Mode.Fly && _docked is { } window => new[]
        {
            Keys(),
            new Command { Grab = false },
            Commands.Focus(window),
        },
        _ => Array.Empty<Command>(),
    };

    // ---- Keys: each mode has its own, and fly mode takes letters while flying ----

    private Command Keys()
    {
        var keys = new List<(string Combo, string Note, bool Repeating)>
        {
            ("super+return", "open a terminal", false),
            ("super+q", "close the focused window", false),
            ("super+shift+e", "end the session", false),
            ("super+1", "the cube of desks", false),
            ("super+2", "the carousel", false),
            ("super+3", "fly among the windows", false),
        };
        switch (_mode)
        {
            case Mode.Cube:
                keys.Add(("super+left", "turn to the desk on the left", false));
                keys.Add(("super+right", "turn to the desk on the right", false));
                keys.Add(("super+shift+left", "take the window to the desk on the left", false));
                keys.Add(("super+shift+right", "take the window to the desk on the right", false));
                break;
            case Mode.Carousel:
                keys.Add(("super+left", "turn the ring left", false));
                keys.Add(("super+right", "turn the ring right", false));
                break;
            case Mode.Fly when _docked is null:
                foreach (var (key, note) in new[]
                {
                    ("w", "fly forward"), ("s", "fly back"), ("up", "fly forward"), ("down", "fly back"),
                    ("a", "slide left"), ("d", "slide right"), ("r", "rise"), ("f", "sink"),
                    ("left", "turn left"), ("right", "turn right"),
                })
                {
                    keys.Add((key, note, true));
                }
                keys.Add(("return", "dock in the window ahead", false));
                break;
            case Mode.Fly:
                keys.Add(("super+escape", "take off", false));
                break;
        }
        return new Command
        {
            Declare = new Declarations
            {
                Bindings =
                {
                    keys.Select(key => new Binding
                    {
                        Combo = key.Combo,
                        Note = key.Note,
                        Repeating = key.Repeating,
                    }),
                },
            },
        };
    }

    private IEnumerable<Command> Switch(Mode mode)
    {
        if (mode == _mode)
        {
            return Array.Empty<Command>();
        }
        var leaving = Groups().ToList();
        _mode = mode;
        if (mode == Mode.Fly)
        {
            _docked = null;
            (_cameraX, _cameraY, _cameraZ, _yaw) = (0, 0, Width() * 1.6, 0);
        }
        var commands = new List<Command>();
        // What the old mode posed goes back to flat as the new mode's poses
        // come in, both by one motion, so nothing jumps between them.
        foreach (var group in leaving)
        {
            commands.Add(Commands.Uncarry(group, Gentle()));
        }
        commands.Add(Keys());
        commands.Add(new Command { Grab = mode == Mode.Fly });
        commands.AddRange(Arrange(Gentle()));
        return commands;
    }

    /// <summary>The groups the mode in charge poses.</summary>
    private IEnumerable<string> Groups() => _mode switch
    {
        Mode.Cube => new[] { "cube" }.Concat(Enumerable.Range(0, 4).Select(desk => $"face-{desk}")),
        Mode.Carousel => new[] { "ring" }.Concat(_windows.Select(id => $"card-{id}")),
        _ => new[] { "eye", "move" }.Concat(_windows.Select(id => $"spot-{id}")),
    };

    private IEnumerable<Command> Arrange(Motion motion) => _mode switch
    {
        Mode.Cube => ArrangeCube(motion),
        Mode.Carousel => ArrangeCarousel(motion),
        _ => ArrangeFly(motion),
    };

    // ---- The cube ----
    //
    // Each desk is a face: its windows tiled across the screen, and the face
    // turned a quarter more than the last about the cube's middle, which is
    // half a screen behind the screen. The cube, the faces' parent, turns
    // them all and is seen in perspective; a face turned away is hidden, so
    // at rest only the front one is drawn and takes the pointer, flat.

    private IEnumerable<Command> ArrangeCube(Motion motion)
    {
        var work = Work();
        for (var desk = 0; desk < 4; desk++)
        {
            var on = _windows.Where(id => _desks.GetValueOrDefault(id) == desk).ToList();
            var gap = 16.0;
            var width = (work.W - gap * (on.Count + 1)) / Math.Max(on.Count, 1);
            for (var column = 0; column < on.Count; column++)
            {
                yield return Commands.Place(on[column], new Rect
                {
                    X = work.X + gap + (width + gap) * column,
                    Y = work.Y + gap,
                    W = width,
                    H = work.H - gap * 2,
                }, motion);
            }
        }
        yield return Commands.Group("cube", Array.Empty<ulong>());
        for (var desk = 0; desk < 4; desk++)
        {
            var on = _windows.Where(id => _desks.GetValueOrDefault(id) == desk);
            yield return Commands.Group($"face-{desk}", on, parent: "cube");
            yield return Commands.Carry($"face-{desk}", new Pose
            {
                RotateY = 90 * desk,
                OriginX = CenterX(),
                OriginY = CenterY(),
                OriginZ = -Width() / 2,
            }, motion, backface: true);
        }
        yield return CubePose(0, motion);
    }

    /// <summary>The cube turned to <see cref="_turn"/>, so far back.</summary>
    private Command CubePose(double back, Motion motion) => Commands.Carry("cube", new Pose
    {
        RotateY = -90 * _turn,
        Z = -back,
        OriginX = CenterX(),
        OriginY = CenterY(),
        OriginZ = -Width() / 2,
        Perspective = Width() * 1.6,
    }, motion);

    private IEnumerable<Command> CubeKey(string combo)
    {
        var step = combo.EndsWith("right", StringComparison.Ordinal) ? 1 : -1;
        if (combo.StartsWith("super+shift+", StringComparison.Ordinal))
        {
            // The window goes to the next desk, and the cube turns after it.
            if (Desktop.Focused is not { } focused || !_desks.ContainsKey(focused.Id))
            {
                return Array.Empty<Command>();
            }
            _desks[focused.Id] = Mod(_desks[focused.Id] + step, 4);
            _turn += step;
            return ArrangeCube(Gentle())
                .Concat(Turned())
                .Append(Commands.Focus(focused.Id));
        }
        _turn += step;
        return Turned().Concat(FocusFront());
    }

    /// <summary>
    /// The cube turning to <see cref="_turn"/>: it backs away as it turns,
    /// and comes back once the turn is mostly done.
    /// </summary>
    private IEnumerable<Command> Turned()
    {
        yield return CubePose(Width() * 0.45, Commands.Motion(0, Ease.Spring(150, 24)));
        yield return Commands.After(ZoomBack, 260);
    }

    private IEnumerable<Command> FocusFront()
    {
        var front = _windows.LastOrDefault(id => _desks.GetValueOrDefault(id) == Mod(_turn, 4));
        return front == 0 ? Array.Empty<Command>() : new[] { Commands.Focus(front) };
    }

    // ---- The carousel ----
    //
    // Every window a card of the same size, the cards round a ring standing
    // on its edge behind the screen, each turned a share of a whole turn
    // about the ring's middle. The ring turns the card asked for to the
    // front, where it is flat, raised and has the keyboard.

    private IEnumerable<Command> ArrangeCarousel(Motion motion)
    {
        var card = Card();
        foreach (var id in _windows)
        {
            yield return Commands.Place(id, card, motion);
        }
        var count = Math.Max(_windows.Count, 1);
        var radius = count switch
        {
            1 => 0.0,
            2 => card.W * 0.75,
            _ => card.W / 2 / Math.Tan(Math.PI / count) + card.W * 0.2,
        };
        yield return Commands.Group("ring", Array.Empty<ulong>());
        for (var at = 0; at < _windows.Count; at++)
        {
            var name = $"card-{_windows[at]}";
            yield return Commands.Group(name, new[] { _windows[at] }, parent: "ring");
            yield return Commands.Carry(name, new Pose
            {
                RotateY = 360.0 / count * at,
                OriginX = CenterX(),
                OriginY = CenterY(),
                OriginZ = -radius,
            }, motion, backface: true);
        }
        yield return Commands.Carry("ring", new Pose
        {
            RotateY = -360.0 / count * _card,
            OriginX = CenterX(),
            OriginY = CenterY(),
            OriginZ = -radius,
            Perspective = Width() * 1.6,
        }, motion);
        if (_windows.Count > 0)
        {
            yield return Commands.Focus(_windows[Mod(_card, _windows.Count)]);
        }
    }

    private IEnumerable<Command> CarouselKey(string combo)
    {
        _card += combo.EndsWith("right", StringComparison.Ordinal) ? 1 : -1;
        return ArrangeCarousel(Commands.Motion(0, Ease.Spring(120, 20)));
    }

    // ---- Flying ----
    //
    // The windows stand about in rows going away from you, each turned a
    // little toward the middle. Three groups carry each one: its spot in the
    // room; the camera's move, which takes the whole room the other way;
    // and the camera's heading, which turns the room about your eye and sees
    // it in perspective. Docking puts the camera where the window ahead
    // comes out flat and full size, so it is used as any window is.

    private IEnumerable<Command> ArrangeFly(Motion motion)
    {
        var card = Card();
        foreach (var id in _windows)
        {
            yield return Commands.Place(id, card, motion);
        }
        yield return Commands.Group("eye", Array.Empty<ulong>());
        yield return Commands.Group("move", Array.Empty<ulong>(), parent: "eye");
        for (var at = 0; at < _windows.Count; at++)
        {
            var name = $"spot-{_windows[at]}";
            var (x, y, z, heading) = Spot(at);
            yield return Commands.Group(name, new[] { _windows[at] }, parent: "move");
            yield return Commands.Carry(name, new Pose
            {
                X = x,
                Y = y,
                Z = z,
                RotateY = heading,
                OriginX = CenterX(),
                OriginY = CenterY(),
            }, motion, backface: true);
        }
        foreach (var command in Camera(motion))
        {
            yield return command;
        }
    }

    /// <summary>Where window <paramref name="at"/> stands: rows of three going away, at heights that vary.</summary>
    private (double X, double Y, double Z, double Heading) Spot(int at)
    {
        var column = at % 3 - 1;
        var row = at / 3;
        return (
            column * Width() * 0.95,
            (at * 2 % 3 - 1) * Height() * 0.22,
            -row * Width() * 1.3,
            -column * 22.0);
    }

    private IEnumerable<Command> Camera(Motion motion)
    {
        yield return Commands.Carry("move", new Pose { X = -_cameraX, Y = -_cameraY, Z = -_cameraZ }, motion);
        yield return Commands.Carry("eye", new Pose
        {
            RotateY = -_yaw,
            OriginX = CenterX(),
            OriginY = CenterY(),
            OriginZ = Eye(),
            Perspective = Eye(),
        }, motion);
    }

    private IEnumerable<Command> FlyKey(string combo)
    {
        if (_docked is not null)
        {
            if (combo != "super+escape")
            {
                return Array.Empty<Command>();
            }
            // Back out of the window, the way it came in, and fly again.
            _docked = null;
            Move(-Width() * 0.7, 0, 0);
            return Camera(Commands.Motion(0, Ease.Spring(70, 16)))
                .Append(Keys())
                .Append(new Command { Grab = true });
        }
        var step = Width() * 0.06;
        switch (combo)
        {
            case "w" or "up":
                Move(step, 0, 0);
                break;
            case "s" or "down":
                Move(-step, 0, 0);
                break;
            case "a":
                Move(0, -step, 0);
                break;
            case "d":
                Move(0, step, 0);
                break;
            case "r":
                Move(0, 0, -step);
                break;
            case "f":
                Move(0, 0, step);
                break;
            case "left":
                _yaw += 3;
                break;
            case "right":
                _yaw -= 3;
                break;
            case "return":
                return Dock();
            default:
                return Array.Empty<Command>();
        }
        return Camera(Commands.Motion(0, Ease.Spring(90, 19)));
    }

    /// <summary>Move the camera along its heading, across it, and up or down.</summary>
    private void Move(double forward, double right, double down)
    {
        var yaw = _yaw * Math.PI / 180;
        _cameraX += -Math.Sin(yaw) * forward + Math.Cos(yaw) * right;
        _cameraZ += -Math.Cos(yaw) * forward - Math.Sin(yaw) * right;
        _cameraY += down;
    }

    /// <summary>
    /// Fly into the window most nearly ahead: the camera comes to where that
    /// window is drawn flat and full size, then the window has the keyboard.
    /// </summary>
    private IEnumerable<Command> Dock()
    {
        var yaw = _yaw * Math.PI / 180;
        var (aheadX, aheadZ) = (-Math.Sin(yaw), -Math.Cos(yaw));
        var eyeX = _cameraX;
        var eyeZ = _cameraZ + Eye();
        int? best = null;
        var bestScore = double.MaxValue;
        for (var at = 0; at < _windows.Count; at++)
        {
            var (x, _, z, _) = Spot(at);
            var (toX, toZ) = (x - eyeX, z - eyeZ);
            var distance = Math.Sqrt(toX * toX + toZ * toZ);
            var along = (toX * aheadX + toZ * aheadZ) / Math.Max(distance, 1);
            if (along <= 0)
            {
                continue;
            }
            // Mostly the angle off ahead, then how far.
            var score = (1 - along) * 10 + distance / Width();
            if (score < bestScore)
            {
                (best, bestScore) = (at, score);
            }
        }
        if (best is not { } chosen)
        {
            return Array.Empty<Command>();
        }
        var (spotX, spotY, spotZ, heading) = Spot(chosen);
        var turned = heading * Math.PI / 180;
        // Where the camera has to be for the window to come out exactly
        // where it lives: its spot, less the eye's offset turned with it.
        _yaw = heading;
        _cameraX = spotX + Eye() * Math.Sin(turned);
        _cameraY = spotY;
        _cameraZ = spotZ + Eye() * Math.Cos(turned) - Eye();
        _docked = _windows[chosen];
        return Camera(Commands.Motion(0, Ease.Spring(60, 15)))
            .Append(Commands.After(Docked, 1100));
    }

    // ---- What every mode stands on ----

    private Command Surface()
    {
        var scene = System.IO.Path.Combine(AppContext.BaseDirectory, "qml", "Backdrop.qml");
        return new Command
        {
            Surface = new Solium.Wm.Surface
            {
                Name = Backdrop,
                Scene = scene,
                Layer = Layer.Background,
                On = new Placement { EveryMonitor = new Unit() },
                Properties = "{}",
                Reserve = new Insets(),
                OutsideClick = new OutsideClick { Default = Outside.Pass },
                Keyboard = KeyPolicy.ExceptClaimed,
            },
        };
    }

    /// <summary>The size every card is in the carousel and in flight: most of the screen, in its middle.</summary>
    private Rect Card()
    {
        var work = Work();
        var (w, h) = (work.W * 0.56, work.H * 0.66);
        return new Rect { X = work.X + (work.W - w) / 2, Y = work.Y + (work.H - h) / 2, W = w, H = h };
    }

    private Rect Screen() => Desktop.Primary?.Area ?? new Rect { W = 1920, H = 1080 };

    private Rect Work() => Desktop.WorkArea ?? Screen();

    private double Width() => Screen().W;

    private double Height() => Screen().H;

    private double CenterX() => Screen().X + Screen().W / 2;

    private double CenterY() => Screen().Y + Screen().H / 2;

    /// <summary>How far in front of the screen the viewer is: the perspective of the room.</summary>
    private double Eye() => Width() * 1.1;

    private static Motion Gentle() => Commands.Motion(0, Ease.Spring(110, 21));

    /// <summary>The cube coming back to the screen once it has turned.</summary>
    private static Motion Settle() => Commands.Motion(0, Ease.Spring(90, 19));

    private static int Mod(int value, int by) => (value % by + by) % by;
}
