# Everything not built yet

The roadmap says what to do next. This says what there *is* to do — the whole
list, so that nothing is missing because nobody thought to look for it.

Written by asking the compositor rather than by remembering: the "have" list
below is `wayland-info` against a running Solium, and the "missing" list is
every protocol in `wayland-protocols`, `wayland-protocols-wlr` and
`wayland-protocols-misc` that is not in it. Twenty-two globals in, about forty
out. Items with an issue number are on the tracker; **items in bold with no
number are not, and writing this is how they were found.**

## What is there today

`wl_compositor` · `wl_subcompositor` · `wl_shm` · `wl_seat` · `wl_output` ·
`wl_data_device_manager` · `xdg_wm_base` · `zxdg_decoration_manager_v1` ·
`zxdg_output_manager_v1` · `xdg_activation_v1` · `zwlr_layer_shell_v1` ·
`zwlr_screencopy_manager_v1` · `ext_session_lock_manager_v1` ·
`ext_idle_notifier_v1` · `zwp_idle_inhibit_manager_v1` · `wp_presentation` ·
`wp_viewporter` · `wp_fractional_scale_manager_v1` · `zwp_linux_dmabuf_v1` ·
`zwp_relative_pointer_manager_v1` · `zwp_pointer_constraints_v1` ·
`zwp_primary_selection_device_manager_v1`, plus `xwayland_shell_v1` to the X
server only.

---

## 1. Blocks a normal day

Each of these is a day where somebody stops using the compositor.

