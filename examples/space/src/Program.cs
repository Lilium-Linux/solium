// Space: a window manager for Solium, in C#, that puts the desktop in three
// dimensions. Three modes over the same windows:
//
//   super+1  cube      four desks on the faces of a cube, turned with
//                      super+left and super+right; super+shift+left and
//                      super+shift+right take the focused window along.
//   super+2  carousel  every window a card on a ring, the front one flat
//                      and in use; super+left and super+right turn the ring.
//   super+3  room      a room in three dimensions, its walls, floor and
//                      ceiling QML scenes, the windows hung on its walls,
//                      walked as in a game: w a s d to walk, the arrows to
//                      turn and look up and down, space to jump, return to
//                      step up to the window ahead (it comes out flat and
//                      takes the keyboard), super+escape to step back.
//
// super+return opens a terminal ($TERMINAL, or foot), super+q closes the
// focused window, super+shift+e ends the session.
//
// Nothing here draws a frame or keeps time. Each mode says where the windows
// live and how each group of them is posed in three dimensions; the
// compositor turns the poses into pixels on every frame, with springs that
// keep their speed when a key changes the target. A key held down in the
// room retargets the camera at the key's repeat rate, and the compositor
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
    Room,
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

    /// <summary>
    /// Where you stand in the room, across it and along it from its middle,
    /// which way you face and how far you look up, in degrees.
    /// </summary>
    private double _x;
    private double _z;
    private double _yaw;
    private double _pitch;

    /// <summary>Whether you are in the air, jumping: no second jump until you land.</summary>
    private bool _airborne;

    /// <summary>The window stepped up to, in the room: none while walking.</summary>
    private ulong? _docked;

    private const ulong ZoomBack = 1;
    private const ulong Docked = 2;
    private const ulong Fall = 3;
    private const ulong Land = 4;
    private const ulong Stand = 5;
    private const ulong RoomGone = 6;

    /// <summary>How long a jump takes to rise, and as long to fall.</summary>
    private const double JumpMs = 330;

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
                return Switch(Mode.Room);
        }
        return _mode switch
        {
            Mode.Cube => CubeKey(combo),
            Mode.Carousel => CarouselKey(combo),
            _ => RoomKey(combo),
        };
    }

    private IEnumerable<Command> Due(ulong timer)
    {
        switch (timer)
        {
            case ZoomBack when _mode == Mode.Cube:
                return new[] { CubePose(0, Settle()) };
            case Docked when _mode == Mode.Room && _docked is { } window:
                return new[] { Keys(), new Command { Grab = false }, Commands.Focus(window) };
            // The top of a jump: fall as things fall, faster and faster.
            case Fall:
                return new[]
                {
                    Hop(0, Commands.Motion(JumpMs, Ease.Bezier(1 / 3.0, 0, 2 / 3.0, 1 / 3.0))),
                    Commands.After(Land, JumpMs),
                };
            // Landing: the knees give a little, and you stand up again.
            case Land:
                _airborne = false;
                return new[]
                {
                    Hop(-Metre() * 0.08, Commands.Motion(90, Ease.OutCubic)),
                    Commands.After(Stand, 90),
                };
            case Stand:
                return new[] { Hop(0, Commands.Motion(0, Ease.Spring(170, 20))) };
            // The room has faded out after you left it: its scenes go.
            case RoomGone when _mode != Mode.Room:
                return Planes().SelectMany(plane => new[]
                {
                    new Command { SurfaceGone = plane.Name },
                    Commands.Group(plane.Name, null),
                });
            default:
                return Array.Empty<Command>();
        }
    }

    // ---- Keys: each mode has its own, and the room takes letters while walking ----

    private Command Keys()
    {
        var keys = new List<(string Combo, string Note, bool Repeating)>
        {
            ("super+return", "open a terminal", false),
            ("super+q", "close the focused window", false),
            ("super+shift+e", "end the session", false),
            ("super+1", "the cube of desks", false),
            ("super+2", "the carousel", false),
            ("super+3", "walk the room of windows", false),
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
            case Mode.Room when _docked is null:
                foreach (var (key, note) in new[]
                {
                    ("w", "walk forward"), ("s", "walk back"), ("a", "step left"), ("d", "step right"),
                    ("left", "turn left"), ("right", "turn right"), ("up", "look up"), ("down", "look down"),
                })
                {
                    keys.Add((key, note, true));
                }
                keys.Add(("space", "jump", false));
                keys.Add(("return", "step up to the window ahead", false));
                break;
            case Mode.Room:
                keys.Add(("super+escape", "step back", false));
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
        var left = _mode;
        _mode = mode;
        var commands = new List<Command>();
        if (mode == Mode.Room)
        {
            _docked = null;
            _airborne = false;
            (_x, _z, _yaw, _pitch) = (0, RoomDepth() / 2 - Metre() * 1.6, 0, 0);
            commands.AddRange(BuildRoom());
        }
        else if (left == Mode.Room)
        {
            // The room fades as the windows leave its walls, and its scenes
            // go once it has.
            foreach (var plane in Planes())
            {
                commands.Add(Commands.Carry(plane.Name, PlanePose(plane), Commands.Motion(300, Ease.OutCubic), backface: true, opacity: 0));
            }
            commands.Add(Commands.After(RoomGone, 320));
        }
        // What the old mode posed goes back to flat as the new mode's poses
        // come in, both by one motion, so nothing jumps between them.
        foreach (var group in leaving)
        {
            commands.Add(Commands.Uncarry(group, Gentle()));
        }
        commands.Add(Keys());
        commands.Add(new Command { Grab = mode == Mode.Room });
        commands.AddRange(Arrange(Gentle()));
        return commands;
    }

    /// <summary>The groups the mode in charge poses.</summary>
    private IEnumerable<string> Groups() => _mode switch
    {
        Mode.Cube => new[] { "cube" }.Concat(Enumerable.Range(0, 4).Select(desk => $"face-{desk}")),
        Mode.Carousel => new[] { "ring" }.Concat(_windows.Select(id => $"card-{id}")),
        _ => new[] { "eye", "turn", "hop", "move" }.Concat(_windows.Select(id => $"spot-{id}")),
    };

    private IEnumerable<Command> Arrange(Motion motion) => _mode switch
    {
        Mode.Cube => ArrangeCube(motion),
        Mode.Carousel => ArrangeCarousel(motion),
        _ => ArrangeRoom(motion),
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

    // ---- The room ----
    //
    // A room 7.2 m by 8 by 2.8 high, a metre a third of a screen across:
    // four walls, a floor and a ceiling, each a QML scene posed in its
    // place, and the windows hung on the walls at eye height. Four groups
    // carry it all, one inside the next: where you stand (`move`), how high
    // a jump has lifted you (`hop`), which way you face (`turn`) and how far
    // you look up (`eye`), the last seeing it in perspective. Its scenes
    // are drawn in the bottom layer and the windows over them, which is the
    // right order from anywhere inside a room with nothing standing in it. Stepping up to a window puts you where it comes out flat and at
    // its own size, so it is used as any window is.

    /// <summary>A flat part of the room: a scene, where its middle is, how it is turned, and its size.</summary>
    private readonly record struct Plane(
        string Name,
        string Scene,
        double X,
        double Y,
        double Z,
        double RotateX,
        double RotateY,
        double W,
        double H,
        string Properties);

    private IEnumerable<Plane> Planes()
    {
        var (w, d, h) = (RoomWidth(), RoomDepth(), RoomHeight());
        var card = Card();
        string Wall(double length, string feature) => string.Format(
            System.Globalization.CultureInfo.InvariantCulture,
            "{{\"slots\":[0.25,0.75],\"hangWidth\":{0},\"hangHeight\":{1},\"hangAt\":{2},\"feature\":\"{3}\"}}",
            card.W / length,
            card.H / h,
            (h - EyeHeight()) / h,
            feature);
        yield return new Plane("room-front", "Wall.qml", 0, -h / 2, -d / 2, 0, 0, w, h, Wall(w, "window"));
        yield return new Plane("room-back", "Wall.qml", 0, -h / 2, d / 2, 0, 180, w, h, Wall(w, "door"));
        yield return new Plane("room-left", "Wall.qml", -w / 2, -h / 2, 0, 0, 90, d, h, Wall(d, "shelf"));
        yield return new Plane("room-right", "Wall.qml", w / 2, -h / 2, 0, 0, -90, d, h, Wall(d, "art"));
        yield return new Plane("room-floor", "Floor.qml", 0, 0, 0, 90, 0, w, d, "{}");
        yield return new Plane("room-ceiling", "Ceiling.qml", 0, -h, 0, -90, 0, w, d, "{}");
    }

    /// <summary>
    /// How much smaller than the room a plane's scene is declared: as large
    /// as fits on the screen, which is the most a scene can be, and posed
    /// that many times larger. The compositor captures it at its posed
    /// size, so it is as sharp as a scene that big would be.
    /// </summary>
    private double Shrink(Plane plane) => Math.Max(plane.W / Width(), plane.H / Height());

    private PixelRect PlaneRect(Plane plane)
    {
        var shrink = Shrink(plane);
        return new PixelRect
        {
            X = (int)Screen().X,
            Y = (int)Screen().Y,
            W = (int)Math.Round(plane.W / shrink),
            H = (int)Math.Round(plane.H / shrink),
        };
    }

    /// <summary>Where a point of the room is drawn from, standing in its middle: what every pose in it is measured from.</summary>
    private (double X, double Y, double Z) Anchor() => (CenterX(), CenterY() + EyeHeight(), Eye());

    private Pose PlanePose(Plane plane)
    {
        var rect = PlaneRect(plane);
        var (middleX, middleY) = (rect.X + rect.W / 2.0, rect.Y + rect.H / 2.0);
        var (ax, ay, az) = Anchor();
        return new Pose
        {
            Scale = Shrink(plane),
            RotateX = plane.RotateX,
            RotateY = plane.RotateY,
            OriginX = middleX,
            OriginY = middleY,
            X = ax + plane.X - middleX,
            Y = ay + plane.Y - middleY,
            Z = az + plane.Z,
        };
    }

    /// <summary>The room's scenes, declared, and each posed where it stands, faded in.</summary>
    private IEnumerable<Command> BuildRoom()
    {
        var folder = System.IO.Path.Combine(AppContext.BaseDirectory, "qml", "Room");
        foreach (var plane in Planes())
        {
            yield return new Command
            {
                Surface = new Solium.Wm.Surface
                {
                    Name = plane.Name,
                    Scene = System.IO.Path.Combine(folder, plane.Scene),
                    // Over the field of stars, which is in the background
                    // layer, and under the windows.
                    Layer = Layer.Bottom,
                    On = new Placement { Rect = PlaneRect(plane) },
                    Properties = plane.Properties,
                    Reserve = new Insets(),
                    OutsideClick = new OutsideClick { Default = Outside.Pass },
                    Keyboard = KeyPolicy.ExceptClaimed,
                },
            };
            yield return Commands.Group(plane.Name, Array.Empty<ulong>(), parent: "move", surfaces: new[] { plane.Name });
            var pose = PlanePose(plane);
            yield return Commands.Carry(plane.Name, pose, Commands.Motion(0), backface: true, opacity: 0);
            yield return Commands.Carry(plane.Name, pose, Commands.Motion(600, Ease.OutCubic), backface: true);
        }
    }

    /// <summary>Where window <paramref name="at"/> hangs: two to a wall, the window wall first, then the sides, then the door's.</summary>
    private (double X, double Z, double Heading) Slot(int at)
    {
        var (w, d) = (RoomWidth(), RoomDepth());
        var slots = new[]
        {
            (-w / 4, -d / 2, 0.0),
            (w / 4, -d / 2, 0.0),
            (-w / 2, -d / 4, 90.0),
            (w / 2, -d / 4, -90.0),
            (-w / 2, d / 4, 90.0),
            (w / 2, d / 4, -90.0),
            (w / 4, d / 2, 180.0),
            (-w / 4, d / 2, 180.0),
        };
        var (x, z, heading) = slots[at % slots.Length];
        // A ninth window and on hangs a hand's width in front of the one
        // whose place it shares, and a little along.
        var over = at / slots.Length;
        var turned = heading * Math.PI / 180;
        var (inX, inZ) = (Math.Sin(turned), Math.Cos(turned));
        var (alongX, alongZ) = (Math.Cos(turned), -Math.Sin(turned));
        var nudge = Metre() * 0.3 * over;
        return (x + inX * over * 4 + alongX * nudge, z + inZ * over * 4 + alongZ * nudge, heading);
    }

    private IEnumerable<Command> ArrangeRoom(Motion motion)
    {
        var card = Card();
        foreach (var id in _windows)
        {
            yield return Commands.Place(id, card, motion);
        }
        yield return Commands.Group("eye", Array.Empty<ulong>());
        yield return Commands.Group("turn", Array.Empty<ulong>(), parent: "eye");
        yield return Commands.Group("hop", Array.Empty<ulong>(), parent: "turn");
        yield return Commands.Group("move", Array.Empty<ulong>(), parent: "hop");
        var (middleX, middleY) = (card.X + card.W / 2, card.Y + card.H / 2);
        var (ax, ay, az) = Anchor();
        for (var at = 0; at < _windows.Count; at++)
        {
            var name = $"spot-{_windows[at]}";
            var (x, z, heading) = Slot(at);
            yield return Commands.Group(name, new[] { _windows[at] }, parent: "move");
            yield return Commands.Carry(name, new Pose
            {
                RotateY = heading,
                OriginX = middleX,
                OriginY = middleY,
                X = ax + x - middleX,
                Y = ay - EyeHeight() - middleY,
                Z = az + z,
            }, motion, backface: true);
        }
        yield return Hop(0, motion);
        foreach (var command in Camera(motion))
        {
            yield return command;
        }
    }

    private IEnumerable<Command> Camera(Motion motion)
    {
        yield return Commands.Carry("move", new Pose { X = -_x, Z = -_z }, motion);
        yield return Commands.Carry("turn", new Pose
        {
            RotateY = -_yaw,
            OriginX = CenterX(),
            OriginY = CenterY(),
            OriginZ = Eye(),
        }, motion);
        yield return Commands.Carry("eye", new Pose
        {
            RotateX = _pitch,
            OriginX = CenterX(),
            OriginY = CenterY(),
            OriginZ = Eye(),
            Perspective = Eye(),
        }, motion);
    }

    /// <summary>You, lifted so far off the floor: the room taken down as far.</summary>
    private static Command Hop(double up, Motion motion) => Commands.Carry("hop", new Pose { Y = up }, motion);

    private IEnumerable<Command> RoomKey(string combo)
    {
        if (_docked is not null)
        {
            if (combo != "super+escape")
            {
                return Array.Empty<Command>();
            }
            // Step back from the window, the way you came, and walk again.
            _docked = null;
            Walk(-Metre() * 1.4, 0);
            return Camera(Commands.Motion(0, Ease.Spring(70, 16)))
                .Append(Keys())
                .Append(new Command { Grab = true });
        }
        var step = Metre() * 0.16;
        switch (combo)
        {
            case "w":
                Walk(step, 0);
                break;
            case "s":
                Walk(-step, 0);
                break;
            case "a":
                Walk(0, -step);
                break;
            case "d":
                Walk(0, step);
                break;
            case "left":
                _yaw += 3;
                break;
            case "right":
                _yaw -= 3;
                break;
            case "up":
                _pitch = Math.Min(_pitch + 2.5, 40);
                break;
            case "down":
                _pitch = Math.Max(_pitch - 2.5, -40);
                break;
            // Up as things thrown up go, slower and slower, then down.
            case "space" when !_airborne:
                _airborne = true;
                return new[]
                {
                    Hop(Metre() * 0.55, Commands.Motion(JumpMs, Ease.Bezier(1 / 3.0, 2 / 3.0, 2 / 3.0, 1))),
                    Commands.After(Fall, JumpMs),
                };
            case "return" when !_airborne:
                return StepUp();
            default:
                return Array.Empty<Command>();
        }
        return Camera(Commands.Motion(0, Ease.Spring(90, 19)));
    }

    /// <summary>
    /// Walk along your heading and across it, and no nearer a wall than
    /// half a metre.
    /// </summary>
    private void Walk(double forward, double right)
    {
        var yaw = _yaw * Math.PI / 180;
        var x = _x - Math.Sin(yaw) * forward + Math.Cos(yaw) * right;
        var z = _z - Math.Cos(yaw) * forward - Math.Sin(yaw) * right;
        var (roomX, roomZ) = (RoomWidth() / 2 - Metre() * 0.5, RoomDepth() / 2 - Metre() * 0.5);
        _x = Math.Clamp(x, -roomX, roomX);
        _z = Math.Clamp(z, -roomZ, roomZ);
    }

    /// <summary>
    /// Step up to the window most nearly ahead: you come to where it is
    /// drawn flat and at its own size, looking straight at it, then it has
    /// the keyboard.
    /// </summary>
    private IEnumerable<Command> StepUp()
    {
        var yaw = _yaw * Math.PI / 180;
        var (aheadX, aheadZ) = (-Math.Sin(yaw), -Math.Cos(yaw));
        int? best = null;
        var bestScore = double.MaxValue;
        for (var at = 0; at < _windows.Count; at++)
        {
            var (x, z, _) = Slot(at);
            var (toX, toZ) = (x - _x, z - _z);
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
        var (slotX, slotZ, heading) = Slot(chosen);
        var turned = heading * Math.PI / 180;
        // Straight out from the window by the eye's distance, facing it:
        // its heading, the near way round from where you face now.
        _x = slotX + Eye() * Math.Sin(turned);
        _z = slotZ + Eye() * Math.Cos(turned);
        _yaw += ((heading - _yaw) % 360 + 540) % 360 - 180;
        _pitch = 0;
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

    /// <summary>The size every card is in the carousel and on the room's walls: most of the screen, in its middle.</summary>
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

    /// <summary>How far in front of the screen you are in the room: a lens of about eighty degrees across.</summary>
    private double Eye() => Width() * 0.6;

    /// <summary>A metre in the room: a third of a screen across.</summary>
    private double Metre() => Width() / 3.2;

    private double RoomWidth() => Metre() * 7.2;

    private double RoomDepth() => Metre() * 8;

    private double RoomHeight() => Metre() * 2.8;

    /// <summary>How high your eye is off the floor, and the windows' middles with it.</summary>
    private double EyeHeight() => Metre() * 1.6;

    private static Motion Gentle() => Commands.Motion(0, Ease.Spring(110, 21));

    /// <summary>The cube coming back to the screen once it has turned.</summary>
    private static Motion Settle() => Commands.Motion(0, Ease.Spring(90, 19));

    private static int Mod(int value, int by) => (value % by + by) % by;
}
