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
| `SOLIUM_DEV_IMAGE=` | The container `dev/run-nested.sh` runs in. |
| `SOLIUM_FORM_FACTOR=` | `desktop` (default), `laptop`, `tablet`, `phone`. Selects the input profile. |
| `SOLIUM_DRAG_MODIFIER=` | `logo` (default) or `alt`. Held to drag a window from anywhere in it. |

## Checks

| Script | What it asserts |
|---|---|
| `dev/gate.sh` | fmt, clippy, tests, build, and that the Lua configuration loads |
| `dev/app-check.sh <program>` | a client runs, draws, and provokes no protocol error |
| `dev/cursor-check.sh` | the pointer is visible over empty desktop |
| `cargo run -p wl-probe` | the protocols answer, from a real client's side |
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
