# Project status

**Solium is alpha.** It runs as a nested compositor and as a hardware session.
The planned v0.1.0 is a public preview, with readiness determined by testing
and the remaining daily-use issues rather than a release date.

This documentation snapshot was checked against `stage` on 2026-10-20.
Use the [project board](https://github.com/orgs/Lilium-Linux/projects/2) for
work in progress and the
[`daily-drive` queue](https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20is%3Aopen%20label%3Adaily-drive)
for current daily-use priorities.

## Implemented

- Floating, dwindle tiling, scrolling and overview; workspaces and layouts per
  monitor, with animated presentation and XWayland support.
- Reloadable Lua configuration, QML frames and hosted desktop scenes, plus
  effect folders written in Lua and GLSL.
- A default preview shell with a bottom bar, top dock, application/window
  search and live desktop icons. It can be disabled or replaced.
- Multiple outputs, hotplug, scaling and screen power; keyboard layouts,
  per-device libinput settings and cursor themes; keyboard-shortcuts-inhibit,
  so a VM or remote desktop can have `super`.
- Session-lock with external lockers, idle inhibition, logind lock/sleep
  handling and installed-session integration with systemd and XDG autostart.
- Checkout installation and a Fedora 44 RPM build script.

Implemented features can still have correctness and compatibility issues.
[Known limitations](gaps.md) names affected workflows, including IMEs,
clipboard privacy, compositor touch, explicit sync, multi-GPU and portals.

## Validation and remaining risk

Earlier hardware checks at commit `dfc95ce` covered an NVIDIA RTX 3070 desktop
and a Microsoft Surface Pro 7 with Intel Ice Lake graphics, both on Fedora 44.
Those checks observed QML GPU rendering, animation, idle screen-off, swaylock
and the keyboard indicator. They are evidence for that revision and those
machines, not certification of current `stage` or other hardware.

Long hardware soaks, suspend/resume and real application use still need to
establish reliability. Nested tests cover compositor behavior but cannot prove
device input, GPU pacing or power-management behavior. A crash can end the
whole session.

## Where to go next

| Need | Page |
|---|---|
| Try it | [README setup](../README.md#building) |
| Understand compatibility | [Known limitations](gaps.md) |
| Understand future work | [Roadmap](roadmap.md) |
| Check release requirements | [Public preview criteria](beta.md) |
| Help develop it | [Contributing](../CONTRIBUTING.md) |
