// The desktop, as the compositor last told it.

using System.Collections.Generic;
using System.Linq;

namespace Solium.Wm;

/// <summary>
/// A window manager's own copy of the desktop: every window and monitor,
/// kept current from each batch's changes before the batch's events are
/// handled, so an event is handled with the desktop as it was when it
/// happened. It is as fresh as the last batch.
/// </summary>
public sealed class Desktop
{
    private readonly Dictionary<ulong, Window> _windows = new();
    private readonly Dictionary<string, IReadOnlyList<DocNode>> _services = new();
    private List<ulong> _stacking = new();
    private List<Monitor> _monitors = new();
    private List<Problem> _problems = new();

    /// <summary>Every window, by its id.</summary>
    public IReadOnlyDictionary<ulong, Window> Windows => _windows;

    /// <summary>The windows' ids, top to bottom.</summary>
    public IReadOnlyList<ulong> Stacking => _stacking;

    public IReadOnlyList<Monitor> Monitors => _monitors;

    /// <summary>
    /// The part of the monitor the pointer is on that bars and docks leave,
    /// once the compositor has said.
    /// </summary>
    public Rect? WorkArea { get; private set; }

    /// <summary>Where the pointer is.</summary>
    public Point Cursor { get; private set; } = new();

    public Keyboard? Keyboard { get; private set; }

    /// <summary>The window that holds the shortcuts, when one does.</summary>
    public ulong? ShortcutsOwner { get; private set; }

    /// <summary>The text field being typed into, when there is one.</summary>
    public TextField? TextField { get; private set; }

    public IReadOnlyList<Problem> Problems => _problems;

    /// <summary>Whether the session is locked.</summary>
    public bool Locked { get; private set; }

    /// <summary>Each session service's view, as a document, by its name.</summary>
    public IReadOnlyDictionary<string, IReadOnlyList<DocNode>> Services => _services;

    /// <summary>The window that has the keyboard, when one does.</summary>
    public Window? Focused => _windows.Values.FirstOrDefault(window => window.Focused);

    /// <summary>The primary monitor, or the first there is.</summary>
    public Monitor? Primary =>
        _monitors.FirstOrDefault(monitor => monitor.Primary) ?? _monitors.FirstOrDefault();

    /// <summary>Take one change.</summary>
    public void Apply(Change change)
    {
        switch (change.KindCase)
        {
            case Change.KindOneofCase.Window:
                if (!_windows.ContainsKey(change.Window.Id))
                {
                    // A new window is on top until a stacking says where.
                    _stacking.Insert(0, change.Window.Id);
                }
                _windows[change.Window.Id] = change.Window;
                break;
            case Change.KindOneofCase.WindowGone:
                _windows.Remove(change.WindowGone);
                _stacking.Remove(change.WindowGone);
                break;
            case Change.KindOneofCase.Stacking:
                _stacking = change.Stacking.Items.ToList();
                break;
            case Change.KindOneofCase.Monitors:
                _monitors = change.Monitors.Items.ToList();
                break;
            case Change.KindOneofCase.WorkArea:
                WorkArea = change.WorkArea;
                break;
            case Change.KindOneofCase.Cursor:
                Cursor = change.Cursor;
                break;
            case Change.KindOneofCase.Keyboard:
                Keyboard = change.Keyboard;
                break;
            case Change.KindOneofCase.ShortcutsOwner:
                ShortcutsOwner = change.ShortcutsOwner.HasValue ? change.ShortcutsOwner.Value : null;
                break;
            case Change.KindOneofCase.TextField:
                TextField = change.TextField.Value;
                break;
            case Change.KindOneofCase.Problems:
                _problems = change.Problems.Items.ToList();
                break;
            case Change.KindOneofCase.Locked:
                Locked = change.Locked;
                break;
            case Change.KindOneofCase.Service:
                _services[change.Service.Name] = change.Service.Document;
                break;
            // A change a newer compositor has and this package does not: no
            // part of this copy.
            case Change.KindOneofCase.None:
            default:
                break;
        }
    }
}