| | what breaks without it |
|---|---|
| **keyboard layout** | `add_keyboard(Default::default(), …)` — every session is US QWERTY with a fixed repeat rate, and there is no way to change either. Most of the world cannot type their own language. Nothing else on this page locks out more people |
| [#26](https://github.com/Lilium-Linux/solium/issues/26) `text-input-v3`, `input-method-v2` | no CJK, no compose key, no emoji picker, no on-screen keyboard — and the on-screen keyboard is what a phone is |
| [#43](https://github.com/Lilium-Linux/solium/issues/43) hotplug | a laptop lid, a dock, a monitor's power switch. "Restart your session" is not an answer |
| [#24](https://github.com/Lilium-Linux/solium/issues/24) `cursor-shape-v1` | clients fall back today, so it costs nothing — until one does not |
| [#50](https://github.com/Lilium-Linux/solium/issues/50) `ext-foreign-toplevel-list` | a dock cannot list windows or switch to them, so Lilium's own shell cannot have a task switcher |
| [#51](https://github.com/Lilium-Linux/solium/issues/51) `wlr-output-management` | monitors live in a file; nothing can move one at runtime and `kanshi` cannot work |
| [#52](https://github.com/Lilium-Linux/solium/issues/52) data-control | no clipboard manager can work |
| **`wlr-output-power-management`** | nothing can turn a screen off. Solium now *reports* idleness and can be told to stay awake, and still cannot blank a backlight — which is the actual point of an idle timeout on a laptop |
| **`zwp_virtual_keyboard_v1`** | the other half of an on-screen keyboard. `input-method-v2` says what was typed; this is how anything types it |
| **window rules** | no way to say "this application starts on that workspace, floating, this size". Not a protocol — a gap in the scripting surface, and the first thing anyone configures |
| **the drag icon** | nothing draws the surface a client attaches to a drag, so a drag between windows is invisible while it is happening |

## 2. An application will hit it

Not a whole day lost — one application, or one workflow, that does not work.

| | who needs it |
|---|---|
| **`zwp_keyboard_shortcuts_inhibit_v1`** | a VM, a remote desktop, or a terminal multiplexer that wants `Super` for itself. Without it, those keys are eaten forever |
| **`linux-drm-syncobj-v1`** (explicit sync) | modern Vulkan and NVIDIA clients. Its absence is stutter and the occasional torn frame, and it is the single most-reported "your compositor is broken" on other projects |
| **`xdg-foreign-v2`** | a file chooser or a screen-share dialog that has to be parented to the window that opened it. Otherwise it lands wherever the layout puts it |
| **`wp-security-context-v1`** | how a Flatpak identifies itself. Without it there is no way to treat sandboxed clients differently, ever |
| **`ext-workspace-v1`** | Solium *has* workspaces, in `workspaces.lua`, and no client can see or switch them. A dock cannot show which one you are on |
| **`zwlr_gamma_control_v1`** | night light. `gammastep` and `redshift` speak only this |
| **KDE `server-decoration`** | some Qt applications ask for decorations with this and nothing else, and draw none when it is missing |
| **`zwp_tablet_manager_v2`** | drawing tablets, and the stylus on any 2-in-1 — which is a form factor this project is explicitly for |
| **`wp_pointer_gestures`** | touchpad pinch and swipe reaching clients. A browser cannot pinch-zoom |
| [#37](https://github.com/Lilium-Linux/solium/issues/37) `tearing-control-v1` | a game that wants to opt out of vsync |
| **`wp_content_type_v1`** | lets a surface say "I am video" so VRR and tearing decisions can be made for it rather than guessed |
| [#47](https://github.com/Lilium-Linux/solium/issues/47) `ext-image-copy-capture-v1` | the successor to `wlr-screencopy`. Nothing speaks it here yet; everything will |
| **`xdg_dialog_v1`** | a modal dialog that says it is modal, so a layout can treat it as one |
| **`xdg_toplevel_icon_v1`** | window icons — which a task switcher needs and cannot get any other way |
| **`wp_single_pixel_buffer_v1`** | three lines of work; a handful of clients use it for solid backgrounds and fail without it |
| **`wp_alpha_modifier_v1`** | per-window opacity without asking the client to redraw. Free, given the render path already scales and warps |

## 3. Later, or for the shape this project is going

| | |
|---|---|
| **`wp_fifo_v1` + `wp_commit_timing_v1`** | the modern frame-pacing pair. Solium's own pacing is good; this is how clients participate in it |
| **`wp_color_management_v1`** | HDR, and the ceiling on [#46](https://github.com/Lilium-Linux/solium/issues/46) 10-bit output |
| **`ext_background_effect_v1`** | blur behind a surface. For a compositor whose whole argument is that it looks like one thing, this is more interesting than it sounds |
| **`xdg_session_management_v1`** | windows come back where they were after a restart |
| **`zwlr_virtual_pointer_v1`** | remote control, accessibility tooling, `ydotool` |
| **`wp_drm_lease_v1`** | VR headsets |
| **`ext_transient_seat_v1`** | a second seat for one program. Solium has exactly one seat, hardcoded |
| **`xdg_toplevel_tag_v1`** | stable identity for a window across restarts |
| **`wp_pointer_warp_v1`**, **`wp_color_representation_v1`** | new staging protocols, no clients yet |

Deliberately not: `wl_drm` (superseded by dmabuf), `zwlr_input_inhibit_v1`
(superseded by session-lock), `fullscreen-shell` (dead), `linux-explicit-sync`
(superseded by drm-syncobj), `zwlr_export_dmabuf_v1` (superseded by
image-copy-capture), `input-timestamps` (no client asks).

---

## 4. Not protocols

### The backend

| | |
|---|---|
| [#43](https://github.com/Lilium-Linux/solium/issues/43) hotplug | listed above because it is tier one, repeated here because it is a udev problem, not a protocol |
| **multi-GPU** | `udev::primary_gpu` and nothing else. A laptop with a discrete card renders on one of them, and a monitor on the other card's port cannot be driven at all |
| [#44](https://github.com/Lilium-Linux/solium/issues/44) monitor identity | a screen is matched by which port it is in, so moving a cable moves the configuration |
| [#45](https://github.com/Lilium-Linux/solium/issues/45) mirroring | no way to put one screen on another. Every presentation wants it |
| [#46](https://github.com/Lilium-Linux/solium/issues/46) 10-bit | |
| **runtime rotation** | `transform` is read from the configuration at startup; a tablet cannot rotate when it is turned |
| [#42](https://github.com/Lilium-Linux/solium/issues/42) absolute devices | a touchscreen is glued to the first monitor |
| **suspend and resume** | never tested. logind pauses and resumes the session's devices; whether Solium comes back is unknown |
| **cursor themes** | the pointer is drawn from QML, which is deliberate, but `XCURSOR_THEME` is what every other application on the machine follows and there is no way to match it |

### Correctness and confidence

| | |
|---|---|
| [#33](https://github.com/Lilium-Linux/solium/issues/33) the per-window leak | ~190 KB and a descriptor per window lifecycle, measured nested, isolated to the window lifecycle and not yet to a line |
| [#48](https://github.com/Lilium-Linux/solium/issues/48) the seat flake | two sessions in twenty-five got no input devices |
| **a hardware soak** | the compositor has never run unattended for hours on a real session. Everything in tiers one and two above is judged from nested runs and from reading |
| [#38](https://github.com/Lilium-Linux/solium/issues/38) synthetic drags | they happen in one instant, so no timing-dependent test means anything |
| [#40](https://github.com/Lilium-Linux/solium/issues/40) xwayland selection flush | a workaround waiting on Smithay |
| **packaging** | there is none. A preview nobody can install is a preview nobody tries |
| **crash recovery** | the compositor dying takes the session with it. There is no supervisor and nothing to come back to |

### The product

| | |
|---|---|
| [E7](https://github.com/Lilium-Linux/solium/issues/7) touch, gestures, form factors | the animation engine takes an initial velocity precisely so a gesture's throw can be handed to it. Nothing yet hands it one |
| [E8](https://github.com/Lilium-Linux/solium/issues/8) settings | a surface built from what scripts declare rather than a fixed schema |
| **the dock, and the morph** | `sol.present_from` already grows a window out of the rectangle an icon occupied. What is missing is a dock to give it a rectangle, and doing that across a process boundary is the open design question in `docs/shell-boundary.md` |
| [#49](https://github.com/Lilium-Linux/solium/issues/49) fullscreen animation | entering fullscreen snaps |
| [#30](https://github.com/Lilium-Linux/solium/issues/30) a minimise state | so the genie animation means something |
| **portals** | `xdg-desktop-portal-wlr` can screen-share through `wlr-screencopy` now, but nothing has been configured or tested end to end, and file chooser and settings portals are separate again |

---

## What this list is not

It is not a plan, and length is not weight: `wp_single_pixel_buffer_v1` is an
afternoon and keyboard layout is the difference between a compositor a person
can use and one they cannot. The roadmap orders these; this only makes sure
none of them is forgotten.
