<img src="docs/brand/logotype.svg" width="132" alt="Solium">

# Solium

Solium is the Wayland compositor of [Lilium DE](https://github.com/Lilium-Linux),
written in Rust on [Smithay](https://github.com/Smithay/smithay) and configured
in Lua. Its window frames, its wallpaper and the desktop shell it hosts are QML
running inside the compositor, in one engine, so the desktop can be one design
that you change without rebuilding anything.

![Two windows on the Solium wallpaper: one in the compositor's own QML titlebar, focused, and one that draws its own](docs/desktop.png)

Captured by the compositor reading back its own framebuffer, in a nested
session with the shipped configuration and nothing else, under the light theme
Solium shipped until 2026-10-04 and not retaken yet: the shipped theme is now
dark grey. The window on the left wears the default frame, a QML scene
rendered in-process; the one on the right asked to draw its own and was let.
The wallpaper is a QML file too.
[docs/modes.md](docs/modes.md) has every mode, frame by frame.

## Status

**Alpha, and not yet anyone's daily desktop.** The first release, v0.1.0,
will be a preview release, and it is not cut until the desktop is
daily-drivable (the `daily-drive` label), the native preview shell is written
(a bar at the bottom, a dock at the top, quick search, desktop icons, widgets
with real data and a native lock screen), and there are packages: the point
where it is ready for an open beta. [docs/beta.md](docs/beta.md) is that road.
It is being readied for daily use, and the daily-driving trial that decides
whether it is ready has not happened yet. What stands in the way is the
[`daily-drive` label][daily-drive], and the honest reasons are specific:

- A hosted shell is not yet a whole desktop ([#169]): it sees no windows or
  workspaces ([#166]).
- Suspend and resume have never been tested ([#64]), and a session on the
  hardware sometimes starts with no input devices and stops itself ([#48]).
- Nothing has run unattended for hours on the hardware ([#65]).
- There is no explicit sync, so Vulkan and NVIDIA clients can stutter ([#59]).
- There are no touchpad settings, so no tap-to-click ([#157]); no volume,
  brightness, media or screenshot keys by default ([#151]); and logind's lock
  and sleep requests are ignored ([#153]).
- There are no packages in a repository: a Fedora 44 package, or an install,
  is built from a checkout ([#66]).
- Two tiling bugs found in use are fixed, and stay open until real use bears
  that out: a window drawing past its tile over its neighbours ([#133]), and a
  closing window keeping its tile until its client had gone ([#128]).

Short of that trial, it has been checked on the hardware at commit `dfc95ce`,
on a desktop with an NVIDIA RTX 3070 and on a Microsoft Surface Pro 7 (Intel
Ice Lake integrated graphics), both on Fedora 44. On both, QML renders on the
GPU and its animations run, the screens go off when idle, `swaylock` locks,
and the Caps Lock and layout pill works with the shipped configuration. On the
Surface Pro 7, closing the lid turned the screen off, though Solium has no lid
handling of its own. Touch reaches applications' windows, but nothing Solium
draws itself reacts to touch yet ([#181]).

And a bug in a compositor takes the session with it. The ones that get found
are the ones real use finds, which is why the trial matters more than the test
suite.

## What works today

- **Layouts.** Dwindle tiling, scrolling, floating and overview, each running
  per monitor, with a workspace per monitor or one for the whole desk. A
  minimum tile, with a new window overflowing to the next empty workspace
  ([#134]); applications' own minimum and maximum sizes ([#115]); focus and
  move by direction, and floating and fullscreen toggles ([#150]); and a reload
  that keeps every layout as it was ([#116], [#118]).
- **Windows.** A window exists from the keypress that launches it, and shows a
  loading window until its application has drawn; an X11 application's window
  does not yet replace its loading window ([#180]). Relaunching a
  single-instance application that is already running brings its existing
  window forward where it is, instead of moving it into a new slot ([#177]).
  Closing and quitting fade out ([#126], [#127]); a resize follows the drag
  ([#113]); a modal dialog floats over its parent ([#72]); a drag shows its
  icon ([#57]); and one stacking order serves drawing and clicks, with a
  fullscreen window covering the bars ([#141], [#142]) and getting its place
  back after ([#92]). X11 clients work through XWayland, and menus and
  tooltips are placed, grabbed and kept on screen.
- **Chrome.** Eleven pane styles and per-corner rounding, QML on the GPU by
  default with a software fallback ([#147]), and cursor themes and the shapes
  clients ask for ([#81], [#24]).
- **Shells.** `shell = { scene = … }` hosts one in the compositor's own QML
  engine, one instance on every monitor (or on the one `shell = { on = … }`
  names), each reading its own monitor live as `Solium.monitor` ([#161]). It
  takes the pointer only where its items take input, so the windows under it
  can still be clicked ([#173]). It gets every mouse button, the wheel and the
  modifiers, a popup holds the pointer with `Grab`, and an item that asks with
  `Solium.keyboard` gets the keyboard ([#163]). A bar reserves its edge,
  whatever the scene's size, with `reserve` on `sol.surface` or
  `Solium.surface.reserve` from QML ([#162]). Any layer-shell client works
  too; bars and lockers both get the frame callbacks they rely on ([#149]),
  and a hosted scene's timers fire on an idle desktop ([#164]).
- **Monitors.** Several at once, each at its own refresh rate, arranged from
  the configuration or guessed; plugged in and unplugged while the session runs
  ([#43]); scaled, worked out from the panel or set. Screens go dark after ten
  minutes with nobody at the machine, and `wlopm` and `swayidle` can turn them
  off too ([#54]); the ten-minute screen-off has been seen working on both
  machines named above.
- **Input.** Keyboard layouts, with bindings that keep working under a
  non-Latin one ([#132]) and on shifted keys ([#121]). Escape reaches
  applications: the overview binds it only while it is open ([#174]). A small
  pill near the text field's caret says Caps Lock is on, or which layout you
  just switched to; it is written in Lua and QML as configuration rather than
  in the compositor, and set with `keyboard.indicator` ([#178]). On touch
  screens, touch reaches applications' windows and a tap focuses the window
  under it, but nothing Solium draws itself answers touch yet ([#181]).
- **The lock and idle.** The lock fails safe: the session is locked before the
  locker has drawn, and stays locked if the locker crashes. An idle inhibitor
  counts only while its window is on screen, and a browser's D-Bus inhibitor
  holds the screens on too ([#152]).
- **The session.** Started from the login screen, Solium tells systemd and
  D-Bus where its display is and starts `graphical-session.target` and XDG
  autostart, and stops them when it ends ([#146]). Programs Solium starts get
  the environment Solium itself was started with, not the settings Qt has
  written into it since, and inherit no descriptor beyond stdio ([#175]).
- **Protocols.** `xdg-shell`, `xdg-decoration`, `xdg-output`,
  `xdg-activation`, `xdg-dialog`, `wlr-layer-shell`, `wlr-screencopy`,
  `wlr-output-power-management`, `ext-session-lock`, `ext-idle-notify`,
  `idle-inhibit`, `cursor-shape`, `wp-viewporter`, `wp-fractional-scale`,
  `wp-presentation`, `wp-single-pixel-buffer`, `linux-dmabuf`,
  `relative-pointer`, `pointer-constraints`, `primary-selection`,
  `text-input-v3` and `xwayland-shell`. Both selections, copy and paste, cross
  the X11 boundary. `text-input-v3` tells the compositor where the focused
  text field's caret is; with no input method yet, no text is sent back
  ([#26]).

## Not yet

No input method, so no IME and no on-screen keyboard ([#26]); no clipboard
manager, which wants `data-control` ([#52]); no window rules ([#56]); no night
light ([#69]); a virtual machine or a remote desktop cannot have Super
([#58]); and portal dialogs cannot be parented to the window that opened them
([#67]). Screenshots and recording come from `wlr-screencopy`, so `grim` and
`wf-recorder` should work, but the portals, screen sharing included, have never
been tested end to end ([#83]). `--help`, and any flag Solium does not know,
starts a compositor instead of answering ([#156]). Nothing the compositor
draws reacts to touch yet (frame buttons, a hosted shell, the overview, window
edges), though touch reaches applications' windows ([#181]); touch gestures
are the touch epic ([#7]) and come after v0.1.0.
[docs/gaps.md](docs/gaps.md) is everything not built yet, at length.

## Building

Solium needs **Rust 1.88 or newer** (edition 2024), a C++17 compiler, **Qt 6.5
or newer** (Qt Quick and Qml), and the development files for Wayland,
libinput, libudev, libseat, xkbcommon, GBM, EGL and libdrm.
Lua is compiled in and needs nothing installed. The build also runs Qt's `moc`
on the headers of Solium's own QML types; `QT_MOC` names it when the build
cannot find it, and [dev/README.md](dev/README.md#building) says where it
looks.

Rust is easiest from [rustup](https://rustup.rs); a distribution's own Rust
works too if it is new enough. The system packages:

**Fedora**

```sh
sudo dnf install gcc gcc-c++ pkgconf-pkg-config wayland-devel libinput-devel \
    systemd-devel libseat-devel libxkbcommon-devel mesa-libgbm-devel \
    mesa-libEGL-devel libdrm-devel qt6-qtbase-devel qt6-qtdeclarative-devel
```

**Arch Linux**

```sh
sudo pacman -S --needed base-devel wayland libinput systemd-libs seatd \
    libxkbcommon mesa libdrm qt6-base qt6-declarative
```

**Debian 13 and Ubuntu 24.10 or newer** (older releases ship a Qt older than
6.5)

```sh
sudo apt install build-essential pkg-config libwayland-dev libinput-dev \
    libudev-dev libseat-dev libxkbcommon-dev libgbm-dev libegl-dev libdrm-dev \
    qt6-base-dev qt6-declarative-dev
```

**openSUSE Tumbleweed**

```sh
sudo zypper install gcc-c++ pkgconf qt6-base-devel qt6-declarative-devel \
    'pkgconfig(wayland-server)' 'pkgconfig(libinput)' 'pkgconfig(libudev)' \
    'pkgconfig(libseat)' 'pkgconfig(xkbcommon)' 'pkgconfig(gbm)' 'pkgconfig(egl)' \
    'pkgconfig(libdrm)'
```

Then, from the checkout:

```sh
cargo build            # target/debug/solium; add --release for an optimised build
```

**Or in a container, on Fedora without the development packages.** The image
has the C toolchain and the libraries, and your own rustup install is mounted
into it. It is an image of Fedora, and what it builds links that release's
libraries, so build it for the release you run: `FEDORA_VERSION` (44 by
default) is a build argument of `dev/Containerfile`. On any other distribution,
install the packages above instead. A release whose Qt is older than 6.5 cannot
run Solium at all; a pull request from one still gets every check except the
GPU one from CI.

```sh
podman build -t solium-build:fc44 -f dev/Containerfile dev/
SOLIUM_GATE_IMAGE=localhost/solium-build:fc44 dev/gate.sh
```

`dev/gate.sh` builds and runs every check; [dev/README.md](dev/README.md#building)
has the plain `podman run` for a build on its own, and
[CONTRIBUTING.md](CONTRIBUTING.md) has the rest of what a change needs.

## Running it

From a free TTY (`Ctrl+Alt+F3`), and not from inside a running desktop session —
`--tty` takes the screen and will fight whatever else has it:

```sh
cargo build
./target/debug/solium --tty
```

Nested, as a window inside an existing session, for development:

```sh
./target/debug/solium
```

Once installed (the Fedora package or `dev/install.sh`, below), the command is
`solium`: from a text console run `solium --tty`, and `solium --probe`, which
is safe inside a running desktop, first shows what the hardware offers.

Three flags worth knowing:

| | |
|---|---|
| `--check` | would the configuration load? which bindings survived? |
| `--probe` | every connector and every mode it offers, without taking the screen |
| `--debug-mode` | adds the Developer Tweaks panel, on `super+shift+d` |

`super+shift+q` ends the session and `super+shift+r` reloads the configuration
without ending it. The rest of the bindings come from `--check`, because they
belong to a script rather than to the compositor, and
[Key bindings](https://lilium-linux.github.io/solium/generated/reference/bindings.html)
lists every one that ships.

On the hardware, anything that goes wrong is also written to
`~/.local/state/solium/session.log`, synchronously — that log exists for the
case where the screen is gone and the power button is the only way out.

## Install

On Fedora 44, either build a package of the checkout and install it with dnf,
or install from the checkout into `~/.local`. The login screen offers Solium
after either. Use one: with both, it lists Solium twice.

**A Fedora package**, a development snapshot of the commit checked out:

```sh
podman build -t solium-build:fc44 -f dev/Containerfile dev/   # once
dev/rpm.sh                                                     # prints the dnf line
sudo dnf install ./target/rpm/RPMS/x86_64/solium-0.0.0~git*.rpm
```

`dev/rpm.sh` builds the release binary in the build container, packages it with
this machine's `rpmbuild` (`sudo dnf install rpm-build`), checks the package
unpacked, and prints the `dnf` line. It installs under `/usr`, so there is no
other step, and `sudo dnf remove solium` takes it out. It is built for
Fedora 44: copy the `.rpm` to another Fedora 44 machine and install it the
same way, with no Rust or build image there. Another release needs a package
built for it. [dev/README.md](dev/README.md#a-fedora-package) has the details.

**From a checkout**, on Fedora 44 (or a Fedora with the same Qt):

```sh
podman build -t solium-build:fc44 -f dev/Containerfile dev/   # once
dev/install.sh
sudo install -Dm644 ~/.local/share/solium/solium.desktop /usr/local/share/wayland-sessions/solium.desktop
```

`dev/install.sh` builds a release binary in the build container, copies it and
the shipped QML and Lua into `~/.local` (`--prefix` picks another place), runs
the installed copy's `--check`, and prints the `sudo` line above — the one step
that needs root, which it leaves to you. Then log out and pick **Solium** from
the session list.

**Getting back.** The login screen remembers the last session, so from then on
it offers Solium first: pick **Plasma** to return. Inside Solium,
Ctrl+Alt+Backspace ends the session whatever your configuration says (unless
the screen is locked), and so does super+shift+q in the shipped configuration.
Ctrl+Alt+F3 switches to a text console, where you can log in and read
`~/.local/state/solium/session.log`. Solium handles both chords itself, so
neither helps if Solium itself hangs.

What it installs is a copy: rebuilding or checking out another branch does not
change it, and running `dev/install.sh` again replaces it. When the login
screen's session file is from an older install, it says so, and the `sudo`
line has to be run again. It refuses while a Solium session is running from
it, and while `~/.local/share/solium` is a link, which it would otherwise
delete through.

**The rest of the session.** It also puts `solium-session.target` and
`solium-autostart.target` in `~/.config/systemd/user`, and
`lilium-portals.conf` in `~/.config/xdg-desktop-portal`. The session file
starts `solium-session`, which starts Solium as the session: Solium tells
systemd and D-Bus where its display is and starts those targets, which start
`graphical-session.target` and XDG autostart, and they are stopped again when
it ends, a crash included. A Solium that has stopped answering ends itself five
seconds after logout asks it to (`session.stop_timeout`), so that it is cleaned
up after too. One Solium session runs per user at a time. Portals, the programs
in `~/.config/autostart`, and user services written for a graphical session
then find Solium. A file there that you have edited, or linked, is left alone.
The portal file sends screen capture to `xdg-desktop-portal-wlr`, which Fedora
packages separately (`sudo dnf install xdg-desktop-portal-wlr`). The Fedora
package puts the units in `/usr/lib/systemd/user` and the portal file in
`/usr/share/xdg-desktop-portal` instead, and recommends
`xdg-desktop-portal-wlr`, so dnf installs it; it also recommends `foot`, so
`super+return` always has a terminal to open. A polkit agent, a keyring,
applets and other separate programs start through XDG autostart or a user unit
with `PartOf=graphical-session.target`.
[docs/shell-boundary.md](docs/shell-boundary.md#how-the-rest-of-the-desktop-starts)
says how, with the one command that starts Fedora's KDE polkit agent under
Solium. `session.systemd = false` in your configuration turns all of it off.

To remove it, `dev/install.sh --uninstall`, then the
`sudo rm -f /usr/local/share/wayland-sessions/solium.desktop` it prints. Your
`~/.config/solium` and the session log stay.

Still to come ([#66](https://github.com/Lilium-Linux/solium/issues/66)): COPR,
so that dnf installs and updates Solium without a checkout, and an Arch
`PKGBUILD`.
[dev/README.md](dev/README.md#installing-it) has the details.

## A first configuration

Everything is a file you write, and none of it needs the compositor rebuilt.
`~/.config/solium/user.lua` holds only what you want changed, and is merged
over the defaults:

```lua
return {
    pane = "left",
}
```

Press `super+shift+r` and every open window is framed again, with its
titlebar down the left side. `solium --check` says, without starting anything,
whether the configuration would load, which bindings it made, and which
settings it sets that nothing reads, with what you probably meant. A reload whose
configuration fails to load keeps the session as it was, and says why in the
log:
`~/.local/state/solium/session.log` for a session from the login screen or
`--tty`, and the terminal for a nested one.

[docs/ricing.md](docs/ricing.md) goes on from there, and
[Every setting](https://lilium-linux.github.io/solium/generated/reference/settings.html)
lists what there is to set. Your own QML in `~/.config/solium/qml` shadows
what ships, file by file. A single `Solium/Theme.qml` there is meant to
restyle every frame without copying the rest; until
[#88](https://github.com/Lilium-Linux/solium/issues/88) is fixed, the shipped
one still wins.

## How it is built

Five ideas carry most of the design. Each is a choice with a cost, and the cost
is named.

### Chrome is QML, inside the compositor

Titlebars, the wallpaper, every pane style and the pointer Solium draws when
no cursor theme is set are QML, rendered in-process through
`QQuickRenderControl` and drawn as ordinary render elements. Not a shell
talking to a compositor over a protocol — the same process, the same frame. A
shell is hosted in that same engine, as QML written against Solium's own API,
through `shell = { scene = ... }` in the configuration: see
[docs/shell-boundary.md](docs/shell-boundary.md). Quickshell support was
removed ([#172]).

That is why a titlebar can be reloaded while the session runs, why a hosted
shell can read the same theme as the window frames, and why a decoration can be
a gradient, a shader or a clock without the compositor learning what any of
those are. It is also why Qt is a hard dependency and why the compositor drives
Qt by hand. There is no Qt event loop: each frame advances Qt's animations on
the compositor's clock, and between frames the compositor's own event loop
serves Qt's timers and the descriptors Qt waits on, on that same clock, so a
`Timer` fires on an idle desktop and costs a frame only when it changes
something.

On the hardware, QML renders on the GPU: Qt draws into a buffer the compositor
allocated, once a trial render in a short-lived child process has shown that
works on this machine. Where it does not, QML renders in software and the log
says why. `solium --tty --qml gpu` or `--qml software` (or `SOLIUM_QML=`) forces
one, and the last log line beginning `QML renderer:` says which a session got —
see [dev/README.md](dev/README.md#qml-on-the-gpu).

### A pane owns a window and its chrome

A *pane* is a window plus the layers drawn around it. Layers have a `depth` —
`behind`, `frame`, `above` — and a `bleed`, which is how far past the window's
own rectangle a layer may paint. The client is drawn between them, which is the
thing a single decoration file cannot express: a glow behind, a titlebar in
front, and the window in the middle of its own frame.

A style is a folder under `qml/panes/`, so a pane style is a bundle you can
copy, not a setting you can only choose from.

### Everything a window does is one transform

Position, size, opacity, depth and rotation pivot are one `Frame` per window per
moment, and animation is interpolation between two of them. A layout does not
move windows; it says where they should be, and the transform layer gets them
there. Which is why every mode animates identically and why a thumbnail in
overview carries its own titlebar — the frame is part of the window's transform,
not a thing drawn beside it.

### The policy is Lua, the arithmetic is a crate

Tiling, scrolling, floating and overview are Lua scripts: when to split, where
a new window goes, what a key does. The arithmetic they ask for — the dwindle
tree and the scrolling strip — is `crates/layout`, which knows nothing of
windows and also arranges the boxes in `dev/preview`'s animation page.
`sol.present` hands a script the same transform the compositor uses, so a
layout somebody writes is not a second-class one. Bindings and the modes
themselves live in Lua too, and `--check` will tell you what actually loaded —
because a configuration that fails to parse at startup leaves a compositor with
no layouts and no bindings at all, and that failure should cost a log line
rather than a session.

### Effects are fragment programs with declared inputs

An effect says what it needs — nothing, or its own pixels — and the compositor
arranges the passes. What is behind it is the third input, the one a blur
needs, and it is declared and not built yet. Rounded corners are a shader over
the window's own texture, which is why they apply to the pane rather than to
the client, and why a corner radius can differ per corner so a titlebar and a
window can meet more than one way.

Drawing is GLES2, through Smithay's `GlesRenderer`, and the code says so
rather than hiding behind Smithay's generic renderer traits: a window drawn
through four corners, the rounded-corner shader and QML on the GPU each need
GLES or EGL, which those traits do not offer.

### And the parts that are deliberately not ours

Solium does not draw a bar or own a notification of its own: it hosts the
shell's QML as configuration, and layer-shell clients beside it. It tells the
session where its display is and leaves starting the rest to systemd. It blanks
its screens after ten minutes alone and no other idle policy is its own: it
reports idleness and lets a policy daemon act on the rest; it exposes
`wlr-screencopy` and lets a portal record.
[docs/shell-boundary.md](docs/shell-boundary.md) is where that line is drawn
and defended.

### Why this stack

**Smithay** is a Wayland compositor library, not a compositor. It hands over
protocol plumbing, input and backends while leaving layout, rendering and
policy to us — which is where a desktop environment's character actually lives.
[niri](https://github.com/niri-wm/niri) is Rust-on-Smithay with working touch and
gesture support, so the multi-form-factor path has a reference implementation
rather than being a bet. **Rust** because a crash in a compositor takes the
session with it, and memory safety removes an entire class of them.

**GLES2**, because Smithay has no Vulkan renderer — its `backend::vulkan` is
device enumeration only — and niri, the reference implementation this stack
was chosen for, uses GLES. The price is that a Vulkan backend would be a port,
not a swap: the render elements are typed on `GlesRenderer`, the four-corner
warp calls GL directly, rounded corners are a GLSL ES program, and QML on the
GPU shares its buffers through EGL.
[The spike](docs/spikes/2026-08-27-vulkan-on-smithay.md) lists what each would
need.

## Documentation

All of it is also a website, <https://lilium-linux.github.io/solium/>, with
reference pages made from the code.

| For | Read |
|---|---|
| using it | [docs/ricing.md](docs/ricing.md), configuring it — start here; [Every setting](https://lilium-linux.github.io/solium/generated/reference/settings.html) and the [configuration reference](crates/solium/lua/config.lua), the shipped [Key bindings](https://lilium-linux.github.io/solium/generated/reference/bindings.html), and [Flags and environment](https://lilium-linux.github.io/solium/generated/reference/environment.html) |
| scripting it in Lua | [docs/modes.md](docs/modes.md), desktop modes and how to write one; [docs/animation.md](docs/animation.md), the animation engine; the [Lua API](crates/solium/lua/meta/sol.lua) |
| styling it in QML | [docs/decorations.md](docs/decorations.md), window frames and everything else drawn in QML; the [pane styles' README](crates/solium/qml/panes/README.md), the contract a style is written against |
| writing a shell | [docs/shell-boundary.md](docs/shell-boundary.md), what a hosted shell is given and what belongs to the compositor |
| working on it | [CONTRIBUTING.md](CONTRIBUTING.md), how to send a change and the rules it is judged by; [docs/architecture.md](docs/architecture.md), how it is built and why; [dev/README.md](dev/README.md), the build, the gate and the instruments it is tested with; [SECURITY.md](SECURITY.md); [THIRD_PARTY.md](THIRD_PARTY.md), what came from other projects |
| where it stands | [docs/status.md](docs/status.md), with the [roadmap](docs/roadmap.md), [everything not built yet](docs/gaps.md) and [the road to a public preview](docs/beta.md) |
| why it is the way it is | the decision records in `docs/spikes/` and `docs/design/`, each with the evidence that settled it |

## Thanks to

- **[Smithay](https://github.com/Smithay/smithay)** — Solium is a Smithay
  compositor, and most of what is hard about being one is Smithay's work rather
  than this project's: DRM, GBM, libinput, the seat, the protocol
  implementations. Its example compositor is also the first place to look when
  something here does not make sense.
- **[niri](https://github.com/niri-wm/niri)** — the reference for how a serious
  Smithay compositor is actually put together, and the answer to more than one
  "surely this cannot be the way" while reading DRM code. Its scrolling layout
  is why `lua/scrolling.lua` exists to be compared against.
- **[Hyprland](https://github.com/hyprwm/Hyprland)** — where this started. Solium
  began as a Hyprland fork, and that work is archived: it proved the decoration
  and window-control ideas, and its post-mortem — including why an
  out-of-process QML shell was the wrong architecture — is in the `lilium-de`
  repository. The ideas carried over and none of the code did, but the case
  that a compositor is allowed to be beautiful and configurable at the same
  time was made there first.
- **[Quickshell](https://quickshell.outfoxxed.me/)** — the demonstration that a
  desktop shell can be QML all the way down. Solium hosts QML *inside* the
  compositor rather than beside it, which is a different answer to the same
  question, and it is a different answer because Quickshell had already shown
  what the question was.

## License

Solium is free software under the [GNU General Public License, version 3
only](LICENSE) (`GPL-3.0-only`).

The logo, logotype and default wallpaper are © 2026 Illia Kotomin, licensed
[CC BY-SA 4.0](LICENSES/CC-BY-SA-4.0.txt); see [docs/brand](docs/brand/README.md).

Contributions are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md). Each
contributor agrees once to the [Contributor License Agreement](CLA.md); the
pull request template carries the one sentence that does it. What came from
other projects is listed, with its licence, in [THIRD_PARTY.md](THIRD_PARTY.md).

[daily-drive]: https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20state%3Aopen%20label%3Adaily-drive
[#7]: https://github.com/Lilium-Linux/solium/issues/7
[#24]: https://github.com/Lilium-Linux/solium/issues/24
[#26]: https://github.com/Lilium-Linux/solium/issues/26
[#43]: https://github.com/Lilium-Linux/solium/issues/43
[#48]: https://github.com/Lilium-Linux/solium/issues/48
[#52]: https://github.com/Lilium-Linux/solium/issues/52
[#54]: https://github.com/Lilium-Linux/solium/issues/54
[#56]: https://github.com/Lilium-Linux/solium/issues/56
[#57]: https://github.com/Lilium-Linux/solium/issues/57
[#58]: https://github.com/Lilium-Linux/solium/issues/58
[#59]: https://github.com/Lilium-Linux/solium/issues/59
[#64]: https://github.com/Lilium-Linux/solium/issues/64
[#65]: https://github.com/Lilium-Linux/solium/issues/65
[#66]: https://github.com/Lilium-Linux/solium/issues/66
[#67]: https://github.com/Lilium-Linux/solium/issues/67
[#69]: https://github.com/Lilium-Linux/solium/issues/69
[#72]: https://github.com/Lilium-Linux/solium/issues/72
[#81]: https://github.com/Lilium-Linux/solium/issues/81
[#83]: https://github.com/Lilium-Linux/solium/issues/83
[#92]: https://github.com/Lilium-Linux/solium/issues/92
[#113]: https://github.com/Lilium-Linux/solium/issues/113
[#115]: https://github.com/Lilium-Linux/solium/issues/115
[#116]: https://github.com/Lilium-Linux/solium/issues/116
[#118]: https://github.com/Lilium-Linux/solium/issues/118
[#121]: https://github.com/Lilium-Linux/solium/issues/121
[#126]: https://github.com/Lilium-Linux/solium/issues/126
[#127]: https://github.com/Lilium-Linux/solium/issues/127
[#128]: https://github.com/Lilium-Linux/solium/issues/128
[#132]: https://github.com/Lilium-Linux/solium/issues/132
[#133]: https://github.com/Lilium-Linux/solium/issues/133
[#134]: https://github.com/Lilium-Linux/solium/issues/134
[#141]: https://github.com/Lilium-Linux/solium/issues/141
[#142]: https://github.com/Lilium-Linux/solium/issues/142
[#146]: https://github.com/Lilium-Linux/solium/issues/146
[#147]: https://github.com/Lilium-Linux/solium/issues/147
[#149]: https://github.com/Lilium-Linux/solium/issues/149
[#150]: https://github.com/Lilium-Linux/solium/issues/150
[#151]: https://github.com/Lilium-Linux/solium/issues/151
[#152]: https://github.com/Lilium-Linux/solium/issues/152
[#153]: https://github.com/Lilium-Linux/solium/issues/153
[#156]: https://github.com/Lilium-Linux/solium/issues/156
[#157]: https://github.com/Lilium-Linux/solium/issues/157
[#161]: https://github.com/Lilium-Linux/solium/issues/161
[#162]: https://github.com/Lilium-Linux/solium/issues/162
[#163]: https://github.com/Lilium-Linux/solium/issues/163
[#164]: https://github.com/Lilium-Linux/solium/issues/164
[#166]: https://github.com/Lilium-Linux/solium/issues/166
[#169]: https://github.com/Lilium-Linux/solium/issues/169
[#172]: https://github.com/Lilium-Linux/solium/issues/172
[#173]: https://github.com/Lilium-Linux/solium/issues/173
[#174]: https://github.com/Lilium-Linux/solium/issues/174
[#175]: https://github.com/Lilium-Linux/solium/issues/175
[#177]: https://github.com/Lilium-Linux/solium/issues/177
[#178]: https://github.com/Lilium-Linux/solium/issues/178
[#180]: https://github.com/Lilium-Linux/solium/issues/180
[#181]: https://github.com/Lilium-Linux/solium/issues/181
