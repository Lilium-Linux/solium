# Per-Corner Radii Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A window's four corners can be rounded by different amounts, so a titlebar and the client it sits on can meet in more than one way.

**Architecture:** `Effect::Rounded` carries four radii instead of one. The shader picks the radius for the quadrant a fragment is in *before* `abs()` folds it, so everything downstream is unchanged and the uniform goes from a `float` to a `vec4`. The opaque region insets each side by the larger of the two corners touching it. QML gains four keys, each defaulting to `client.radius`.

**Tech Stack:** Rust edition 2024, Smithay 0.7, GLSL ES 1.00, QML.

**Spec:** `docs/superpowers/specs/2026-09-12-panes-and-effects-design.md` — *An effect declares its inputs*. This extends that effect's parameters; it does not change what a pass is.

## Why, because it decides the shape of the whole plan

Two styles the user asked for, from two reference screenshots:

| | client top corners | titlebar | how they meet |
|---|---|---|---|
| **Finder** | **square** | rounded | flush. The titlebar *is* the window's top. |
| **nested** | **rounded** | rounded | the client's cut corners show the titlebar behind them. |

**Both remove the overhang that exists today.** `panes/rounded/Frame.qml` currently draws its bar `insetTop + clientRadius` tall so it can cover the notches where its square bottom meets the client's cut top corners — and that overhang **covers the top `clientRadius` rows of the client**, which in a terminal cuts a line of the prompt in half. With square top corners there is no notch to fill; with rounded ones the thing behind the notch is the titlebar, which is what should be there.

So this is not a workaround for a badly placed seam. It is the capability that makes the seam unnecessary.

## Global Constraints

- Rust edition 2024. Workspace lints **deny** `unwrap_used`, `expect_used`, `panic`, `todo`. `unsafe_code` is `warn` — every `unsafe` block needs `#[expect(unsafe_code, reason = "…")]`.
- **All builds run in podman** via `dev/gate.sh`, which takes NO arguments. The host has no Qt 6 development files. The built binary *does* run on the host.
- **A node with no effect emits exactly what it emits now.** All four radii zero is *no effect*, not an effect that rounds by nothing — the difference is an offscreen pass per window per frame.
- Radii are **logical** pixels in QML and on `Effect`; `crate::pass` multiplies by the output scale. Invisible at 1x and wrong on every HiDPI screen if confused.
- **An opaque region larger than the truth corrupts the screen**; smaller only costs drawing. Smithay draws opaque damage with blending disabled (`gles/mod.rs:2585`), so an over-claim is last-frame garbage *plus* a hard-edged dark patch.
- The shader's contract, read from source and not from Smithay's docs: the marker is `//_DEFINES_` with the trailing underscore, a texture program supplies its own `#version 100`, and it gets no `size` uniform. `crates/effects/src/fragment.rs` states all three; do not "correct" them.
- Never launch `claude-desktop`. `sudo` is not available.
- The user has a live Solium on tty3 out of this worktree, a `cava` of their own, and possibly a nested demo: **never `pkill` by name.** Kill only pids you started.
- Verify rendering with the nested harness at `/tmp/claude-1000/-home-kotoxik-Development/690887c8-d6b5-43b1-abc8-b985cf8f39fd/scratchpad/shoot.sh`. `SOLIUM_QML_GPU=1` nested renders no QML at all; `--check-qml` exits 0 on failure and does not load a `Pane.qml`'s `source:` file.

## The configuration shape, and why it is four keys and not a map

`bleed` accepts `40` or `{"top":160,"left":8}`, and that would be the consistent spelling. **It is not available here without new C++.** `bleed` is read with `Scene::layer_field`, which returns a `String` for a *layer* property; `client.radius` is read with `Scene::get_int`, the only accessor that resolves a dotted path, and there is no `get_string` that does. Making `ClientTreatment.radius` a `var` to hold a map would break `get_int` — a QVariant holding a JS object reads back as 0 — so the map form costs a new FFI entry point and a `QQmlProperty` string read in `host.cpp`.

So: **four int keys, each defaulting to `client.radius`.** Two levels of fallback, no new FFI, and every read is the `get_int` that already works.

