# Developing Solium

How to build Solium, check a change, run it nested, drive and photograph it
without a hand on the keyboard, run it on a TTY and install it.
[CONTRIBUTING.md](../CONTRIBUTING.md) has how to send a change and the rules
it is judged by; this file is the instruments.

Where this file, or a comment in the source, quotes a measurement without
saying where it was taken, it was taken in September 2026 on one desktop: an
NVIDIA RTX 3070 on the proprietary driver (610.x), Qt 6.11, Fedora 44. None of
them has been taken again since, so read them as orders of magnitude.

Settings are not here. [docs/ricing.md](../docs/ricing.md) is how to
configure Solium with `~/.config/solium/user.lua`, and the
[configuration reference](../crates/solium/lua/config.lua) has every setting.
The keys that ship are the
[Key bindings](https://lilium-linux.github.io/solium/generated/reference/bindings.html)
page, made from what `solium --check` prints.

## Building

Natively, with the development packages the top-level README lists under
*Building*:

```sh
cargo build
```

The build runs Qt's `moc` on `qml/attached.h`, `qml/rows.h` and
`qml/keyboard.h`, which declare the native `Solium` QML types. `build.rs`
looks for it in this order: `QT_MOC` if it is set; the `libexecdir` Qt6Core's
pkg-config file names; `qt6/libexec/moc` or `qt6/bin/moc` beside Qt's
libraries; `/usr/lib64/qt6/libexec/moc`, `/usr/lib/qt6/libexec/moc` and
`/usr/lib/x86_64-linux-gnu/qt6/libexec/moc`; and last, `moc` on `PATH`. If it
cannot run moc, the build stops with `could not run moc at …` and
`set QT_MOC to its path, or install Qt 6's development tools`. Setting or
changing `QT_MOC` runs the build script again. On Fedora, moc comes with
`qt6-qtbase-devel`, which the build image already has. `dev/wirecheck/build.rs`
runs moc on the same headers from a list of its own, so a new header that
declares a `Q_OBJECT` type goes into `MOC_HEADERS` in both build scripts.

Or, on a Fedora host without the development packages, in the build image,
which needs only podman and a rustup install. The image has the C toolchain and
the system libraries; Rust comes from your own `CARGO_HOME` and `RUSTUP_HOME`,
mounted at the same paths. Build the image for the Fedora release you run
(`--build-arg FEDORA_VERSION=<n>`, 44 by default), for the reasons below; on
another distribution, install the packages natively:

```sh
podman build -t solium-build:fc44 -f dev/Containerfile dev/
podman run --rm --userns=keep-id --security-opt label=disable \
    -v "$PWD:$PWD" -v "$HOME/.cargo:$HOME/.cargo" -v "$HOME/.rustup:$HOME/.rustup" \
    -e CARGO_HOME="$HOME/.cargo" -e RUSTUP_HOME="$HOME/.rustup" \
    -e PATH="$HOME/.cargo/bin:/usr/bin:/bin" \
    -w "$PWD" localhost/solium-build:fc44 cargo build
```

**The build image matches the host's distribution on purpose.** Two things go
wrong otherwise, and both were hit:

* A vendored C dependency (Lua) is compiled by whatever toolchain builds the
  project. A container with a newer C library produces a binary the host cannot
  start at all — `GLIBC_2.44 not found`.
* Working around *that* by running the compositor inside the container costs
  hardware acceleration. The container has Mesa but no GPU driver, so
  everything falls back to llvmpipe. Measured on 2026-09-05:

  | | Renderer | Frame rate |
  |---|---|---|
  | In the container | `llvmpipe` | 55 fps |
  | On the host | `NVIDIA RTX 3070` | 260 fps |

  On a 260 Hz display, 55 fps puts a dragged window four frames behind the
  cursor, which is exactly what it looks like.

**Never glob `target/debug/build/solium-*/out/`.** The Qt host — `host.cpp`,
`attached.cpp`, `rows.cpp` and `keyboard.cpp`, and the moc output of their
headers — is compiled into `libsolium_qml_host.a` under that path, and there is
more than one such directory: cargo makes a separate one per unit metadata, so
`cargo build -p solium` and `cargo build` (or `cargo test`) each own one. A
test harness or a script that links "the" archive by glob picks whichever the
shell sorts first, which is not the newest, and there is no error — the link
succeeds and you measure a build from an hour ago. It cost a full round of
debugging a fix that was already in the tree.

This is not something one commit introduced and another can remove; it is how
cargo lays the directory out. Either pin the newest,

```sh
ls -td target/debug/build/solium-*/out | head -1
```

or compile the host from source in the harness's own `build.rs`: `host.cpp`
together with `attached.cpp`, `rows.cpp` and `keyboard.cpp` from
`crates/solium/qml/`, with moc run on `attached.h`, `rows.h` and `keyboard.h`,
as `dev/wirecheck/build.rs` does. That is the only way to be certain that what
runs is what is checked out.

### The gate

`dev/gate.sh` checks the formatting with `cargo fmt --check`, then runs clippy
with warnings denied, the tests and a build, then two checks on the built
binaries that nothing else reaches: `solium --check`, which loads the Lua
configuration and builds the scenes it declares, and `dev/wirecheck`, which
drives the QML GPU path against the machine's own render node. Run
`cargo fmt --all` first if the formatting step fails. It runs cargo natively
unless told otherwise:

| Variable | Effect |
|---|---|
| `SOLIUM_GATE_IMAGE=<image>` | Build in this podman image, e.g. `localhost/solium-build:fc44`. If it does not exist, the gate prints the `podman build` line that makes it. Only the checkout, `CARGO_HOME` and `RUSTUP_HOME` are mounted. `solium --check` runs in the image too; `dev/wirecheck` runs on the host, on its render node, and is skipped with a message if the host cannot load what the image built. |
| `SOLIUM_GATE_PODMAN_ARGS=<args>` | Extra `podman run` arguments, split on spaces — e.g. `"--memory=6g --memory-swap=6g"` to cap a build that would otherwise use all the memory there is. |
| `SOLIUM_GATE_JOBS=<n>` | `cargo -j<n>`. Unset, cargo uses every CPU. |
| `SOLIUM_GATE_CPUS=<list>` | Pin the build to these CPUs, as a `taskset` list such as `14,15`. |
| `SOLIUM_GATE_NO_GPU=1` | Build the GPU check but do not run it. It is skipped anyway on a machine with no render node. |

For example, capped and in the container:

```sh
SOLIUM_GATE_IMAGE=localhost/solium-build:fc44 \
SOLIUM_GATE_PODMAN_ARGS="--memory=6g --memory-swap=6g" SOLIUM_GATE_JOBS=2 \
    dev/gate.sh
```

The cargo steps run under `nice -n 19` and, where it exists, `ionice -c 3`, so
a gate in the background leaves the machine usable. `solium --check` and
`dev/wirecheck` are short and run at normal priority.

The tests include the scenarios in `crates/solium/tests/scenarios/`, played by
`scenario::tests::every_scenario_with_a_client_passes` and
`every_scenario_on_the_qt_thread_passes`. Run natively on a machine whose
`$XDG_CONFIG_HOME/solium` (`~/.config/solium`) holds any `.lua` file, the
client scenarios (`every_scenario_with_a_client_passes`) and the script tests
built on `script::tests::shipped_init_with_user` pass without running, because
that directory comes first on `package.path`; the scenario test prints
`skipped: ~/.config/solium holds Lua of its own, which a scenario would load`.
Pointing `XDG_CONFIG_HOME` at an empty directory runs them. With
`SOLIUM_GATE_IMAGE` the container's `HOME` is `/tmp`, so they always run.

One test is known to fail now and then under the gate's load
([#176](https://github.com/Lilium-Linux/solium/issues/176)):
`qml::wake::tests::a_clock_scene_repaints_once_a_second_with_no_other_damage`
counts exactly three frames in three and a half seconds, and a one-second
`Timer` that drifts past the end of that window gives two. If it is the only
failure, run it on its own (`cargo test -p solium a_clock_scene_repaints`)
before suspecting the change.

CI runs the formatting check, clippy, the build, the tests and
`solium --check`, and not `dev/wirecheck`, which needs a GPU. A source tarball
made with `git archive` leaves `dev/wirecheck` out, along with the other
hardware harnesses and probes `.gitattributes` names, and the gate then skips
that step and says so.

## Running it nested

```sh
./target/debug/solium          # a window inside your session
dev/run-nested.sh              # the same, logging to a file and printing the socket
```

Nested, Solium runs as a window inside the session you are using. That is how
almost all of it is developed and checked. `dev/run-nested.sh` runs it **on the
host**, on the host's own GPU driver, whether it was built natively or in the
build image — the image only builds it, which is why it matches the host's
distribution. It refuses to start without a `WAYLAND_DISPLAY`, because an empty
one means the default socket, which is your real session.
[host-window-rule.md](host-window-rule.md) keeps the nested window from taking
focus or landing on the wrong monitor.

Three things are different nested, and they matter for what a nested run
proves:

* QML renders in software: the nested backend hands the compositor no GBM
  device, so there is no GPU path to take (see *QML on the GPU*).
* The host draws a pointer over Solium's own, so a missing pointer does not
  show (`dev/cursor-check.sh`).
* The knobs below that drive and photograph a session are read by the nested
  backend only.

### Running programs inside Solium

`Super`+`Return` starts a terminal: `SOLIUM_TERMINAL` names one, or else the
shipped `init.lua` takes the first that is installed. A binding in `user.lua`
starts anything else (`bindings = { ["super+b"] = "firefox" }`; see
[docs/ricing.md](../docs/ricing.md#your-own-bindings)), and so does `sol.spawn`
from a script:

```lua
sol.bind("super+e", function() sol.spawn("foot", "-e", "htop") end)
```

Programs start as clients of *this* compositor — `sol.spawn` overrides
`WAYLAND_DISPLAY` in the child, or it would inherit the host's and open its
window next to the nested compositor rather than inside it.

Apart from the session's own variables (`WAYLAND_DISPLAY`; `DISPLAY` for
Solium's Xwayland, or removed; `XDG_CURRENT_DESKTOP`; `XDG_SESSION_TYPE` when
unset; `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID`), a program gets the
environment Solium itself was started with (#175). That environment is noted
before Qt or EGL has changed it, so nothing they set inside the compositor
leaks into a child, and the program holds no descriptor beyond stdio. The
compositor's own Qt drops `QT_IM_MODULE` and `QT_IM_MODULES`, but a program
Solium starts still gets yours.

Solium runs on the host, so `sol.spawn` can start anything installed there —
with one family of exceptions:

**Do not test with KDE's own applications.** They are launched through DBus
activation, so the process that actually opens the window inherits the
*session's* environment and connects to the session's compositor. The spawn
succeeds, the program keeps running, its window appears on the host desktop,
and nothing is logged anywhere — the most confusing failure available.

`dev/foot` is the way round it: a terminal from the build image, connected to
whichever compositor invoked it.

```sh
SOLIUM_TERMINAL=$PWD/dev/foot dev/run-nested.sh
```

Anything that is not DBus-activated can be pointed at the socket directly — the
launcher prints its name:

```sh
WAYLAND_DISPLAY=wayland-1 <program>
```

To run something else from the build image the way `dev/foot` runs foot, add
its package to the `dnf install` line in `dev/Containerfile` and rebuild the
image — a `--rm` container throws anything installed into it by hand away with
itself.

## The knobs

The ones used most. Every flag and every environment variable Solium reads,
with where in the code each is read, is on the [Flags and
environment](https://lilium-linux.github.io/solium/generated/reference/environment.html)
page, made from `crates/solium/environment.txt`; a test fails when the code
reads one that file does not list.

| Variable | Effect | Nested only |
|---|---|---|
| `SOLIUM_CAPTURE=<path>` | Write one rendered frame to `<path>` as a binary PPM. See *Capturing a frame*. | yes |
| `SOLIUM_CAPTURE_AT=<ms>` | Capture at this moment after startup instead of once a window has settled. Naming a moment is what makes capturing an *animation* possible. | yes |
| `SOLIUM_CAPTURE_FRAMES=<n>`, `SOLIUM_CAPTURE_INTERVAL=<ms>` | A burst of `n` frames, `<ms>` apart (16 by default), written beside the path with a number on its stem: `/tmp/tile.ppm` gives `/tmp/tile-000.ppm`, `/tmp/tile-001.ppm`, … One frame shows a pose; a burst shows whether the motion is smooth. | yes |
| `SOLIUM_TRIGGER_AT=<ms>:<combo>,...` | Run what these combinations are bound to, at these moments: `5200:super+space,6200:super+space`. Not a keypress: the binding is looked up by the name written, so it fires while the screen is locked, cannot press Ctrl+Alt+Backspace, and cannot show whether a binding works under another keyboard layout. | yes |
| `SOLIUM_KEY_AT=<ms>:<key>,...` | Press real keys, by keysym name, through the real input path: `1500:caps_lock,3000:shift+alt_l`. Caps Lock locks, an xkb layout switch switches, and a bound combination runs its binding. See *Keys by name*. | yes |
| `SOLIUM_DRAG_AT=<ms>:<x1>,<y1>><x2>,<y2>;...` | Drag the pointer through the real input path: the real grab, hit-testing and layout scripts. A drag from a point to itself is a click. | yes |
| `SOLIUM_CLICK_AT=<ms>:<x>,<y>;...` | Hand a press to the mode holding the pointer, such as overview. It never touches the input path; see *Driving it without a keyboard*. | yes |
| `SOLIUM_OUTPUTS=<n>` | Give the nested backend `n` monitors, 1 to 4, side by side in its one window. See *Two monitors, without a second monitor*. | yes |
| `SOLIUM_OUTPUTS_AT=<ms>:<n>,...` | Change how many nested monitors there are at these moments: hotplug without a cable. | yes |
| `SOLIUM_LOADING_AT=<ms>:<program>,...` | Open a window for an application that never arrives, to exercise the loading window. | yes |
| `SOLIUM_LUA_INIT=<path>` | Load this configuration instead of `~/.config/solium/init.lua` or the shipped one. | |
| `SOLIUM_TERMINAL=<command line>` | The terminal `super+return` opens, split on spaces. | |
| `SOLIUM_PANE=<name or path>` | The frame style for this run. | |
| `SOLIUM_FENCE_WAIT=off` | A window capture drops its fence instead of waiting for it on the CPU. The default waits, and each session's log says which it ran with. | |
| `SOLIUM_RECAPTURE=always` | Draw every window capture (a warp's, a rounded client's, a self effect's part) on every pass, as before captures were kept until what they show changes, so every self effect's chain runs every pass too. For an A/B on one build, and the control of `dev/pacing-nested.sh`'s capture count. | |
| `SOLIUM_PACING` | Say where a pass's time went, on passes that overran the tightest monitor's frame; with it, GPU time, clocks and late flips. Misses are counted without it. | |
| `SOLIUM_TRACE=<path>` | One JSON line per pass and per flip, and `SOLIUM_PACING` on. See *Measuring frame pacing on a TTY*. | |
| `SOLIUM_QML=<mode>` | `auto`, `gpu` or `software`; see *QML on the GPU*. | |
| `SOLIUM_SESSION_BUS=<address>` | The D-Bus bus to tell about the session and to own `org.freedesktop.ScreenSaver` on, instead of the session bus. A nested run, or `solium --tty` without `--session`, tells nobody anything without it. To check the calls against a private bus: `dbus-run-session -- sh -c 'SOLIUM_SESSION_BUS=$DBUS_SESSION_BUS_ADDRESS ./target/debug/solium'`. | |
| `SOLIUM_LOGIND_BUS=<address>` | The D-Bus bus to hear logind's `Lock` and sleep signals on (`lock.command`, `lock.before_sleep`), instead of the system bus. A nested run, or `solium --tty` without `--session`, hears neither without it. | |
| `RUST_LOG=<filter>` | Log levels, `info` by default. `solium::qml` carries Qt's own messages. | |

On the hardware there is no `SOLIUM_CAPTURE`. A screenshot there is a
`wlr-screencopy` client such as `grim`, or `wl-probe`'s `WL_PROBE_SHOT`.

## Capturing a frame

```sh
SOLIUM_CAPTURE=/tmp/frame.ppm dev/run-nested.sh
```

A compositor cannot be verified by looking at it: a desktop screenshot tool
captures the *host* session, which proves nothing about what Solium composited,
and using a shell's own capture to test that shell is circular. So Solium reads
its own framebuffer back.

The capture waits until a window has been mapped for 30 frames, which is half a
second at 60 Hz and less on a faster host, because a capture of an empty
compositor is exactly the misleading result the mechanism exists to avoid. A
captured frame is not presented — reading the framebuffer back invalidates the
bind, and the following `submit` would fail to reallocate its EGL surface.

Under `timeout`, a nested run ends the way a quit binding ends it, so a capture
can be scripted end to end:

```sh
SOLIUM_CAPTURE=/tmp/frame.ppm SOLIUM_CAPTURE_AT=3000 timeout -s TERM 5 ./target/debug/solium
magick ppm:/tmp/frame.ppm /tmp/frame.png
```

## Driving it without a keyboard

Because a mode that can only be checked by someone pressing a key is a mode
nobody checks twice. Enter overview, click the top-right thumbnail, and
photograph the result:

```sh
SOLIUM_TRIGGER_AT=5200:super+space SOLIUM_CLICK_AT=6000:1200,250 \
    SOLIUM_CAPTURE=/tmp/frame.ppm SOLIUM_CAPTURE_AT=7000 dev/run-nested.sh
```

Enter and leave, then compare the frame with one taken at rest. With nothing
but Solium on screen the whole frame must be identical, because leaving a mode
restores the layout exactly:

```sh
SOLIUM_TRIGGER_AT="5200:super+space,6200:super+space" \
    SOLIUM_CAPTURE=/tmp/after.ppm SOLIUM_CAPTURE_AT=7400 dev/run-nested.sh
```

`SOLIUM_DRAG_AT` performs a drag through the real input path — the real grab,
the real hit-testing, the real layout scripts:

```sh
SOLIUM_DRAG_AT="8000:400,25>1200,500"    # at 8s, drag from (400,25) to (1200,500)
```

Aim the *start* at a titlebar. A press in a client's own area does not begin a
move grab, so a drag that starts there proves nothing — which it did, the first
time this was used. Solium's own frame starts the move itself; the drag
modifier (`Super`, or `Alt` under `SOLIUM_DRAG_MODIFIER=alt`) works anywhere in
the window.

It exists because dragging was the one interaction that needed a hand on a
mouse, and two bugs shipped there in three commits; the second froze the
machine. Both are reproducible from a script now: restoring the deadlock and
running the line above freezes the compositor on demand, CPU flat, which is how
the fix was confirmed to be a fix rather than a rearrangement.

`SOLIUM_CLICK_AT` and `SOLIUM_DRAG_AT` are not two spellings of the same thing,
and the difference cost an hour. `SOLIUM_CLICK_AT` calls `trigger_click`, which
is the *script* click handler — it never touches the input path. `SOLIUM_DRAG_AT`
goes through `synth`, `input::handle` and the real pointer, so it is the one
that exercises hit-testing, grabs and anything a surface does with a press. A
drag from a point to itself is a click:

    SOLIUM_DRAG_AT="6500:1436,472>1436,472"

Testing the Developer Tweaks panel with `SOLIUM_CLICK_AT` showed nothing
happening and looked exactly like the panel being broken, when the panel was
fine and the instrument was measuring something else.

`SOLIUM_TRIGGER_AT` has the same limit on the keyboard side: it runs the
handler a combination is bound to, and skips what a real key goes through
first. A binding under a Cyrillic layout, which answers to its key cap as well
as its keysym, needs a real key, and `SOLIUM_KEY_AT` (*Keys by name*, below)
presses one. With a second layout in the configuration's `keyboard` section,
switch to it first (`shift+alt_l` under `grp:alt_shift_toggle`), then press
the binding's keys, e.g. `SOLIUM_KEY_AT="3000:shift+alt_l,4000:super+q"`;
`wl-probe`'s `WL_PROBE_KEYBOARD` shows which layout the client was told is
live.

### Keys by name

`SOLIUM_KEY_AT` is the keyboard's `SOLIUM_DRAG_AT`: real key events, through
`input::handle`, the keyboard filter and xkb, so it does what a key on the
keyboard does. That is the only way to exercise what xkb does by itself --
Caps Lock, a layout switch through an option such as `grp:alt_shift_toggle`
-- since none of that is a binding `SOLIUM_TRIGGER_AT` could run:

```sh
SOLIUM_KEY_AT="3000:caps_lock,5000:caps_lock,6000:shift+alt_l" dev/run-nested.sh
```

A key is keysym names joined by `+`, case aside, as xkb spells them:
`caps_lock`, `num_lock`, `alt_l`, `return`, `a`. The keys go down in the order
written and up in reverse. `shift`, `ctrl`, `alt` and `super` are the
left-hand keys, and `control` and `logo` are taken for `ctrl` and `super`.
Each name is looked up in the live keymap, in any of its layouts, so `a` is
the same key with Russian live; a name no key of the keymap types means
nothing of that combination is pressed, and the log says so. The keyboard is the
nested session's own: the configuration's `keyboard` section, or the
`XKB_DEFAULT_*` environment.

## Checking an animation frame by frame

```sh
SOLIUM_TRIGGER_AT=9000:super+t SOLIUM_CAPTURE=/tmp/tile.ppm \
  SOLIUM_CAPTURE_AT=9000 SOLIUM_CAPTURE_FRAMES=16 SOLIUM_CAPTURE_INTERVAL=20 \
  dev/run-nested.sh
```

The frames land as `/tmp/tile-000.ppm` to `/tmp/tile-015.ppm`. The bounding box
of what is drawn, per frame, is the animation's curve. A good ease-out moves
most on the first frame and less on each one after, never jumps, and reaches
zero at the end. Measured across the modes on 2026-09-05:

| Mode | Per-frame movement (px) |
|---|---|
| window opens | 16, 12, 8, 4, 4, 0 |
| overview enters | 108, 84, 76, 56, 44, 32, 20, 16, 4, 4, 4, 0 |
| tiling arranges | 120, 92, 84, 56, 48, 32, 24, 16, 8, 4, 4, 4, 4, 4, 0 |
| scrolling arranges | 224, 100, 72, 64, 44, 32, 20, 16, 8, 4, 4, 4, 4, 4, 0 |

The window-open row predates `open.motion`, which is `outBack` now: it
overshoots, so a correct open grows past its size and settles back rather than
shrinking monotonically. A jump mid-sequence means a layout wrote geometry
without animating it; a stall means something is being recomputed per frame
that should not be.

## Checks

| Script | What it asserts |
|---|---|
| `dev/gate.sh` | formatting, clippy, tests, build, that the Lua configuration loads, and the QML GPU path against a real driver (see *The gate*) |
| `dev/app-check.sh <program>` | a client runs, draws, and provokes no protocol error |
| `dev/cursor-check.sh` | the pointer is visible over empty desktop |
| `cargo run -p wl-probe` | the protocols answer, a bar lands on the monitor it named, and a screenshot has the desktop in it the right way up |
| `dev/clipboard-check.sh` | copy and paste across the X11 boundary, all four ways |
| `dev/present-check.sh` | a `pivot` is the point the matrix leaves alone, a raised window is drawn in front, clicks follow the rect a window is drawn at without following the `z` it is drawn above, a genie on a second monitor lands on its target, and a tilted window's menu is drawn whole and in front of it |
| `dev/fence-check.sh` | skipping a capture's CPU fence wait (`SOLIUM_FENCE_WAIT=off`) changes no pixel |
| `dev/pulse-control.sh [env…]` | the renderer is not frozen: a focused window in the `pulse` style moves in 150 ms |
| `dev/effects-check.sh [section]…` | effects on screen, nested, one compositor per section, each judged beside `dev/pulse-control.sh`; `overlay` breaks the configuration and reloads it, and the problems overlay appears in the top-right corner while nothing else changes; `t0` draws a generated ring behind a window; `blur` blurs a video player's own pixels through a rule; `none` shows nothing captured and nothing run with no rules; `fail` shows every part drawn when every effect fails |
| `dev/install-check.sh [--no-build]` | `dev/install.sh` installs into a `DESTDIR` under `/tmp`: every file (the systemd units and the portal configuration included), the absolute `Exec`, the printed `sudo` lines, `--check` from the installed copy using its own `share/solium`, refusing while it runs (a session started during the build included), refusing to delete through a link, refusing `/` and a `DESTDIR` with a space, saying so when the check fails after the files are in place, keeping a unit or portal configuration of the user's own through an install and an uninstall, `solium-session` cleaning up after a stand-in Solium that crashed (and only then, and only once it has gone, and unsetting the variables after one that crashed before starting its target while no other desktop holds `graphical-session.target`) and refusing a second session while one runs, a reinstall saying when the login screen's session file is stale, and an uninstall that leaves nothing. `--no-check` skipping the installed binary's check. A system prefix (`--prefix /usr` and `/usr/local`): everything under the prefix and nothing in `XDG_CONFIG_HOME`, no `config.sha256` and no `sudo` line, `--session-dir` refused, and `--check` passing from the staged `/usr/share/solium` with a broken user configuration. The Fedora package: `dev/rpm/solium.spec`'s `%files` against that install both ways; its `License` naming every installed `.license`; each `Requires` and `Recommends` naming the package that has the file (the Qt QML modules the shipped QML imports with the Qt version clause, Xwayland, flock, xdg-desktop-portal and the backends `lilium-portals.conf` names, and foot); and the spec refusing to parse without `commit` and `commitdate`. See *Installing it* and *A Fedora package* |
| `dev/rpm.sh [--jobs N] [--image IMAGE]` | the package built from the commit checked out, unpacked without installing, passes `solium --check` with an empty configuration and takes its QML and Lua from its own `usr/share/solium`. See *A Fedora package* |

And the tools that measure rather than assert:

| Tool | What it is for |
|---|---|
| `dev/leak.sh <cycles> <windows>` | open and close windows in settled cycles, to tell a leak from a cache |
| `dev/soak.sh <minutes>` | churn a session for a long time and sample its memory, threads, descriptors and CPU; see *Soaking on a TTY* |
| `dev/bench.sh solium\|sway <seconds>` | what a nested compositor costs under a fixed client load, Solium and sway alike |
| `dev/preview` | the animation and effects engines in a browser; see *Tuning animations without running the compositor* |
| [`dev/wirecheck`](wirecheck/README.md) | the QML GPU path against a real driver, which the gate runs |
| [`dev/qtprobe`](qtprobe/README.md) | whether this machine's Qt can render on the GPU at all, outside Solium |

`cursor-check.sh` exists because the pointer was invisible for the whole life
of the project and nothing noticed: nested, the host session draws a cursor
over the top, so the only place the failure shows is the hardware.

`present-check.sh` exists because the wiring between a script and the screen is
the one part of a presentation transform that no unit test reaches. `z` and
`pivot` are read in `script.rs`, carried through `present::Frame`, and spent in
`render::by_depth` and `warp::mesh_part` — all of which are pure functions with
tests of their own. What sits between them is two field initialisers in the
`Command::Present` arm in `state/commands.rs`, and reverting *both* of those to
their defaults passes the entire suite. So this measures pixels instead: it
places one window at a known rect, turns it twenty degrees about two different
points, and reports where the corner went.

The corner is the whole difficulty, and "look at it" is not a weaker version of
this check — it is not a check at all. A rotated window's top-left is not the
top-left of anything in the picture: it is one particular vertex of a quad, and
which one cannot be read off the image. Drop the `pivot` and the frame that
asked to turn about its own corner comes back **byte for byte identical** to the
frame that asked to turn about its centre — `cmp -l` counts zero differing
bytes, against 382,961 between the two correct frames. There is no visual signal
to miss because there is none to have. So the client prints a marker block at
its home position, the marker travels with the corner, and
`present-check/measure.py` names the quad vertex nearest it.

The last two claims are the ones no screenshot can make, and they are two halves
of the same rule. `Solium::window_under` (`state/hit_test.rs`) walks panes in
stacking order and asks whether each one, as it is drawn at that moment
(`drawn_at`), owns the point:

* **`z` never enters the walk.** A window raised over the one covering it is
  drawn in front and still does not take its clicks. Checked with *three*
  windows, the third parked out of the way holding focus — with only two, the
  window that should take the click is already focused, "it worked" and "nothing
  happened" produce the same log, and the check rests entirely on the compositor
  emitting a focus event for a no-change refocus.
* **`rect` does.** A window presented somewhere else takes clicks where it is
  drawn and not where it lives. This is the likelier regression by far —
  `outer.contains` is the obvious-looking simplification, it inverts both clicks,
  and it passes every other check in here.

The last claim, `menu`, needs a client with a popup on cue, so its window is
`wl-probe`'s (`WL_PROBE_POPUP`) rather than kitty's, and it needs `wl-probe`
built beside `solium`. The window is tilted four degrees, and its menu, which
reaches past the window's bottom-right corner, must be drawn whole: at least
the 160x120 pixels of the untilted menu. A warped window's menu is a capture of
its own drawn in front of the window's warp; drawn inside the window's capture,
it was cut at the window's edge and lay under its frame.

    cargo build -p wl-probe
    SOLIUM_CHECK_DIR=/tmp/present ./dev/present-check.sh   # keep the frames

Kept out of `gate.sh` deliberately: it needs a host compositor to nest in and a
client to open, and a gate that cannot run headless is a gate that gets skipped.

`dev/fence-check.sh` captures the same two windows with the capture's fence
wait on twice, off once, and tilted a degree more once, and fails unless the
first three are byte-identical and the fourth is not. Both windows are rounded
and both are tilted, so a warp's capture runs for each on every frame: a
rounded window is no longer captured for its corners, so only a warp still
captures. Every run
sets `SOLIUM_RECAPTURE=always`: the windows' content is fixed, so a kept capture
would be drawn once at startup and the fence wait would have nothing to skip on
the frames compared. Forced, every capture is drawn, and waited for or not, on
every frame. A race that shows once in several hundred frames is wirecheck's to
catch (cases 11c and 11d), not this.

`dev/pulse-control.sh` runs one focused window in the `pulse` style nested and
fails unless two captures 150 ms apart differ: the known-animating control a
nested capture is judged beside. Its arguments are extra environment for the
compositor; `dev/pulse-control.sh SOLIUM_PANE=none` is its own fail-first, since
nothing on screen moves then.

`dev/effects-check.sh` proves effects on screen, nested: each section starts a
compositor of its own with `dev/effects-check/<section>.lua` as its whole
configuration and a configuration directory of its own for effect folders,
captures, and judges the captures with `dev/effects-check/judge.py`. The pulse
control runs first, and a failure there fails the whole check, because a frozen
renderer makes every capture prove nothing. Run it under `timeout`, from a
nested-capable session:

    timeout -s TERM 300 dbus-run-session -- dev/effects-check.sh overlay
    SOLIUM_CHECK_DIR=/tmp/effects ./dev/effects-check.sh   # every section, keep the frames

`overlay` captures the desktop, appends a line that is not Lua to the
configuration, reloads it, and captures again: the top-right corner must have
changed (the problems overlay, listing `…/init.lua:6: …`) and the bottom half
must not. Leaving `require("problems")` out of `dev/effects-check/overlay.lua`
is its fail-first.

`t0` places one window at a known rectangle twice, once plain and once with a
rule putting the `ring` fixture (`crates/solium/tests/fixtures/effects/ring/`,
which reads only `shape`) behind its client: the 12 px band left of the window
must change and the window itself must not, and the trace
(`SOLIUM_TRACE`, read by `judge.py trace` and `judge.py quiet`) must show the
ring run once or twice and never in the last 2 s before the capture. A
`judge.py` region number ending in `px` is pixels, not a fraction of the frame.
A build whose `render::run_slots` skips a generated chain is its fail-first:
nothing is drawn around the window, and the ring runs 0 times.

`blur` opens a still terminal on the left and a video player on the right
(ffplay's `testsrc2` at 60 fps, or a kitty printing the time when ffplay is
missing), four times: with no rule, with the shipped `blur` blurring the
player through `{ "blur", source = "self" }` matched by its title, with it
blurring the still terminal, and that again with `SOLIUM_RECAPTURE=always`.
The player's edge energy (`judge.py edges`) must fall to under half while the
terminal stays byte-equal, and the player's chain must run. The still
terminal's chain must run only on passes that captured it (`judge.py only`:
its own few commits as kitty starts and loses the focus) and on none of the
last 3 s, while the player keeps the passes coming (`judge.py since`); with
`SOLIUM_RECAPTURE=always` it runs on every pass, the control that shows the
count means something. Its fail-firsts: the rule's title misspelled in
`blur.lua` (the player is not blurred, its chain never runs), and a build
whose `SlotState::needs_run` always answers yes (the still window's chain
runs on every pass, most of them capturing nothing).

`none` opens the same two windows with no rule: the trace must show no pass
that captured and no chain run (spec §8.4); adding a rule to `none.lua` is
its fail-first. `fail` opens one window in the `top` style twice, once plain
and once with a rule naming the `fail-compile` fixture (lint-clean, refused
by every GPU) in every slot of every part of it: a problem naming it must be
logged, and the frame must be the plain one within 1. Its fail-first is a
`fail-compile` frag that compiles and draws red, with the check that
something failed set aside: the frames differ.

`clipboard-check.sh` runs its X11 half in a container, so the host needs no
`xclip`. **Run it more than once.** The bug it was
written for failed about one time in three, so a single green run proves
nothing — which is exactly how it nearly shipped.

### wl-probe

`wl-probe` is a Wayland client, and exists because a compositor cannot test its
own protocol support from the inside. "The global is advertised" is a different
claim from "a client that uses it gets the right answers", and the gap between
those two has been where every protocol bug this week actually lived. Real
applications make fine oracles — Firefox's `WAYLAND_DEBUG` log is excellent —
right up until nothing installed happens to use the protocol you just added.
Firefox never binds `wp_presentation`, so nothing installed could say
whether presentation feedback worked at all.

    ./target/debug/solium &
    WAYLAND_DISPLAY=wayland-1 cargo run -p wl-probe

The socket name is chosen when Solium starts, and `wayland-1` is only the usual
answer: the log line with `socket=` says which, and `dev/run-nested.sh` prints
it. `wl-probe` exits non-zero if anything it asked for went unanswered, so it
can be a gate.

It also anchors a bar to the top of every monitor with an exclusive zone and
checks that each was configured to *that* monitor's width — a layer surface
that landed on the wrong screen comes back with the wrong number, and there is
no way to see that from inside the compositor. Two knobs:

    WL_PROBE_BAR=DP-2    only that monitor. One bar on one screen can tell
                         "each monitor's own work area" from "every monitor's";
                         a bar on all of them cannot.
    WL_PROBE_HOLD=10     keep the bars up for ten seconds, so the compositor's
                         own frame can be captured — an exclusive zone is a
                         claim about where everybody *else's* windows go, and
                         no client can see those.

That check found that layer surfaces had never worked at all: they were mapped
and never sent an initial configure, and a client may not attach a buffer until
it has been configured once. Every bar and every dock was invisible, for as
long as `layer.rs` has claimed that any existing panel works.

`WL_PROBE_SHOT=/tmp/shot.ppm` writes the capture it took out as a binary PPM,
because "some pixels were not zero" and "that is my desktop" are different
claims and only one of them can be checked by a program. It checks what it can:
that the buffer is the size that was asked for, that it holds more than one
colour, and — the one thing no event can tell a client — that it is the right
way up, by looking for the top-anchored bar in the top rows. That check earned
itself immediately: the first capture came back upside down, which is perfectly
legible and reads as a compositor bug rather than a row-order one.

It also reports each monitor's scale and logical size as a client sees them,
which is the only place that can be checked from — and the second thing it
caught was itself: it compared a layer surface's configure against the
monitor's *mode* and called a correct 2x answer a bar on the wrong monitor.
Layer sizes are logical. A probe that is wrong about the protocol is worse than
no probe, so its expectations are worth as much scrutiny as the compositor's
behaviour.

With `WL_PROBE_HOLD` set it also reports where the compositor said the pointer
was, in the bar's own coordinates. Drive the pointer somewhere known and the
two numbers should match:

    SOLIUM_DRAG_AT="4000:400,20>400,20" ./target/debug/solium &
    WL_PROBE_HOLD=7 WAYLAND_DISPLAY=wayland-1 ./target/debug/wl-probe

That is the only way to check it: the compositor works in its own coordinates
and subtracts the surface's corner to get the client's, and subtracting the
wrong corner is invisible from the inside. It was subtracting the wrong corner.
Every bar and dock was told the pointer was at `0,0` no matter where it really
was — so every button on every panel would have missed, and the one at the
top-left corner would have swallowed every click.

`WL_PROBE_LOCK=<seconds>` locks the session, covers every monitor in a green
nothing else draws, holds it, and unlocks. Kept out of the ordinary run on
purpose: a check that locks the screen partway through a gate is a check that
locks the screen of whoever ran the gate. Two knobs:

    WL_PROBE_LOCK_SKIP=DP-2   give every monitor a lock surface except that
                              one. It must go blank, not show the desktop —
                              and with one lock surface per screen that is
                              the only failure the picture can distinguish.
    WL_PROBE_LOCK_ABANDON=1   leave without unlocking, as a crashed lock
                              program would. The session must stay locked.

Without a lock client installed there is otherwise no way to exercise the
protocol at all: a compositor cannot lock itself, and every
question worth asking about a lock screen is a question about what a *second*
process can see. Capture a burst while it runs
(`SOLIUM_CAPTURE=/tmp/lock.ppm SOLIUM_CAPTURE_FRAMES=<n>`) and read the frames:
once the desktop has gone, nothing of the session may appear in any of them,
before the client's colour or after it.

The ordinary run also reports the keyboard: the layout names in group order,
which one is active, and the repeat rate. That is the only way to check a
compositor's keyboard from outside it — the keymap arrives as a file descriptor
and no event describes what is in it. Two knobs:

    WL_PROBE_KEYMAP=/tmp/km.xkb   write the keymap out. Forty thousand bytes
                                  behind a file descriptor is the compositor's
                                  most opaque answer, and "the layouts look
                                  right" is a different claim from "here is
                                  what it sent".
    WL_PROBE_KEYBOARD=10          map a window, take focus, and report every
                                  layout change the compositor announces.
                                  Pair it with a scripted binding:

        XDG_CONFIG_HOME=/path/to/config \
            SOLIUM_TRIGGER_AT="6000:super+shift+k" ./target/debug/solium &
        WL_PROBE_KEYBOARD=10 WAYLAND_DISPLAY=wayland-1 ./target/debug/wl-probe

The compositor switching its own layout and the *client being told* are
different claims, and only the second one matters: a layout that changed and
was not announced is a keyboard that types the wrong letters.

Reading the keymap took three wrong turns, all of which looked right, and they
are written down in `layouts_in_keymap_text` because each would cost the next
person the same hour. The short version: the fd's offset is at the end, so
`read` returns zero bytes and looks exactly like a compositor that sent
nothing — clients are told to mmap it, which is why every real toolkit works.
The `xkb_symbols` section name looks like a parseable recipe and is a trap.
And libxkbcommon writes `name[1]=`, not the `name[Group1]=` that xkbcomp writes
and every example online shows.

`WL_PROBE_IDLE=1` runs the whole idle sequence against a real window: go idle
on a short timeout, take an inhibitor and come back, hold it through more than
the timeout and stay awake, drop it and go idle again. The sequence is the
check, not any one event in it — a compositor that advertises the inhibitor and
ignores it passes the first step, and the third step looks exactly like a
compositor whose timer stopped. Two more, both opt-in because of what they do:

    WL_PROBE_IDLE_LOCK=1      lock the session with the inhibitor still held.
                              It must go idle anyway: a player left running
                              behind a lock screen would otherwise keep the
                              machine awake all night showing a lock screen.
    WL_PROBE_IDLE_HIDDEN=8    hold an inhibitor and wait for something else to
                              hide the window. Pair it with a workspace switch:

        SOLIUM_TRIGGER_AT="6000:super+2" ./target/debug/solium &
        WL_PROBE_IDLE=1 WL_PROBE_IDLE_HIDDEN=8 WAYLAND_DISPLAY=wayland-1 \
            ./target/debug/wl-probe

That last one earned itself on the first run. The compositor was asking whether
the window's *slot* was on a monitor, and a workspace switch does not move a
window's slot — it slides the window away from it with a presentation
transform. So a video on a workspace nobody was looking at went on holding the
machine awake, and no amount of reading the inhibitor code would have shown it.

`SOLIUM_DRAG_AT` fires during a lock too, and that combination is worth keeping:
it is what found that a locked screen would still let a drag resize a window
underneath it. The window was measured before the lock and after the unlock,
which is the only way that bug is visible — while locked, nothing of it is on
screen to see.

Four more modes. `WL_PROBE_WINDOWS=<seconds>` maps two windows, one framed
by the compositor and one that draws its own, and holds them up to be looked
at; with `WL_PROBE_FULLSCREEN=1` the first asks for fullscreen a second in, so
the change can be photographed. `WL_PROBE_POPUP=<seconds>` maps one window and
a menu reaching past its bottom-right corner, each in a colour nothing else
draws, for `dev/present-check.sh`'s `menu` case. `WL_PROBE_CONNECT_ONLY=1`
connects, counts the globals and leaves, for hunting what a client that never
draws costs.

## Hotplug, and how to test it without a cable

Monitors arriving and leaving
([#43](https://github.com/Lilium-Linux/solium/issues/43)) work, and were
checked on the hardware, with real cables, before #43 was closed. What that
testing found is fixed, and how the first hardware test failed is below.

On the hardware there are two ways to exercise it, and the second does not
involve reaching behind the desk:

1. Turn a monitor off at its own power switch, or unplug it, with the session
   running. The log should say `monitor gone`, the remaining screen should
   take the windows, and turning it back on should say `monitor arrived`.
2. Set `enabled = false` on a monitor in `config.monitors` and press
   `super+shift+r`. That goes through *the same* `resync_screens` -- an
   `enabled` change is an unplug as far as everything downstream is concerned
   -- so it exercises the whole path from a keyboard.

The second one existing is why the two share a path rather than each having
their own: a configuration reload is something people do at a desk many times
an evening, and unplugging a monitor is something they do once. The path that
gets exercised is the one that works.

Nested, the DRM half cannot run: the nested backend has no connectors, and a
virtual one (`vkms`) needs a kernel module loaded as root. The half that broke
on the hardware both times — what happens to a window whose monitor went —
runs nested exactly as it does there, because both backends end in the same
`settle_monitors`. `SOLIUM_OUTPUTS_AT` takes a nested monitor away and gives it
back:

```sh
SOLIUM_OUTPUTS=2 SOLIUM_OUTPUTS_AT="5000:1,9000:2" dev/run-nested.sh
```

The part of the DRM half with reasoning in it, `tty::gone`, which decides which
screens went, has unit tests of its own.

The first hardware test failed, and how it failed is worth keeping: the uevent
arrived three times, `the connectors changed` is in the log three times, and
nothing else happened. `get_connector` was being asked with `force_probe =
false`, so the kernel handed back the answer it had cached before the cable
moved. The uevent is the kernel saying *come and look*; a compositor that comes
and looks at its own notes learns nothing. There is now a log line for exactly
that shape -- `the connectors changed and the set of screens did not` -- because
from the log it was indistinguishable from the event never arriving.

What to watch for, because these are the ways it goes wrong quietly: a
`wl_output` that stays in `wayland-info` after the monitor is gone; windows
left on a screen nobody can see; a bar that does not come back when the monitor
does; and the second monitor failing to light when it is moved from one port to
another, which is the case where dropping has to happen before adding because
there are fewer CRTCs than connectors.

### Two monitors, without a second monitor

    SOLIUM_OUTPUTS=2 dev/run-nested.sh

Two monitors side by side in the nested window, each with its own layer map,
work area and render pass, each drawn into a texture of its own — which is what
having its own scanout buffer means — and each with its own instance of every
`sol.surface` declared on it, the hosted shell's included (`shell.on` in
`config.lua` says which monitors), each reading its own `Solium.monitor`, so
`SOLIUM_OUTPUTS=2 dev/run-shell.sh <shell-dir>` shows a shell on two monitors.
`SOLIUM_OUTPUTS_AT` builds or drops instances as monitors arrive or go, and
resizing the nested window, which changes the monitors' sizes without adding
or removing one, gives each surface an instance on every monitor it now
covers. Every per-output path runs for each of them; what it cannot simulate
is a second *pipeline*, one refresh rate and one page flip per screen, which
is `tty.rs`'s half of the problem.

It exists for the same reason `cursor-check.sh` does, and it earned itself
immediately. The window-resize handler only ever resized the first output, so
the left screen took the whole window's width and covered the right one, and a
pointer over the second monitor was answered with the first — a window opening
on the screen you are not looking at. That would otherwise have been found on a
TTY, where nothing can be read and each attempt costs a session.

The nested monitors are called `winit-1`, `winit-2` and so on. Give one of
them a scale in `user.lua` and you have a HiDPI screen to look at without
owning one:

```lua
return {
    monitors = {
        { name = "winit-1" },
        { name = "winit-2", scale = 2, right_of = "winit-1" },
    },
}
```

Each monitor takes its share of the window's *pixels* and its logical size is
that share divided by its scale, so you are looking at the pixels that monitor
would scan out rather than a picture of them. A 2x monitor's half of the window
holds half as much desktop at twice the detail.

Combine it with the scripted input knobs to place windows on a chosen screen:
`SOLIUM_DRAG_AT` moves the pointer through the real input path, and the active
monitor is the one the pointer is on.

    SOLIUM_OUTPUTS=2 \
      SOLIUM_DRAG_AT="1000:100,500>400,500;3500:400,500>1200,500" \
      SOLIUM_LOADING_AT="1500:one,2500:two,4000:three,5000:four" \
      SOLIUM_TRIGGER_AT=6000:super+t \
      SOLIUM_CAPTURE=/tmp/tile.ppm SOLIUM_CAPTURE_AT=8000 \
      dev/run-nested.sh

Two windows tiled on each screen, in one capture, with no hands.

### Turning screens off

Screens can be turned off without being taken away
([#54](https://github.com/Lilium-Linux/solium/issues/54)): by a client over
`wlr-output-power-management` (`wlopm`, or `swayidle` driving it), by
`sol.monitor_power` from a binding, and by the idle blank after
`idle.screens_off_after`. Nested there is no display to power off, so a
monitor's share of the window is drawn black once and then not drawn, its
client is told `off`, and the log says it was blanked. All of it can be tried
in a window that way. The DRM half — a black frame, then the CRTC cleared, and
a modeset back on the next frame — has run on the hardware at `dfc95ce`: the
screens going off after `idle.screens_off_after` works on an NVIDIA RTX 3070
desktop and on a Microsoft Surface Pro 7 (Intel Ice Lake), both on Fedora 44.
All three ways of asking end in `Solium::set_power` (`power.rs`).

## Tuning animations without running the compositor

```sh
dev/preview
xdg-open crates/animation/preview/preview.html
```

Plain boxes animating through the scenarios the compositor actually has —
tiling, scrolling, overview entering and leaving, a window opening and a window
closing — on a mock output in Solium's own coordinates. Curve, duration,
playback speed, the arrangement's own numbers (gap, split ratio, column width)
and the spring's stiffness, damping and throw are all live. `dev/preview` also
writes `crates/effects/preview/preview.html`, which shows where each vertex of
a window goes for a named deformation, such as the genie.

**Deliberately plain.** The page is a test harness, not a mockup: recreating the
compositor's chrome here would be a second copy of its design, drifting from the
real one, and motion reads more clearly without decoration anyway.

**The engines are compiled to WebAssembly and called from the page**, so the
curve tuned in a browser is the code that will move real windows. Verified
rather than asserted: the same calls through wasm and through native Rust agree
to six decimal places. A reimplementation of the curves in JavaScript would
drift the first time either side changed, and the drift would be invisible —
the page would still animate plausibly.

That includes the geometry. The tiling and scrolling scenarios are arranged by
`crates/layout`, the same arrangements the compositor's scripts ask for, also
compiled to WebAssembly. What the page still owns is where a window starts and
ends in the overview and open and close scenarios.

Curves live in `crates/animation`, which depends on nothing so it keeps building
for the browser. Add one there and it is available to scripts by name
(`sol.animate{ easing = "spring" }`) and to the preview at once.

## Running a shell inside Solium

A shell is hosted in the compositor's own QML engine, as configuration — see
[docs/shell-boundary.md](../docs/shell-boundary.md). To work on one, point
`dev/run-shell.sh` at its checkout:

```sh
dev/run-shell.sh <shell-dir>                  # its shell.qml
dev/run-shell.sh <shell-dir> dock/Dock.qml    # one file of it
SOLIUM_SHELL=<shell-dir> dev/run-shell.sh
```

The checkout is required and only ever read. The scene goes in as
`SOLIUM_SHELL_SCENE`, so your own `shell.scene` is left alone, and editing
anything in the checkout reloads the scene within half a second.
`SOLIUM_SHELL` is read by the script, not by the compositor.

`solium --check-qml <file>` loads one file without starting a compositor and
prints `ok` or the errors Qt reported — the quick way through a chain of "type
X unavailable" errors while writing a shell. It exits with 1 when the file
does not load.

## QML on the GPU

QML renders on the GPU by default. Qt comes up on its OpenGL scene graph and
renders each scene into a dmabuf the compositor allocated through GBM, rather
than rasterising it on the CPU into a `QImage` the compositor then uploads.
Software is the fallback. A machine where the probe below fails still gets a
desktop; one where the probe passes and the GPU then fails inside the
compositor gets one from its next start — see *When the compositor's own start
fails*.

| Mode | What it does |
|---|---|
| `auto` | The default. On the hardware, the GPU pre-flight runs first in a short-lived child process — this binary, as `--probe-qml-gpu` — and QML renders on the GPU if it passes, in software if it fails or takes longer than `qml.probe_timeout` (5000 ms, after which the child is killed). Nested, software: winit hands the compositor no GBM device. |
| `gpu` | The GPU, with no child probe. Nested this draws no QML at all, for the same reason. |
| `software` | Qt's software rasteriser. |

Asked for in this order, and the first that says anything decides:

1. `--qml <mode>` or `--qml=<mode>`, after the backend: `solium --tty --qml software`
2. `SOLIUM_QML=<mode>`
3. `SOLIUM_QML_GPU` set to anything, the older spelling of `gpu`
4. `qml = { renderer = "<mode>" }` in the configuration (`config.lua` or `user.lua`)
5. `auto`

A value that is not a mode warns and means `auto`, at the level it was written:
`SOLIUM_QML=sofware` does not fall through to the configuration. The
configuration can hold this because it is read before the first scene starts
Qt. It is read once, though: Qt fixes its scene graph for the life of the
process, so a reload does not change it and a restart does.

**Why a child process.** Qt picks its scene graph inside `QGuiApplication` and
there is no way back. The pre-flight — a render node, the KMS config that keeps
Qt off the card, a real allocation, a real QML render, the import and the fence
— used to run only inside the compositor, after that choice was made, so a
failure left no software path to fall back to. Run in a child, a failure costs
the child. Once it passes, the compositor runs the same pre-flight again in
its own process, as it always did.

**When the compositor's own start fails.** A passing probe is not proof. The
compositor's own pre-flight runs later, inside the compositor and once Qt is
committed, and the probe did not share its conditions: the child holds none of
the compositor's seat, card or display. If it fails there, the session cannot
fall back: scenes do not draw, Solium's own QML arrow for the pointer
included. If Qt aborts, the compositor goes with it. So under `auto` the
compositor writes `$XDG_STATE_HOME/solium/qml-gpu-pending`
(`~/.local/state/solium/`) just before committing Qt, and removes it once the
pre-flight passes. A failure replaces it with `qml-gpu-failed`, holding the
reason, and a pending file still there at the next start — the compositor
stopped inside the start — counts as a failure too. The next start of the same
build then renders in software without probing, and says so once, naming the
file. Delete the file to let `auto` try again, or force the GPU with `--qml
gpu`. A rebuild tries again by itself, because each file begins with the build
that wrote it: its version, its binary and when that was written. A machine
where this happens loses one start, not every login. `gpu` and `software`
neither read nor write these files.

**Which one is running.** A line at startup begins `QML renderer:` and says
which, and why. When the compositor's own GPU start then goes wrong it logs
another, and the last one is the one that stands:

```
INFO solium::qml::renderer: QML renderer: gpu, the probe passed from="qml.renderer" probe_ms=160 said="gpu: Qt rendered QML into a buffer allocated on /dev/dri/renderD128, fenced"
INFO solium::qml: QML on the GPU: Qt rendered into a buffer we allocated node=/dev/dri/renderD128 fenced=true
```

A fallback is a single warning with the reason and both ways out:

```
WARN solium::qml::renderer: QML renderer: software, because the GPU probe failed: no DRM render node could be found (exit status 1). `--qml gpu` or SOLIUM_QML=gpu forces the GPU; `--qml software` or SOLIUM_QML=software skips the probe
```

A compositor start that fails after the probe passed, and the start after it:

```
ERROR solium::qml: QML renderer: gpu, and it does not work: <reason>. Scenes will not draw in this session. The next start of this build renders QML in software, as recorded in ~/.local/state/solium/qml-gpu-failed
WARN solium::qml::renderer: QML renderer: software, because this build's last GPU start in the compositor did not work: <reason>. Delete ~/.local/state/solium/qml-gpu-failed to let auto try the GPU again, or force it with `--qml gpu` or SOLIUM_QML=gpu
```

**The probe on its own.** It needs only a render node, not DRM master, so it is
safe to run from inside another desktop session:

```
$ ./target/debug/solium --probe-qml-gpu; echo $?
gpu: Qt rendered QML into a buffer allocated on /dev/dri/renderD128, fenced
0
```

Exit 0 and one line on stdout when the GPU path works here; non-zero and one
line on stderr saying why when it does not. `RUST_LOG=info` shows its steps.
Measured on 2026-09-29 it took 155–180 ms, once 456 ms on a cold first run,
and that is what `auto` adds to a hardware session's startup.

`QML on the GPU: Qt rendered into a buffer we allocated`, and the probe's
`fenced`, mean a buffer was allocated, imported into Qt's context as a texture,
drawn into by real QML, and fenced with a `sync_file` the driver exported.
`fenced=false` — the probe's `unfenced` — is also a pass: the driver declined to
export a fence and the host waited with `glFinish` instead, which costs a stall
and nothing else.

Five things worth knowing about the GPU path on a TTY:

* **Qt must be kept off the card node.** Solium writes
  `$XDG_RUNTIME_DIR/solium-eglfs-kms.json` naming the *render* node and
  `"headless"`, and sets `QT_QPA_EGLFS_KMS_CONFIG` before Qt starts. Without
  it eglfs opens `/dev/dri/card1`, and because Qt starts while the scripts load
  — before `open_gpu` — the kernel hands *Qt* DRM master, logind's `SetMaster`
  then fails, and the only thing said about it is Smithay's `unable to become
  drm master`, which is benign noise every other run. A black screen on a TTY
  with nothing to read. Both JSON keys are needed: `device` alone fails the
  plugin with `drmModeGetResources failed (Permission denied)`, `headless`
  alone still opens the card.

* **Qt is told not to install signal handlers, and it matters more than it
  sounds.** eglfs builds a `QFbVtHandler`, which takes `SIGINT`, `SIGTERM`,
  `SIGCONT` and `SIGTSTP`. Those handlers do not exit; each writes a byte to a
  socketpair, and the `_exit(1)` happens whenever Qt's event queue is next
  drained — here, `qml::tick`'s `processEvents`, which `render::prepare` only
  reaches when something wants a frame. So a `SIGTERM` to an idle compositor
  does nothing at all, and the *next redraw* turns it into an `_exit(1)` from
  inside a render, skipping every Rust destructor: the libseat session, the DRM
  master release, the VT restore. Whether it happens depends on whether anything
  asked for a frame afterwards, which is not a property you want in the path
  that puts your console back.

  `QT_QPA_NO_SIGNAL_HANDLER=1` is set beside the `QT_QPA_EGLFS_*` variables and
  removes it: measured, the process dies on `SIGTERM` with the knob on, both
  idle and while drawing. `QT_QPA_ENABLE_TERMINAL_KEYBOARD=1` goes with it —
  despite the name it tells Qt to *leave the console keyboard alone*, which it
  otherwise mutes when stdin is a terminal and un-mutes from a destructor this
  process never runs.

  `Ctrl`+`Alt`+`Backspace` was never affected either way: it is decided in
  `input/mod.rs` from Solium's own libinput devices, and
  `QT_QPA_EGLFS_DISABLE_INPUT=1` means eglfs creates no input handlers to
  compete with them.

* **Qt's diagnostics go through `tracing` now, and needed to.** As of `870aacc`
  `host.cpp` installs a `qInstallMessageHandler` that forwards to
  `solium_qml_log_from_qt` in `qml.rs`, so QML binding errors, `console.warn`
  and every `qWarning` in Qt and in the host come out in the compositor's own
  log with Qt's logging category as a field. `QT_FORCE_STDERR_LOGGING` is no
  longer needed for any of it.

  What it was before is worth keeping, because it is why four failures in the
  rice spike were silent. Qt's default handler picks its destination from
  whether stderr is a console: **stderr when it is, journald when it is not** —
  measured both ways on this Fedora Qt 6.11. So a TTY session, where stderr is
  the VT the compositor has just covered, printed the whole diagnostic half of
  the host onto a screen nobody could read and into no log at all, while a
  `2>&1 | tee` run quietly put it in journald where nobody was looking. Neither
  ever reached `session.log`.

  `RUST_LOG` now gates Qt as well, under the `solium::qml` target like the rest
  of that module. One consequence: a QML `console.log` is a `QtDebugMsg`, so it
  needs `RUST_LOG=debug` — **and** `QT_LOGGING_RULES='qml.debug=true'`, because
  Qt's own category filter drops it before the handler is called. `console.info`
  and above need neither.

* **A one-frame probe cannot see the bug that matters.** Qt's
  `QOpenGLContext::currentContext()` is a thread-local Qt sets in its own
  `makeCurrent`. The compositor takes the thread back with a raw
  `eglMakeCurrent`, which Qt never sees, so that thread-local goes *stale rather
  than null* — and `QRhiGles2::ensureContext()` reads exactly it, concludes its
  context is already current, and issues the whole frame against whichever
  context really is: the compositor's.

  Nothing fails when this happens. `beginFrame`, `sync`, `render` and `endFrame`
  all return, the fence is a real `sync_file` and signals in under a
  millisecond, and the dmabuf stays full of zeros. Measured on a 64x64 scene:
  16384 of 16384 bytes zero with the compositor's context current across the
  render, 0 of 16384 bytes wrong with Qt's. The *first* frame after a scene is
  built works either way, because `initialize()` left Qt's context current and
  nothing has taken it yet — so a probe that builds a scene, renders once and
  reads the buffer passes while every frame after it draws nothing.

  `clear_stale_current_context` in `host.cpp` is the fix: `doneCurrent()` on
  whatever Qt believes is current, when EGL says otherwise. `restore` in
  `qml/paint.rs` is the other half, and neither works without the other.

* **A GPU scene's buffer is not stored the way you would guess.** QRhi leaves an
  OpenGL texture render target in the framebuffer's own orientation, origin
  bottom-left, so the scene's *top* row lands in the buffer's *last* row. A
  dmabuf is top-down unless it says otherwise and ours does not, so the scene
  comes out upside down. `host.cpp` calls
  `QQuickRenderTarget::setMirrorVertically` (`mirror_for_the_compositor`) on
  every GPU render target for that reason — the one a scene starts with, and
  the ones `solium_qml_scene_resize` and `solium_qml_scene_rebind` build again,
  which are easy to miss.

  Do not try to correct it on the compositor's side. Smithay's `y_inverted`
  texture flag negates the texture matrix's y row without the matching
  translation, and a `Transform::Flipped180` on the render element mirrors
  within the element's *logical* size while its source rectangle is in device
  pixels — right at scale 1, wrong on every scaled monitor.

## Running it on a TTY, as a real session

The compositor picks its backend from the environment: nested when there is a
compositor to nest in, on the hardware otherwise. `--tty` asks for the
hardware wherever it is on the line, and a flag Solium does not know starts
nothing: it is refused with exit status 2. `solium --help` lists the flags.

**First, from your desktop, check what the hardware offers.** This opens the
card read-only and takes no DRM master, so it is safe to run inside a running
session:

```sh
target/debug/solium --probe
```

It names the seat and the GPU, then lists each connector: for one that is
connected, how many modes it offers, the one it would choose, its size, dpi and
the scale it would pick, and then every mode, in the form the configuration's
`mode` takes.

**Then, on a free TTY.** `Ctrl`+`Alt`+`F3` (or any free one), log in, and:

```sh
cd path/to/solium
./target/debug/solium --tty
```

Leave `SOLIUM_TERMINAL` unset, or name kitty or foot: see *Terminals* below for
why not konsole.

No `sudo`: the session, the GPU and the input devices are all opened through
libseat, and a compositor that needs root is one nobody should get used to
running.

**Getting back.** `Ctrl`+`Alt`+`F1` or `F2` returns to your desktop session;
Solium keeps running on its own VT until you switch back and stop it.
`Ctrl`+`Alt`+`Backspace` stops it outright, unless the screen is locked.

Both of those are Solium's own doing, and that is not a detail. Once libseat
puts the VT into graphics mode the *kernel* stops acting on `Ctrl`+`Alt`+F-keys,
so a compositor that does not handle them itself cannot be escaped from the
keyboard at all — which is how the first run of this backend ended in a reboot.

Two more things stand between you and that: if libinput reports no input
devices within twenty seconds, Solium stops on its own rather than hold a
display nobody can talk to; and from another VT, `pkill -x solium` always ends
it. The first `SIGTERM` stops it cleanly. A Solium that cannot stop, because
something inside it has stopped answering, ends itself five seconds later
(`session.stop_timeout`), or at once on a second `pkill -x solium`.

That second one is worth a qualification, because it is a last-resort escape
and you are reading it before taking a VT. It holds because Solium's handlers
never wait on the event loop: the second signal ends the process from the
handler itself, and the timeout from a thread of its own
(`signals::tests::the_same_signal_twice_ends_a_process_that_cannot_stop` and
`a_process_that_cannot_stop_ends_when_its_stop_timeout_has_passed`). With QML
on the GPU it holds only because Solium sets `QT_QPA_NO_SIGNAL_HANDLER` before
starting Qt — without it eglfs installs its own `SIGTERM` handler, and the
process then neither dies nor cleanly survives. See *QML on the GPU*. On a
build that predates that, or one where `QT_QPA_PLATFORM` was set from outside,
reach for `pkill -9 -x solium`.

**Reading what happened.** A session started with `--tty` as its first
argument writes to `~/.local/state/solium/session.log` (under
`$XDG_STATE_HOME` when that is set) as well as to the terminal, because the
terminal is underneath the compositor and cannot be read while it runs. A bare
`solium` on a VT also takes the hardware, and writes no log file. The log is
appended to, so the log of a run that went wrong survives the run that was
meant to fix it — and, being under `state`, it survives a reboot.

**Terminals.** `SOLIUM_TERMINAL` picks one; otherwise `lua/init.lua` takes the
first that is actually installed. That fallback exists because naming a
terminal that is not installed looks, from the keyboard, exactly like the
binding being broken — which is how the first hardware session went.

The order is deliberate: plain Wayland terminals (kitty, alacritty, wezterm,
foot) come before konsole, and xterm is last. A KDE app started under Solium
sits for around twenty seconds before its window appears — close enough to
the D-Bus activation timeout to be worth naming, since KDE apps ask for portal
and session services that are not running here and wait for them to time out.
`vkcube` and kitty both map in a second or two, so this is the app waiting,
not the compositor failing to map it. konsole stays on the list as a fallback
for a machine that has nothing else, never as a preference.

**What the hardware has and has not been through.** Hotplug has (see *Hotplug*
above). So has `dfc95ce`, on an NVIDIA RTX 3070 desktop and on a Microsoft
Surface Pro 7 (Intel Ice Lake), both on Fedora 44: QML on the GPU and its
animations, the screens going off when idle (see *Turning screens off*),
`swaylock` locking the session, and the Caps and layout pill with no
configuration. On the Surface Pro 7 the screen also went off when the lid was
closed, though Solium itself has no lid handling. Touch reaches a client's
window (Firefox), but nothing Solium draws reacts to it: frame buttons, a
hosted shell, overview and the edges
([#181](https://github.com/Lilium-Linux/solium/issues/181)).

### Soaking on a TTY, which is the only honest soak

A nested soak measures the nested backend as much as the compositor: Solium is
a client of the host there, with its own EGL surface, its own cursor theme and
its own client-side libraries, none of which exist on a real session. A leak
found nested is a leak *somewhere*, and saying which needs the other backend.

```sh
SOLIUM_SOAK_TTY=1 dev/soak.sh 60       # on the TTY: start the session and soak it
dev/soak.sh 60 --attach wayland-1      # from another VT or over ssh: sample one already running
```

On a TTY only half the workload runs. The hardware backend reads none of the
scripted knobs, `SOLIUM_TRIGGER_AT` included, so the cycle of key presses
`dev/soak.sh` hands the compositor does nothing there, and neither does a list
made with `dev/soak.sh --triggers`. What churns is the client side: terminals
opened and closed, and an X11 client now and then. Nested, the whole cycle
runs. Until the hardware backend reads the triggers, a soak of the window
lifecycle on a TTY needs the windows opened and closed from outside it.

### Measuring frame pacing on a TTY

`dev/pacing-tty.sh` runs pinned scenes on the hardware and measures every
pass of them, with `SOLIUM_TRACE` writing a line per pass and per flip. S1 is
the `rounded` pane style, four idle terminals and one player at fixed
rectangles on the monitor you name, over the shipped wallpaper; S1 tilted is
the same with the player held at 6 degrees, so its picture is captured again
whenever it draws; and the tilt scene is two tilted terminals, one idle and
one ticking. It is what Phase 0 of the shader work is judged by (FX-S1), and
the 260 Hz numbers come only from here: nested, there is no page flip and
QML is software. The tilts are made by the scenes' Lua, because the hardware
backend reads none of the scripted knobs.

    dev/pacing-tty.sh before DP-1     # fourteen runs, about 21 minutes
    dev/pacing-tty.sh after DP-1      # nine runs, about 14 minutes
    dev/pacing-tty.sh fence DP-1      # one fence pair, about 3 minutes
    dev/pacing-tty.sh one DP-1        # one run, to try it

Run it on a free VT, logged in, from the worktree the measured binary was
built in, and keep your hands off the keyboard and mouse until it says
`done`. Nothing else should be building or running on the GPU meanwhile;
each run's `meta.txt` records the load and any `cargo` or `rustc` it saw.
Each run is capped by a timeout; Ctrl+Alt+Backspace stops the compositor at
any moment and the script with it. It writes under
`~/.local/state/solium/pacing/<date>-<protocol>/`: per run, `meta.txt` (the
build, the player, the kernel, the load, the driver and its clocks before
the run), `stderr.log`, the trace and `summary.txt`; and `results.txt` with
every summary. The compositor's own state and configuration directories are
the run's, so your `session.log` and `user.lua` are untouched. Without mpv
and `PACING_CLIP`, ffplay plays a test pattern; every run that is compared
must use the same player.

`PACING_BINARY` runs a binary frozen for a measurement. Freeze a release
build (`cargo build --release`) in a worktree of its own and run the script
from that worktree: the binary finds its QML and Lua in the tree it was
built from, so a worktree that keeps changing would change what is
measured, and the dev profile's `missed` is not the shipped one's.
`after` also runs `PACING_ANCHOR`, the build an earlier run measured, once,
so two sittings can be compared through it. `dev/pacing-nested.sh <label>`
runs the same scenes nested (`PACING_SCENE=tilt`, `PACING_TILT=6`) and
prints the same summary; its numbers are not the TTY's. Nested, only the
captures are GPU-timed: timing the window's own draw needs a GL call after
its swap, which leaves the window's surface uncurrent, so the next pass
could not read its buffer age and would redraw everything.

A pass record carries `pass`, `t_ns` (CLOCK_MONOTONIC at its start),
`total_us`, `deadline_us`, `monitor`, `missed`, each phase as `<phase>_us`,
`captures`, `effect_runs` (chains of effects run this pass; a self effect
runs only when its part commits), `panes`, `drew`, `scenes`, `animating`,
`rendered`, `built`, `rebound`, `qml` (Qt's microseconds per scene),
`clocks`, `gpu_mhz`, `mem_mhz`, `pstate`, `gpu` (`ok`, `unsupported`, `late`
or `disjoint`), `gpu_us`, `gpu_prep_us` (the captures), `gpu_effects_us` (the
effect chains, timed as one region a run phase) and `gpu_out_us`. A flip
record carries `flip` (the pass), `monitor`, `seq`, `at_ns`, `queued_ns` and
`late`, the vblanks it missed. `dev/pacing-summary.py` reads them.

## Installing it

For picking Solium at the login screen rather than starting it from a TTY.
This is an install from a checkout on Fedora 44; *A Fedora package* below
builds an RPM of the checkout instead. There is no COPR or `PKGBUILD` yet
(#66).

```sh
dev/install.sh                 # build, install into ~/.local, check, print the sudo line
sudo install -Dm644 ~/.local/share/solium/solium.desktop /usr/local/share/wayland-sessions/solium.desktop
dev/install.sh --uninstall     # remove it again; prints `sudo rm -f …` for the session file
```

| | |
|---|---|
| `--prefix DIR` | where to install, default `~/.local`: `DIR/bin/solium`, `DIR/bin/solium-session` (what the session file starts: `solium --tty --session`, and the clean-up after a Solium that crashed), `DIR/share/solium/{qml,lua,effects}`, and the generated `DIR/share/solium/solium.desktop`. `/usr` and `/usr/local` are system prefixes, below |
| `XDG_CONFIG_HOME=` | where the session's other files go, default `~/.config`: `systemd/user/solium-session.target`, `systemd/user/solium-autostart.target` and `xdg-desktop-portal/lilium-portals.conf`, copied from `dev/session/` |
| `--session-dir DIR` | where the display manager reads sessions, default `/usr/local/share/wayland-sessions`. Only the printed `sudo` line writes there. Refused with a system prefix |
| `--no-build` | install the release binary already in `target/install` |
| `--no-check` | skip the installed binary's `--check`: for a package built from source, whose binary still finds its build tree during `%install` and would fail the asset check |
| `--jobs N`, `--image IMAGE` | the build's cargo jobs (2) and container (`localhost/solium-build:fc44`) |
| `--uninstall` | remove `bin/solium`, `bin/solium-session`, `share/solium/{qml,lua,effects}` and the generated `solium.desktop` under the same `--prefix`, whatever put them there, and the three files in `XDG_CONFIG_HOME` only while they are what install wrote |
| `DESTDIR=` | stage the prefix and `XDG_CONFIG_HOME` under this directory instead; the session file's `Exec` still names the real prefix. `--session-dir` is used as given, so a test can point it at `/tmp`. It may contain a `~`, as rpmbuild's buildroot does |
| `SOLIUM_BUILD_LOCK=`, `SOLIUM_BUILD_MEMORY=` | the lock the build takes (`$XDG_CACHE_HOME/solium-build.lock`, or `~/.cache/solium-build.lock` when `XDG_CACHE_HOME` is unset) and the container's memory cap (`6g`). `dev/build-release.sh` reads both, so they apply to `dev/rpm.sh` too |

`dev/install-check.sh` runs all of that into `/tmp` and checks every step.

**A system prefix,** `--prefix /usr` or `--prefix /usr/local`, is the layout a
package installs, and what the Fedora package's `%install` runs. Everything
goes under the prefix and nothing into `XDG_CONFIG_HOME`: the units in
`DIR/lib/systemd/user`, the portal choice in `DIR/share/xdg-desktop-portal`
and the session file in `DIR/share/wayland-sessions`, where systemd,
xdg-desktop-portal and the display manager read system files, so there is no
`sudo` line. Those files are the installation's, like the binary, so there is
no `config.sha256` and uninstall removes them whatever put them there. The
check after installing runs with an empty configuration directory, because
what every user gets is the shipped configuration, not yours.

**The units and the portal configuration go beside your own configuration,**
so they are treated as yours once you change them. Install records a
checksum of each file it writes in `share/solium/config.sha256`, and replaces
or removes a file only while it still matches. A file edited since, or a link
(a dotfiles checkout, say), is kept, and install and uninstall both name it.
Solium starts the targets itself (`session.rs`), so nothing needs enabling.
systemd reads new unit files at the next login, and install prints the
`systemctl --user daemon-reload` that has it read them at once. That command
changes nothing that is running, and install does not run it for you.

**Why it builds at `/solium-src`, a path the host does not have.** `assets.rs`
tries the build tree before the install layout:

```rust
const BUILD_TREE: &str = env!("CARGO_MANIFEST_DIR");
// …
places.extend(baked.map(PathBuf::from));
places.push(PathBuf::from(build_tree));
// … then "the install layout answering for itself":
places.extend(
    exe.and_then(Path::parent)
        .and_then(Path::parent)
        .map(|prefix| prefix.join("share").join("solium")),
);
```

A binary compiled at the checkout's own path finds the checkout there, so once
installed it would go on reading the checkout's QML and Lua, and a
`git checkout` would change a running session. Measured: `target/debug/solium`
copied into a prefix with its own `share/solium` still logs the tree it was
compiled in as its `shipped assets root`, and `install.sh`'s check refuses it.
Compiled at `/solium-src` in the container, the build tree it names is not on
the host, so the installed copy falls through to `<prefix>/share/solium`. That
build goes to its own `target/install`, apart from the builds made at the
checkout's path. Nothing is baked into `SOLIUM_DATADIR`, so the same binary
works from any prefix, a `DESTDIR` staging area included.

`dev/build-release.sh [--jobs N] [--image IMAGE]` is that build on its own
(defaults `2` and `localhost/solium-build:fc44`), and it builds
`target/install/release/solium`. It runs `cargo build --release -p solium` in
the build image, with the checkout mounted at `/solium-src` and
`CARGO_TARGET_DIR` at `/solium-src/target/install`, under
`nice -n 19 ionice -c 3`. It holds the shared lock `SOLIUM_BUILD_LOCK`
(default `$XDG_CACHE_HOME/solium-build.lock`, or
`~/.cache/solium-build.lock` when that is unset) and caps the container at
`SOLIUM_BUILD_MEMORY` (default `6g`, and swap the same). An exit status of 137
means the container ran out of memory, and the script says to run it again
with `--jobs 1`. `dev/install.sh` runs it (unless `--no-build`), and so does
`dev/rpm.sh`, so a package holds the same binary as an install from the
checkout.

**Checked before it starts:** a Solium running from the binary it would
replace (found through `/proc/<pid>/exe`, as `fuser` does; it refuses and
stops nothing), checked again once the build is over, since a session can
start from the old install while it runs; that the build image exists; and
that nothing it would delete is reached through a link. Install replaces
`share/solium/{qml,lua,effects}` and uninstall removes them with `rm -rf`, so a
`share/solium` left linked to a checkout would lose the checkout's QML and
Lua: both refuse while `share/solium` is a link, or while it resolves into
the checkout. A prefix or a `share` that is itself a link (moved to another
disk) is fine, and the staged session file is written beside its target and
renamed over it, so it never writes through a link. **After:** the installed
`solium --check` has to pass, the gate's scripts check, and the asset root it
logs (`RUST_LOG=solium::assets=debug`, `shipped assets root=…`) has to be the
installed `share/solium`. If either fails, the new files are already in place,
and it says so and prints the uninstall line. It also warns if the `solium` on
`PATH` is not the one it installed.

**Which display manager, and where it looks.** Fedora 44 KDE runs Plasma Login
(`systemctl status display-manager` names `plasmalogin.service`), a fork of
SDDM. `/etc/plasmalogin.conf` has no session-directory setting; the greeter
lists sessions with
`QStandardPaths::locateAll(GenericDataLocation, "wayland-sessions")`
(`src/frontend/settings/models/sessionmodel.cpp` in plasma-login-manager
6.7.5), which is `/usr/local/share/wayland-sessions` and then
`/usr/share/wayland-sessions`. The default is the `/usr/local` one, because no
package owns it. It only watches directories that existed when it started, and
that one does not exist until `install -D` makes it, so log out after running
the `sudo` line rather than expecting a greeter already on screen to notice.

**Getting back.** Plasma Login remembers the last session
(`#RememberLastSession=true` in `/etc/plasmalogin.conf`, commented out at its
default), so after one Solium login it offers Solium first: pick **Plasma** (`/usr/share/wayland-sessions/plasma.desktop`)
to return. Inside Solium, Ctrl+Alt+Backspace ends the session whatever the
configuration says, unless the screen is locked, and super+shift+q does in the
shipped `init.lua`. Ctrl+Alt+F3 switches to a text console, to log in and read
`~/.local/state/solium/session.log`. Solium handles both chords itself
(`escape` in `input/mod.rs`, with the tests `ctrl_alt_f3_asks_for_the_third_terminal`
and `ctrl_alt_backspace_stops_the_compositor`), because the kernel stops acting
on them once a graphical session owns the VT, so neither helps if Solium
itself hangs.

**The `Exec` is absolute** (`Exec=/home/you/.local/bin/solium-session`, which
runs the `solium --tty --session` beside it): Plasma
Login starts a session with `PATH=/usr/local/bin:/usr/bin:/bin` (`DefaultPath`
in `/etc/plasmalogin.conf`), and whether `~/.local/bin` is added after that is
up to your shell profile.

**Where the log goes.** `--tty`, which `solium-session` passes, is what turns
the log file on (`main.rs`):

```rust
let log = matches!(backend.as_deref(), Some("--tty"))
    .then(open_log)
    .flatten();
```

and `open_log` puts it in `state_directory()`, which is `$XDG_STATE_HOME/solium`
falling back to `~/.local/state/solium`, the directory the QML GPU marker files
share:

```rust
fn state_directory() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(base.join("solium"))
}
```

So `~/.local/state/solium/session.log`, appended to by every session. Plasma
Login also sends the session's stderr, which carries the same lines, to
`~/.local/share/plasmalogin/wayland-session.log`, and truncates that at each
login (`O_TRUNC` in its `src/helper/UserSession.cpp`).

### A Fedora package

A development snapshot of the commit checked out, as an RPM that dnf installs
and removes, for Fedora 44 machines that have neither Rust nor the build image:

```sh
dev/rpm.sh                     # build, package, check unpacked, print the dnf line
sudo dnf install ./target/rpm/RPMS/x86_64/solium-0.0.0~git<date>.<commit>-1.fc44.x86_64.rpm
sudo dnf remove solium
```

`dev/rpm.sh` builds the release binary with `dev/build-release.sh`, the same
container build `dev/install.sh` runs, then makes the commit's source tarball
and runs `rpmbuild -bb --with prebuilt` on the host (it needs `rpm-build`,
which the build image does not have) into `target/rpm`, never `~/rpmbuild`.
Its options are `--jobs N` and `--image IMAGE`, the same as
`dev/build-release.sh` with the same defaults. It needs `rpmbuild` (from
`rpm-build`), `rpm2cpio`, `cpio`, `git` and `podman` on the host, and refuses
to run on a host that is not Fedora. It takes the Qt version for the
`Requires` from the build image (`%_qt6_version`). It deletes and rebuilds
`target/rpm` on every run, and refuses if `target/rpm` is a link. rpmbuild's
whole log is `target/rpm/rpmbuild.log`, and the unpacked package's `--check`
log is `target/rpm/check.log`.
It refuses uncommitted changes, because the package is named after its commit
(`0.0.0~git<commit date>.<short hash>`), and a build image of another Fedora
release than the host's. Then it checks the package without installing it:
unpacked into `target/rpm/unpacked`, its `solium --check` has to pass with an
empty configuration and log `shipped assets root=` that root's
`usr/share/solium`, which is what the binary finds beside itself once it is
`/usr/bin/solium`. It prints rpmbuild's warnings, if there are any, the
package's path and the `sudo dnf install` line.

The package is built for one Fedora release, the build image's, and the
`.fc44` in its name says which. Its library dependencies are worked out by rpm
from the binary, so on another Fedora 44 machine dnf brings Qt and the rest;
another release needs its own build, and COPR is where that will happen.

`dev/rpm/solium.spec` drives `%install` through
`dev/install.sh --no-build --prefix /usr`, the system layout, so the package
holds exactly what that installs: `/usr/bin/{solium,solium-session}`,
`/usr/share/solium/{qml,lua,effects}`, `/usr/share/wayland-sessions/solium.desktop`
with `Exec=/usr/bin/solium-session`, the two units in `/usr/lib/systemd/user`
and `lilium-portals.conf` in `/usr/share/xdg-desktop-portal`, plus the licences
and two documents. rpmbuild fails on a file one has and the other does not, and
`dev/install-check.sh`'s *the Fedora package* compares the two on every run,
without building a package. It also checks what the spec adds to rpm's
automatic library dependencies: `Requires` qt6-qtdeclarative (the QtQuick and
QtQuick.Shapes modules the shipped QML imports), no older than the build
image's Qt, since rpm's own dependencies take any Qt 6 (a Fedora 44 installed
from the release image and never updated has 6.10, and dnf brings the newer
one from updates), xorg-x11-server-Xwayland and
util-linux-core (`solium-session`'s `flock`), each the package that has the
file here, and `Recommends` xdg-desktop-portal and each backend
`lilium-portals.conf` names, gtk and wlr, and foot: the shipped configuration
has no launcher, so `super+return`'s terminal is how a program starts, and
none of the terminals `init.lua` looks for is on Fedora Workstation.

Without `--with prebuilt`, `%build` compiles the tarball with
`cargo build --locked --release`, `SOLIUM_DATADIR` baked as
`/usr/share/solium`, and `%install` passes `--no-check`, since that binary
still finds its build tree during the build; `%check` runs its `--check`
instead. That is the build COPR will run, with internet access for cargo. It
has not been run here: the build image has no `rpm-build`, and the host cannot
link Solium.

Use one way of installing, not both. With the package and an install from the
checkout, the login screen lists Solium twice, the units in `~/.config` come
before the package's, and `~/.local/bin` comes before `/usr/bin` on `PATH`.
`dev/rpm.sh` warns when it finds one; `dev/install.sh --uninstall` takes it
out.
