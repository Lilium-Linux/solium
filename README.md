# Solium

The compositor of [Lilium DE](https://github.com/Lilium-Linux). Wayland, written
in Rust on [Smithay](https://github.com/Smithay/smithay), rendering with Vulkan.

One compositor for phone, tablet, laptop and desktop — tiling, scrolling,
floating and overview modes, all scriptable, all animated by the same engine.

## Why this stack

**Smithay** is a Wayland compositor library, not a compositor. It hands over
protocol plumbing, input and backends while leaving layout, rendering and
policy to us — which is where a desktop environment's character actually lives.
[niri](https://github.com/YaLTeR/niri) is Rust-on-Smithay with working touch and
gesture support, so the multi-form-factor path has a reference implementation
rather than being a bet.

**Rust** because this codebase is meant to last. Memory safety removes an entire
class of compositor crash, and a crash in a compositor takes the session with it.

**Vulkan** because every mode we want — overview, app switcher, peek, the
icon→window genie — is the same operation: place a window's texture somewhere
other than its real geometry and animate between the two. That wants explicit,
modern GPU control, not a compatibility layer.

## Status

Pre-alpha. Nothing runs yet. See `docs/roadmap.md` and the issue tracker.

## History

Solium began as a Hyprland fork. That work is archived: the fork proved the
decoration and window-control ideas, and its post-mortem — including why an
out-of-process QML shell was the wrong architecture — is recorded in the
`lilium-de` repository. The ideas carried over; none of the code did.
