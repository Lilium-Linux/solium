# Project status

**Solium is alpha.** It runs as a nested compositor and as a hardware session.
The planned v0.1.0 is a public preview, with readiness determined by testing
and the remaining daily-use issues rather than a release date.

This documentation snapshot was checked against `stage` on 2026-11-05.
Use the [project board](https://github.com/orgs/Lilium-Linux/projects/2) for
work in progress and the
[`daily-drive` queue](https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20is%3Aopen%20label%3Adaily-drive)
for current daily-use priorities.

## Implemented

- Floating, dwindle tiling, scrolling and overview; workspaces and layouts per
  monitor, with animated presentation and XWayland support.
- Reloadable Lua configuration, QML frames and hosted desktop scenes, plus
  effect folders written in Lua and GLSL.
- A default configuration that is a whole desktop: taskbar, floating
  titlebar with quick settings, snapping, a stage-manager strip, quick search,
  Alt+Tab cards and a 3D workspace stack. It is copied on install and can be changed.
- Multiple outputs, hotplug, scaling and screen power; keyboard layouts,
  per-device libinput settings and cursor themes; focus-scoped shortcut
  inhibition for Wayland and XWayland, with configurable permission and reclaim
  keys, so games, VMs and remote desktops can have `super`.
- Solium's own lock screen, with an optional PIN, and session-lock with
  external lockers; idle inhibition, logind lock/sleep handling and
  installed-session integration with systemd and XDG autostart.
- Touch on windows and on compositor UI (frames and hosted scenes), with a
  touchscreen bound to its monitor by `input.touchscreen.output`.
- A notification server, PipeWire audio with every application's stream
  matched to its window, media players over MPRIS and a tray, published to QML
  scenes and Lua.
- Protocols for shells that are their own programs: listing and managing
  windows, seeing and switching workspaces, arranging monitors, capturing one
  window for a screen share and blurring what is behind a surface (#50, #60,
  #51, #47, #78).
- Lua that reads, writes and watches files without blocking, keeps settings
  across restarts in `sol.store` (#473), reads a program's output back and
  sets timers (#472).
- Fedora 44 and 45 packages in
  [COPR](https://copr.fedorainfracloud.org/coprs/liliumlinux/solium/),
  `solium-stage` from every push to `stage` (#486); checkout installation and
  a local RPM build script.

Implemented features can still have correctness and compatibility issues.
[Known limitations](gaps.md) names affected workflows, including IMEs,
clipboard privacy, gestures, explicit-sync hardware validation, multi-GPU and
portals.

## Validation and remaining risk

Earlier hardware checks at commit `dfc95ce` covered an NVIDIA RTX 3070 desktop
and a Microsoft Surface Pro 7 with Intel Ice Lake graphics, both on Fedora 44.
Those checks observed QML GPU rendering, animation, idle screen-off, swaylock
and the keyboard indicator. They are evidence for that revision and those
machines, not certification of current `stage` or other hardware.

Long hardware soaks and real application use still need to establish
reliability; suspend and resume have been checked by hand on two machines. Nested tests cover compositor behavior but cannot prove
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
