# Roadmap

Epics in dependency order. Each should leave the compositor running and testable.

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

Embed Lua. Expose enumeration (`windows`, `monitor`, `cursor`), transforms
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

## Sequencing notes

- **E2 and E3 are the whole bet.** If overview cannot be a script, the
  architecture is wrong and it is cheap to find out early.
- **E5 comes after E3 deliberately.** Decorations were what the Hyprland fork
  attempted first, and they turned into a plumbing project that never reached
  the interesting work.
- **E7 is late but must not be an afterthought.** Design the input layer in E1
  so touch is a profile, not a retrofit.
