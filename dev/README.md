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

**Never glob `target/debug/build/solium-*/out/`.** The Qt host is compiled into
`libsolium_qml_host.a` under that path, and there is more than one such
directory: cargo makes a separate one per unit metadata, so `cargo build -p
solium` and `cargo build` (or `cargo test`) each own one. A test harness or a
script that links "the" archive by glob picks whichever the shell sorts first,
which is not the newest, and there is no error — the link succeeds and you
measure a build from an hour ago. It cost a full round of debugging a fix that
was already in the tree.

This is not something one commit introduced and another can remove; it is how
cargo lays the directory out. Either pin the newest,

```sh
ls -td target/debug/build/solium-*/out | head -1
```

or compile `crates/solium/qml/host.cpp` from source in the harness's own
`build.rs`, which is the only way to be certain that what runs is what is
checked out.

### The gate

`dev/gate.sh` checks the formatting with `cargo fmt --check`, then runs clippy
with warnings denied, the tests and a build, then two checks on the built
binaries that nothing else reaches: `solium --check`, which loads the Lua
configuration, and `dev/wirecheck`, which drives the QML GPU path against the
machine's own render node. Run `cargo fmt --all` first if the formatting step
fails. It runs cargo natively unless told otherwise:

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
with where in the code each is read, is on the
[Flags and environment](../crates/solium/environment.txt) page, made from
`crates/solium/environment.txt`; a test fails when the code reads one that file
does not list.

| Variable | Effect | Nested only |
|---|---|---|
| `SOLIUM_CAPTURE=<path>` | Write one rendered frame to `<path>` as a binary PPM. See *Capturing a frame*. | yes |
| `SOLIUM_CAPTURE_AT=<ms>` | Capture at this moment after startup instead of once a window has settled. Naming a moment is what makes capturing an *animation* possible. | yes |
| `SOLIUM_CAPTURE_FRAMES=<n>`, `SOLIUM_CAPTURE_INTERVAL=<ms>` | A burst of `n` frames, `<ms>` apart (16 by default), written beside the path with a number on its stem: `/tmp/tile.ppm` gives `/tmp/tile-000.ppm`, `/tmp/tile-001.ppm`, … One frame shows a pose; a burst shows whether the motion is smooth. | yes |
| `SOLIUM_TRIGGER_AT=<ms>:<combo>,...` | Run what these combinations are bound to, at these moments: `5200:super+space,6200:super+space`. Not a keypress: the binding is looked up by the name written, so it fires while the screen is locked, cannot press Ctrl+Alt+Backspace, and cannot show whether a binding works under another keyboard layout. | yes |
| `SOLIUM_DRAG_AT=<ms>:<x1>,<y1>><x2>,<y2>;...` | Drag the pointer through the real input path: the real grab, hit-testing and layout scripts. A drag from a point to itself is a click. | yes |
| `SOLIUM_CLICK_AT=<ms>:<x>,<y>;...` | Hand a press to the mode holding the pointer, such as overview. It never touches the input path; see *Driving it without a keyboard*. | yes |
| `SOLIUM_OUTPUTS=<n>` | Give the nested backend `n` monitors, 1 to 4, side by side in its one window. See *Two monitors, without a second monitor*. | yes |
| `SOLIUM_OUTPUTS_AT=<ms>:<n>,...` | Change how many nested monitors there are at these moments: hotplug without a cable. | yes |
| `SOLIUM_LOADING_AT=<ms>:<program>,...` | Open a window for an application that never arrives, to exercise the loading window. | yes |
| `SOLIUM_LUA_INIT=<path>` | Load this configuration instead of `~/.config/solium/init.lua` or the shipped one. | |
| `SOLIUM_TERMINAL=<command line>` | The terminal `super+return` opens, split on spaces. | |
| `SOLIUM_PANE=<name or path>` | The frame style for this run. | |
| `SOLIUM_QML=<mode>` | `auto`, `gpu` or `software`; see *QML on the GPU*. | |
| `SOLIUM_SESSION_BUS=<address>` | The D-Bus bus to tell about the session and to own `org.freedesktop.ScreenSaver` on, instead of the session bus. A nested run, or `solium --tty` without `--session`, tells nobody anything without it. To check the calls against a private bus: `dbus-run-session -- sh -c 'SOLIUM_SESSION_BUS=$DBUS_SESSION_BUS_ADDRESS ./target/debug/solium'`. | |
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
as its keysym, can only be checked with a real keyboard with the second layout
active; `wl-probe`'s `WL_PROBE_KEYBOARD` shows which layout the client was told
is live.

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
| `dev/present-check.sh` | a `pivot` is the point the matrix leaves alone, a raised window is drawn in front, and clicks follow the rect a window is drawn at without following the `z` it is drawn above |
| `dev/install-check.sh [--no-build]` | `dev/install.sh` installs into a `DESTDIR` under `/tmp`: every file (the systemd units and the portal configuration included), the absolute `Exec`, the printed `sudo` lines, `--check` from the installed copy using its own `share/solium`, refusing while it runs (a session started during the build included), refusing to delete through a link, refusing `/` and a `DESTDIR` with a space, saying so when the check fails after the files are in place, keeping a unit or portal configuration of the user's own through an install and an uninstall, `solium-session` cleaning up after a stand-in Solium that crashed (and only then, and only once it has gone, and unsetting the variables after one that crashed before starting its target while no other desktop holds `graphical-session.target`) and refusing a second session while one runs, a reinstall saying when the login screen's session file is stale, and an uninstall that leaves nothing. See *Installing it* |

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
`render::by_depth` and `warp::mesh` — all of which are pure functions with tests
of their own. What sits between them is two field initialisers in the
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

    SOLIUM_CHECK_DIR=/tmp/present ./dev/present-check.sh   # keep the frames