```qml
client.radius: 12                    // all four
client.radiusTopLeft: 0              // …except this one
client.radiusTopRight: 0
```

A later want for `{"top": 0}` shorthand is a third level of fallback and is the job of whoever wants it — the same argument `Style::effects` already makes about ordering.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/effects/src/fragment.rs` (modify) | `Corners`, `Effect::Rounded` carrying four, and the shader selecting per quadrant. |
| `crates/solium/qml/Solium/ClientTreatment.qml` (modify) | The four new properties. |
| `crates/solium/src/style.rs` (modify) | Read them, each defaulting to `radius`. |
| `crates/solium/src/pass.rs` (modify) | The `vec4` uniform, and `opaque_inside` per side. |
| `crates/solium/src/decoration.rs` (modify) | Layers told four numbers instead of one. |
| `dev/wirecheck/src/main.rs` (modify) | The readback's corner assertions become per-corner. |
| `crates/solium/qml/panes/rounded/` (modify) | Drop the overhang; show the nested style. |
| `crates/solium/qml/panes/flush/` (new) | The Finder style — square client top, titlebar owns the corners. |

---

### Task 1: four radii, and a shader that picks one

**Files:**
- Modify: `crates/effects/src/fragment.rs`

**Interfaces:**
- Produces: `Corners { top_left: f64, top_right: f64, bottom_left: f64, bottom_right: f64 }`, `Corners::all(f64)`, `Corners::is_none(&self) -> bool`, `Corners::max_of_side(&self) -> (f64, f64, f64, f64)` returning `(top, right, bottom, left)`. `Effect::rounded(Corners)`, `Effect::radii(&self) -> Corners`. `RADIUS_UNIFORM` stays the same name, type `_4f`.

- [ ] **Step 1: Write the failing test**

```rust
    /// Four corners, and the shader has to tell them apart. A single radius is
    /// the common case and gets a constructor; it is not the only case.
    #[test]
    fn corners_can_differ() {
        let finder = Corners { top_left: 0.0, top_right: 0.0, bottom_left: 12.0, bottom_right: 12.0 };
        let effect = Effect::rounded(finder);
        assert_eq!(effect.radii(), finder);
        assert!(!effect.is_none_effect(), "two corners rounded is still an effect");
    }

    /// All four zero is *no effect*, and that is what keeps the ordinary
    /// window off the pass path entirely.
    #[test]
    fn every_corner_zero_is_no_effect() {
        assert!(Effect::rounded(Corners::all(0.0)).is_none_effect());
        assert!(Effect::rounded(Corners::all(-3.0)).is_none_effect());
        assert!(Effect::rounded(Corners::all(f64::NAN)).is_none_effect());
        // But ONE corner is enough to be one.
        let one = Corners { top_left: 8.0, ..Corners::all(0.0) };
        assert!(!Effect::rounded(one).is_none_effect());
    }

    /// Each side is inset by the larger of the two corners touching it. The
    /// asymmetry is the point: a square-topped, round-bottomed window must not
    /// give up its top rows.
    #[test]
    fn a_side_is_measured_by_its_larger_corner() {
        let corners = Corners { top_left: 0.0, top_right: 0.0, bottom_left: 12.0, bottom_right: 20.0 };
        let (top, right, bottom, left) = corners.max_of_side();
        assert!((top - 0.0).abs() < f64::EPSILON, "no top corner is cut, so no top rows are lost");
        assert!((right - 20.0).abs() < f64::EPSILON, "the right side touches top-right and bottom-right");
        assert!((bottom - 20.0).abs() < f64::EPSILON);
        assert!((left - 12.0).abs() < f64::EPSILON);
    }

    /// The shader picks a radius per quadrant BEFORE `abs()` folds the
    /// coordinate, which is the one line that makes four radii possible at
    /// all. Pinned by text here and drawn for real in wirecheck.
    #[test]
    fn the_shader_selects_a_radius_per_quadrant() {
        assert!(
            has_line("uniform vec4 corner_radius;"),
            "four radii, as tl/tr/bl/br"
        );
        assert!(
            ROUNDED_CORNERS.contains("v_coords.x < 0.5"),
            "the quadrant is chosen from the unfolded coordinate; after `abs()` \
             every corner looks like the top-left and the four are indistinguishable"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL to compile — no type `Corners`.

- [ ] **Step 3: Write it**

`Corners` is plain data with `#[derive(Clone, Copy, Debug, PartialEq)]`. `is_none` is true when no corner is `> 0.0` — reuse the existing spelling (`<= 0.0 || is_nan()` per corner, which avoids `clippy::neg_cmp_op_on_partial_ord`). `max_of_side` returns the four sides.

In the shader, **before** the existing fold:

```glsl
    // Which corner this fragment belongs to, decided on the UNFOLDED
    // coordinate. `abs()` below makes all four look like the top-left, which
    // is what lets one expression draw four corners -- and is exactly why the
    // radius has to be chosen first. `corner_radius` is (tl, tr, bl, br).
    float picked = (v_coords.x < 0.5)
        ? ((v_coords.y < 0.5) ? corner_radius.x : corner_radius.z)
        : ((v_coords.y < 0.5) ? corner_radius.y : corner_radius.w);
    float r = min(picked, min(half_size.x, half_size.y));
```

and the two distance-field lines keep using `r` exactly as they do now. Change the declaration to `uniform vec4 corner_radius;`.

**The existing clamp test asserts `float r = min(corner_radius, min(half_size.x, half_size.y));` as a whole line and the "raw uniform appears on no line but its declaration and the clamp" walk.** Both need updating to the new spelling — update them, do not delete them, and say in your report what the walk now permits.

- [ ] **Step 4: Run the tests**

```bash
dev/gate.sh
```

- [ ] **Step 5: See each fail**

- Swap `corner_radius.y` and `corner_radius.z` in the select → `the_shader_selects_a_radius_per_quadrant` will NOT catch it (it is a text test). Say so in the report; wirecheck in Task 4 is what catches a transposed `vec4`, and that is the division of labour.
- `max_of_side` returning the *min* → `a_side_is_measured_by_its_larger_corner` fails.
- `is_none` true when *any* corner is zero → `every_corner_zero_is_no_effect` fails on its last assertion.

- [ ] **Step 6: Commit**

```bash
git add crates/effects/src/fragment.rs
git commit -m "effects: four radii, chosen before the fold that makes them one"
```

---

### Task 2: a style declares them

**Files:**
- Modify: `crates/solium/qml/Solium/ClientTreatment.qml`, `crates/solium/src/style.rs`

**Interfaces:**
- Consumes: `Corners` from Task 1.
- Produces: `Style::effects` carrying `Effect::rounded(Corners)`.

- [ ] **Step 1: Write the failing test**

Beside `a_declared_radius_becomes_an_effect` in `style.rs`, using the same `on_the_qt_thread` + `fixture` pattern:

```rust
    /// The Finder shape: the titlebar owns the top of the window, so the
    /// client's top corners are square and nothing has to overhang them.
    #[test]
    fn a_corner_can_be_squared_while_the_others_round() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "flushtop",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    insets.top: 32
                    client.radius: 12
                    client.radiusTopLeft: 0
                    client.radiusTopRight: 0
                    Layer { depth: "frame"; name: "bar" }
                }
                "#,
            );
            let style = load(&dir).expect("the fixture loads");
            assert_eq!(
                style.effects,
                vec![solium_effects::fragment::Effect::rounded(
                    solium_effects::fragment::Corners {
                        top_left: 0.0,
                        top_right: 0.0,
                        bottom_left: 12.0,
                        bottom_right: 12.0,
                    }
                )],
                "each corner defaults to `client.radius` and is overridden on its own"
            );
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
```

Keep the existing `a_declared_radius_becomes_an_effect` (bare `client.radius: 12` → all four at 12) — that is the default-inheritance half and it must not regress.

- [ ] **Step 2: Run it and watch it fail**

Expected: FAIL — `radiusTopLeft` reads 0 because the property does not exist, so every corner is 0 and the effect is empty.

- [ ] **Step 3: Declare and read them**

In `ClientTreatment.qml`, beside `radius`:

```qml
    // Each corner, defaulting to `radius`. **-1 and not 0 is the default**,
    // because 0 is a value someone means -- squaring one corner is half the
    // point of these -- and a default that is also a legal value cannot be
    // told from one. The compositor reads a negative as "not declared".
    property int radiusTopLeft: -1
    property int radiusTopRight: -1
    property int radiusBottomLeft: -1
    property int radiusBottomRight: -1
```

In `style.rs`, where `client.radius` is read:

```rust
    let all = scene.get_int("client.radius");
    // A negative means the key was not declared, so the corner takes `radius`.
    // Reading each with its own `get_int` rather than one dotted walk, because
    // `QObject::property` takes a name and not a path -- the bug this codebase
    // has already shipped once, where `get_int("insets.top")` silently read 0
    // for every style.
    let corner = |name: &str| {
        let declared = scene.get_int(name);
        f64::from(if declared < 0 { all } else { declared })
    };
    let radii = solium_effects::fragment::Corners {
        top_left: corner("client.radiusTopLeft"),
        top_right: corner("client.radiusTopRight"),
        bottom_left: corner("client.radiusBottomLeft"),
        bottom_right: corner("client.radiusBottomRight"),
    };
```

- [ ] **Step 4: Run the tests**

- [ ] **Step 5: See each fail**

- Default the corners to `0` instead of `all` → `a_declared_radius_becomes_an_effect` fails, because a bare `client.radius: 12` stops rounding anything.
- Treat `declared == 0` as "not declared" → the new test fails, because a squared corner comes back at 12.
- Transpose `top_right` and `bottom_left` → the new test fails. **Check this one specifically**: the fixture above has `bottom_left == bottom_right`, so a transposition of the two *bottom* corners would pass. Use distinct values for all four in at least one case, or say plainly that the bottom pair is unpinned.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/qml/Solium/ClientTreatment.qml crates/solium/src/style.rs
git commit -m "style: a corner is declared on its own, and -1 is what absent means"
```

---

### Task 3: the pass carries four, and claims less

**Files:**
- Modify: `crates/solium/src/pass.rs`, `crates/solium/src/decoration.rs`

**Interfaces:**
- Consumes: `Corners`, `Corners::max_of_side`.
- Produces: `opaque_inside(rect, sides)` taking four insets. `Rounded` holds `Corners` in physical pixels. Layers are told `clientRadiusTopLeft` and its three siblings.

- [ ] **Step 1: Write the failing test**

```rust
    /// A square-topped window keeps its top rows. The old signature insets all
    /// four sides by one radius, which for the Finder shape would have thrown
    /// away twelve rows of a window whose top corners are not cut at all --
    /// costing drawing, never correctness, but costing it for no reason.
    #[test]
    fn a_side_with_no_cut_corner_is_not_inset() {
        let rect = Rectangle::<i32, Physical>::new((100, 100).into(), (300, 200).into());
        let inside = opaque_inside(rect, (0.0, 20.0, 20.0, 12.0));
        assert_eq!(inside.loc.y, 100, "nothing is cut along the top");
        assert_eq!(inside.loc.x, 112, "the left side is inset by its larger corner");
        assert_eq!(inside.size.h, 180, "only the bottom is taken");
        assert_eq!(inside.size.w, 268);
    }
```

Keep `a_radius_bigger_than_the_window_claims_nothing` and the fractional-radius case, adapted to the new signature.

- [ ] **Step 2: Run it and watch it fail**

- [ ] **Step 3: Write it**

`opaque_inside` takes `(top, right, bottom, left)` and insets each independently, each `ceil`ed and clamped exactly as the single radius is today. The uniform becomes `Uniform::new(RADIUS_UNIFORM, (tl, tr, bl, br))` with `UniformType::_4f` at both the `compile_custom_texture_shader` call and wirecheck's.

In `decoration.rs`, `client_radius(style)` becomes four writes:

```rust
        // All four, and not the one a titlebar happens to need. Same reason
        // all four insets are written: a layer that reads a property the
        // compositor decided not to send gets 0, and 0 is a legal radius.
        scene.set_int("clientRadiusTopLeft", …);
```

- [ ] **Step 4–5: Run, then see each fail**

- Inset every side by the maximum of all four → `a_side_with_no_cut_corner_is_not_inset` fails on `loc.y`.
- Transpose `right` and `left` in the inset → the same test fails on `loc.x`.
- Write only `clientRadiusTopLeft` and not the other three → the layer test from Task 4 fails. If no test covers the other three yet, **say so** rather than assuming.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/pass.rs crates/solium/src/decoration.rs
git commit -m "pass: four radii to the shader, and an opaque region that only gives up the sides that are cut"
```

---

### Task 4: two styles, and a gate that can tell them apart

**Files:**
- Modify: `dev/wirecheck/src/main.rs`, `crates/solium/qml/panes/rounded/`
- Create: `crates/solium/qml/panes/flush/`
- Modify: `docs/ricing.md`

- [ ] **Step 1: Make wirecheck tell the corners apart**

The readback currently draws at one radius and asserts **all four** corners at alpha 0. Draw a second picture with `(0, 0, r, r)` and assert the **top two are opaque and the bottom two cut**. That is the case a transposed `vec4` fails and no text test can.

Keep the existing all-four case: it is what proves the fold still folds.

- [ ] **Step 2: See it fail**

Swap `corner_radius.x` and `corner_radius.z` in the shader's select. Expected: the new wirecheck case reds; every `cargo test` stays green. **Report both halves** — the point is that the text tests cannot see this and the readback can.

- [ ] **Step 3: Drop the overhang**

`panes/rounded/Frame.qml` draws its bar `insetTop + clientRadius` tall and squares off its own bottom corners, to cover notches. With the client's top corners rounded, the titlebar behind them is what should show — so the bar becomes `insetTop` tall and the square-off child goes. **Verify on screen that the notches do not come back**, and that the top row of the client is no longer covered.

- [ ] **Step 4: Add the flush style**

`panes/flush/` — `client.radius: 12`, `radiusTopLeft: 0`, `radiusTopRight: 0`, a titlebar with `radius` on its top corners only. The client's square top meets the bar's square bottom flush.

- [ ] **Step 5: Look at both**

For each of `rounded` and `flush`, with the nested harness and a small client:
1. The client's top row is **not** covered — read the pixels where the prompt's first line is.
2. `flush`: the join between bar and client shows **no wallpaper** — sample along it.
3. `rounded`: the client's cut top corners show the **titlebar's colour**, not the wallpaper.

Numbers, not adjectives. If any fails, that is the result; report it rather than adjusting the shader.

- [ ] **Step 6: Document and commit**

`docs/ricing.md` gains the four keys and both idioms, with the table from this plan's *Why* section.

```bash
git add -A
git commit -m "panes: two ways a titlebar and a rounded window can meet"
```

---

## Self-Review

**Spec coverage.** This extends `Effect`'s parameters and touches nothing about what a pass is, which is the spec's *An effect declares its inputs*. The spec's node model says `effects: Vec<Effect>` without constraining an effect's shape, so nothing there needs amending. The final review of the passes plan noted `Effect::radius()` is "a total accessor a `Shadow` variant would have to answer falsely" — `radii()` inherits that and this plan does not fix it; generalising `Pass` is shadow's job and is recorded as such.

**Placeholders.** Task 3's `client_radius` replacement shows one `set_int` and a `…` for the value, because the existing helper's shape should be followed rather than guessed from here. Task 4's steps are deliberately outcomes rather than code — it is the looking task, and prescribing its script is how a verification step becomes a formality.

**Type consistency.** `Corners` has the same four `f64` fields everywhere; `max_of_side` returns `(top, right, bottom, left)` and `opaque_inside` takes that tuple in that order — the one place an ordering mistake is silent, which is why Task 3's second mutation transposes it. `RADIUS_UNIFORM` keeps its name and changes type to `_4f` in both the compositor and wirecheck; the QML keys are `radiusTopLeft`/`TopRight`/`BottomLeft`/`BottomRight` in the QML, the reader, the tests and the docs.
