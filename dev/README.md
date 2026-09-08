# Development knobs

Environment variables Solium reads. All of them exist so the compositor can be
exercised and photographed without a human at the keyboard — that is what turns
a demo into a regression test.

| Variable | Effect |
|---|---|
| `SOLIUM_CAPTURE=<path>` | Write one rendered frame to `<path>` as a binary PPM. |
| `SOLIUM_CAPTURE_AT=<ms>` | Capture at this moment instead of "once a window has settled". Naming a moment is what makes capturing an *animation* possible. |
| `SOLIUM_CAPTURE_FRAMES=<n>` | Capture a burst of `n` frames, numbered `frame-000.ppm`, `frame-001.ppm`, … One frame shows a pose; a burst shows whether the motion is smooth. |
| `SOLIUM_CAPTURE_INTERVAL=<ms>` | Time between frames of a burst (default 16). |
| `SOLIUM_TRIGGER_AT=` | Fire key bindings, as `<ms>:<combo>` separated by commas — e.g. `5200:super+space,6200:super+space`. Goes through the same path a keypress does. |
| `SOLIUM_CLICK_AT=` | Fire pointer presses, as `<ms>:<x>,<y>` separated by semicolons. |
| `SOLIUM_OUTPUTS=<n>` | Give the nested backend `n` monitors, side by side in its one window (1–4). Each gets its own layer map, work area and render pass, drawn into a texture of its own exactly as it would be into its own buffer. One is the default and takes the ordinary path unchanged. |
| `SOLIUM_LUA_INIT=<path>` | Load this configuration instead of `~/.config/solium/init.lua` or the bundled one. |
| `SOLIUM_QML_TOPBAR=`, `SOLIUM_QML_TITLEBAR=` | Load chrome from elsewhere, so it can be restyled without a rebuild. |
| `SOLIUM_QML_GPU=1` | Bring Qt up on the OpenGL scene graph and render QML into a dmabuf the compositor allocated, rather than rasterising it on the CPU. **Not yet usable for a session** — see *QML on the GPU*. |
| `SOLIUM_DEV_IMAGE=` | The container `dev/run-nested.sh` runs in. |
| `SOLIUM_FORM_FACTOR=` | `desktop` (default), `laptop`, `tablet`, `phone`. Selects the input profile. |
| `SOLIUM_DRAG_MODIFIER=` | `logo` (default) or `alt`. Held to drag a window from anywhere in it. |

## Checks

| Script | What it asserts |
|---|---|
| `dev/gate.sh` | fmt, clippy, tests, build, and that the Lua configuration loads |
| `dev/app-check.sh <program>` | a client runs, draws, and provokes no protocol error |
| `dev/cursor-check.sh` | the pointer is visible over empty desktop |
| `cargo run -p wl-probe` | the protocols answer, a bar lands on the monitor it named, and a screenshot has the desktop in it the right way up |
| `dev/clipboard-check.sh` | copy and paste across the X11 boundary, all four ways |

`cursor-check.sh` exists because the pointer was invisible for the whole life
of the project and nothing noticed: nested, the host session draws a cursor
over the top, so the only place the failure shows is the hardware.

`wl-probe` is a Wayland client, and exists because a compositor cannot test its
own protocol support from the inside. "The global is advertised" is a different
claim from "a client that uses it gets the right answers", and the gap between
those two has been where every protocol bug this week actually lived. Real
applications make fine oracles — Firefox's `WAYLAND_DEBUG` log is excellent —
right up until nothing installed happens to use the protocol you just added.
Firefox never binds `wp_presentation`, so nothing on this machine could say
whether presentation feedback worked at all.

    ./target/debug/solium &
    WAYLAND_DISPLAY=wayland-1 cargo run -p wl-probe

It exits non-zero if anything it asked for went unanswered, so it can be a
gate.

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

There is no lock client installed on this machine, so without this there is no
way to exercise the protocol at all: a compositor cannot lock itself, and every
question worth asking about a lock screen is a question about what a *second*
process can see. Point `SOLIUM_CAPTURE` at a directory while it runs and the
answer is in the frames — the desktop, then one frame of the compositor's own
backdrop between the lock and the client's first buffer, then the client's
colour, and nothing of the session anywhere in between.

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

## Hotplug, and how to test it without a cable

