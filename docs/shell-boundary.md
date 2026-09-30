# Where Solium ends and Lilium begins

Solium is the compositor. Lilium is the desktop — its bar, its dock, its
launcher. This is the line between them, and the line is not where a Wayland
tutorial would put it.

**Where it is today:** a shell is a separate program, a client of the
compositor, and its bar, dock and launcher are `wlr-layer-shell` surfaces.
Solium hosts no shell in its own process: there is no way to load a shell's
QML into the compositor, and no compatibility layer for any shell toolkit. The
QML engine inside the compositor draws the compositor's own scenes only —
window frames, the pointer, the wallpaper, loading windows, pane styles, the
tweaks panel, and whatever a script declares with `sol.surface`. The rest of
this file is the argument that got it here, including the part that argues the
other way.

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

The compositor hosts **one QML engine**. Window decorations are scenes in it.
The original design put the shell's surfaces — bar, dock, launcher — in it
too, importing the same `Solium.Theme` singleton, because there is only one of
it.

That design gave, in order of how hard they would otherwise be:

- **One design system.** `qml/Solium/Theme.qml` was read by every surface the
  desktop drew. Changing a colour there changed the titlebars and the dock
  together, with no rebuild, because they were the same object and not two
  copies.
- **Objects that travel.** An item lifted from the dock into a titlebar is a
  reparent inside one scene graph. It keeps its colours because it never left
  the design system, and it can be animated across because both ends are on the
  compositor's clock.
- **No protocol for shell geometry.** The dock's icon rectangles are not
  published to the compositor; the compositor *has* them. The genie animation
  reads a rectangle out of the same engine that drew the icon, so it cannot be
  animating against a stale copy — the failure mode a side-channel protocol
  would have made possible.

This is the arrangement Apple has, and it is unavailable to anyone configuring
an existing compositor. It is the reason for writing one.

All three held while the shell was hosted in the compositor, which it no longer
is. Today `Solium.Theme` reaches the compositor's own scenes only, and a shell,
being a client, brings its own.

## What is still a client

Ordinary applications, over `xdg-shell`.

**And, today, the bar and the dock too** — over `wlr-layer-shell`. The
in-compositor dock described below existed, proved the morph, and was then
taken back out: it made the compositor own a design, a font stack and a layout
it had no reason to, and it made the interesting question — how does a
*replaceable* shell attach? — disappear rather than get answered. `layer.rs`
records that in full.

So the boundary as it stands is: window-coupled things are scenes in the
compositor's engine, and everything that merely sits on a screen is a client
that anchors itself. The one-engine argument above is not withdrawn — it is
what the titlebars are built on, and it is what a dock would come back inside
for if the morph is ever wanted for real. It is a claim about where the line
*can* be drawn, and drawing it there is a later decision than this file
originally assumed.

### Where a dock goes, with more than one monitor

A layer surface names the output it wants, and the compositor honours it. That
is how a shell puts a bar on every screen: one surface per output, each with
its own exclusive zone, each reserving from *that* monitor's work area.

A surface that names no output gets the **primary** monitor — the one
`primary = true` picks out in `monitors`, or the first if nothing does. Not the
monitor the pointer is on, which is what it briefly was: a dock connects once,
at startup, and pinning it to whichever screen the mouse happened to be over
then means it appears somewhere different depending on where the mouse was
left. That looks like the compositor placing it at random, because it is.

`sol.monitors()` gives a script the list, with `primary` and `focused` flags,
so a shell written in Lua can decide for itself which screens get a bar.

There is no in-process alternative. A shell that wants a bar per screen writes
layer surfaces, one per output; a scene a script declares with `sol.surface`
belongs to the configuration, not to a shell.

## What this cost, honestly

This was the price of the in-process shell, which no longer exists. Hosted in
the compositor, the shell could not crash independently of it. A separate
process can be restarted; a QML error in the dock would have taken the session
with it unless the compositor was careful. So, and still for every scene the
compositor hosts:

- Every scene is loaded defensively. A scene that fails to load is skipped and
  logged; it never stops the compositor starting.
- Scene errors are contained per surface — a broken wallpaper must not take the
  window frames with it.

That was the trade made deliberately: robustness through care inside one
process, in exchange for a desktop that could do what the design asked for. A
shell is a separate process now, and gets a separate process's robustness; the
window frames and the compositor's other scenes still depend on that care.

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
crash or a SIGKILL leaves it no chance, so `solium-session` does the same once
Solium has gone, if the target is still active (`dev/install-check.sh` checks
both). Two things are left behind even so. A Solium that crashes before it has
started the target, while it waits up to five seconds for XWayland, leaves the
variables in systemd until the next session sets them again. And the D-Bus
activation environment keeps them in every case, because D-Bus has no call
that removes a variable. The next Solium session stops any target an earlier
one left running before it says anything.

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
| Bar, dock, launcher | The shell, a separate client | `wlr-layer-shell` |
| The default wallpaper, other scripted scenes | The compositor's QML engine | `sol.surface` from Lua |
| A wallpaper program (`swaybg` and the like) | Clients | `wlr-layer-shell` |
| A polkit agent, a keyring, applets and other separate programs | Clients, started by systemd or D-Bus | XDG autostart, or a unit `PartOf=graphical-session.target` |
| Which monitor a bar is on | The client names an output | `zwlr_layer_surface_v1` |
| Colours and metrics | `Solium.Theme`, one singleton | imported by every scene |
| What an animation *does* | Lua script | `sol.present_from`, `sol.on("open")` |


## What the dock proved, before it was removed

Kept because it is the evidence for the architecture, not because the code is
still there — `shell.rs` and `sol.dock` are gone, and a dock is a layer-shell
client now.

It was a QML scene rendered by the same host that draws the window frames,
importing the same `Solium.Theme`, reserving its own strip out of `work_area`.
The morph was the whole argument made concrete: pressing an icon fired a `dock`
event carrying **the rectangle that icon occupies**, a script spawned the
program and handed that rectangle to `sol.present_from`, and the window grew
out of it. Captured frame by frame in `docs/morph.png` — 460x208 near the dock,
then 928x504, 1192x672, settled at full size about a third of a second later.

`sol.present_from` is still there and still does that. What is missing is a
dock to give it a rectangle. `config.lua` carried a `dock.morph` duration for
the day one exists, and #117 removed it — nothing had ever read it, and a
setting configuring a component that does not exist cannot be told apart, from
outside, from one that is broken. `--check` reports that spelling as an
unrecognised key now, and a dock brings its settings back to that spot when it
arrives. A layer-shell dock could hand over an icon rect through a protocol, and
that is exactly the side-channel the one-engine argument says will go stale — so
if the morph is wanted for real, this is the decision to reopen rather than
route around.

None of that is reachable from a separate process. A client dock can pass a
rectangle over IPC, but by the time the window exists the two are separate
scenes and nothing holds both at once to interpolate between them. That is why
Quickshell was the wrong tool here, and it is worth saying plainly: the shell
being a client is what makes an icon and a window unable to be the same thing.
