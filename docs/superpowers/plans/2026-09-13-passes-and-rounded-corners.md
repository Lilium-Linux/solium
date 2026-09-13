# Passes, and One Effect Through Them Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An effect declares what it reads, and that declaration is what splits a frame into passes — landed with rounded corners as the only effect.

**Architecture:** An `Effect` is data: a name, parameters, and `inputs`. An effect declaring nothing draws inline exactly as today. One declaring `self` makes the renderer render the pane's client into a texture first — which `offscreen::capture` already does and already caches — and then draw that texture through a GLES fragment program instead of drawing the surfaces straight. `GlesFrame::override_default_tex_program` is the seam. The element that results reports a *smaller* opaque region than the rect it covers, because a rounded window is no longer opaque at its corners, and the renderer's culling reads that.

**Tech Stack:** Rust edition 2024, Smithay 0.7 (`GlesRenderer::compile_custom_texture_shader`, `GlesFrame::override_default_tex_program`, `TextureRenderElement`), GLSL ES 1.00, QML for the declaration.

**Spec:** `docs/superpowers/specs/2026-09-12-panes-and-effects-design.md` — the sections *An effect declares its inputs, and that is what creates a pass* and *What stays cheap*.

## Global Constraints

- Rust edition 2024. Workspace lints **deny** `unwrap_used`, `expect_used`, `panic`, `todo`. `unsafe_code` is `warn` — every `unsafe` block needs `#[expect(unsafe_code, reason = "…")]`.
- **All builds run in podman** via `dev/gate.sh`, which takes NO arguments. The host has no Qt 6 development files. The built binary *does* run on the host.
- **A node with no transform, no effect and no bleed emits exactly the element it emits now, through exactly the path it takes now.** The spike's rule is load-bearing: "a compositor that renders every window through a mesh to support an effect nobody is currently running has made every frame worse to make one frame possible."
- **Rounded corners and nothing else.** `backdrop` is designed for here but not implemented; blur is out of scope.
- `client.radius` is already a reserved key read by `style.rs` and consumed by nothing. This plan is what consumes it.
- Depth vocabulary is exactly `"behind"`, `"frame"`, `"above"`.
- Never launch `claude-desktop`. `sudo` is not available.
- The user has a live Solium on tty3 out of this worktree: **never `pkill` by name.** Kill only pids you started.
- Verify rendering with the nested harness, not by reasoning. `SOLIUM_QML_GPU=1` nested renders no QML at all (no GBM device); `--check-qml` exits 0 on failure. See the memory `solium-verifying-qml-nested`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/effects/src/fragment.rs` (new) | `Effect`, `Inputs`, and the rounded-corner shader source. Pure data and a string; no GL, no compositor types, unit-testable. |
| `crates/solium/src/style.rs` (modify) | Read `client.radius` off `PaneStyle` into `Style::effects`. |
| `crates/solium/src/pass.rs` (new) | The pass: compile-and-cache the program, capture the client, build the element. The only file that knows both `solium_effects` and `GlesRenderer`. |
| `crates/solium/src/render.rs` (modify) | `Piece::Client` asks `pass` whether this pane needs a pass before emitting surfaces. |
| `dev/wirecheck/src/main.rs` (modify) | A census case for the compiled program, so a shader that stops compiling reds the gate. |

---

### Task 1: `Effect` is data, and says what it reads

**Files:**
- Create: `crates/effects/src/fragment.rs`
- Modify: `crates/effects/src/lib.rs` (add `pub mod fragment;`)

**Interfaces:**
- Produces: `solium_effects::fragment::{Effect, Inputs, ROUNDED_CORNERS, RADIUS_UNIFORM, SIZE_UNIFORM}`. `Effect::rounded(radius: f64) -> Effect`, `Effect::inputs(&self) -> Inputs`, `Effect::radius(&self) -> f64`, `Effect::is_none_effect(&self) -> bool`.

- [ ] **Step 1: Write the failing test**

Append to `crates/effects/src/fragment.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// An effect that reads nothing draws inline; one that reads `self` needs
    /// the node rendered to a texture first. That distinction is the whole
    /// mechanism, so it is a value and not a comment.
    #[test]
    fn rounded_corners_reads_the_node_itself() {
        let effect = Effect::rounded(12.0);
        assert_eq!(effect.inputs(), Inputs::SelfTexture);
        assert!((effect.radius() - 12.0).abs() < f64::EPSILON);
    }

    /// A radius of zero is not a rounded window with no rounding: it is no
    /// effect at all, and has to stay off the pass path entirely or every
    /// window on the machine pays for a capture to be drawn square.
    #[test]
    fn a_zero_radius_is_not_an_effect() {
        assert!(Effect::rounded(0.0).is_none_effect());
        assert!(Effect::rounded(-4.0).is_none_effect());
        assert!(!Effect::rounded(1.0).is_none_effect());
    }

    /// The shader is handed to Smithay, whose contract for a *texture* program
    /// is not the one its own documentation states. Read from the source, not
    /// the doc comment:
    ///
    /// | | texture program | pixel program |
    /// |---|---|---|
    /// | marker | `//_DEFINES_` | `//_DEFINES_` |
    /// | `#version` | **the shader supplies it** | smithay prepends it |
    /// | `size` uniform | **not provided** | provided |
    ///
    /// `gles/mod.rs:1964` says the marker is `//_DEFINES`, without the
    /// trailing underscore. `shaders/mod.rs:125` is what actually runs and it
    /// replaces `//_DEFINES_`. A shader carrying the documented spelling has
    /// its marker left in place as a comment, compiles, and then fails to link
    /// because the `#define`s it needed were never substituted.
    ///
    /// None of this fails until there is a GPU, which is why it is asserted
    /// here and compiled for real in wirecheck.
    #[test]
    fn the_shader_is_shaped_the_way_smithay_actually_requires() {
        // Whole line, not `contains`: `contains("//_DEFINES")` is true of the
        // WRONG spelling too, because it is a prefix of the right one. That
        // near-miss is the bug this test exists to catch, so matching a
        // substring here would make the test agree with the defect.
        assert!(
            ROUNDED_CORNERS.lines().any(|line| line.trim() == "//_DEFINES_"),
            "smithay replaces a line that is exactly `//_DEFINES_` with its #defines"
        );
        assert!(
            ROUNDED_CORNERS.starts_with("#version 100"),
            "texture_program does NOT prepend a version -- the built-in \
             texture.frag carries its own, and so must this"
        );
        assert!(ROUNDED_CORNERS.contains(RADIUS_UNIFORM));
        assert!(
            ROUNDED_CORNERS.contains(SIZE_UNIFORM),
            "a texture program gets no `size` uniform from smithay; only a \
             pixel program does, so this one has to declare its own"
        );
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL to compile — `fragment` is not a module.

- [ ] **Step 3: Write the module**

Create `crates/effects/src/fragment.rs`:

```rust
//! What an effect *reads*, which is what decides whether a frame needs a pass.
//!
//! Most effects draw over what is already there and need nothing: a wavy
//! border, a glow, spikes. They are ordinary elements and this module has
//! nothing to say about them. An effect that needs the node's own pixels --
//! rounded corners masks them, a shadow is derived from their silhouette --
//! cannot be one element in a flat list, because a flat list has nowhere to
//! say "after the things below me, before the things above me".
//!
//! So `inputs` is the declaration, and the renderer reads it. This crate holds
//! the declaration and the shader text; `crates/solium/src/pass.rs` is what
//! compiles and runs it.

/// What an effect needs before it can draw.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Inputs {
    /// Draws over what is there. No pass, no capture, no cost.
    Nothing,
    /// The node's own pixels, rendered to a texture first.
    SelfTexture,
    /// What is already composited *beneath* the node.
    ///
    /// Declared here and implemented nowhere. It is what blur needs, and
    /// naming it is most of why this enum exists -- an effect system whose
    /// vocabulary cannot express blur has decided blur is impossible rather
    /// than unimplemented. The renderer refuses it for now and says so.
    Backdrop,
}

/// The corner radius a rounded-corner program takes, in physical pixels.
pub const RADIUS_UNIFORM: &str = "corner_radius";

/// The texture's size in physical pixels.
///
/// Ours, not Smithay's. A *pixel* program is given a `size` uniform; a
/// *texture* program is not -- the built-in `texture.frag` declares only
/// `tex`, `alpha` and `v_coords`. So a texture shader that needs to measure in
/// pixels has to be told how big it is.
pub const SIZE_UNIFORM: &str = "tex_size";

/// Rounded corners, as a fragment program over the node's own texture.
///
/// Smithay's contract for a TEXTURE program, read from `shaders/mod.rs:125`
/// rather than from the doc comment on `compile_custom_texture_shader`, which
/// is wrong about the first of these:
///
/// * the source must contain a line that is exactly `//_DEFINES_` -- with the
///   trailing underscore; the doc comment omits it;
/// * the source supplies its own `#version 100`. `texture_program` does not
///   prepend one, and the built-in `texture.frag` carries its own. (The
///   *pixel* program is the one where smithay prepends it.)
///
/// `alpha` is Smithay's; `corner_radius` and `tex_size` are ours. A texture
/// program gets no `size` uniform -- only a pixel program does.
/// The distance field is the standard rounded-box one: fold the coordinate
/// into one quadrant, and measure from the centre of that corner's circle.
/// Antialiased over one pixel with `smoothstep`, because a hard cut on a
/// curve is a staircase.
pub const ROUNDED_CORNERS: &str = r"#version 100

//_DEFINES_

precision mediump float;
uniform sampler2D tex;
uniform float alpha;
uniform float corner_radius;
uniform vec2 tex_size;
varying vec2 v_coords;

void main() {
    vec4 colour = texture2D(tex, v_coords);

    // Into pixels, then into one corner's quadrant: abs() folds all four
    // corners onto one, so the distance field is written once rather than
    // four times. `v_coords` is `(tex_matrix * position).xy`, which for a
    // whole texture runs 0..1 -- see smithay's texture.vert.
    vec2 half_size = tex_size * 0.5;
    vec2 p = abs(v_coords * tex_size - half_size) - (half_size - vec2(corner_radius));
    float away = length(max(p, 0.0)) - corner_radius;

    // Every channel, not just alpha: a wayland surface is PREMULTIPLIED, so
    // colour and alpha have to be scaled together or a faded edge comes out
    // too bright. Smithay's own texture.frag does `color * alpha` for the
    // same reason.
    gl_FragColor = colour * alpha * (1.0 - smoothstep(-0.5, 0.5, away));
}
";

/// One effect on one node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// Rounded corners, with a radius in logical pixels.
    Rounded { radius: f64 },
}

