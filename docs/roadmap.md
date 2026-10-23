# Roadmap

Solium's direction is one configurable compositor across desktop, laptop,
tablet and phone. Lua, QML and effect files select the experience over common
Rust primitives. Device support and a complete mobile desktop are goals, not
claims about today's support.

This page groups the work by outcome. The
[project board](https://github.com/orgs/Lilium-Linux/projects/2) and
[`daily-drive` issues](https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20is%3Aopen%20label%3Adaily-drive)
carry current status and priority. [Known limitations](gaps.md) describes
compatibility; [preview criteria](beta.md) describes release readiness.

## Foundations already in place

The compositor boots nested and on hardware, hosts QML window frames and
scenes, runs Lua modes, and shares an animation/presentation engine across
floating, tiling, scrolling and overview. Multi-output support, session
integration, device settings, user effect folders and the first preview-shell
components are implemented. These are foundations to improve, not unfinished
epics to start from scratch.

## Reliable daily use

Fix correctness issues found in applications, window management, focus and
frame input. Validate hotplug and long sessions on hardware.
Complete portal workflows and make failures diagnosable. This work gates a
credible public preview more directly than adding another visual mode.

Tracking: [daily-drive queue](https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20is%3Aopen%20label%3Adaily-drive),
[#64](https://github.com/Lilium-Linux/solium/issues/64),
[#65](https://github.com/Lilium-Linux/solium/issues/65),
[#83](https://github.com/Lilium-Linux/solium/issues/83).

## A complete, replaceable desktop

The preview already supplies a bar, dock, search and desktop icons. Complete
service-backed UI, widgets and native locking while keeping the whole desktop
replaceable configuration. Improve external-shell compatibility through the
missing management protocols rather than requiring every shell to be hosted.

Tracking: [hosted desktop](https://github.com/Lilium-Linux/solium/issues/169),
[window management](https://github.com/Lilium-Linux/solium/issues/50),
[workspaces](https://github.com/Lilium-Linux/solium/issues/60).

## Effects and presentation

User-authored effect folders and shipped blur, glass, shade and genie already
exist. Continue work on live tuning, effect quality and performance, minimize
and restore, live anchors, and cross-scene flights/morphs. A shared QML engine
enables shared data and components; it does not by itself implement morphing.

Tracking: [effects](https://github.com/Lilium-Linux/solium/issues/87),
[live tuning](https://github.com/Lilium-Linux/solium/issues/259),
[presentation/layouts](https://github.com/Lilium-Linux/solium/issues/84),
[modes](https://github.com/Lilium-Linux/solium/issues/85),
[flights](https://github.com/Lilium-Linux/solium/issues/204),
[morphs](https://github.com/Lilium-Linux/solium/issues/206).

## Input, settings and form factors

Extend touch from client surfaces to compositor UI, then add configurable
trackpad/touchscreen gestures that can drive animation progress. Complete
input methods, on-screen keyboards and tablet tools. Build settings UI over
declared configuration instead of introducing another source of truth.

Phones using Android drivers require a separate compatibility investigation;
the desktop compositor running on Linux does not establish that support.

Tracking: [touch/form factors](https://github.com/Lilium-Linux/solium/issues/7),
[gestures](https://github.com/Lilium-Linux/solium/issues/252),
[settings](https://github.com/Lilium-Linux/solium/issues/8),
[input methods](https://github.com/Lilium-Linux/solium/issues/26),
[Halium](https://github.com/Lilium-Linux/solium/issues/255).

## Graphics and gaming compatibility

Add explicit sync, multi-GPU output and color management/HDR. Improve fullscreen
presentation, shortcut inhibition, VR support and game-streaming workflows.
These need real driver and device checks alongside automated coverage.

Tracking: [explicit sync](https://github.com/Lilium-Linux/solium/issues/59),
[multi-GPU](https://github.com/Lilium-Linux/solium/issues/63),
[HDR](https://github.com/Lilium-Linux/solium/issues/77),
[daily-drive gaming](https://github.com/Lilium-Linux/solium/issues/254).

## Distribution and extensibility

Checkout installs and Fedora RPM builds exist. Publish installable, updatable
packages and broaden distribution coverage once their session and runtime
requirements are validated. WebAssembly components are a longer-term extension
direction; Lua remains the current scripting API.

Tracking: [packaging](https://github.com/Lilium-Linux/solium/issues/66),
[WebAssembly components](https://github.com/Lilium-Linux/solium/issues/239).
