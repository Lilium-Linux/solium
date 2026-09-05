# Development knobs

Environment variables Solium reads. All of them exist so the compositor can be
exercised and photographed without a human at the keyboard — that is what turns
a demo into a regression test.

| Variable | Effect |
|---|---|
| `SOLIUM_CAPTURE=<path>` | Write one rendered frame to `<path>` as a binary PPM. |
| `SOLIUM_CAPTURE_AT=<ms>` | Capture at this moment instead of "once a window has settled". Naming a moment is what makes capturing an *animation* possible. |
| `SOLIUM_TRIGGER_AT=` | Fire key bindings, as `<ms>:<combo>` separated by commas — e.g. `5200:super+space,6200:super+space`. Goes through the same path a keypress does. |
| `SOLIUM_CLICK_AT=` | Fire pointer presses, as `<ms>:<x>,<y>` separated by semicolons. |
| `SOLIUM_LUA_INIT=<path>` | Load this configuration instead of `~/.config/solium/init.lua` or the bundled one. |
| `SOLIUM_QML_TOPBAR=`, `SOLIUM_QML_TITLEBAR=` | Load chrome from elsewhere, so it can be restyled without a rebuild. |
| `SOLIUM_DEV_IMAGE=` | The container `dev/run-nested.sh` runs in. |
| `SOLIUM_FORM_FACTOR=` | `desktop` (default), `laptop`, `tablet`, `phone`. Selects the input profile. |
| `SOLIUM_DRAG_MODIFIER=` | `logo` (default) or `alt`. Held to drag a window from anywhere in it. |

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

Mock windows — with titlebars, on a mock output at Solium's own coordinates —
animating through the scenarios the compositor actually has: a window opening,
overview entering and leaving, the app switcher, a drag, a maximise. Curve,
duration, playback speed and the spring's stiffness, damping and throw are all
live.

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

## Where it runs

`dev/run-nested.sh` runs Solium **in the build container**, not on the host. It
is built there, and the container's C library is newer than the host's — once a
vendored C dependency (Lua) was compiled in, the resulting binary would not
start on the host at all. Building and running in one place removes the skew
rather than papering over it. `/tmp` is shared so captures land where you can
read them.

## Bindings

| Input | Effect |
|---|---|
| `Super` + `Return` | Open a terminal (`SOLIUM_TERMINAL` picks which) |
| `Super` + `Q` | Close the focused window |
| `Super` + `Space` | Overview on/off (bound in `lua/overview.lua`, not in Rust) |
| `Escape` | Leave overview |
| `Super` + drag | Move a window from anywhere in it |
| Titlebar drag | Move a window (the client asks, via `xdg_toplevel.move`) |
| Click | Focus and raise; in overview, focus that window and leave |
