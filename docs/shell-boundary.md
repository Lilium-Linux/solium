# Where Solium ends and a shell begins

Solium is the compositor. A shell — the bar, the dock, the launcher — is a
project of its own, in its own repository, and it runs **inside** Solium as its
configuration: its QML is hosted in the compositor's own engine, beside the
window frames, the pointer and every other scene the compositor draws. That is
the line, and it is not where a Wayland tutorial would put it.

**Where it is today:**

- A shell is named in the configuration, `shell = { scene = "<its root QML
  file>" }`, and hosted in-process through `sol.surface`, by `lua/shell.lua`.
  The shipped configuration names none.
- A shell written against [Quickshell](https://quickshell.outfoxxed.me/)'s QML
  API is hosted the same way. Solium's Quickshell compatibility layer answers
  its `import Quickshell` lines from inside the compositor; it is an
  API-compatible shim, not Quickshell (see [THIRD_PARTY.md](../THIRD_PARTY.md)).
- A shell that is a separate program — Waybar, a Quickshell instance run on
  its own, any `wlr-layer-shell` panel — is supported too, as an ordinary
  client. It needs nothing from the configuration and gets nothing from the
  compositor's engine.

The rest of this file is why hosting is the design, how to host one, and
exactly what a hosted shell is given and what it is not.

## The requirement that decides the architecture

> An object should be able to move from the dock into a window's titlebar.

Not "look similar in both". *Move* — one object, travelling, unbroken.

That single requirement rules out the conventional answer. If the shell is a
separate process painting its own pixels, then a dock icon and a titlebar are
two scene graphs in two processes with two stylesheets, and an object cannot
cross between them; the best available is a fake — dissolve one, appear in the
other — which is exactly the kind of thing this project exists not to build.

The same requirement, in a weaker form, rules it out again: "the shell changes
colour, so the decorations change too" is trivial when there is one theme
object, and a synchronisation problem forever when there are two.

## So: one engine

The compositor hosts **one QML engine**. Window decorations are scenes in it,
and so is a hosted shell's bar, dock and launcher. A scene that imports
`Solium` gets the same `Solium.Theme` singleton the frames read, because there
is only one of it.

That gives, in order of how hard they would otherwise be:

- **One design system.** `qml/Solium/Theme.qml` is read by every scene that
  imports it. Changing a colour there changes the titlebars and a shell that
  uses it together, with no rebuild, because they are the same object and not
  two copies. A shell brought in with a theme of its own keeps its own until
  it is pointed at this one.
- **Objects that travel.** An item lifted from the dock into a titlebar is a
  reparent inside one scene graph. It keeps its colours because it never left
  the design system, and it can be animated across because both ends are on the
  compositor's clock.
- **No protocol for shell geometry.** The dock's icon rectangles need not be
  published to the compositor; the compositor's engine *has* them. A genie
  animation can read a rectangle out of the same engine that drew the icon, so
  it cannot be animating against a stale copy — the failure mode a
  side-channel protocol would have made possible.

This is the arrangement Apple has, and it is unavailable to anyone configuring
an existing compositor. It is the reason for writing one.

## Hosting a shell

### Naming it

In `~/.config/solium/user.lua` (or your own `config.lua`):

```lua
return {
    shell = { scene = "~/.config/solium/shell/shell.qml" },
}
```

`~` is expanded. The scene is drawn once, over the windows, on the primary
monitor — the one `primary = true` marks in `monitors`, or the first — across
that monitor's usable area, and it takes the pointer there — all of it, which
[What it is not given](#what-it-is-not-given) spells out. `super+shift+r` picks up
a change to the setting. Editing a file under the scene's own directory
rebuilds the scene with no reload at all: that is checked every half second
while frames are being drawn, and a staged copy (below) has to be staged again
first. `false`, the default, hosts nothing, and taking the setting out takes
the shell away on the next reload.

`SOLIUM_SHELL_SCENE=<file>` overrides the setting for one run. It is how
`dev/run-shell.sh` hosts a shell under development without touching the
configuration you normally run; see `dev/README.md`.

### Installing one next to your configuration

A shell is its own repository. Clone it beside your configuration and name its
root file:

```sh
git clone <the shell's repository> ~/.config/solium/shell
```

```lua
shell = { scene = "~/.config/solium/shell/shell.qml" },
```

A shell whose files import one another as `qs.<directory>` — the convention
Quickshell sets, by writing a `qmldir` for every directory at load time — also
needs those modules staged where Solium's engine looks. The script that does
it is not installed with Solium yet, so for now this needs a Solium checkout;
from one:

```sh
dev/stage-shell.sh ~/.config/solium/shell ~/.config/solium/qml
```

That writes `~/.config/solium/qml/qs/`, which is on the QML search path
already, and replaces only that directory. Run it again after updating the
shell. A shell that imports its own files by relative path needs no staging.

`solium --check-qml <file>` loads one file without starting a compositor and
prints `ok` or what Qt reported, which is the quick way through a chain of
"type X unavailable" errors. It exits 0 either way.

## What a hosted shell is given

**A canvas.** One scene, sized to the primary monitor's usable area, on the
`top` layer: over the windows, under a layer-shell client's own `top` layer
surfaces, and covered by a fullscreen window unless `fullscreen.covers` says
otherwise. It gets pointer motion and presses, so a `MouseArea` works. It is
given `screenInfo` as an initial property, which a scene reads by declaring
`property var screenInfo`: `name`, `x`, `y`, `width`, `height` and `scale`.

**The compositor's clock and frames.** Its animations advance on the same
clock as every window transform, a running animation asks for the next frame,
and the scene is redrawn only when Qt says it changed. Qt is served between
frames too: a `Timer` fires on time on an idle desktop, and what Qt waits on a
descriptor for — a `Process`'s output, a socket, an answer from another
thread — arrives when it is ready, with no frame drawn unless the scene
changed. A `Timer` is on that same clock, so one beside an animation nothing
draws still fires.

**The `Solium` QML module.** `Solium.Theme` above all: the colours, fonts and
metrics the frames are drawn with, overridable by one file in
`~/.config/solium/qml/Solium/`.

**A way back to the configuration.** A scene sets a string property named
`action`, the compositor takes it, and `sol.on("surface", function(name,
action) ... end)` in Lua is told. Anything Lua can do — `sol.spawn`, switching
workspaces, any binding — a hosted shell can ask for this way.

**The Quickshell compatibility layer**, on the QML search path after the
compositor's own modules, so it can never shadow `Solium.*`:

| Module | What answers |
|---|---|
| `Quickshell` | `Singleton`, `Scope`, `Variants` (an `Instantiator`), `SystemClock`; the `Quickshell` singleton's `execDetached`, `iconPath`, `env`, `processId`, `appId`, `configDir` and `shellDir`; and the `Quickshell.Io` types again |
| `Quickshell.Io` | `Process`, `StdioCollector`, `SplitParser`, `FileView`, `Socket` — real, in C++ |
| `Quickshell.Wayland` | `ToplevelManager.toplevels`, filled from the compositor's own window list; `PanelWindow` (and `WlrLayershell`) as a window whose content item the compositor draws across the scene; `Toplevel`'s shape |
| `Quickshell.Hyprland` | `Hyprland.toplevels` and `Hyprland.activeToplevel`, from the same list; `HyprlandToplevel`'s shape |
| `Quickshell.Widgets` | `IconImage`, `ClippingRectangle`, `WrapperItem` |
| `Quickshell.Services.Pipewire`, `.Notifications`, `.Mpris`, `Quickshell.Networking`, `Quickshell.Bluetooth` | The types, with empty data: enough for a shell to load, not to work |

The window list carries each window's `title`, `appId` and whether it is
`activated`, and is built only once some scene has read it. Icons are served
at `image://theme/<name>` from the icon theme, and `iconPath` answers with
such a URL; a name that is a path, or has a `/` in it, gets the generic
application icon rather than that file (#145).

## What it is not given

Said plainly, because a shim that loads is easy to mistake for one that works:

- **No input region: the scene takes every press and hover on its monitor.**
  The pointer is claimed anywhere inside the scene's area, which is the whole
  usable area of the primary monitor, not only where the scene draws — a
  36-pixel bar claims the screen under it too. So while a shell is hosted, the
  windows on that monitor cannot be clicked, focused with the pointer or
  dragged with `super`. What it wants is an input region: a point claimed only
  where the scene has an item under it, or a surface sized from
  `PanelWindow`'s anchors and `implicitHeight`.
- **One monitor.** One scene, on the primary monitor. A hosted shell on every
  screen is a later design step, and `Quickshell.screens` is empty.
- **No reserved space, and no placement.** `PanelWindow`'s anchors and
  `exclusiveZone` are accepted and ignored: its content fills the scene, and a
  hosted bar does not take its strip out of the work area, so windows are
  placed under it. The attached `WlrLayershell.layer`, `.namespace` and
  `.keyboardFocus` are not there at all. Its `width` and `height` are
  read-only, so `PanelWindow { height: 30 }` fails to load, and there is no
  `margins` group, so `margins { top: 4 }` fails too; either stops the whole
  shell loading.
- **No keyboard, and every button is the left one.** Pointer motion and
  presses — no keyboard focus, no grabs (#85), no wheel, and a right or middle
  press arrives as a left press, so a right-click on a hosted button activates
  it. A launcher's text field cannot be typed into.
- **Windows can be read, not driven.** The list's entries are plain records,
  with no `activate()` or `close()`; `Hyprland.dispatch` logs that it has no
  mapping; `Hyprland.monitors` and `workspaces` are empty. Driving the
  compositor goes through `action` and Lua.
- **Programs it starts are not Solium's clients.** `Process` and
  `execDetached` start children with the compositor's own environment, which
  does not point `WAYLAND_DISPLAY` at Solium the way `sol.spawn` does. And a
  `Process` declared with `running: true` never starts, whatever its
  `command`: `running` is acted on the moment it is set, before the command is,
  where Quickshell waits for the component to complete. Set `running` from
  `Component.onCompleted` instead.
- **Services are shapes, not services.** PipeWire, notifications, MPRIS,
  networking and Bluetooth report nothing, and there is no `Quickshell.
  Services.UPower`, `SystemTray` or `Pam` at all.
- **No Quickshell windows of other kinds.** There is no `ShellRoot`,
  `FloatingWindow`, `PopupWindow` or `WlSessionLock`; a lock screen is a
  separate `ext-session-lock` client.
- **`configDir` is not the shell's directory.** `Quickshell.configDir` and
  `shellDir` answer `SOLIUM_SHELL_DIR`, or `~/.config/solium` when it is unset,
  where Quickshell would answer the shell's own directory.

## What is still a client

Ordinary applications, over `xdg-shell`.

**And any shell that is a program of its own**, over `wlr-layer-shell`:
Waybar, a wallpaper program, a Quickshell instance started as a process, a
lock screen. They are started like any other program and need nothing from
the configuration. Their `top` layer is drawn over a hosted shell, and the
compositor reserves their exclusive zones from the work area.

### Where a client's bar goes, with more than one monitor

A layer surface names the output it wants, and the compositor honours it. That
is how a client shell puts a bar on every screen: one surface per output, each
with its own exclusive zone, each reserving from *that* monitor's work area.

A surface that names no output gets the **primary** monitor — the one
`primary = true` picks out in `monitors`, or the first if nothing does. Not the
monitor the pointer is on, which is what it briefly was: a dock connects once,
at startup, and pinning it to whichever screen the mouse happened to be over
then means it appears somewhere different depending on where the mouse was
left. That looks like the compositor placing it at random, because it is.

`sol.monitors()` gives a script the list, with `primary` and `focused` flags,
so a shell written in Lua can decide for itself which screens get a bar.

## What hosting costs, honestly

A hosted shell cannot crash independently of the compositor. A separate
process can be restarted; a QML error in the dock takes the session with it
unless the compositor is careful. So:

- Every scene is loaded defensively. A scene that fails to load is skipped and
  logged; it never stops the compositor starting.
- Scene errors are contained per surface — a broken dock must not take the
  window frames with it.
- An edit that does not parse leaves the last scene drawing.

That is the trade being made deliberately: robustness through care inside one
process, in exchange for a desktop that can actually do what the design asks
for. A shell that would rather have a separate process's robustness can have
it, as a client, and gives up the one engine to get it.

## Summary

| | Where it runs | How it reaches the compositor |
|---|---|---|
| Applications | Clients | `xdg-shell` |
| Window frames | Compositor's QML engine | directly |
| Loading windows, the pointer | The same QML engine | directly |
| The wallpaper, other scripted scenes | The same QML engine | `sol.surface` from Lua |
| A hosted shell: bar, dock, launcher | The same QML engine | `shell.scene`, through `sol.surface` |
| A client shell (Waybar and the like), wallpaper programs | Clients | `wlr-layer-shell` |
| Which monitor a bar is on | Hosted: the primary. A client: the output it names | `zwlr_layer_surface_v1` |
| Colours and metrics | `Solium.Theme`, one singleton | imported by every scene that wants it |
| What an animation *does* | Lua script | `sol.present_from`, `sol.on("open")` |

## History: the dock that proved it

The first in-compositor dock was compositor code — `shell.rs` and `sol.dock` —
and it was taken back out: it made the compositor own a design, a font stack
and a layout it had no reason to, and it hid the question of how a
*replaceable* shell attaches instead of answering it. `layer.rs` records that
in full. The answer is the one above: the shell is configuration, and the
compositor hosts whatever the configuration names.

The dock was a QML scene rendered by the same host that draws the window
frames, importing the same `Solium.Theme`, reserving its own strip out of
`work_area`. The morph was the whole argument made concrete: pressing an icon
fired a `dock` event carrying **the rectangle that icon occupies**, a script
spawned the program and handed that rectangle to `sol.present_from`, and the
window grew out of it. Captured frame by frame in `docs/morph.png` — 460x208
near the dock, then 928x504, 1192x672, settled at full size about a third of a
second later.

`sol.present_from` is still there and still does that. What is missing is a
dock to give it a rectangle. `config.lua` carried a `dock.morph` duration for
the day one exists, and #117 removed it — nothing had ever read it, and a
setting configuring a component that does not exist cannot be told apart, from
outside, from one that is broken. `--check` reports that spelling as an
unrecognised key now, and a dock brings its settings back to that spot when it
arrives.

None of that is reachable from a separate process. A client dock can pass a
rectangle over IPC, but by the time the window exists the two are separate
scenes and nothing holds both at once to interpolate between them. That is why
a shell written for Quickshell is hosted here rather than run beside the
compositor under Quickshell itself: hosted, its icons and the windows are in
one engine, and the morph is a reparent rather than a fake.
