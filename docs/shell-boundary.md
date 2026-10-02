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
- A shell is QML written against Solium's own API: `import Solium` for
  `Theme`, and an `action` property for what it asks Lua to do
  ([below](#what-a-hosted-shell-is-given)). Quickshell support was removed
  (#172); Quickshell itself may add Solium support on its own side.
- A shell that runs as its own program — Waybar, a Quickshell instance run
  on its own, any `wlr-layer-shell` panel — is supported too, as an ordinary
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

- **One design system.** `Solium.Theme`
  (`crates/solium/qml/Solium/Theme.qml`) is what the window frames and the
  loading window are drawn with, and any hosted scene that imports `Solium`
  can read it. Changing a colour there changes the titlebars and a shell that
  uses it together, with no rebuild, because they read the same object and
  not two copies. A shell brought in with a theme of its own keeps its own
  until it is pointed at this one. Two of the compositor's own scenes keep
  fixed colours instead: the fallback pointer, which has to stay legible over
  whatever a client drew, and the default wallpaper.
- **Objects that travel, planned.** The aim is that an item can leave the
  dock and land in a titlebar, one object the whole way. None of it is built,
  and it will not be a reparent: every scene has a `QQuickWindow` of its own,
  so each is its own scene graph, and an item cannot move from one to
  another. The plan is a flight or a morph: a third, live instance of the same
  component, drawn over both ends while they are hidden and moved on the
  compositor's clock. One engine is what lets that instance be the component
  itself, with the same theme, rather than a picture of it.
- **No protocol for shell geometry, planned.** A hosted dock's icon
  rectangles need not be published to the compositor, because the
  compositor's engine already has them. The plan is for a scene to name an
  item as an anchor (`Solium.region`) and for an animation to aim at that
  name, read again on every frame, so a genie follows an icon while it moves
  and can never animate against a stale copy. Nothing of that is built yet,
  and no issue tracks it. Today a genie aims at a window, a `sol.surface`
  scene or a fixed rectangle ([modes.md](modes.md)). Only a dock that runs as
  its own program would need a protocol to hand its rectangles over.

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

`~` is expanded. The scene is drawn on every monitor, one instance each, over
the windows and across the whole monitor; each instance reads its own monitor
as `Solium.monitor`. `shell = { on = "primary" }`, or a connector name, draws
one instance instead
(`script::tests::the_shell_is_on_every_monitor_unless_the_configuration_names_one`).
It takes the pointer there — all of it, which
[What it is not given](#what-it-is-not-given) spells out. `super+shift+r` picks up
a change to the setting, and tries again a scene that would not load, so a
typo in the shell costs a reload rather than the session
(`state::tests::real_client::a_reload_tries_again_a_scene_that_would_not_load`).
Editing a file under the scene's own directory
rebuilds the scene with no reload at all: that is checked every half second
while frames are being drawn.
`false`, the default, hosts nothing, and taking the setting out takes
the shell away on the next reload.

A reload does not rebuild a hosted scene. `sol.surface` declared again with the
same scene file writes the properties that changed into the live scene, so an
open popup, a running animation or a half-typed query survives
`super+shift+r`; only a different scene file builds the scene again
(`scripted::tests::a_redeclared_property_is_written_into_the_live_scene`).

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

A shell that imports its own files by relative path needs nothing more.

`solium --check-qml <file>` loads one file without starting a compositor and
prints `ok` or what Qt reported, which is the quick way through a chain of
"type X unavailable" errors. It exits 0 either way.

## What a hosted shell is given

**A canvas.** One scene per monitor, each the size of its whole monitor, on
the `top` layer: over the windows, under a layer-shell client's own `top` layer
surfaces, and covered by a fullscreen window unless `fullscreen.covers` says
otherwise
(`scripted::tests::a_surface_on_every_monitor_has_one_live_scene_per_monitor`).
It gets pointer motion, every mouse button as itself, the wheel, and the
modifiers held, so a `MouseArea` or a `WheelHandler` works as it does anywhere
(`qml::hosted::tests::a_right_press_reaches_a_mouse_area_as_the_right_button`,
`qml::hosted::tests::the_wheel_reaches_a_wheel_handler_with_its_angle`).
Each event carries its time, so a double press is a double-click and a
`TapHandler` counts its taps, by Qt's own double-click interval and distance
(`qml::hosted::tests::a_double_press_on_a_mouse_area_is_one_double_click`,
`qml::hosted::tests::a_tap_handler_counts_taps_by_when_they_happened`,
`qml::hosted::tests::two_presses_further_apart_than_the_interval_are_two_single_clicks`).
The wheel with `super` held stays the compositor's
(`state::tests::real_client::reflow_on_close::hosted::super_and_the_wheel_stay_the_compositors_over_a_scene`),
and while the session is locked none of it reaches the scene
(`state::tests::real_client::lock_focus::the_wheel_over_a_hosted_scene_is_not_the_scenes_while_locked`,
`state::tests::real_client::lock_focus::a_button_let_go_behind_the_lock_is_not_held_after_it`).
A press the scene held when the session locked is cancelled, not clicked: a
`MouseArea`, a `Button` or a `TapHandler` holding it hears `canceled`
(`state::tests::real_client::lock_focus::the_lock_lets_go_of_a_hosted_scenes_press_and_its_hover`,
`qml::hosted::tests::a_press_let_go_of_unseen_is_cancelled_not_clicked`).
A monitor that arrives gets its instance there and then, and one that goes
takes its instance with it
(`scripted::tests::an_instance_goes_with_its_monitor_and_comes_with_a_new_one`).
It reads where it is from `Solium.monitor`, below; `screenInfo` is gone.

**Clickable only where it takes input.** The compositor asks the live item
tree under the pointer, so a point is the scene's only where a visible,
enabled item that is not fully transparent takes input: a `MouseArea`, a
pointer handler, a link in a `Text`, or an item marked `Solium.input: true`.
An item at opacity 0, itself or through an ancestor, takes nothing, though Qt
would still deliver to it
(`qml::hosted::tests::the_item_tree_decides_what_a_point_claims`). The rest of
a `Text` takes no press, a plain
label none at all
(`qml::hosted::tests::a_text_takes_a_press_only_on_a_link`).
`Solium.input: "hover"` takes the pointer's motion and
leaves presses to what is under it, which is how an edge strip reveals a
hidden dock; `Solium.input: false` takes an item out. An item's shape counts,
through its `containmentMask`, so a rounded popup's corners pass clicks
through; a mask written in QML has to be typed,
`function contains(point: point): bool`, or Qt ignores it, and it answers for
the whole item, its bounds too. A disabled handler takes nothing
(`qml::hosted::tests::a_disabled_handler_claims_nothing`), and an open Qt
Quick Controls popup, a `Popup`, a `Menu` or a `ComboBox`'s list, takes the
points it is drawn on, though Qt draws it in the window's overlay rather than
under the scene's root
(`qml::hosted::tests::an_open_controls_popup_claims_its_press`), and
`Solium.input` written on the `Popup` is that item's
(`qml::hosted::tests::solium_input_on_a_controls_popup_is_its_items`). A
popup is laid out against the whole scene, so a `Menu` opens where it is
asked to
(`qml::hosted::tests::a_controls_popup_is_laid_out_against_the_whole_scene`),
and a modal one takes every point of its scene while it is open, as Qt's own
modality does
(`qml::hosted::tests::a_modal_popup_takes_every_point_of_its_scene_while_it_is_open`).
Everywhere else the window under the shell gets the press and the wheel
(`state::tests::real_client::reflow_on_close::hosted::the_wheel_where_the_shell_draws_nothing_is_the_windows_under_it`),
and where the scene takes a press, the window under it does not have the
pointer, nor the keyboard when focus follows the mouse. A press the scene
took is its until every button is up, wherever the pointer goes meanwhile,
and the pointer keeps the shape it had at the press
(`state::tests::real_client::reflow_on_close::hosted::a_press_where_the_shell_draws_nothing_reaches_the_window_under_it`,
`state::tests::real_client::reflow_on_close::hosted::a_release_after_dragging_off_a_shell_button_reaches_the_scene`,
`state::tests::real_client::reflow_on_close::hosted::a_press_a_scene_holds_keeps_its_shape_over_a_resize_border`,
`state::tests::real_client::reflow_on_close::hosted::focus_follows_the_mouse_through_a_shell_only_where_it_takes_no_press`,
`qml::hosted::tests::the_item_tree_decides_what_a_point_claims`).

**The compositor's clock and frames.** Its animations advance on the same
clock as every window transform, a running animation asks for the next frame,
and the scene is redrawn only when Qt says it changed. Qt is served between
frames too: a `Timer` fires on time on an idle desktop, and what Qt waits on a
descriptor for — a socket, an answer from another thread — arrives when it
is ready, with no frame drawn unless the scene changed. A `Timer` is on that
same clock, so one beside an animation nothing draws still fires.

**The `Solium` QML module.** `Theme` above all: the colours, fonts and
metrics the frames are drawn with. Beside it, `import Solium` brings the
attached `Solium` object, which any item can read: `Solium.monitor` is the
monitor this instance of the scene is on
(`qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`).
The module's types are written unqualified, as `Theme` is. A `Theme.qml` of
your own in `~/.config/solium/qml/Solium/` is meant to override it, and does
not yet: the shipped module is found first
([#88](https://github.com/Lilium-Linux/solium/issues/88)).

**Its monitor, live.** `Solium.monitor` is the row of the monitor this
instance is on: `name`, `whole` and `area` (rectangles in the global space,
`area` being the work area), `scale`, `transform`, `primary`, and `present`
(`valid` reads the same). It changes in place, once per frame and all at once,
when the monitor does, and a monitor that goes reads `present: false` and
keeps its name; the same monitor coming back is the same row again
(`qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`).
Until the compositor publishes a monitor, its row carries only the `name`, and
`present` and `valid` read false
(`qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`).
A scene that wants its own coordinates subtracts `whole.x` and `whole.y`.
`transform` is spelled as `sol.monitors()` spells it: `"normal"`, `"_90"`,
`"_180"`, `"_270"`, `"flipped"`, `"flipped90"`, `"flipped180"` or
`"flipped270"`, and not `"90"` or `"flipped-90"` as `sol.monitors{ ... }` takes
it (`models::monitors::tests::a_turned_monitor_row_names_its_transform_as_smithay_does`).

**A way back to the configuration.** A scene sets a string property named
`action`, the compositor takes it, and `sol.on("surface", function(name,
action) ... end)` in Lua is told. Anything Lua can do — `sol.spawn`, switching
workspaces, any binding — a hosted shell can ask for this way.

## What it is not given

Said plainly, because a shell that loads is easy to mistake for one that works:

- **No reserved space, and no placement.** The scene fills its whole
  monitor, and a hosted bar does not take its strip out of the work area, so
  windows are placed under it.
- **No keyboard.** No keyboard focus and no grabs (#85): a launcher's text
  field cannot be typed into.
- **No window list, and no icons.** Nothing tells a hosted scene which
  windows exist, and there is no `image://` provider for the icon theme.
  Driving the compositor goes through `action` and Lua.

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

## How the rest of the desktop starts

Whatever Solium loads itself starts with it and needs nothing below: its own
QML scenes, and everything `init.lua` declares. A shell that Solium loads
through its configuration is one of those, and needs none of this either.
Every other program of the desktop is a separate process, and on a systemd
machine it is started by the user's systemd, or by D-Bus when something first
asks for it. That covers a polkit authentication agent, a keyring, applets
such as `nm-applet`, a bar that is a program of its own, and the portals.

So Solium tells both where it is (#146), when it is started as the session.
The session file starts `solium-session`, which runs `solium --tty --session`.
Once its Wayland socket is up, Solium sends `WAYLAND_DISPLAY`,
`XDG_CURRENT_DESKTOP` (`Lilium`, unless the session file's `DesktopNames` set
it already) and `XDG_SESSION_TYPE=wayland` to systemd's user manager
(`SetEnvironment`) and to D-Bus activation (`UpdateActivationEnvironment`). It
sends them again with `DISPLAY` once XWayland has one. Then it starts
`solium-session.target`, which starts `graphical-session.target`, and
`solium-autostart.target`, which starts `xdg-desktop-autostart.target`.
`session.rs` has the tests, and
`the_calls_reach_systemd_and_dbus_as_their_methods` checks the calls on the
wire.

**On the way out** Solium stops `solium-session.target`, which takes the
autostart target with it, and unsets the variables in systemd, so a Plasma
login afterwards does not inherit a dead socket. It does so on a clean exit,
and on SIGTERM (how logind ends a session), SIGINT or SIGHUP (`signals.rs`). A
Solium that cannot stop, because something inside it has stopped answering,
ends itself `session.stop_timeout` (five seconds) after the signal, or at once
when the same signal comes again half a second or more later. Neither that nor
a crash gives Solium the chance to clean up, so `solium-session` does it once
Solium has gone, if the target is still active. If Solium failed before it
started the target, while it waited up to five seconds for XWayland,
`solium-session` unsets the variables, as long as no other desktop holds
`graphical-session.target` (`dev/install-check.sh` checks each case).

Two things are left behind even so. A SIGKILL of the whole session, which
systemd sends to whatever of it is still running once its stop timeout has
passed (45 seconds on Fedora 44), takes `solium-session` with it; the next
Solium session stops any target an earlier one left running before it says
anything. And under `dbus-daemon`, the D-Bus activation environment keeps the
variables, because D-Bus has no call that removes one. Fedora's `dbus-broker`
hands `UpdateActivationEnvironment` on to systemd's `SetEnvironment` (its
launcher names that method), so there the unset should clear them for D-Bus
activation too; that has still to be seen on a real session.

One Solium session runs per user at a time: `solium-session` holds a lock for
as long as its session runs, and refuses to start a second, which would take
the first one's target for a leftover and stop it.

**When Solium is not the session, it tells nobody.** `solium --tty` started by
hand, from a text console, leaves systemd and D-Bus alone: a desktop on
another VT shares them, and its display is not Solium's to replace. A nested
Solium leaves the session it runs in alone for the same reason. And a Solium
session that finds `graphical-session.target` already held by another desktop
of the same user leaves systemd and D-Bus to that desktop, and says so in the
log.

`session.systemd = false` turns all of it off. `session.autostart = false`
leaves out `solium-autostart.target`, and so XDG autostart, while
`solium-session.target` and every unit attached to it still start.
`dev/install.sh` puts both targets in `~/.config/systemd/user` and
`lilium-portals.conf` in `~/.config/xdg-desktop-portal`.

A program that should start with a Solium session can do it in one of two
ways.

**XDG autostart.** Put a `.desktop` file in `~/.config/autostart`, or in
`/etc/xdg/autostart` for every user. systemd's autostart generator turns each
into a service when `xdg-desktop-autostart.target` starts. It skips any whose
`OnlyShowIn=` does not name `Lilium`, whose `NotShowIn=` does, or that says
`X-systemd-skip=true`:

```ini
[Desktop Entry]
Type=Application
Name=Network applet
Exec=nm-applet --indicator
OnlyShowIn=Lilium;
```

**A systemd user unit.** It is tied to the graphical session, so it stops
when Solium does, and attached to `solium-session.target`, so it starts
whether autostart is on or not:

```ini
[Unit]
Description=A bar
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=/usr/bin/waybar
Restart=on-failure

[Install]
WantedBy=solium-session.target
```

Save it as `~/.config/systemd/user/bar.service` and enable it with
`systemctl --user enable bar.service`. `WantedBy=graphical-session.target`
also starts it under any other desktop that runs a systemd session, such as
Plasma or GNOME. `solium-session.target` starts it only under Solium.

**Polkit agents and keyrings start the same way, and most say which desktops
they are for.** Fedora's KDE polkit agent, for instance, ships
`/etc/xdg/autostart/polkit-kde-authentication-agent-1.desktop` with
`OnlyShowIn=KDE;` and `X-systemd-skip=true`, so autostart skips it under
Lilium, and a copy of it would be skipped too. It also ships
`plasma-polkit-agent.service`, which is already
`PartOf=graphical-session.target`, so a single command starts it with every
Solium session, autostart on or off:

```sh
systemctl --user add-wants solium-session.target plasma-polkit-agent.service
```

A keyring that D-Bus starts on demand, such as KWallet's `kwalletd6`, needs no
entry: once the environment is exported, the first program that asks for a
secret starts it. It is not unlocked with the login password, though. Under
Plasma, `plasma-kwallet-pam.service` runs `/usr/libexec/pam_kwallet_init`,
which hands the wallet what PAM kept at login, through the socket named by
`PAM_KWALLET5_LOGIN`. Solium does not export that variable to systemd, and the
script does nothing without it, so the wallet asks for its password the first
time it opens.

## Summary

| | Where it runs | How it reaches the compositor |
|---|---|---|
| Applications | Clients | `xdg-shell` |
| Window frames | Compositor's QML engine | directly |
| Loading windows, the pointer | The same QML engine | directly |
| The wallpaper, other scripted scenes | The same QML engine | `sol.surface` from Lua |
| A hosted shell: bar, dock, launcher | The same QML engine | `shell.scene`, through `sol.surface` |
| A client shell (Waybar and the like), wallpaper programs | Clients | `wlr-layer-shell` |
| A polkit agent, a keyring, applets and other separate programs | Clients, started by systemd or D-Bus | XDG autostart, or a unit `PartOf=graphical-session.target` |
| Which monitor a bar is on | Hosted: every monitor, or the one `shell.on` names. A client: the output it names | the output argument of `zwlr_layer_shell_v1.get_layer_surface` |
| Colours and metrics | `Solium.Theme`, one singleton | imported by every scene that wants it |
| What an animation *does* | Lua script | `sol.present_from`, `sol.on("open")` |

## History: the dock that proved it

The first in-compositor dock was compositor code — `shell.rs` and `sol.dock` —
and it was taken back out, as the compiled-in bar before it had been: it made
the compositor own a design, a font stack and a layout it had no reason to,
and it hid the question of how a *replaceable* shell attaches instead of
answering it. `layer.rs` records that for the bar, and `surface.rs` for the
dock. The answer is the one above: the shell is configuration, and the
compositor hosts whatever the configuration names.

The dock was a QML scene rendered by the same host that draws the window
frames, importing the same `Solium.Theme`, reserving its own strip out of
`work_area`. The morph was the whole argument made concrete: pressing an icon
fired a `dock` event carrying **the rectangle that icon occupies**, a script
spawned the program and handed that rectangle to `sol.present_from`, and the
window grew out of it. Captured frame by frame in `docs/morph.png`, a record
of 2026-09-06 and of a dock that no longer exists — 460x208 near the dock, then
928x504, 1192x672, settled at full size about a third of a second later.

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
a shell is hosted here rather than run beside the compositor as a program of
its own: hosted, its icons are in the same engine as the window frames, so the
flight planned above can be a live instance of the icon's own component rather
than a fake.