Kept out of `gate.sh` deliberately: it needs a host compositor to nest in and a
client to open, and a gate that cannot run headless is a gate that gets skipped.

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

Three more modes. `WL_PROBE_WINDOWS=<seconds>` maps two windows, one framed
by the compositor and one that draws its own, and holds them up to be looked
at; with `WL_PROBE_FULLSCREEN=1` the first asks for fullscreen a second in, so
the change can be photographed. `WL_PROBE_CONNECT_ONLY=1` connects, counts the
globals and leaves, for hunting what a client that never draws costs.

## Hotplug, and how to test it without a cable

Monitors arriving and leaving
([#43](https://github.com/Lilium-Linux/solium/issues/43)) work, and were
checked on the hardware, with real cables, before #43 was closed. Two bugs came
out of that testing and are fixed; how the first one failed is below.

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
having its own scanout buffer means. Every per-output path runs for each of
them; what it cannot simulate is a second *pipeline*, one refresh rate and one
page flip per screen, which is `tty.rs`'s half of the problem.

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
a modeset back on the next frame — has not yet been run on the hardware;
[docs/beta.md](../docs/beta.md) tracks it.

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
X unavailable" errors while writing a shell. It exits 0 either way, so read
what it prints.

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
compositor to nest in, on the hardware otherwise. Only the first argument
chooses what Solium does, so write `solium --tty --debug-mode`, not the other
way round, and don't pass a flag it does not know: anything else in first
place, `--help` included, starts a compositor
([#156](https://github.com/Lilium-Linux/solium/issues/156)).

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

**What the hardware has and has not been through.** Hotplug has (see
*Hotplug* above). Turning a screen off has not; see *Turning screens off*.

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

## Installing it

For picking Solium at the login screen rather than starting it from a TTY.
This is an install from a checkout on Fedora 44, not packaging: there is no
`.spec`, COPR or `PKGBUILD` yet (#66).

```sh
dev/install.sh                 # build, install into ~/.local, check, print the sudo line
sudo install -Dm644 ~/.local/share/solium/solium.desktop /usr/local/share/wayland-sessions/solium.desktop
dev/install.sh --uninstall     # remove it again; prints `sudo rm -f …` for the session file
```

| | |
|---|---|
| `--prefix DIR` | where to install, default `~/.local`: `DIR/bin/solium`, `DIR/bin/solium-session` (what the session file starts: `solium --tty --session`, and the clean-up after a Solium that crashed), `DIR/share/solium/{qml,lua}`, and the generated `DIR/share/solium/solium.desktop` |
| `XDG_CONFIG_HOME=` | where the session's other files go, default `~/.config`, whatever the prefix: `systemd/user/solium-session.target`, `systemd/user/solium-autostart.target` and `xdg-desktop-portal/lilium-portals.conf`, copied from `dev/session/` |
| `--session-dir DIR` | where the display manager reads sessions, default `/usr/local/share/wayland-sessions`. Only the printed `sudo` line writes there |
| `--no-build` | install the release binary already in `target/install` |
| `--jobs N`, `--image IMAGE` | the build's cargo jobs (2) and container (`localhost/solium-build:fc44`) |
| `--uninstall` | remove `bin/solium`, `bin/solium-session`, `share/solium/{qml,lua}` and the generated `solium.desktop` under the same `--prefix`, whatever put them there, and the three files in `XDG_CONFIG_HOME` only while they are what install wrote |
| `DESTDIR=` | stage the prefix and `XDG_CONFIG_HOME` under this directory instead; the session file's `Exec` still names the real prefix. `--session-dir` is used as given, so a test can point it at `/tmp` |
| `SOLIUM_BUILD_LOCK=`, `SOLIUM_BUILD_MEMORY=` | the lock the build takes (`~/.cache/solium-build.lock`) and the container's memory cap (`6g`) |

`dev/install-check.sh` runs all of that into `/tmp` and checks every step.

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

**Checked before it starts:** a Solium running from the binary it would
replace (found through `/proc/<pid>/exe`, as `fuser` does; it refuses and
stops nothing), checked again once the build is over, since a session can
start from the old install while it runs; that the build image exists; and
that nothing it would delete is reached through a link. Install replaces
`share/solium/{qml,lua}` and uninstall removes them with `rm -rf`, so a
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
