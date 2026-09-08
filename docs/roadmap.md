# Roadmap

Epics in dependency order. Each should leave the compositor running and testable.

**E1 to E6 have landed.** The compositor boots on hardware, transforms and
animates windows through one engine, is scripted in Lua, has floating, tiling
and scrolling layouts, draws its own decorations from QML, and expresses every
mode as a script over the transform — which was the bet, and it held: none of
the modes needed new Rust.

E7 and E8 are open. So is the protocol work that turns a working compositor into
a usable one; that lives on the issue tracker rather than here, prioritised by
whether an application can be used at all without it.

For what has to be true before anyone else can run this, and the order to do it
in, see [beta.md](beta.md).

Each epic below keeps its original acceptance criteria, because what "done"
meant at the time is the more useful record.

## E1 — Boots and shows a window

Smithay skeleton with a Vulkan renderer. Winit backend for development, DRM for
real hardware. One xdg-shell client rendered on screen, keyboard and pointer
input working, clean shutdown.

Done when: a terminal opens in Solium under a nested session and accepts typing.

## E2 — Presentation transform and animation clock

The core abstraction. Every window gains a target rect, opacity, radius and
z-order used instead of real geometry, with one animation clock ticked from the
render loop. Input hit-testing follows the transform.

Done when: a hardcoded call scales a window to half size in a corner, animates
smoothly, and clicking it still focuses it. Layout is unchanged on reset.

## E3 — Lua scripting surface

Embed Lua. Expose enumeration (`windows`, `monitors`, `cursor`), transforms
(`present`, `present_clear`, `animate`), and binding (`on`, `grab_input`).

Done when: **overview mode exists as a script only**, no Rust changes. A key
scales every window onto a grid, a second press restores them, clicking a
thumbnail focuses that window.

This epic is the architecture's proof. If overview needs Rust, E2 is incomplete.

**Done.** `lua/overview.lua` is overview; `src/mode.rs` was deleted rather than
wrapped. Leaving restores the layout to the pixel — measured, not asserted.

## E4 — Layout engine: floating, tiling, scrolling

Three layouts behind one interface, switchable at runtime. Scrolling follows
niri's model: an infinite horizontal strip with a viewport.

Done when: all three are usable for real work and switching between them
animates through the transform layer rather than snapping.

## E5 — Decorations, hosted in-compositor

Window frames drawn by the compositor, reserving their own space so the client
shrinks and frame plus window read as one object. Titlebar, buttons, shadows,
rounding.

Done when: windows have working titlebars whose buttons respond, and the client
area is never covered.

## E6 — Modes as scripts

App switcher, peek-on-hover, and the icon→window genie — each a script over E2,
each shipping with the compositor as a default that users can replace.

The genie needs a dock icon rect. That rect arrives with the dock's frame
commit, never on a side channel.

Done when: all three work, and none required new Rust.

## E7 — Touch, gestures, form factors

Touch input, gesture recognition, and per-form-factor input profiles and mode
defaults. niri is the reference for touch on Smithay.

Done when: the same binary is usable by touch on a tablet and by pointer on a
desktop, with no code fork.

## E8 — Settings

Settings surface reading tunables declared by scripts rather than a fixed
compositor schema, so adding a mode adds its settings automatically.

Done when: changing a setting is visible immediately, with no restart.

---

## What is left to be a complete compositor

The epics above describe the *architecture* and it is finished: one engine, one
clock, modes as scripts, decorations in-process. What is left is almost none of
it. A compositor is complete when a person can use it all day without meeting
something it cannot do, and that is a different list — mostly protocols, mostly
unglamorous, and each one invisible until the day it is missing.

Three tiers, and the order matters more than the contents. This is the
*prioritised* view; **[docs/gaps.md](gaps.md)** is the exhaustive one — every
protocol not implemented and every non-protocol gap, whether or not it is worth
doing soon.

### 1. Things a desktop cannot do without

These are not features. Each one is a day where somebody stops using the
compositor and does not come back.

