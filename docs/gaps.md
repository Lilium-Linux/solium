# Known limitations

Solium is alpha. This page describes compatibility gaps and known risks in the
current implementation, checked against `stage` and the tracker on 2026-10-20.
It is a guide to affected workflows, not an exhaustive protocol inventory or a
promise about priority.

Use the [project board](https://github.com/orgs/Lilium-Linux/projects/2) for
current work and the [`daily-drive` queue](https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20is%3Aopen%20label%3Adaily-drive)
for daily-use priorities. [Status](status.md) describes what is implemented;
[roadmap](roadmap.md) describes the longer-term direction.

## Input and accessibility

| Limitation | Impact | Tracking |
|---|---|---|
| Input methods | `text-input-v3` supplies focus and caret information, but there is no input-method integration for IMEs or an on-screen keyboard. | [#26](https://github.com/Lilium-Linux/solium/issues/26), [#55](https://github.com/Lilium-Linux/solium/issues/55) |
| Gestures | Client pinch/swipe protocols and configurable compositor gestures remain incomplete. | [#70](https://github.com/Lilium-Linux/solium/issues/70), [#252](https://github.com/Lilium-Linux/solium/issues/252), [#7](https://github.com/Lilium-Linux/solium/issues/7) |
| Tablet tools | No tablet-v2 support for pen pressure and stylus tools. | [#62](https://github.com/Lilium-Linux/solium/issues/62) |
| Shortcut inhibition | VMs, remote desktops and nested compositors can take the compositor's shortcuts through keyboard-shortcuts-inhibit, and `super+Escape` takes them back. Any client may ask, with no per-application policy, no indicator, and no XWayland keyboard grab for X11 games. | [#245](https://github.com/Lilium-Linux/solium/issues/245) |
| Absolute-device mapping | Touchscreens and other absolute devices need configurable output mapping. | [#42](https://github.com/Lilium-Linux/solium/issues/42) |

Keyboard layouts, repeat, tap-to-click and per-device libinput settings are
implemented; they are not missing features. See
[input configuration](ricing.md) for defaults and overrides.

## External shells and desktop tools

Hosted QML scenes can read `Windows`, `Workspaces`, `Monitors`, `Apps` and
`Folder`. Separate clients need Wayland protocols for equivalent access, and
several are missing:

| Limitation | Impact | Tracking |
|---|---|---|
| Window and workspace management | External taskbars and workspace switchers lack foreign-toplevel and workspace-management interfaces. | [#50](https://github.com/Lilium-Linux/solium/issues/50), [#60](https://github.com/Lilium-Linux/solium/issues/60) |
| Display configuration clients | Tools such as kanshi cannot arrange outputs through output-management; use Lua monitor configuration instead. | [#51](https://github.com/Lilium-Linux/solium/issues/51) |
| Clipboard managers | Data-control (wlr and ext) works, but every client is offered it, so any client can read every copy without focus. Limiting it to trusted clients needs security-context. | [#68](https://github.com/Lilium-Linux/solium/issues/68) |
| Client-supplied window icons | Application icons resolve through desktop files; per-toplevel icons through xdg-toplevel-icon are unavailable. | [#73](https://github.com/Lilium-Linux/solium/issues/73) |
| External background effects | Compositor effects exist, but clients cannot request blur through ext-background-effect. | [#78](https://github.com/Lilium-Linux/solium/issues/78) |
| Portals | Screencopy and session portal configuration exist; screen sharing and the wider portal flow still need end-to-end validation. | [#83](https://github.com/Lilium-Linux/solium/issues/83) |
| Preview desktop services | The preview has a bar, dock, search and desktop icons. Solium is the notification server, and reads audio from PipeWire, for scenes and Lua (#346, #347); media, power, network, Bluetooth and the tray are not services yet, audio has no per-application streams, levels or ports, and notifications have no inline reply, grouping, history on disk or focus modes. | [#169](https://github.com/Lilium-Linux/solium/issues/169) |
| Native lock screen | Solium's own lock is the session's by default, taken by logind's `Lock`, sleep, `idle.lock_after`, `sol.lock_now` and `super+ctrl+l`, but it has not yet been run on hardware from a text console. A PIN (`solium pin set`) unlocks it in place of the password, with the password needed after a restart, after `lock.pin.attempts` wrong PINs and every `lock.pin.password_after_hours`; it waits for a security review and a run on hardware. What is drawn over it, the lock's scenes and the pointer's, sees no windows, workspaces or apps at all, only notifications (#346): what a lock may show of the rest (redaction in place), media on the lock, hiding notifications there, fingerprint, and glass behind the passcode are later work. | [#281](https://github.com/Lilium-Linux/solium/issues/281) |

## Displays, rendering and gaming

| Limitation | Impact | Tracking |
|---|---|---|
| Explicit sync | linux-drm-syncobj-v1 is conditional on dmabuf and DRM syncobj-eventfd support. Vulkan/NVIDIA session, scanout and resume validation remains a hardware check; use `dev/explicit-sync-tty.sh`. | [#59](https://github.com/Lilium-Linux/solium/issues/59) |
| Multiple GPUs | Outputs on another GPU are not supported by the current single-GPU path. | [#63](https://github.com/Lilium-Linux/solium/issues/63) |
| Monitor identity and mirroring | Configuration matches connectors; stable panel identity and mirrored outputs remain open. | [#44](https://github.com/Lilium-Linux/solium/issues/44), [#45](https://github.com/Lilium-Linux/solium/issues/45) |
| Runtime rotation | Output transform is applied when an output is added; automatic/runtime rotation is incomplete. | [#80](https://github.com/Lilium-Linux/solium/issues/80) |
| HDR and color management | 10-bit output, HDR metadata, client color descriptions and a linear-light composition pipeline remain open. | [#46](https://github.com/Lilium-Linux/solium/issues/46), [#77](https://github.com/Lilium-Linux/solium/issues/77), [#248](https://github.com/Lilium-Linux/solium/issues/248), [#249](https://github.com/Lilium-Linux/solium/issues/249), [#250](https://github.com/Lilium-Linux/solium/issues/250), [#251](https://github.com/Lilium-Linux/solium/issues/251) |
| Night light | Gamma-control is missing. | [#69](https://github.com/Lilium-Linux/solium/issues/69) |
| Game presentation | Tearing control, fullscreen-only VRR policy and preserving direct scanout need further work and hardware checks. | [#37](https://github.com/Lilium-Linux/solium/issues/37), [#243](https://github.com/Lilium-Linux/solium/issues/243), [#244](https://github.com/Lilium-Linux/solium/issues/244) |
| Client frame pacing | fifo and commit-timing are not implemented. | [#76](https://github.com/Lilium-Linux/solium/issues/76) |
| Modern capture | wlr-screencopy exists; ext-image-copy-capture does not. | [#47](https://github.com/Lilium-Linux/solium/issues/47) |
| VR and streaming | DRM leasing and game-streaming validation are separate open work. | [#247](https://github.com/Lilium-Linux/solium/issues/247), [#246](https://github.com/Lilium-Linux/solium/issues/246) |

## Window behavior and effects

| Limitation | Impact | Tracking |
|---|---|---|
| Window rules | There is no general declarative rule system for placing or sizing applications by identity. | [#56](https://github.com/Lilium-Linux/solium/issues/56) |
| Close confirmation | An application asking for confirmation can disappear during close grace and return afterward. | [#231](https://github.com/Lilium-Linux/solium/issues/231) |
| Focus policy | Focus-stealing prevention and related correctness follow-ups remain open. | [#136](https://github.com/Lilium-Linux/solium/issues/136), [#258](https://github.com/Lilium-Linux/solium/issues/258) |
| Tiled client state | Some clients still need correct tiled-state reporting. | [#228](https://github.com/Lilium-Linux/solium/issues/228) |
| Frame styles | Per-window style selection, style-switch stalls and individual frame input problems remain tracked. | [#234](https://github.com/Lilium-Linux/solium/issues/234), [#226](https://github.com/Lilium-Linux/solium/issues/226), [#158](https://github.com/Lilium-Linux/solium/issues/158) |
| User QML overrides | The shipped Solium module can win over user QML; do not assume a user Theme.qml will override it. | [#88](https://github.com/Lilium-Linux/solium/issues/88) |
| Live effect tuning | Effect files and shipped effects exist; schema-driven live tuning and presets remain planned. | [#259](https://github.com/Lilium-Linux/solium/issues/259) |
| Fast-resize glass | Backdrop quality during very fast resize still needs work. | [#262](https://github.com/Lilium-Linux/solium/issues/262) |
| Minimizing and morphs | Minimize and its way back exist (#30), with the genie into a fixed strip; an effects-driven minimize, live icon anchors and cross-scene flights/morphs do not. | [#202](https://github.com/Lilium-Linux/solium/issues/202), [#204](https://github.com/Lilium-Linux/solium/issues/204), [#205](https://github.com/Lilium-Linux/solium/issues/205), [#206](https://github.com/Lilium-Linux/solium/issues/206), [#207](https://github.com/Lilium-Linux/solium/issues/207) |
| Other client protocols | KDE server-decoration, security-context and alpha-modifier remain missing. | [#61](https://github.com/Lilium-Linux/solium/issues/61), [#68](https://github.com/Lilium-Linux/solium/issues/68), [#75](https://github.com/Lilium-Linux/solium/issues/75) |

## Session and reliability

| Limitation | Impact | Tracking |
|---|---|---|
| Hardware validation | Long hardware soaks remain a release-readiness check; nested tests cannot establish their reliability. Suspend and resume, closing and opening the lid included, were checked by hand on a Surface Pro 7 and a laptop (#64). | [#65](https://github.com/Lilium-Linux/solium/issues/65) |
| X11 launch placeholders | An X11 application may fail to replace its loading pane. | [#180](https://github.com/Lilium-Linux/solium/issues/180) |
| XWayland selections | The selection-flush workaround still needs upstream follow-up. | [#40](https://github.com/Lilium-Linux/solium/issues/40) |
| Crash recovery | A compositor crash ends the graphical session; applications are not recovered into a replacement compositor. | [#82](https://github.com/Lilium-Linux/solium/issues/82) |
| Distribution packages | Checkout installation and Fedora RPM builds exist; a published package repository and Arch packaging remain open. | [#66](https://github.com/Lilium-Linux/solium/issues/66) |
| Test reliability | Parallel test crashes and timing-sensitive scenarios can undermine gate confidence and need investigation. | [#237](https://github.com/Lilium-Linux/solium/issues/237), [#241](https://github.com/Lilium-Linux/solium/issues/241), [#256](https://github.com/Lilium-Linux/solium/issues/256) |

Logind lock/sleep handling is implemented, as is the input-startup fix. logind's
`Lock` and sleep take Solium's own lock screen (#281), or run `lock.command`
when one is set.
Their implementation does not replace suspend/resume testing on each machine.

For protocol validation, use the [external probes](../dev/README.md#wl-probe)
and `wayland-info` against the Solium socket. A global being advertised does
not establish every request's behavior. Backend-specific globals can differ.
Additional protocol requests are tracked under
[#79](https://github.com/Lilium-Linux/solium/issues/79).