impl Effect {
    /// Rounded corners at `radius` logical pixels.
    #[must_use]
    pub const fn rounded(radius: f64) -> Self {
        Self::Rounded { radius }
    }

    /// What this effect needs before it can draw.
    #[must_use]
    pub const fn inputs(&self) -> Inputs {
        match self {
            Self::Rounded { .. } => Inputs::SelfTexture,
        }
    }

    /// The radius, in logical pixels.
    #[must_use]
    pub const fn radius(&self) -> f64 {
        match self {
            Self::Rounded { radius } => *radius,
        }
    }

    /// Whether this is an effect that should not be run at all.
    ///
    /// A radius of zero is not "rounded by nothing", it is *no effect*, and
    /// the difference is a whole offscreen pass per window per frame. Every
    /// window on a machine with no styling declares one, so this is the arm
    /// that keeps the ordinary case ordinary.
    #[must_use]
    pub fn is_none_effect(&self) -> bool {
        match self {
            Self::Rounded { radius } => !(*radius > 0.0),
        }
    }
}
```

Add to `crates/effects/src/lib.rs`, next to `pub mod ffi;`:

```rust
pub mod fragment;
```

- [ ] **Step 4: Run the tests**

```bash
dev/gate.sh
```

Expected: PASS, three new tests.

- [ ] **Step 5: See each test fail**

For each, mutate, run `dev/gate.sh`, record the failure, revert:
- `Inputs::SelfTexture` → `Inputs::Nothing` in `inputs()` → `rounded_corners_reads_the_node_itself` fails.
- `!(*radius > 0.0)` → `false` → `a_zero_radius_is_not_an_effect` fails.
- Change the marker to `//_DEFINES` (drop the trailing underscore — the spelling Smithay's own doc comment gives) → `the_shader_is_shaped_the_way_smithay_actually_requires` fails. **Run this one first**: it is the real defect the test was written for, and a `contains` assertion would have passed it.
- Delete the leading `#version 100` → the same test fails on its second assertion.