| | why it stops someone |
|---|---|
| ~~[#27](https://github.com/Lilium-Linux/solium/issues/27) session lock~~ | **done.** `ext-session-lock-v1`. It locks before the lock program draws, and stays locked if that program dies |
| ~~[#53](https://github.com/Lilium-Linux/solium/issues/53) keyboard layout~~ | **done.** `config.keyboard`, plus the repeat rate, which was the part with no way round it |
| [#26](https://github.com/Lilium-Linux/solium/issues/26) IME | no CJK, no compose key, no emoji picker. Unusable for most of the world's writers |
| [#55](https://github.com/Lilium-Linux/solium/issues/55) virtual-keyboard | the other half of an on-screen keyboard, which is what a phone is |
| [#56](https://github.com/Lilium-Linux/solium/issues/56) window rules | nothing matches on `app_id`. The first thing anybody configures |
| [#54](https://github.com/Lilium-Linux/solium/issues/54) output power | nothing can turn a screen off, which is what an idle timeout is for on a laptop |
| [#63](https://github.com/Lilium-Linux/solium/issues/63) multi-GPU | a monitor on the second card cannot be driven at all — which is docking an ordinary laptop |
| ~~[#36](https://github.com/Lilium-Linux/solium/issues/36) idle-inhibit~~ | **done**, with `ext-idle-notify` beside it — the two are one feature. Blanking policy is still nobody's: see `wlr-output-power-management` |
| [#43](https://github.com/Lilium-Linux/solium/issues/43) hotplug | **written, never run.** A monitor plugged in mid-session is picked up and one unplugged is dropped -- on paper. It cannot be tested here (no connectors nested, no `vkms` without root) and needs a session on the hardware |
| [#24](https://github.com/Lilium-Linux/solium/issues/24) cursor-shape | clients fall back today, so it costs nothing — until one does not |
| [#50](https://github.com/Lilium-Linux/solium/issues/50) foreign-toplevel | a dock cannot list windows or switch to them, so Lilium's own shell cannot have a task switcher |
| [#51](https://github.com/Lilium-Linux/solium/issues/51) output-management | monitors are configured in a file; E8's settings surface cannot move one at runtime, and `kanshi` cannot work |
| [#52](https://github.com/Lilium-Linux/solium/issues/52) data-control | no clipboard manager can work |

The last three were found by writing this section, which is the argument for
writing it. The shell is the reason all three matter more here than elsewhere:
a dock that cannot enumerate windows is a launcher, and a settings panel that
cannot move a monitor is a text editor with buttons.

### 2. Things that have to be true, not built

Correctness and confidence rather than capability. Nothing here adds a feature
and all of it decides whether the thing is trustworthy.

| | |
|---|---|
| [#33](https://github.com/Lilium-Linux/solium/issues/33) the per-window leak | ~5 MB and ~1 descriptor per window, measured nested. Whether it is real on hardware is untested |
| [#48](https://github.com/Lilium-Linux/solium/issues/48) the seat flake | two sessions in twenty-five got no input devices and were stopped by the watchdog |
| [#65](https://github.com/Lilium-Linux/solium/issues/65) a hardware soak | the compositor has never run unattended for hours on a real session |
| [#66](https://github.com/Lilium-Linux/solium/issues/66) packaging | there is none, and a preview nobody can install is a preview nobody tries |
| [#64](https://github.com/Lilium-Linux/solium/issues/64) suspend and resume | never tested once. A laptop that cannot be closed and opened is not a laptop |
| [#83](https://github.com/Lilium-Linux/solium/issues/83) portals | screen sharing is reasoning, not evidence: nothing has been run end to end |

### 3. The part that is not parity

Everything above brings Solium level with a good tiling compositor. None of it
is why this project exists.

- **[E7](https://github.com/Lilium-Linux/solium/issues/7) — touch, gestures, form factors.** One binary usable by touch on a
  tablet and by pointer on a desktop. The animation engine already takes an
  initial velocity precisely so a gesture's throw can be handed to it; nothing
  yet hands it one.
- **[E8](https://github.com/Lilium-Linux/solium/issues/8) — settings.** A surface built from what scripts declare rather than a
  fixed schema, so adding a mode adds its settings.
- **The dock, and the morph.** `sol.present_from` still does what
  `docs/shell-boundary.md` records: a window grows out of the rectangle an icon
  occupied. What is missing is a dock to give it a rectangle — and doing that
  across a process boundary is the design question that file says to reopen
  rather than route around.

### What "complete" would mean here

Not "has every protocol". A compositor is complete for this project when the
same binary runs a phone, a tablet and a desktop; when every mode is a script
somebody can rewrite; and when nothing in a normal day makes the user notice
they are running something unusual. The first is E7. The second is done. The
third is tier one.

**Honest position on evidence.** Everything in tiers one and two is judged from
nested runs and from reading. The hardware backend brings up both monitors,
picks modes and CRTCs correctly, and has been used with real applications on a
TTY — but no soak, no leak measurement and no HiDPI or screencopy test has ever
run on it. That is not a small caveat and it belongs in the plan rather than in
a footnote.

---

## Sequencing notes

- **E2 and E3 are the whole bet.** If overview cannot be a script, the
  architecture is wrong and it is cheap to find out early.
- **E5 comes after E3 deliberately.** Decorations were what the Hyprland fork
  attempted first, and they turned into a plumbing project that never reached
  the interesting work.
- **E7 is late but must not be an afterthought.** Design the input layer in E1
  so touch is a profile, not a retrofit.
