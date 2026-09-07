# Solium

The compositor of [Lilium DE](https://github.com/Lilium-Linux). Wayland, written
in Rust on [Smithay](https://github.com/Smithay/smithay).

One compositor for phone, tablet, laptop and desktop — tiling, scrolling,
floating and overview modes, all scriptable, all animated by the same engine.

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
| `--probe` | what the hardware offers, without taking it |
| `--debug-mode` | adds the Developer Tweaks panel, on `super+shift+d` |

`super+shift+q` ends the session and `super+shift+r` reloads the configuration
without ending it. The rest of the bindings come from `--check`, because they
belong to a script rather than to the compositor.

On the hardware, anything that goes wrong is also written to
`~/.local/state/solium/session.log`, synchronously — that log exists for the
case where the screen is gone and the power button is the only way out.

## What works

Windows open, tile, scroll, float and animate. Server-side decorations are QML
and reload while the session runs. X11 clients work through XWayland. Copy and
paste works, both selections. A window's life begins when the user asks for the
application rather than when its client connects, so it takes its place in the
layout immediately and the application appears inside it.

Protocols: `xdg-shell`, `wlr-layer-shell`, `xdg-decoration`, `xdg-output`,
`xdg-activation`, `wp-viewporter`, `wp-fractional-scale`, `linux-dmabuf`,
`relative-pointer`, `pointer-constraints`, `primary-selection`,
`xwayland-shell`.

Not yet: any scale but 1x, screen capture, session locking, IME. Everything
known is on the issue tracker, prioritised by whether an application can be
used at all without it rather than by how hard it is.

## Configuring it

Everything is a file you write, and none of it needs the compositor rebuilt.
`~/.config/solium/user.lua` holds only what you want changed; your own QML
shadows what ships, file by file. See **[docs/ricing.md](docs/ricing.md)**.

## Why this stack

**Smithay** is a Wayland compositor library, not a compositor. It hands over
protocol plumbing, input and backends while leaving layout, rendering and
policy to us — which is where a desktop environment's character actually lives.
[niri](https://github.com/YaLTeR/niri) is Rust-on-Smithay with working touch and
gesture support, so the multi-form-factor path has a reference implementation
rather than being a bet.

**Rust** because this codebase is meant to last. Memory safety removes an entire
class of compositor crash, and a crash in a compositor takes the session with it.

**GLES2 to start**, through Smithay's renderer traits. Smithay has no Vulkan
renderer — its `backend::vulkan` is device enumeration only — and niri, the
reference implementation we chose this stack for, uses GLES. Vulkan would mean
writing a renderer backend plus NVIDIA dmabuf import before a single window
appeared. See `docs/spikes/2026-08-27-vulkan-on-smithay.md`.

The transform layer is written against Smithay's `Renderer`/`Frame` traits, not
against GLES directly, so a Vulkan backend stays a contained change later.

## Status

Alpha. It runs on hardware and is used to develop itself, which is the only
test that counts for a compositor. It is not something to depend on yet: there
is no lock screen, no screen capture, and a bug in here takes the session with
it.

## Documentation

| | |
|---|---|
| [docs/ricing.md](docs/ricing.md) | configuring it — settings, decorations, bindings, modes |
| [docs/architecture.md](docs/architecture.md) | how it is built, and why |
| [docs/shell-boundary.md](docs/shell-boundary.md) | what belongs to the compositor and what to the shell |
| [docs/roadmap.md](docs/roadmap.md) | epics, in dependency order |
| [dev/README.md](dev/README.md) | the knobs and checks it is tested with |
| `docs/spikes/` | decisions, with the evidence that settled them |

## History

Solium began as a Hyprland fork. That work is archived: the fork proved the
decoration and window-control ideas, and its post-mortem — including why an
out-of-process QML shell was the wrong architecture — is recorded in the
`lilium-de` repository. The ideas carried over; none of the code did.