- [ ] **Step 6: Commit**

```bash
git add crates/effects/src/fragment.rs crates/effects/src/lib.rs
git commit -m "effects: an effect says what it reads, and rounded corners read the node"
```

---

### Task 2: `client.radius` stops being a reserved key

**Files:**
- Modify: `crates/solium/src/style.rs:163-172` (the `Style` struct), and `load`
- Test: `crates/solium/src/style.rs` (its existing `mod tests`)

**Interfaces:**
- Consumes: `solium_effects::fragment::Effect` from Task 1.
- Produces: `Style::effects: Vec<solium_effects::fragment::Effect>`. Empty for a style that declares no `client.radius`, or declares it as `0`.

- [ ] **Step 1: Write the failing test**

Add to `style.rs`'s `mod tests`:

```rust
    /// `client.radius` has been a key the loader reads and nothing consumes
    /// since the group work. This is the commit that consumes it.
    #[test]
    fn a_declared_radius_becomes_an_effect() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "rounded",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    insets.top: 0
                    client.radius: 12
                    Layer { depth: "frame"; name: "bar" }
                }
                "#,
            );
            let style = load(&dir).expect("the fixture loads");
            assert_eq!(
                style.effects,
                vec![solium_effects::fragment::Effect::rounded(12.0)],
                "a declared radius is the one effect this style runs"
            );
            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// And the ordinary case stays ordinary: no radius, no effect, no pass.
    /// Every shipped style is this case, so an effect list that is non-empty
    /// here is an offscreen capture per window per frame for the whole desktop.
    #[test]
    fn a_style_with_no_radius_runs_no_effects() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "plain",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    insets.top: 32
                    Layer { depth: "frame"; name: "bar" }
                }
                "#,
            );
            let style = load(&dir).expect("the fixture loads");
            assert!(style.effects.is_empty());
            let _ = std::fs::remove_dir_all(&dir);
        });

        on_the_qt_thread(|| {
            let dir = fixture(
                "zero",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    insets.top: 0
                    client.radius: 0
                    Layer { depth: "frame"; name: "bar" }
                }
                "#,
            );
            let style = load(&dir).expect("the fixture loads");
            assert!(
                style.effects.is_empty(),
                "a radius of zero is no effect, not an effect that rounds by zero"
            );
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL to compile — `Style` has no field `effects`.

- [ ] **Step 3: Add the field and fill it**

In `style.rs`, add to `Style` after `layers`:

```rust
    /// The effects this style runs on the client, in declaration order.
    ///
    /// Empty is the ordinary case and the one worth protecting: an effect
    /// declaring `self` costs an offscreen pass per frame, so a list that is
    /// non-empty when nobody asked for anything is the whole desktop paying
    /// for a feature nobody turned on. `client.radius: 0` is *no effect*
    /// rather than an effect that rounds by nothing, for exactly that reason.
    pub(crate) effects: Vec<solium_effects::fragment::Effect>,
