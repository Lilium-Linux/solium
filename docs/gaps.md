# Everything not built yet

The roadmap says why things come in the order they do. This says what there
*is* to do — the whole list, so that nothing is missing because nobody thought
to look for it. What to do *first* is the
[`daily-drive` label](https://github.com/Lilium-Linux/solium/issues?q=is%3Aissue%20is%3Aopen%20label%3Adaily-drive).
This page carries no status: a row leaves when its gap is closed.

Written by asking the compositor rather than by remembering: the "have" list
below is `wayland-info` against a running Solium, and the "missing" list is
every protocol in `wayland-protocols`, `wayland-protocols-wlr` and
`wayland-protocols-misc` that is not in it. Twenty-six globals in, about forty
out; the list below is the count.

Everything here is on the tracker except the dock row near the end: writing
this page found thirty-one gaps that were not, and they were filed as #53–#83.

A shell can run inside Solium, hosted in its QML engine, or as a program of its
own over `wlr-layer-shell` (see [shell-boundary.md](shell-boundary.md)). Where a
row below says a shell cannot do something, it means the second kind unless it
says otherwise.

## What is there today

`wl_compositor` · `wl_subcompositor` · `wl_shm` · `wl_seat` · `wl_output` ·
`wl_data_device_manager` · `xdg_wm_base` · `zxdg_decoration_manager_v1` ·
`zxdg_output_manager_v1` · `xdg_activation_v1` · `zwlr_layer_shell_v1` ·
`zwlr_screencopy_manager_v1` · `ext_session_lock_manager_v1` ·
`ext_idle_notifier_v1` · `zwp_idle_inhibit_manager_v1` ·
`zwlr_output_power_manager_v1` · `wp_presentation` ·
`wp_viewporter` · `wp_fractional_scale_manager_v1` · `zwp_linux_dmabuf_v1` ·
`xdg_wm_dialog_v1` · `wp_single_pixel_buffer_manager_v1` ·
`zwp_relative_pointer_manager_v1` · `zwp_pointer_constraints_v1` ·
`zwp_primary_selection_device_manager_v1` · `wp_cursor_shape_manager_v1`, plus
`xwayland_shell_v1` to the X server only.

---

## 1. Blocks a normal day

Each of these is a day where somebody stops using the compositor.

| | what breaks without it |
|---|---|
| [#26](https://github.com/Lilium-Linux/solium/issues/26) `text-input-v3`, `input-method-v2` | no CJK, no emoji picker, no on-screen keyboard — and the on-screen keyboard is what a phone is. A compose key does work meanwhile: `keyboard = { options = "compose:ralt" }` |
| [#50](https://github.com/Lilium-Linux/solium/issues/50) `ext-foreign-toplevel-list` | a shell cannot list windows, so it cannot have a task switcher; switching to one needs `zwlr_foreign_toplevel_management_v1` as well. A hosted shell has no window list either |
| [#51](https://github.com/Lilium-Linux/solium/issues/51) `wlr-output-management` | monitors are arranged by the configuration: `sol.monitors` places and scales them, and `super+shift+r` applies a change. No client can do it, so `kanshi` and a settings panel cannot work, and a monitor's mode and rotation are read only when it is added |
| [#52](https://github.com/Lilium-Linux/solium/issues/52) data-control | no clipboard manager can work |
| [#55](https://github.com/Lilium-Linux/solium/issues/55) `zwp_virtual_keyboard_v1` | the other half of an on-screen keyboard. `input-method-v2` says what was typed; this is how anything types it |
| [#56](https://github.com/Lilium-Linux/solium/issues/56) window rules | a script can read a window's `app_id`, and `tiling.client_size_ignore` matches on it, but there is no way to say "this application starts on that workspace, floating, this size". Not a protocol — a gap in the scripting surface, and the first thing anyone configures |

## 2. An application will hit it

Not a whole day lost — one application, or one workflow, that does not work.

| | who needs it |
|---|---|
| [#58](https://github.com/Lilium-Linux/solium/issues/58) `keyboard-shortcuts-inhibit` | a VM, a remote desktop, or a terminal multiplexer that wants `Super` for itself. Without it, those keys are eaten forever |
| [#59](https://github.com/Lilium-Linux/solium/issues/59) `linux-drm-syncobj` (explicit sync) | modern Vulkan and NVIDIA clients. Its absence is stutter and the occasional torn frame, and it is the single most-reported "your compositor is broken" on other projects |
| [#67](https://github.com/Lilium-Linux/solium/issues/67) `xdg-foreign-v2` | a file chooser or a screen-share dialog that has to be parented to the window that opened it. Otherwise it lands wherever the layout puts it |
| [#68](https://github.com/Lilium-Linux/solium/issues/68) `wp-security-context-v1` | how a Flatpak identifies itself. Without it there is no way to treat sandboxed clients differently, ever |
| [#60](https://github.com/Lilium-Linux/solium/issues/60) `ext-workspace-v1` | Solium *has* workspaces, in `workspaces.lua`, and no client can see or switch them. A shell cannot show which one you are on |
| [#69](https://github.com/Lilium-Linux/solium/issues/69) `zwlr_gamma_control_v1` | night light. `gammastep` and `redshift` speak only this |
| [#61](https://github.com/Lilium-Linux/solium/issues/61) KDE `server-decoration` | some Qt applications ask for decorations with this and nothing else, and draw none when it is missing |
| [#62](https://github.com/Lilium-Linux/solium/issues/62) `zwp_tablet_manager_v2` | drawing tablets, and the stylus on any 2-in-1 — which is a form factor this project is explicitly for |
| [#70](https://github.com/Lilium-Linux/solium/issues/70) `wp_pointer_gestures` | touchpad pinch and swipe reaching clients. A browser cannot pinch-zoom |
| [#37](https://github.com/Lilium-Linux/solium/issues/37) `tearing-control-v1` | a game that wants to opt out of vsync |
| [#71](https://github.com/Lilium-Linux/solium/issues/71) `wp_content_type_v1` | lets a surface say "I am video" so VRR and tearing decisions can be made for it rather than guessed |
| [#47](https://github.com/Lilium-Linux/solium/issues/47) `ext-image-copy-capture-v1` | the successor to `wlr-screencopy`. Nothing speaks it here yet; everything will |
| [#73](https://github.com/Lilium-Linux/solium/issues/73) `xdg_toplevel_icon_v1` | window icons — which a task switcher needs and cannot get any other way |
| [#75](https://github.com/Lilium-Linux/solium/issues/75) `wp_alpha_modifier_v1` | per-window opacity without asking the client to redraw. Free, given the render path already scales and warps |

## 3. Later, or for the shape this project is going

| | |
|---|---|
| [#76](https://github.com/Lilium-Linux/solium/issues/76) `wp_fifo_v1` + `wp_commit_timing_v1` | the modern frame-pacing pair. Solium's own pacing is good; this is how clients participate in it |
| [#77](https://github.com/Lilium-Linux/solium/issues/77) `wp_color_management_v1` | HDR, and the ceiling on [#46](https://github.com/Lilium-Linux/solium/issues/46) 10-bit output |
| [#78](https://github.com/Lilium-Linux/solium/issues/78) `ext_background_effect_v1` | blur behind a surface. For a compositor whose whole argument is that it looks like one thing, this is more interesting than it sounds |
| [#79](https://github.com/Lilium-Linux/solium/issues/79) the long tail | `xdg-session-management`, `wlr-virtual-pointer`, `drm-lease`, `ext-transient-seat`, `xdg-toplevel-tag`, `pointer-warp`, `color-representation`. One issue rather than seven, because none of them has a client or a story yet. Split one out the moment something real needs it |

Deliberately not, with reasons in [#79](https://github.com/Lilium-Linux/solium/issues/79): `wl_drm`, `zwlr_input_inhibit_v1`,
`fullscreen-shell`, `linux-explicit-synchronization`, `zwlr_export_dmabuf_v1`,
`input-timestamps`.

---

## 4. Not protocols

### The session

| | |
|---|---|
| [#146](https://github.com/Lilium-Linux/solium/issues/146) the session environment | nothing exports `WAYLAND_DISPLAY` and the rest to the systemd user manager or to D-Bus activation, and nothing starts `graphical-session.target`. Only programs Solium starts itself can find the display, so portals, D-Bus-activated applications, XDG autostart and a shell started as a user unit cannot |
| [#152](https://github.com/Lilium-Linux/solium/issues/152) D-Bus idle inhibit | browsers ask to keep the screen on over D-Bus (`org.freedesktop.ScreenSaver`, or the portal), and none of those requests reaches Solium. The screens go off after `idle.screens_off_after`, ten minutes by default, so a film in a browser can go dark |
| [#153](https://github.com/Lilium-Linux/solium/issues/153) logind | the `Lock` and `PrepareForSleep` signals are ignored, so `loginctl lock-session` and whatever locks that way do nothing, and locking before suspend is up to `swayidle -w` |
| [#157](https://github.com/Lilium-Linux/solium/issues/157) libinput device settings | none are set: no tap-to-click, which libinput leaves off, so tapping a touchpad does nothing; no acceleration profile or speed, disable-while-typing, left-handed mode or middle-button emulation; and natural scrolling comes only from the form factor, for every device at once |

### The backend

| | |
|---|---|
| [#63](https://github.com/Lilium-Linux/solium/issues/63) multi-GPU | `udev::primary_gpu` and nothing else. A laptop with a discrete card renders on one of them, and a monitor on the other card's port cannot be driven at all |
| [#44](https://github.com/Lilium-Linux/solium/issues/44) monitor identity | a screen is matched by which port it is in, so moving a cable moves the configuration |
| [#45](https://github.com/Lilium-Linux/solium/issues/45) mirroring | no way to put one screen on another. Every presentation wants it |
| [#46](https://github.com/Lilium-Linux/solium/issues/46) 10-bit | |
| [#80](https://github.com/Lilium-Linux/solium/issues/80) runtime rotation | `transform` is read from the configuration when a screen is added; a tablet cannot rotate when it is turned |
| [#42](https://github.com/Lilium-Linux/solium/issues/42) absolute devices | a touchscreen is glued to the first monitor |
| [#64](https://github.com/Lilium-Linux/solium/issues/64) suspend and resume | never tested. logind pauses and resumes the session's devices; whether Solium comes back is unknown |

### Correctness and confidence

| | |
|---|---|
| [#48](https://github.com/Lilium-Linux/solium/issues/48) the seat flake | three of forty-three logged sessions got no input devices. Devices arrive within a few seconds or not at all, so a longer watchdog would not have saved one; the cause is not known |
| [#65](https://github.com/Lilium-Linux/solium/issues/65) a hardware soak | the compositor has never run unattended for hours on a real session. `SOLIUM_SOAK_TTY=1 dev/soak.sh` can do it, though only its clients churn there, since the TTY backend ignores the scripted key presses; no run is recorded |
| [#38](https://github.com/Lilium-Linux/solium/issues/38) synthetic drags | they happen in one instant, so no timing-dependent test means anything |
| [#40](https://github.com/Lilium-Linux/solium/issues/40) xwayland selection flush | a workaround waiting on Smithay |
| [#66](https://github.com/Lilium-Linux/solium/issues/66) packaging | `dev/install.sh` installs from a checkout, but there are no packages: no Fedora `.spec` or COPR, no Arch `PKGBUILD`. `--config` ([#106](https://github.com/Lilium-Linux/solium/issues/106)) and the configuration directory's name ([#107](https://github.com/Lilium-Linux/solium/issues/107)) are worth settling first |
| [#82](https://github.com/Lilium-Linux/solium/issues/82) crash recovery | the compositor dying takes the session with it. There is no supervisor and nothing to come back to |

### The product

| | |
|---|---|
| [E7](https://github.com/Lilium-Linux/solium/issues/7) touch, gestures, form factors | the animation engine takes an initial velocity precisely so a gesture's throw can be handed to it. Nothing yet hands it one |
| [E8](https://github.com/Lilium-Linux/solium/issues/8) settings | a surface built from what scripts declare rather than a fixed schema |
| **the dock, and the morph** | `sol.present_from` already grows a window out of the rectangle an icon occupied, and a genie can aim at a window or a scripted surface. What is missing is a dock icon to aim at. A dock hosted in the compositor is in the same engine, and the plan is for its QML to name the icon so an animation can follow it while it moves; nothing of that is built, and no issue tracks it yet. Only a dock that runs as its own program would need a protocol to hand the rectangle over |
| [#49](https://github.com/Lilium-Linux/solium/issues/49) fullscreen animation | entering fullscreen snaps |
| [#30](https://github.com/Lilium-Linux/solium/issues/30) a minimise state | so the genie animation means something |
| [#83](https://github.com/Lilium-Linux/solium/issues/83) portals | `xdg-desktop-portal-wlr` can screen-share through `wlr-screencopy`, but nothing has been configured or tested end to end, a portal started by D-Bus cannot find the display until #146, and file chooser and settings portals are separate again |

---

## What this list is not

It is not a plan, and length is not weight: `wp_alpha_modifier_v1` is close to
free, since the render path already carries an opacity, while #146 is the
difference between a session whose portals and autostart work and one where
they cannot start. The label orders these and the roadmap says why; this only
makes sure none of them is forgotten.