Monitors arriving and leaving ([#43](https://github.com/Lilium-Linux/solium/issues/43)) cannot be exercised here. The
nested backend has no connectors, and a virtual one (`vkms`) needs a kernel
module this machine will not load without a password. So the resync path is
covered by unit tests on the part with reasoning in it -- `tty::gone`, which
decides which screens went -- and the rest has never run.

**It needs testing on the hardware, and there are two ways.** The second does
not involve reaching behind the desk:

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

`clipboard-check.sh` runs its X11 half in a container, because the host has no
`xclip` and cannot install one. **Run it more than once.** The bug it was
written for failed about one time in three, so a single green run proves
nothing — which is exactly how it nearly shipped.

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

Give one of them a scale and you have a HiDPI screen to look at without owning
one:

```lua
sol.monitors({
    { name = "winit-1" },
    { name = "winit-2", scale = 2, right_of = "winit-1" },
})
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

### Soaking on a TTY, which is the only honest soak

A nested soak measures the nested backend as much as the compositor: Solium is
a client of the host there, with its own EGL surface, its own cursor theme and
its own client-side libraries, none of which exist on a real session. A leak
found nested is a leak *somewhere*, and saying which needs the other backend.

`--attach` samples a compositor it did not start, so it cannot pass the
scripted input — which used to leave an attached soak with nothing but the
client spawner for churn, and the window lifecycle is the thing worth
churning. Generate the input separately:

    # on the TTY
    SOLIUM_TRIGGER_AT="$(dev/soak.sh --triggers 60)" ./target/debug/solium --tty

    # from another VT or over ssh
    dev/soak.sh 60 --attach wayland-1

The trigger list is start-time only, which is why it has to be built before
the session rather than sent to it.

## Capturing a frame

```sh
SOLIUM_CAPTURE=/tmp/frame.ppm dev/run-nested.sh
```

A compositor cannot be verified by looking at it: a desktop screenshot tool
captures the *host* session, which proves nothing about what Solium composited,
and using a shell's own capture to test that shell is circular. So Solium reads
its own framebuffer back.

The capture waits until a window has been mapped for half a second, because a
capture of an empty compositor is exactly the misleading result the mechanism
exists to avoid. A captured frame is not presented — reading the framebuffer
back invalidates the bind, and the following `submit` would fail to reallocate
its EGL surface.

## Driving a mode without a keyboard

Because a mode that can only be checked by someone pressing a key is a mode
nobody checks twice. Enter overview, click the top-right thumbnail, and
photograph the result:

```sh
SOLIUM_TRIGGER_AT=5200:super+space SOLIUM_CLICK_AT=6000:1200,250   SOLIUM_CAPTURE=/tmp/frame.ppm SOLIUM_CAPTURE_AT=7000 dev/run-nested.sh
```

Enter and leave, then compare the frame with one taken at rest — the pixels
below the bar must be identical, because leaving a mode restores the layout
exactly:

```sh
SOLIUM_TRIGGER_AT="5200:super+space,6200:super+space"   SOLIUM_CAPTURE=/tmp/after.ppm SOLIUM_CAPTURE_AT=7400 dev/run-nested.sh
```

## Tuning animations without running the compositor

```sh
dev/preview
xdg-open crates/animation/preview/preview.html
```

Plain boxes animating through the scenarios the compositor actually has —
opening, closing, overview entering and leaving, the app switcher, a drag, a
maximise — on a mock output in Solium's own coordinates. Curve, duration,
playback speed and the spring's stiffness, damping and throw are all live.

**Deliberately plain.** The page is a test harness, not a mockup: recreating the
compositor's chrome here would be a second copy of its design, drifting from the
real one, and motion reads more clearly without decoration anyway.

**The engine is compiled to WebAssembly and called from the page**, so the curve
tuned in a browser is the code that will move real windows. Verified rather than
asserted: the same calls through wasm and through native Rust agree to six
decimal places. A reimplementation of the curves in JavaScript would drift the
first time either side changed, and the drift would be invisible — the page
would still animate plausibly.

What the page *does* own is geometry: where a window starts and ends in each
scenario. That is layout, and it belongs to the compositor and its scripts. The
engine only ever answers "how far along?".

Curves live in `crates/animation`, which depends on nothing so it keeps building
for the browser. Add one there and it is available to scripts by name
(`sol.animate{ easing = "spring" }`) and to the preview at once.

## Running programs inside Solium

`Super`+`Return` starts one, and `sol.spawn` binds any other:

```lua
sol.bind("super+b", function() sol.spawn("firefox") end)
sol.bind("super+e", function() sol.spawn("foot", "-e", "htop") end)
```

Programs start as clients of *this* compositor — `sol.spawn` overrides
`WAYLAND_DISPLAY` in the child, or it would inherit the host's and open its
window next to the nested compositor rather than inside it.

Because Solium runs in the build container, anything `sol.spawn` starts runs
there too, and only what is installed there can be started that way — which is
`foot` and not much else.

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

To add programs to the container instead, install and commit — a `--rm`
container throws the installation away with itself:

```sh
podman run --name lilium-add localhost/lilium-base:v1 pacman -Sy --noconfirm weston
podman commit lilium-add localhost/lilium-base:v1
podman rm lilium-add
```

## Building

```sh
podman build -t solium-build:fc44 -f dev/Containerfile dev/
podman run --rm --userns=keep-id --security-opt label=disable \
    -v "$HOME:$HOME" -e CARGO_HOME="$HOME/.cargo" -e PATH="$HOME/.cargo/bin:/usr/bin:/bin" \
    -w "$PWD" localhost/solium-build:fc44 cargo build
```

**The build image matches the host's distribution on purpose.** Two things go
wrong otherwise, and both were hit:

* A vendored C dependency (Lua) is compiled by whatever toolchain builds the
  project. A container with a newer C library produces a binary the host cannot
  start at all — `GLIBC_2.44 not found`.
* Working around *that* by running the compositor inside the container costs
  hardware acceleration. The container has Mesa but no GPU driver, so
  everything falls back to llvmpipe:

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

## QML on the GPU

```sh
SOLIUM_QML_GPU=1 ./target/debug/solium
```

Qt comes up on its OpenGL scene graph instead of the software rasteriser, and
renders each scene into a dmabuf the compositor allocated through GBM rather
than into a `QImage` it then has to upload. Off by default.

**It cannot run a session yet.** Qt picks one scene graph per process and there
is no way back, so the moment this succeeds every *software* scene stops
loading — the wallpaper, the window frames and the cursor all fail with `a
software scene cannot render on it`, and the desktop comes up empty. What the
knob does today is answer, in the real compositor process, whether the path
works on this machine:

```
INFO solium::qml: QML on the GPU: Qt rendered into a buffer we allocated
                  node=/dev/dri/renderD128 fenced=true
```

That line means a buffer was allocated, imported into Qt's context as a
texture, drawn into by real QML, and fenced with a `sync_file` the driver
exported. `fenced=false` is also a pass — it means the driver declined to
export a fence and the host waited with `glFinish` instead, which costs a stall
and nothing else.

Three things worth knowing before running it on a TTY:

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

* **Qt's `qWarning` does not go to stderr on this Fedora build.** It goes to
  journald, so the whole diagnostic half of the host — every EGL and GL error
  code the import path prints — is invisible in the terminal and in
  `session.log`. Set `QT_FORCE_STDERR_LOGGING=1` or you debug blind:

  ```sh
  SOLIUM_QML_GPU=1 QT_FORCE_STDERR_LOGGING=1 ./target/debug/solium
  ```

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
  whatever Qt believes is current, when EGL says otherwise. `surface.rs`'s
  `restore` is the other half, and neither works without the other.

* **A GPU scene's buffer is not stored the way you would guess.** QRhi leaves an
  OpenGL texture render target in the framebuffer's own orientation, origin
  bottom-left, so the scene's *top* row lands in the buffer's *last* row. A
  dmabuf is top-down unless it says otherwise and ours does not, so the shell
  comes out upside down. `host.cpp` calls
  `QQuickRenderTarget::setMirrorVertically` on every GPU render target for that
  reason — including the one rebuilt inside `solium_qml_scene_resize`, which is
  easy to miss.

  Do not try to correct it on the compositor's side. Smithay's `y_inverted`
  texture flag negates the texture matrix's y row without the matching
  translation, and a `Transform::Flipped180` on the render element mirrors
  within the element's *logical* size while its source rectangle is in device
  pixels — right at scale 1, wrong on every scaled monitor.

## Checking an animation frame by frame

```sh
SOLIUM_TRIGGER_AT=9000:super+t SOLIUM_CAPTURE=/tmp/tile.ppm \
  SOLIUM_CAPTURE_AT=9000 SOLIUM_CAPTURE_FRAMES=16 SOLIUM_CAPTURE_INTERVAL=20 \
  dev/run-nested.sh
```

The bounding box of what is drawn, per frame, is the animation's curve. A good
one moves most on the first frame and less on each one after, never jumps, and
reaches zero at the end. Measured across the modes:

| Mode | Per-frame movement (px) |
|---|---|
| window opens | 16, 12, 8, 4, 4, 0 |
| overview enters | 108, 84, 76, 56, 44, 32, 20, 16, 4, 4, 4, 0 |
| tiling arranges | 120, 92, 84, 56, 48, 32, 24, 16, 8, 4, 4, 4, 4, 4, 0 |
| scrolling arranges | 224, 100, 72, 64, 44, 32, 20, 16, 8, 4, 4, 4, 4, 4, 0 |

Monotonically decreasing and settling is what an ease-out looks like from the
outside. A jump mid-sequence means a layout wrote geometry without animating it;
a stall means something is being recomputed per frame that should not be.

## Running it on a TTY, as a real session

The compositor picks its backend from the environment: nested when there is a
compositor to nest in, on the hardware otherwise.

**First, from your desktop, check what the hardware offers.** This opens the
card read-only and takes no DRM master, so it is safe to run inside a running
session:

```sh
target/debug/solium --probe
```

It should name the GPU and list the connected outputs with the mode it would
choose. If it says it could not open the card for modesetting, that is expected
while another compositor holds the display.

**Then, on a free TTY.** `Ctrl`+`Alt`+`F3` (or any free one), log in, and:

```sh
cd ~/personal_projects/solium
SOLIUM_TERMINAL=konsole ./target/debug/solium --tty
```

No `sudo`: the session, the GPU and the input devices are all opened through
libseat, and a compositor that needs root is one nobody should get used to
running.

**Getting back.** `Ctrl`+`Alt`+`F1` or `F2` returns to your desktop session;
Solium keeps running on its own VT until you switch back and stop it.
`Ctrl`+`Alt`+`Backspace` stops it outright.

Both of those are Solium's own doing, and that is not a detail. Once libseat
puts the VT into graphics mode the *kernel* stops acting on `Ctrl`+`Alt`+F-keys,
so a compositor that does not handle them itself cannot be escaped from the
keyboard at all — which is how the first run of this backend ended in a reboot.

Two more things stand between you and that: if libinput reports no input devices
within five seconds, Solium stops on its own rather than hold a display nobody
can talk to; and from another VT, `pkill -x solium` always works.

That second one is worth one sentence of qualification, because it is a
last-resort escape and you are reading it before taking a VT. It holds for an
ordinary session, and it holds with `SOLIUM_QML_GPU=1` only because Solium sets
`QT_QPA_NO_SIGNAL_HANDLER` before starting Qt — without it eglfs installs its
own `SIGTERM` handler, and the process then neither dies nor cleanly survives.
See *QML on the GPU*. On a build that predates that, or one where
`QT_QPA_PLATFORM` was set from outside, reach for `pkill -9 -x solium`.

**Reading what happened.** A hardware session writes to
`~/.local/state/solium/session.log` as well as to the terminal, because the
terminal is underneath the compositor and cannot be read while it runs. It is
appended to, so the log of a run that went wrong survives the run that was meant
to fix it — and, being under `state`, it survives a reboot.

**Terminals.** `SOLIUM_TERMINAL` picks one; otherwise `lua/init.lua` takes the
first that is actually installed. That fallback exists because naming a
terminal that is not installed looks, from the keyboard, exactly like the
binding being broken — which is how the first hardware session went.

The order is deliberate: plain Wayland terminals (kitty, alacritty, wezterm,
foot) come before konsole. A KDE app started under Solium sits for around
twenty seconds before its window appears — close enough to the D-Bus
activation timeout to be worth naming, since KDE apps ask for portal and
session services that are not running here and wait for them to time out.
`vkcube` and kitty both map in a second or two, so this is the app waiting,
not the compositor failing to map it. konsole stays on the list as a fallback
for a machine that has nothing else, never as a preference.

**What is not there yet.** One output: it drives the first connected connector
and ignores the rest. Several places take the first output rather than the right
one — `work_area`, the snapshot handed to scripts, the layer map — and those are
the lines multi-output has to fix.

## Where it runs

`dev/run-nested.sh` runs Solium **in the build container**, not on the host. It
is built there, and the container's C library is newer than the host's — once a
vendored C dependency (Lua) was compiled in, the resulting binary would not
start on the host at all. Building and running in one place removes the skew
rather than papering over it. `/tmp` is shared so captures land where you can
read them.

## Driving input without a mouse

`SOLIUM_DRAG_AT` performs a drag through the real input path — the real grab,
the real hit-testing, the real layout scripts:

```sh
SOLIUM_DRAG_AT="8000:400,25>1200,500"    # at 8s, drag from (400,25) to (1200,500)
```

Aim the *start* at a titlebar. A press in a client's own area does not begin a
move grab, so a drag that starts there proves nothing — which it did, the first
time this was used.

It exists because dragging was the one interaction that needed a hand on a
mouse, and two bugs shipped there in three commits; the second froze the
machine. Both are reproducible from a script now: restoring the deadlock and
running the line above freezes the compositor on demand, CPU flat, which is how
the fix was confirmed to be a fix rather than a rearrangement.

## Settings

Every mode is a Lua script, not a compiled-in mode: `lua/tiling.lua`,
`lua/scrolling.lua`, `lua/workspaces.lua`, `lua/overview.lua`. The compositor
holds no opinion about any of them — it offers events and a layout engine, and
the scripts decide what a layout *is*.

Your own copies win. `~/.config/solium/` is searched before the bundled
scripts, so dropping a single `config.lua` there overrides just that file, and
a `tiling.lua` there replaces the whole layout without touching anything else.
Copying the entire set to change one number is not configurability.

`crates/solium/lua/config.lua` holds everything tunable — gaps, the master
ratio, column width, animation durations and easings, and how workspaces are
arranged. Editing it needs no rebuild.

Workspaces come in three arrangements, and the arrangement is the only
difference between them: `horizontal` puts them in a row that slides sideways,
`vertical` in a column, `grid` in both with `columns` × `rows`. A workspace to
the right enters from the right, because that is where it is.

## Bindings

| Input | Effect |
|---|---|
| `Super` + `Return` | Open a terminal (`SOLIUM_TERMINAL` picks which) |
| `Super` + `Q` | Close the focused window |
| `Super` + `T` | Tiling (`lua/tiling.lua`) — pressing it again returns to floating |
| `Super` + `S` | Scrolling (`lua/scrolling.lua`) — one layout at a time, see `lua/modes.lua` |
| `Super` + `[` / `]` | Focus the column left / right |
| `Super`+`Ctrl` + `[` / `]` | Move the column left / right |
| `Super`+`Shift` + `[` / `]` | Focus up / down within a column |
| `Super` + `,` / `.` | Pull a window into this column / push it back out |
| `Super` + `R` | Cycle the column through the preset widths |
| `Super` + `-` / `=` | Move the seam a tiled window sits on |
| Drag a window edge | Tiled: moves the seam. Scrolling: widens the column. Floating: resizes |
| `Super` + right-drag | Resize from anywhere in the window, in any direction |
| `Super` + `Space` | Overview on/off (bound in `lua/overview.lua`, not in Rust) |
| `Super` + `1`…`9` | Go to that workspace |
| `Super`+`Shift` + `1`…`9` | Send the focused window there |
| `Super`+`Ctrl` + arrows | Step to the next workspace in that direction |
| `Super` + wheel | Scroll the viewport (scrolling layout) |
| Drag a window | In a tiled layout, drops it where you let go |
| Hover a window | Focuses it (focus follows the pointer) |
| `Ctrl`+`Alt`+`F1`…`F12` | Switch virtual terminal (hardware session only) |
| `Super`+`Shift`+`Q` | Stop the compositor (`sol.quit`, rebindable) |
| `Ctrl`+`Alt`+`Backspace` | Stop the compositor (built in, cannot be rebound) |

The last two are taken before scripts see them, and cannot be rebound. They are
the keys that have to work when everything else is broken.
| `Escape` | Leave overview |
| `Super` + drag | Move a window from anywhere in it |
| Titlebar drag | Move a window (the client asks, via `xdg_toplevel.move`) |
| Click | Focus and raise; in overview, focus that window and leave |