```

In `load`, where `insets` are read from the scene, add alongside:

```rust
    // `client.radius` has been read by `get_int` and consumed by nothing since
    // the group work landed; this is what consumes it. Through the same
    // `QQmlProperty` path the insets take, because `QObject::property` takes a
    // name and not a path -- `scene.get_int("client.radius")` silently
    // returning 0 for every style is a bug this codebase has already shipped
    // once.
    let mut effects = Vec::new();
    let rounded = solium_effects::fragment::Effect::rounded(f64::from(
        scene.get_int("client.radius").max(0),
    ));
    if !rounded.is_none_effect() {
        effects.push(rounded);
    }
```

and add `effects` to the `Style { .. }` literal it returns.

Add the dependency to `crates/solium/Cargo.toml` if `solium_effects` is not already there (it is — `crates/effects` is used by `present.rs`).

- [ ] **Step 4: Run the tests**

```bash
dev/gate.sh
```

Expected: PASS.

- [ ] **Step 5: See each test fail**

- Change `if !rounded.is_none_effect()` to `if true` → `a_style_with_no_radius_runs_no_effects` fails on both fixtures.
- Change `Effect::rounded(f64::from(...))` to `Effect::rounded(1.0)` → `a_declared_radius_becomes_an_effect` fails.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/style.rs
git commit -m "style: a declared client.radius is an effect, and zero is no effect"
```

---

### Task 3: The program, compiled once

**Files:**
- Create: `crates/solium/src/pass.rs`
- Modify: `crates/solium/src/main.rs` (add `mod pass;`)

**Interfaces:**
- Consumes: `solium_effects::fragment::{Effect, Inputs, ROUNDED_CORNERS, RADIUS_UNIFORM}`.
- Produces: `pass::Programs`, `Programs::rounded(&mut self, renderer: &mut GlesRenderer) -> Option<&GlesTexProgram>`, and `pass::needs_pass(effects: &[Effect]) -> Option<Effect>`.

- [ ] **Step 1: Write the failing test**

In `crates/solium/src/pass.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use solium_effects::fragment::Effect;

    /// The question the renderer asks every pane, every frame. It has to be
    /// cheap and it has to answer `None` for the overwhelmingly common case,
    /// or the ordinary window stops being ordinary.
    #[test]
    fn a_pane_with_no_effects_needs_no_pass() {
        assert_eq!(needs_pass(&[]), None);
    }

    #[test]
    fn an_effect_reading_self_needs_a_pass() {
        let rounded = Effect::rounded(10.0);
        assert_eq!(needs_pass(&[rounded]), Some(rounded));
    }

    /// A zero radius reaches here only if `style::load` let it through, and
    /// `needs_pass` refusing it too is deliberate belt and braces: the cost of
    /// being wrong is every window on the machine rendering offscreen.
    #[test]
    fn a_none_effect_needs_no_pass() {
        assert_eq!(needs_pass(&[Effect::rounded(0.0)]), None);
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL — no module `pass`.

- [ ] **Step 3: Write the module**

Create `crates/solium/src/pass.rs`:

```rust
//! Passes: what an effect reading its own node costs, and where it is paid.
//!
//! An effect that declares `Inputs::Nothing` never reaches this file. One that
//! declares `Inputs::SelfTexture` cannot be a single element in a flat list,
//! because it needs the node's pixels before it can draw: the client's
//! surfaces are rendered into a texture of their own, and *that* is drawn,
//! through a fragment program, in the client's place.
//!
//! The capture is [`crate::offscreen::capture`], which already exists for the
//! genie and already keeps its texture on the pane rather than allocating one
//! a frame. That was made a prerequisite of this work rather than a follow-up
//! for exactly this reason: an effect system multiplies a per-frame allocation
//! by the number of animating windows.

