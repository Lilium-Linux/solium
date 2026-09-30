<img src="docs/brand/logotype.svg" width="132" alt="Solium">

# Solium

The compositor of [Lilium DE](https://github.com/Lilium-Linux). Wayland, written
in Rust on [Smithay](https://github.com/Smithay/smithay).

One compositor for phone, tablet, laptop and desktop — tiling, scrolling,
floating and overview modes, all scriptable, all animated by the same engine.

![Two windows on the Solium wallpaper, one with a titlebar the compositor drew in QML](docs/desktop.png)

Captured by the compositor reading back its own framebuffer. The window on the
right wears a titlebar that is QML rendered in-process and reloadable while the
session runs; the one on the left asked to draw its own and was let. The
wallpaper is drawn by the compositor too, and is a QML file you can replace.
[docs/modes.md](docs/modes.md) has the same picture for every mode, frame by
frame.

## Building

Solium needs **Rust 1.88 or newer** (edition 2024), a C++17 compiler, **Qt 6.5
or newer** (Qt Quick, Qml and Network), and the development files for
Wayland, libinput, libudev, libseat, xkbcommon, GBM, EGL and libdrm.
Lua is compiled in and needs nothing installed.

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

Three flags worth knowing:

| | |
|---|---|
| `--check` | would the configuration load? which bindings survived? |
| `--probe` | every connector and every mode it offers, without taking the screen |
| `--debug-mode` | adds the Developer Tweaks panel, on `super+shift+d` |

`super+shift+q` ends the session and `super+shift+r` reloads the configuration
without ending it. The rest of the bindings come from `--check`, because they
belong to a script rather than to the compositor.

On the hardware, anything that goes wrong is also written to
`~/.local/state/solium/session.log`, synchronously — that log exists for the
case where the screen is gone and the power button is the only way out.

## Install

There is no package yet. From a checkout, on Fedora 44 (or a Fedora with the
same Qt), so the login screen offers Solium:

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
up after too. One Solium session runs per user at a time. Portals, the
programs in `~/.config/autostart`, and user services written for a graphical
session then find Solium. A file there
that you have edited, or linked, is left alone. The portal file sends screen
capture to `xdg-desktop-portal-wlr`, which Fedora packages separately
(`sudo dnf install xdg-desktop-portal-wlr`). A polkit agent, a keyring,
applets and other separate programs start through XDG autostart or a user unit
with `PartOf=graphical-session.target`.
[docs/shell-boundary.md](docs/shell-boundary.md#how-the-rest-of-the-desktop-starts)
says how, with the one command that starts Fedora's KDE polkit agent under
Solium. `session.systemd = false` in your configuration turns all of it off.

To remove it, `dev/install.sh --uninstall`, then the
`sudo rm -f /usr/local/share/wayland-sessions/solium.desktop` it prints. Your
`~/.config/solium` and the session log stay.

Still to come ([#66](https://github.com/Lilium-Linux/solium/issues/66)): a
Fedora `.spec` and COPR, and an Arch `PKGBUILD`.
[dev/README.md](dev/README.md#installing-it) has the details.

## What works

Windows open, tile, scroll, float and animate. Server-side decorations are QML
and reload while the session runs. X11 clients work through XWayland. Copy and
paste works, both selections. A window's life begins when the user asks for the
application rather than when its client connects, so it takes its place in the
layout immediately and the application appears inside it.

Several monitors, each with its own display pipeline, refresh rate and layout —
one global coordinate space, arranged from the configuration or guessed left to
right. Every layout runs per screen, and the pointer crosses between them.

Protocols: `xdg-shell`, `wlr-layer-shell`, `wlr-screencopy`, `ext-session-lock`,
`ext-idle-notify`, `idle-inhibit`, `wlr-output-power-management`,
`xdg-decoration`, `xdg-output`,
`xdg-activation`, `wp-viewporter`, `wp-fractional-scale`, `wp-presentation`,
`wp-single-pixel-buffer`, `linux-dmabuf`, `relative-pointer`,
`pointer-constraints`, `primary-selection`, `xwayland-shell`.
`_NET_WM_WINDOW_TYPE` is read on the X11 side, so menus and tooltips are not
managed as windows.

The desktop has a wallpaper before anything else is running, because a session
whose first frame is flat grey looks the same as a broken one. It is
`qml/wallpaper.qml` — a QML file, so it can be a gradient, a shader or a clock
instead — and a layer surface on the background layer still wins, so `swaybg`
and anything like it work unchanged.

Screenshots, recording and screen sharing all come from `wlr-screencopy`, so
`grim`, `wf-recorder` and `xdg-desktop-portal-wlr` work — which is what a
conferencing application asks the portal for.

The screen locks, through `ext-session-lock-v1`, and it fails safe: the session
is locked before the locking program has drawn anything, so there is no moment
where the desktop is still up; if that program then crashes, the screen stays
locked and blank rather than falling open. While locked nothing of the session
sees input — no bindings, no window under the pointer, no titlebars, and
`Ctrl+Alt+Backspace` will not end the session. Switching virtual terminal still
works, deliberately: whoever can press it is standing at the machine.

`ext-idle-notify` is what makes that happen on its own rather than only when
asked: `swayidle` and anything like it can run the locker after so long with
nobody at the machine. `idle-inhibit` is the other half — a video player, a
presentation or a game says "not now" and the screen stays up. An inhibitor
counts only while its window is actually drawn, so one on a workspace you are
not looking at stops holding the machine awake, and nothing at all holds it
awake behind a lock screen.

The screens go dark on their own after ten minutes with nobody at the machine
(`idle.screens_off_after` in `config.lua`; 0 turns it off), unless something on
screen is holding an inhibitor — a film does not go dark, the ten minutes start
again when it lets go, and nothing holds a lock screen lit. Any key, click or motion turns every screen back on and is
delivered as usual, so the first key typed at a lock screen that went dark is
part of the password rather than lost. A monitor that is off keeps its place,
its work area and its windows; nothing moves and no client is told it went.
`wlr-output-power-management` is there too, so `wlopm` and `swayidle` driving it
can do the same, and `sol.monitor_power` does it from a binding. The recipe is
in [docs/ricing.md](docs/ricing.md#turning-screens-off).

Menus behave. An X11 client that says what kind of window it is gets it: a menu
or a tooltip is placed by the application rather than tiled like a window, a Wayland popup is grabbed so clicking outside dismisses
it and constrained so it flips at a screen edge instead of running off, and a
client that draws its own decorations is clicked where it is drawn rather than
where its invisible resize shadow starts.

Not yet: an input method, so no IME and no on-screen keyboard
([#26](https://github.com/Lilium-Linux/solium/issues/26)); no clipboard manager,
which wants `data-control`
([#52](https://github.com/Lilium-Linux/solium/issues/52)); portals have never
been tested end to end
([#83](https://github.com/Lilium-Linux/solium/issues/83)). Everything known is
on the issue tracker, prioritised by whether an application can be used at all
without it rather than by how hard it is — the `daily-drive` label is that
list.

## Configuring it

Everything is a file you write, and none of it needs the compositor rebuilt.
`~/.config/solium/user.lua` holds only what you want changed; your own QML in
`~/.config/solium/qml` shadows what ships, file by file. A single
`Solium/Theme.qml` there is meant to restyle every frame and surface without
copying the rest; until [#88](https://github.com/Lilium-Linux/solium/issues/88)
is fixed, the shipped one still wins. See **[docs/ricing.md](docs/ricing.md)**.

## How it is built

Five ideas carry most of the design. Each is a choice with a cost, and the cost
is named.

### Chrome is QML, inside the compositor

Titlebars, the pointer, the wallpaper and every pane style are QML, rendered
in-process through `QQuickRenderControl` and drawn as ordinary render elements.
Not a shell talking to a compositor over a protocol — the same process, the
same frame. A shell can be hosted in that same engine too, as a plain Qt Quick
scene named with `shell = { scene = ... }` in the configuration: see
[docs/shell-boundary.md](docs/shell-boundary.md). The Quickshell compatibility
layer, which lets a shell written for Quickshell load, is deprecated and will
be removed.

That is why a titlebar can be reloaded while the session runs, why a hosted
shell can read the same theme as the window frames, and why a decoration can be
a gradient, a shader or a clock without the compositor learning what any of
those are. It is also why Qt is a hard dependency and why the render loop has
to drive Qt's animations by hand: a `Timer` in a settled QML scene never fires,
because nothing advances it but a frame the compositor decided to draw.

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

### Layouts are scripts, not features

Tiling, scrolling, floating and overview are Lua. `sol.present` hands a script
the same transform the compositor uses, so a layout somebody writes is not a
second-class one. Bindings, rules and the modes themselves live there too, and
`--check` will tell you what actually loaded — because a configuration that
fails to parse leaves a compositor with no layouts and no bindings at all, and
that failure should cost a log line rather than a session.

### Effects are fragment programs with declared inputs

An effect says what it needs — nothing, its own pixels, or what is behind it —
and the compositor arranges the passes. Rounded corners are a shader over the
window's own texture, which is why they apply to the pane rather than to the
client, and why a corner radius can differ per corner so a titlebar and a window
can meet more than one way.

Drawing is GLES2, through Smithay's `GlesRenderer`, and the code says so
rather than hiding behind Smithay's generic renderer traits: a window drawn
through four corners, the rounded-corner shader and QML on the GPU each need
GLES or EGL, which those traits do not offer.

### And the parts that are deliberately not ours

Solium does not manage sessions, draw a bar or own a notification. It blanks
its screens after ten minutes alone and no other idle policy is its own: it
reports idleness and lets a policy daemon act on the rest; it exposes
`wlr-screencopy` and lets a portal record; it hosts layer surfaces and lets a
shell be a shell. [docs/shell-boundary.md](docs/shell-boundary.md) is where that
line is drawn and defended.

## Why this stack

**Smithay** is a Wayland compositor library, not a compositor. It hands over
protocol plumbing, input and backends while leaving layout, rendering and
policy to us — which is where a desktop environment's character actually lives.
[niri](https://github.com/niri-wm/niri) is Rust-on-Smithay with working touch and
gesture support, so the multi-form-factor path has a reference implementation
rather than being a bet.

**Rust** because this codebase is meant to last. Memory safety removes an entire
class of compositor crash, and a crash in a compositor takes the session with it.

**GLES2**, through Smithay's GLES renderer. Smithay has no Vulkan renderer —
its `backend::vulkan` is device enumeration only — and niri, the reference
implementation we chose this stack for, uses GLES. Vulkan would have meant
writing a renderer backend plus NVIDIA dmabuf import before a single window
appeared. See
[docs/spikes/2026-08-27-vulkan-on-smithay.md](docs/spikes/2026-08-27-vulkan-on-smithay.md).

The price is that a Vulkan backend would be a port, not a swap. The render
elements are typed on `GlesRenderer`, the four-corner warp calls GL directly,
rounded corners are a GLSL ES program, and QML on the GPU shares its buffers
through EGL. Each of those needs a Vulkan counterpart; the spike lists them.

## Status

Alpha. It runs on hardware and is used to develop itself, which is the only
test that counts for a compositor. It is not something to depend on yet, and
the honest reasons are specific: it has never run for a whole day
([#65](https://github.com/Lilium-Linux/solium/issues/65)), suspend and resume
have never been tested once
([#64](https://github.com/Lilium-Linux/solium/issues/64)), there are no
packaging recipes yet ([#66](https://github.com/Lilium-Linux/solium/issues/66) —
an installed copy now finds its own files, but nothing builds a package), there
is no input method at all
([#26](https://github.com/Lilium-Linux/solium/issues/26)), and a bug in here
takes the session with it.

The bugs that get found are the ones real use finds. A recent afternoon of
actually working in it turned up menus being placed by the tiling layout,
a pointer whose clicks landed 45 pixels from the cursor on any GTK or Qt
application, and windows reserving space for titlebars that were never drawn —
none exotic, none caught by the test suite, all found by opening a browser.
That is what alpha means here.

[docs/gaps.md](docs/gaps.md) is the whole list rather than the flattering part
of it.

## Documentation

| | |
|---|---|
| [docs/ricing.md](docs/ricing.md) | configuring it — start here |
| [docs/modes.md](docs/modes.md) | desktop modes, and how to write one — including per monitor |
| [docs/animation.md](docs/animation.md) | the animation engine, and how to change the feel |
| [docs/decorations.md](docs/decorations.md) | window frames and everything else drawn in QML |
| [docs/architecture.md](docs/architecture.md) | how it is built, and why |
| [docs/shell-boundary.md](docs/shell-boundary.md) | what belongs to the compositor and what to the shell |
| [docs/roadmap.md](docs/roadmap.md) | epics, in dependency order |
| [docs/beta.md](docs/beta.md) | what has to be true before a public preview |
| [docs/gaps.md](docs/gaps.md) | everything not built yet, exhaustively |
| [dev/README.md](dev/README.md) | the knobs and checks it is tested with |
| [CONTRIBUTING.md](CONTRIBUTING.md) | how to send a change, and the design rules it is judged by |
| [SECURITY.md](SECURITY.md) | reporting a vulnerability privately |
| [THIRD_PARTY.md](THIRD_PARTY.md) | what came from other projects, and every dependency's licence |
| `docs/spikes/` | decisions, with the evidence that settled them |

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