use smithay::backend::renderer::gles::{GlesRenderer, GlesTexProgram, UniformName, UniformType};
use solium_effects::fragment::{Effect, Inputs, RADIUS_UNIFORM, ROUNDED_CORNERS, SIZE_UNIFORM};

/// Whether this node's effects need the node rendered to a texture first, and
/// which effect wants it.
///
/// `None` is the answer for every window on a machine nobody has styled, and
/// it is asked once per pane per frame, so it is a slice walk and nothing
/// more. The first effect wins: one pass, one program, and a node wanting two
/// fragment effects at once is a thing to design when something wants it.
pub(crate) fn needs_pass(effects: &[Effect]) -> Option<Effect> {
    effects
        .iter()
        .copied()
        .find(|effect| !effect.is_none_effect() && effect.inputs() == Inputs::SelfTexture)
}

/// The compiled fragment programs, one of each, for the life of the renderer.
///
/// Compiling a shader is not a per-frame cost anybody should pay, and
/// `compile_custom_texture_shader` calls `make_current` -- which is not free
/// and, worse, is exactly the kind of thing that has broken Qt's stale
/// thread-local `currentContext` five separate times in this codebase.
#[derive(Debug, Default)]
pub(crate) struct Programs {
    rounded: Option<GlesTexProgram>,
    /// Set once a compile has been tried and failed, so the warning is logged
    /// once rather than at sixty or two hundred and sixty hertz.
    rounded_failed: bool,
}

impl Programs {
    /// The rounded-corner program, compiling it on first use.
    ///
    /// `None` means the shader did not compile, and the caller draws the
    /// window square rather than not at all: a driver that cannot build this
    /// program should cost someone their rounded corners, not their desktop.
    pub(crate) fn rounded(&mut self, renderer: &mut GlesRenderer) -> Option<&GlesTexProgram> {
        if self.rounded.is_none() && !self.rounded_failed {
            match renderer.compile_custom_texture_shader(
                ROUNDED_CORNERS,
                &[
                    UniformName::new(RADIUS_UNIFORM, UniformType::_1f),
                    // Ours because smithay gives a texture program no `size`.
                    UniformName::new(SIZE_UNIFORM, UniformType::_2f),
                ],
            ) {
                Ok(program) => self.rounded = Some(program),
                Err(err) => {
                    self.rounded_failed = true;
                    tracing::warn!(
                        ?err,
                        "the rounded-corner shader did not compile; windows will be drawn square"
                    );
                }
            }
        }
        self.rounded.as_ref()
    }
}
```

In `crates/solium/src/main.rs`, add `mod pass;` in the module list, alphabetically.

- [ ] **Step 4: Run the tests**

```bash
dev/gate.sh
```

Expected: PASS, three new tests.

- [ ] **Step 5: See each test fail**

- Change `find(...)` to `first().copied()` → `a_none_effect_needs_no_pass` fails.
- Drop the `!effect.is_none_effect() &&` clause → `a_none_effect_needs_no_pass` fails.
- Change `Inputs::SelfTexture` to `Inputs::Nothing` → `an_effect_reading_self_needs_a_pass` fails.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/pass.rs crates/solium/src/main.rs
git commit -m "pass: the question a pane is asked every frame, and the program it may need"
```

---

### Task 4: The client is drawn through the program

**Files:**
- Modify: `crates/solium/src/render.rs` — the `Piece::Client` arm of the pane walk
- Modify: `crates/solium/src/state.rs` — hold `pass::Programs` on `Solium`

**Interfaces:**
- Consumes: `pass::needs_pass`, `pass::Programs::rounded`, `offscreen::capture`, `render::Element::Screen`.
- Produces: no new public API. The client piece emits either the surfaces it emits today, or one `Element::Screen`.

- [ ] **Step 1: Add the field**

On `Solium` in `state.rs`, beside the other renderer-lifetime caches:

```rust
    /// Fragment programs, compiled on first use and kept for the life of the
    /// renderer. Not on the renderer because it is Smithay's; not per pane
    /// because a program is per GL context.
    pub(crate) programs: crate::pass::Programs,
```

and `programs: crate::pass::Programs::default()` in the constructor.

- [ ] **Step 2: Write the failing test**

In `render.rs`'s `mod tests`, beside the existing `pane_pieces` walk test:

```rust
    /// The whole of the cheap path, stated as a test because it is the rule
    /// the spike put hardest: a node with no effect emits exactly what it
    /// emits today, through the path it takes today.
    ///
    /// Driven through `pane_pieces` with a closure that records what each
    /// piece asked for, the same way the depth-order test is -- no renderer,
    /// no GPU, no window.
    #[test]
    fn a_pane_with_no_effect_asks_for_no_capture() {
        let mut asked: Vec<&'static str> = Vec::new();
        crate::render::pane_pieces(&mut asked, |into, piece| {
            if let crate::render::Piece::Client = piece {
                into.push(if crate::pass::needs_pass(&[]).is_some() {
                    "captured"
                } else {
                    "surfaces"
                });
            }
        });
        assert_eq!(asked, vec!["surfaces"]);
    }

    #[test]
    fn a_pane_with_a_radius_asks_for_a_capture() {
        let effects = [solium_effects::fragment::Effect::rounded(12.0)];
        let mut asked: Vec<&'static str> = Vec::new();
        crate::render::pane_pieces(&mut asked, |into, piece| {
            if let crate::render::Piece::Client = piece {
                into.push(if crate::pass::needs_pass(&effects).is_some() {
                    "captured"
                } else {
                    "surfaces"
                });
            }
        });
        assert_eq!(asked, vec!["captured"]);
    }
```

- [ ] **Step 3: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL to compile — `crate::pass` is not reachable from `render`'s tests until Task 3's `mod pass;` is in, which it is; if both tests pass immediately, that is correct and expected — they pin the branch, and Step 5's mutation is what proves them.

- [ ] **Step 4: Draw the captured texture**

In `render.rs`, in the function that handles `Piece::Client` for a pane, before emitting the client's surfaces:

```rust
    // An effect that reads the node itself cannot be an element over the
    // client -- it needs the client's pixels. So the client's surfaces are
    // rendered into the pane's own texture and that texture is drawn in their
    // place, through the effect's program.
    //
    // Everything below this branch is the path every unstyled window takes and
    // is deliberately untouched: no capture, no bind, no program, no extra
    // element. `needs_pass` answering `None` is what keeps it that way.
    if let Some(effect) = crate::pass::needs_pass(&style.effects)
        && let Some((texture, size)) = crate::offscreen::capture(state, renderer, pane, window, scale)
        && let Some(program) = state.programs.rounded(renderer)
    {
        // Physical pixels, because the shader measures in the texture's own
        // pixels and the texture was captured at the monitor's scale. A
        // logical radius here is a corner that is right on one screen and
        // wrong on the other -- a bug that only appears on a desk with two
        // monitors at different scales.
        let radius = (effect.radius() * scale) as f32;
        elements.push(Element::Rounded(crate::pass::Rounded::new(
            texture,
            size,
            outer.loc.to_physical_precise_round(scale),
            radius,
            program.clone(),
            alpha,
        )));
        return;
    }
```

**`TextureRenderElement` has no `with_texture_program`.** Checked against
`smithay-0.7.0/src/backend/renderer/element/texture.rs`: the constructors are
`from_texture`, `from_texture_render_buffer`, `from_texture_buffer`,
`from_texture_with_damage` and `from_static_texture`, and none of them take a
program. So the snippet above cannot be written and the program is applied on
the **frame** instead:

```rust
impl RenderElement<GlesRenderer> for Rounded {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        // Override, draw, clear. The override lives on the frame and outlasts
        // this element if it is not cleared, so the very next window drawn
        // would come out with rounded corners it never asked for -- and only
        // when it happened to be drawn after this one, which is a bug that
        // moves around as windows are raised.
        frame.override_default_tex_program(
            self.program.clone(),
            vec![
                Uniform::new(RADIUS_UNIFORM, self.radius),
                Uniform::new(SIZE_UNIFORM, (self.size.w as f32, self.size.h as f32)),
            ],
        );
        let drawn = frame.render_texture_from_to(
            &self.texture, src, dst, damage, opaque_regions, Transform::Normal, self.alpha, None, &[],
        );
        frame.clear_tex_program_override();
        drawn
    }
}
```

`crates/solium/src/warp.rs` is the worked example of a hand-written
`RenderElement` in this tree — follow its shape, including adding a variant to
the `render_elements!` block in `render.rs` (call it `Rounded`, beside
`Warped`). Check `render_texture_from_to`'s exact signature in
`smithay-0.7.0/src/backend/renderer/gles/mod.rs` before writing the call; the
argument list above is from the plan's author reading the type, not from
compiling it, and it is the one thing here most likely to need a comma moved.

- [ ] **Step 5: Run the tests, then see them fail**

```bash
dev/gate.sh
```

Expected: PASS. Then mutate `needs_pass` to return `None` unconditionally → `a_pane_with_a_radius_asks_for_a_capture` fails. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/render.rs crates/solium/src/state.rs crates/solium/src/pass.rs
git commit -m "render: a client with a radius is drawn from its own texture"
```

---

### Task 5: A rounded window is not opaque at its corners

**Files:**
- Modify: `crates/solium/src/pass.rs` (or `render.rs`, wherever Task 4 put the element)

**Interfaces:**
- Consumes: the element from Task 4.
- Produces: no new API. The element reports an opaque region inset by the radius.

- [ ] **Step 1: Write the failing test**

In `pass.rs`:

```rust
    /// The question the whole design rests on, and the reason rounded corners
    /// were chosen as the first effect rather than something prettier.
    ///
    /// A square window is opaque everywhere, and the renderer uses that to
    /// skip drawing whatever is behind it. Round the corners and that stops
    /// being true at four places -- so an element that keeps claiming the
    /// whole rect leaves the wallpaper undrawn under each corner, and what is
    /// there instead is whatever the last frame left, which reads as four
    /// smears that follow the window around.
    ///
    /// Inset by the radius on every side: the largest rectangle that is
    /// certainly inside a rounded rect. Not the tightest possible region --
    /// the tightest is a cross -- but it is right, and a region that is
    /// smaller than the truth only costs drawing, where one larger than the
    /// truth costs correctness.
    #[test]
    fn a_rounded_rect_is_opaque_only_inside_its_corners() {
        let rect = Rectangle::<i32, Physical>::new((100, 100).into(), (300, 200).into());
        let opaque = opaque_inside(rect, 20.0);
        assert_eq!(opaque.loc.x, 120);
        assert_eq!(opaque.loc.y, 120);
        assert_eq!(opaque.size.w, 260);
        assert_eq!(opaque.size.h, 160);
    }

    /// A radius larger than the window is not a negative rectangle.
    #[test]
    fn a_radius_bigger_than_the_window_claims_nothing() {
        let rect = Rectangle::<i32, Physical>::new((0, 0).into(), (30, 30).into());
        let opaque = opaque_inside(rect, 40.0);
        assert_eq!(opaque.size.w, 0);
        assert_eq!(opaque.size.h, 0);
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL — no function `opaque_inside`.

- [ ] **Step 3: Write it, and use it**

```rust
/// The largest rectangle certainly inside a rounded rect.
pub(crate) fn opaque_inside(
    rect: Rectangle<i32, Physical>,
    radius: f64,
) -> Rectangle<i32, Physical> {
    let inset = radius.ceil().max(0.0) as i32;
    let w = (rect.size.w - inset * 2).max(0);
    let h = (rect.size.h - inset * 2).max(0);
    Rectangle::new((rect.loc.x + inset, rect.loc.y + inset).into(), (w, h).into())
}
```

Have the element's `opaque_regions` return `OpaqueRegions::from_slice(&[opaque_inside(geometry, radius)])` when the region is non-empty, and `OpaqueRegions::default()` when it is not.

- [ ] **Step 4: Run the tests, then see them fail**

```bash
dev/gate.sh
```

Expected: PASS. Then change `radius.ceil()` to `0.0` → the first test fails; change `.max(0)` to a bare subtraction → the second fails. Revert both.

- [ ] **Step 5: Commit**

```bash
git add crates/solium/src/pass.rs
git commit -m "pass: a rounded window stops claiming its corners are opaque"
```

---

### Task 6: Watch it, and keep the gate honest

**Files:**
- Modify: `dev/wirecheck/src/main.rs` (the census loop, beside the `delegate` case)
- Modify: `crates/solium/qml/panes/example/Pane.qml` (declare `client.radius: 14`)
- Modify: `docs/ricing.md` (one paragraph on `client.radius`)

- [ ] **Step 1: Compile the shader in wirecheck**

In the census section of `dev/wirecheck/src/main.rs`, after the scene cases, add:

```rust
    // The rounded-corner program, compiled against a real GL context.
    //
    // Nothing else in the gate can fail on a bad shader: `cargo test` has no
    // GPU, and the unit tests can only check that the source is the SHAPE
    // smithay wants -- that it has a `//_DEFINES` line and no `#version`. That
    // a driver accepts it is a different question and this is the only place
    // in the tree that can ask it.
    match renderer.compile_custom_texture_shader(
        solium_effects::fragment::ROUNDED_CORNERS,
        &[
            UniformName::new(
                solium_effects::fragment::RADIUS_UNIFORM,
                UniformType::_1f,
            ),
            UniformName::new(
                solium_effects::fragment::SIZE_UNIFORM,
                UniformType::_2f,
            ),
        ],
    ) {
        Ok(_) => println!("  rounded-corner shader: compiled"),
        Err(err) => {
            return Err(anyhow!(
                "the rounded-corner shader did not compile: {err}. Every window with \
                 a `client.radius` is drawn square until this builds, and no test \
                 outside this file can see it -- `cargo test` has no GL context"
            ));
        }
    }
```

- [ ] **Step 2: Run the gate and see the line**

```bash
dev/gate.sh
```

Expected: `gate passed`, with `rounded-corner shader: compiled` in the wirecheck output.

- [ ] **Step 3: See it fail**

Insert a deliberate syntax error into `ROUNDED_CORNERS` (`float x = ;`), run `dev/gate.sh`, record that it reds with the message above, revert.

- [ ] **Step 4: Look at it**

`panes/example/Pane.qml` gains `client.radius: 14`. Then, using the nested harness described in the memory `solium-verifying-qml-nested`:

```bash
QML_IMPORT_PATH=crates/solium/qml SOLIUM_PANE=example \
  SOLIUM_CAPTURE=/tmp/round SOLIUM_CAPTURE_FRAMES=1 SOLIUM_CAPTURE_AT=6000 \
  ./target/debug/solium
```

with a client launched into its socket at a size smaller than the output, and the wallpaper left on. Compare against the same capture with `client.radius: 0`.

Expected, and all three must hold:
1. The window's corners are cut and the wallpaper shows through them.
2. The cut is antialiased — sample the pixels along one corner's arc and confirm intermediate alpha rather than a hard step.
3. **The wallpaper under each corner is drawn, not smeared.** This is the opaque-region half of Task 5 and it is the one that cannot be seen from a single still: capture two frames with the window moved between them and confirm the corner shows wallpaper both times rather than a copy of the previous frame.

Record the pixel evidence in the report. If any of the three fails, that is the finding — do not adjust the shader until you can say which.

- [ ] **Step 5: Document it**

In `docs/ricing.md`, beside the pane-style recipe:

```markdown
### Rounded corners

    client.radius: 12

A style may round the client's corners. It is the one effect that reads the
window's own pixels, so a pane that declares it is rendered to a texture first
and then drawn through a fragment program — one extra pass per frame, for that
window only. `client.radius: 0` is no effect at all rather than a radius of
nothing, so a style that does not want it pays for none of this.
```

- [ ] **Step 6: Commit**

```bash
git add dev/wirecheck/src/main.rs crates/solium/qml/panes/example/Pane.qml docs/ricing.md
git commit -m "pass: rounded corners, seen on a screen and pinned in the gate"
```

---

## Self-Review

**Spec coverage.** The spec's pass section names four effects and one table. `rounded corners / self` is Tasks 1–6. `blur / backdrop` is represented as `Inputs::Backdrop` in Task 1 and implemented nowhere, which is the plan's stated scope. `wavy border, glow, spikes / —` is `Inputs::Nothing` and already works — `panes/wave/` is the shipped proof. `shadow / self` reuses Tasks 3–5 unchanged and is item 10's work, not this plan's. The opaque-region sentence ("a property the node declares and the renderer's culling reads") is Task 5. The prerequisite sentence about `offscreen::capture` allocating per frame is satisfied — that is phase 1 item 2, already done.

**Placeholders.** The conditional that was in Task 4 is resolved: `TextureRenderElement` has no `with_texture_program`, so the program is applied on the frame through a hand-written `RenderElement`, and that is now the only route the task gives. One uncertainty is left and is labelled where it sits — the exact argument list of `render_texture_from_to`, which the task tells the implementer to check before writing rather than trusting.

**Corrected before dispatch, from reading Smithay's source rather than its docs.** Three errors in the first draft of Task 1, all of which would have compiled and none of which would have failed before a GPU was present: the defines marker is `//_DEFINES_` and not `//_DEFINES` (Smithay's own doc comment at `gles/mod.rs:1964` gives the wrong spelling; `shaders/mod.rs:125` is what runs); a texture program supplies its own `#version 100` rather than having one prepended, which is the opposite of the pixel-program rule the draft had copied; and a texture program gets no `size` uniform at all, so the shader has to declare and be passed its own. The draft's own test would not have caught the first of these, because `contains("//_DEFINES")` is true of the wrong spelling — it is a prefix of the right one. The test is now a whole-line match and says why.

**Type consistency.** `Effect::rounded(f64)`, `Effect::radius() -> f64`, `Effect::inputs() -> Inputs`, `Effect::is_none_effect() -> bool`, `needs_pass(&[Effect]) -> Option<Effect>`, `Programs::rounded(&mut GlesRenderer) -> Option<&GlesTexProgram>`, `opaque_inside(Rectangle<i32, Physical>, f64) -> Rectangle<i32, Physical>`, `RADIUS_UNIFORM: &str` — each is defined once and used with the same signature everywhere after.
