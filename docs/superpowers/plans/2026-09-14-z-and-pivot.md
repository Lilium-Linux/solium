# `z` and `pivot` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish `present::Frame` — a node can say how deep it is drawn and what its matrix turns about.

**Architecture:** Two fields on `Frame`, each answered where it is already computed. `z` is a stable sort of the panes at the one place draw order is decided; equal values keep the order the stack gave them, so the ordinary case is untouched. `pivot` replaces the rect's centre at the two lines in `warp.rs` that compute it today — which is where it has to live, because composing `translate(-p) · R · translate(p)` in `script.rs` needs the window's size and `transform_from` only sees the options table.

**Tech Stack:** Rust edition 2024, Smithay 0.7, `crates/solium/src/{present,warp,render,script}.rs`, Lua for the surface.

**Spec:** `docs/superpowers/specs/2026-09-12-panes-and-effects-design.md` — *What every node carries* and *Hit-testing does not move*, plus phase 1 item 7.

## Global Constraints

- Rust edition 2024. Workspace lints **deny** `unwrap_used`, `expect_used`, `panic`, `todo`. `unsafe_code` is `warn` — every `unsafe` block needs `#[expect(unsafe_code, reason = "…")]`.
- **All builds run in podman** via `dev/gate.sh`, which takes NO arguments. The host has no Qt 6 development files. The built binary *does* run on the host.
- **A node with no transform emits exactly the element it emits now, through exactly the path it takes now.** A default `z` must not sort anything into a different order, and a default `pivot` must produce a bit-identical mesh to today's centre.
- **Hit-testing does not move.** `rect` stays the truth for input. `z` changes draw order and nothing else — a window lifted above another is still clicked where the layout put it. Inverting a projective transform per pointer event is the alternative the spec rejects.
- `pivot` is a fraction of the rect, `0..1`, `(0.5, 0.5)` meaning the centre. It is **not** pixels.
- Never launch `claude-desktop`. `sudo` is not available.
- The user has a live Solium on tty3 out of this worktree: **never `pkill` by name.** Kill only pids you started.
- Verify rendering with the nested harness at `/tmp/claude-1000/-home-kotoxik-Development/690887c8-d6b5-43b1-abc8-b985cf8f39fd/scratchpad/shoot.sh`, not by reasoning. `SOLIUM_QML_GPU=1` nested renders no QML at all (no GBM device). See the memory `solium-verifying-qml-nested`.

## What is already done, so nobody builds it twice

The spec's item 7 says "`z`, `pivot`, node alpha". **Node alpha exists.** `Frame::opacity` is on every node and is honoured by the client's surface, its popups, the warped texture, the decoration's layers (`decoration.rs:249`, `drawing.alpha` — added because a closing window faded away under a titlebar that stayed solid) and scripted surfaces, which carry a whole `Transform<Frame>`. `sol.present` and `sol.present_group` both read `opacity` already. Verified by reading each site before this plan was written; there is nothing to add.

So this plan is two fields, not three.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/solium/src/present.rs` (modify) | `Frame` gains `z` and `pivot`, and the constructors that build one. |
| `crates/solium/src/warp.rs` (modify) | The two lines computing the rect's centre take the pivot instead. |
| `crates/solium/src/render.rs` (modify) | Panes are drawn in `z` order — one stable sort, at the one place draw order is decided. |
| `crates/solium/src/script.rs` (modify) | `z`, `pivot_x`, `pivot_y` on `sol.present` and `sol.present_group`. |
| `docs/ricing.md` (modify) | What the two knobs do and what they deliberately do not. |

---

### Task 1: `Frame` carries them

**Files:**
- Modify: `crates/solium/src/present.rs:200-240`
- Test: `crates/solium/src/present.rs` (its `mod tests`)

**Interfaces:**
- Produces: `Frame { rect, opacity, matrix, deform, z: f32, pivot: (f32, f32) }`. `Frame::real` sets `z: 0.0, pivot: (0.5, 0.5)`. `Frame::is_flat()` — if it exists — must not start answering differently.

- [ ] **Step 1: Write the failing test**

In `present.rs`'s `mod tests`:

```rust
    /// The identity frame is the one every undecorated, unanimated window on
    /// the machine gets, so its defaults are the whole of the cheap path.
    #[test]
    fn the_identity_frame_is_centred_and_at_depth_zero() {
        let frame = Frame::real(Rectangle::new((10, 20).into(), (300, 200).into()));
        assert!((frame.z - 0.0).abs() < f32::EPSILON, "depth zero, so nothing sorts");
        assert!((frame.pivot.0 - 0.5).abs() < f32::EPSILON);
        assert!((frame.pivot.1 - 0.5).abs() < f32::EPSILON, "the centre, as `matrix`'s doc has always said");
    }

    /// `scaled` is what "smaller, in place" means and is used by the open
    /// animation and by peek. It must carry both new fields through, because a
    /// frame that loses its pivot mid-animation turns about a different point
    /// for one frame and jumps.
    #[test]
    fn scaling_a_frame_keeps_its_depth_and_its_pivot() {
        let mut frame = Frame::real(Rectangle::new((0, 0).into(), (100, 100).into()));
        frame.z = 3.0;
        frame.pivot = (0.0, 1.0);
        let smaller = frame.scaled(0.5);
        assert!((smaller.z - 3.0).abs() < f32::EPSILON);
        assert_eq!(smaller.pivot, (0.0, 1.0));
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL to compile — no field `z` on `Frame`.

- [ ] **Step 3: Add the fields**

In `present.rs`, on `Frame`, after `deform`:

```rust
    /// How deep this node is drawn. Higher is nearer the viewer.
    ///
    /// **Equal values keep the order the stack gave them**, which is what
    /// makes the default free: every window is 0.0, the sort is stable, and
    /// the list comes out exactly as it went in. A script lifting one card of
    /// a stack does not have to restack the whole session to do it.
    ///
    /// Draw order only. `rect` is still the truth for input, so a window
    /// raised above its neighbour is still clicked where the layout put it --
    /// see the spec's *Hit-testing does not move*. The alternative is
    /// inverting a projective transform per pointer event and then explaining
    /// to a script why the window it placed is not where clicks land.
    pub(crate) z: f32,
    /// What `matrix` turns about, as a fraction of `rect`: `(0.5, 0.5)` is the
    /// centre, `(0.0, 0.0)` the top-left corner.
    ///
    /// A fraction and not pixels, so it survives the window being resized
    /// mid-animation -- which is the case that made `scaled` carry it.
    ///
    /// It cannot be composed in `script.rs`: `translate(-p) · R · translate(p)`
    /// needs the window's size, and `transform_from` sees only the options
    /// table, so it would be right when a script passed an explicit rect and
    /// silently wrong otherwise. It is resolved in `warp.rs`, at the two lines
    /// that compute the centre.
    pub(crate) pivot: (f32, f32),
```

and in `Frame::real`:

```rust
            z: 0.0,
            pivot: (0.5, 0.5),
```

`scaled` builds a new `Self { .. }` by hand; add `z: self.z, pivot: self.pivot`. **Search `present.rs` and the rest of the tree for every other `Frame {` literal and carry both fields through** — a `..Default::default()` would hide the next one that is added, which is why the struct has no `Default`.

- [ ] **Step 4: Run the tests**

```bash
dev/gate.sh
```

Expected: PASS.

- [ ] **Step 5: See each test fail**

- `pivot: (0.5, 0.5)` → `(0.0, 0.0)` in `real` → the first test fails on its pivot assertion.
- Drop `z: self.z` from `scaled` (use `0.0`) → the second fails.
- Drop `pivot: self.pivot` from `scaled` → the second fails on its pivot assertion.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/present.rs
git commit -m "present: a frame says how deep it is and what it turns about"
```

---

### Task 2: `pivot` is what the matrix turns about

**Files:**
- Modify: `crates/solium/src/warp.rs:149-170`
- Test: `crates/solium/src/warp.rs` (its `mod tests`)

**Interfaces:**
- Consumes: `Frame::pivot` from Task 1.
- Produces: `warp::mesh` — or whichever function holds those lines — takes the pivot. Its caller passes `frame.pivot`.

- [ ] **Step 1: Write the failing test**

The mesh is plain arithmetic on `f32`, so this is a unit test with no GPU:

```rust
    /// A quarter turn about the centre moves every corner. The same turn about
    /// the top-left corner leaves that corner exactly where it was -- which is
    /// the whole difference, and the reason a card stack needs this.
    #[test]
    fn a_pivot_is_the_point_the_matrix_leaves_alone() {
        let rect = Rectangle::<f64, Logical>::new((100.0, 100.0).into(), (200.0, 100.0).into());
        let turn = Mat4::rotate_z(std::f32::consts::FRAC_PI_2);

        let centred = mesh(rect, turn, None, (0.5, 0.5), 1.0).expect("a mesh");
        let cornered = mesh(rect, turn, None, (0.0, 0.0), 1.0).expect("a mesh");

        // Corner 0 is (u, v) = (0, 0): the rect's top-left, at (100, 100).
        let (cx, cy) = (cornered.corners[0].x, cornered.corners[0].y);
        assert!(
            (cx - 100.0).abs() < 0.01 && (cy - 100.0).abs() < 0.01,
            "a turn about the top-left leaves the top-left alone, got ({cx}, {cy})"
        );
        // And about the centre it does not, or the test above proves nothing.
        let (mx, my) = (centred.corners[0].x, centred.corners[0].y);
        assert!(
            (mx - 100.0).abs() > 1.0 || (my - 100.0).abs() > 1.0,
            "a turn about the centre moves the top-left, got ({mx}, {my})"
        );
    }

    /// And the default is bit-for-bit what the centre produced before this
    /// existed. Every unanimated window on the machine takes this path.
    #[test]
    fn the_default_pivot_is_the_centre_it_replaced() {
        let rect = Rectangle::<f64, Logical>::new((0.0, 0.0).into(), (300.0, 200.0).into());
        let turn = Mat4::rotate_y(0.3);
        let by_pivot = mesh(rect, turn, None, (0.5, 0.5), 1.0).expect("a mesh");
        for corner in &by_pivot.corners {
            assert!(corner.x.is_finite() && corner.y.is_finite());
        }
        // The centre of the rect is (150, 100); a rotation about it leaves the
        // centre of the mesh there.
        let centre = by_pivot.corners.len() / 2;
        let _ = centre;
    }
```

**Replace the second test's body with a real assertion once you have read `mesh`'s actual return type and corner ordering.** If `Mesh` does not expose corners as a slice, assert through whatever it does expose — but the property must be "the default pivot reproduces today's centre", checked against a value, not eyeballed. If the only honest way to check it is to keep a copy of the old arithmetic in the test, do that and say so.

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL to compile — `mesh` takes no pivot.

- [ ] **Step 3: Thread it through**

In `warp.rs`, replace

```rust
    let (centre_x, centre_y) = (
        rect.loc.x + rect.size.w / 2.0,
        rect.loc.y + rect.size.h / 2.0,
    );
```

with

```rust
    // The point the matrix turns about, and the point the projection is
    // measured from. `(0.5, 0.5)` is the rect's centre, which is what this
    // computed before `pivot` existed and is what every flat window still
    // passes -- so the default is not a special case, it is the same two
    // multiplications with a 0.5 that used to be spelled `/ 2.0`.
    let (centre_x, centre_y) = (
        rect.loc.x + rect.size.w * f64::from(pivot.0),
        rect.loc.y + rect.size.h * f64::from(pivot.1),
    );
```

Both uses below — the per-vertex `offset` and the `origin_x`/`origin_y` that puts it back — already read those two names, so they follow. Add `pivot: (f32, f32)` to the signature and pass `frame.pivot` from the caller.

- [ ] **Step 4: Run the tests**

```bash
dev/gate.sh
```

Expected: PASS.

- [ ] **Step 5: See each test fail**

- Hard-code `pivot` to `(0.5, 0.5)` inside `mesh`, ignoring the argument → `a_pivot_is_the_point_the_matrix_leaves_alone` fails on its first assertion.
- Use `pivot.1` for x and `pivot.0` for y → **the two cases above do NOT catch this, and that was a defect in this plan.** `(0.5, 0.5)` and `(0.0, 0.0)` are each their own transpose, so a non-square *rect* does not help — the *pivot* has to be asymmetric too. Add a `(1.0, 0.0)` case and check the mutation fails at that assertion. Corrected after the task was implemented, from the implementer's report; the original text claimed the 200x100 rect carried it, which was wrong about which symmetry mattered.

**And the same mistake has now been made three times in this one test, so make the fixture asymmetric in EVERY dimension it is asked about.** The pivot was symmetric (my error), then the rect's `loc` was symmetric — `(100, 100)` and `(0, 0)`, so `loc.x` and `loc.y` are interchangeable and a transposed origin passes both tests. Size, location and pivot each need two different numbers, or a swap in that dimension is invisible.

**A trap when re-running these mutations:** making `mesh` ignore its `pivot` argument needs a `let _ = pivot;`, or clippy's unused-variable error fires *before* the tests and the compile failure reads as the mutation being caught.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/warp.rs
git commit -m "warp: turn about the pivot, which at its default is the centre it replaced"
```

---

### Task 3: `z` orders what is drawn

**Files:**
- Modify: `crates/solium/src/render.rs:791` (the draw walk)
- Test: `crates/solium/src/render.rs`

**Interfaces:**
- Consumes: `Frame::z`.
- Produces: no new public API. The draw walk is ordered; `prepare` and every hit-test keep the stacking order they have.

- [ ] **Step 1: Write the failing test**

`Solium` is not constructible in a test, so test the ordering as a free function over `(PaneId, f32)` pairs — the same shape Task 5 of the passes plan used for `anywhere_on`:

```rust
    /// Equal depths keep the order the stack gave them. This is the whole of
    /// the cheap path: every window is 0.0, so the list must come out exactly
    /// as it went in, and `sort_by` being stable is what guarantees it.
    #[test]
    fn equal_depths_keep_the_stacking_order() {
        let mut order = vec![("a", 0.0_f32), ("b", 0.0), ("c", 0.0)];
        by_depth(&mut order);
        assert_eq!(
            order.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    /// And a raised node is drawn nearer the viewer -- which, in a list the
    /// renderer walks topmost-first, means EARLIER.
    #[test]
    fn a_higher_depth_is_drawn_in_front() {
        let mut order = vec![("under", 0.0_f32), ("over", 2.0), ("between", 1.0)];
        by_depth(&mut order);
        assert_eq!(
            order.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec!["over", "between", "under"]
        );
    }

    /// A NaN depth does not reorder anything and does not panic. `sort_by` on
    /// a comparator that returns `Ordering::Equal` for an incomparable pair is
    /// the only total answer available, and a script can reach this with one
    /// division.
    #[test]
    fn a_depth_that_is_not_a_number_is_left_where_it_is() {
        let mut order = vec![("a", 0.0_f32), ("nan", f32::NAN), ("b", 0.0)];
        by_depth(&mut order);
        assert_eq!(order.len(), 3);
        assert_eq!(order[1].0, "nan", "unmoved, not sorted to an end");
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL — no function `by_depth`.

- [ ] **Step 3: Write it and use it**

```rust
/// Order nodes for drawing: deepest last, equal depths as they came.
///
/// Generic over what is carried so the compositor's walk and the test drive
/// the same code -- `Solium` is not constructible in a test, and an ordering
/// rule checked against a second copy of itself is not checked.
///
/// `total_cmp` is deliberately not used. It orders NaN, and a script reaching
/// NaN with one division would then find its window teleported to the front or
/// the back of the stack for reasons nothing on screen explains. Answering
/// `Equal` leaves it exactly where the stack put it, which is the same thing
/// the default depth does and the only answer a user can predict.
fn by_depth<T>(nodes: &mut [(T, f32)]) {
    nodes.sort_by(|(_, left), (_, right)| {
        right.partial_cmp(left).unwrap_or(std::cmp::Ordering::Equal)
    });
}
```

At the draw walk (`render.rs:791`), collect `(pane, window, z)` — the `z` is `frame.z`, which that loop already resolves — then `by_depth` before emitting. **`prepare` at `render.rs:288` keeps `on_screen()` untouched**: it decides what to capture, not what covers what, and sorting it would be work for no reason.

- [ ] **Step 4: Run the tests**

```bash
dev/gate.sh
```

Expected: PASS.

- [ ] **Step 5: See each test fail**

- `right.partial_cmp(left)` → `left.partial_cmp(right)` → `a_higher_depth_is_drawn_in_front` fails.
- `sort_by` → `sort_unstable_by` → **run this one and report what happens.** It may pass: three equal elements can survive an unstable sort by luck. If it passes, say so and add a case with enough elements to make it fail, or state plainly that stability is unpinned.
- `unwrap_or(Equal)` → `unwrap_or(Ordering::Less)` → the NaN test fails.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/render.rs
git commit -m "render: draw nodes deepest last, and equal depths as the stack gave them"
```

---

### Task 4: a script can set them

**Files:**
- Modify: `crates/solium/src/script.rs:1285-1330` (the `present` option reads) and `transform_from`
- Modify: `docs/ricing.md`
- Test: `crates/solium/src/script.rs`

**Interfaces:**
- Consumes: `Frame::{z, pivot}`.
- Produces: `sol.present(id, { z = 2, pivot_x = 0, pivot_y = 1 })` and the same three keys on `sol.present_group`.

- [ ] **Step 1: Write the failing test**

Beside the existing `present` option tests in `script.rs`:

```rust
    /// The three new keys, read the way every other option is.
    #[test]
    fn present_reads_depth_and_pivot() {
        let frame = frame_from_options(r#"{ z = 2.5, pivot_x = 0.0, pivot_y = 1.0 }"#);
        assert!((frame.z - 2.5).abs() < f32::EPSILON);
        assert_eq!(frame.pivot, (0.0, 1.0));
    }

    /// And a table that mentions none of them is the frame every window has
    /// had until now -- depth zero, turning about its centre.
    #[test]
    fn present_without_them_is_unchanged() {
        let frame = frame_from_options(r#"{ x = 10, y = 20 }"#);
        assert!((frame.z - 0.0).abs() < f32::EPSILON);
        assert_eq!(frame.pivot, (0.5, 0.5));
    }

    /// One axis given and not the other keeps the centre on the axis that was
    /// not mentioned. `pivot_x = 0` means "the left edge", not "the top-left
    /// corner", and a script saying one thing should not get two.
    #[test]
    fn one_pivot_axis_leaves_the_other_centred() {
        let frame = frame_from_options(r#"{ pivot_x = 0.0 }"#);
        assert_eq!(frame.pivot, (0.0, 0.5));
    }
```

`frame_from_options` is a helper you write if none exists: build a `Lua`, evaluate the table, and run it through the same reader `sol.present` uses. **If the existing tests reach the option reader another way, use theirs** — a second path to the same code is a test of the wrong thing.

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: FAIL — `z` is not read.

- [ ] **Step 3: Read them**

Where `opacity` is read:

```rust
            let z = options.get::<Option<f32>>("z")?.unwrap_or(0.0);
            // Each axis defaults on its own. `pivot_x = 0` means the left edge
            // and nothing about the vertical, so a script that names one gets
            // one -- defaulting the pair together would silently move the
            // other axis to a corner.
            let pivot = (
                options.get::<Option<f32>>("pivot_x")?.unwrap_or(0.5),
                options.get::<Option<f32>>("pivot_y")?.unwrap_or(0.5),
            );
```

and carry both into the `Frame` built below, and into `present_group`'s.

- [ ] **Step 4: Run the tests**

```bash
dev/gate.sh
```

Expected: PASS.

- [ ] **Step 5: See each test fail**

- `unwrap_or(0.5)` → `unwrap_or(0.0)` on `pivot_y` → `one_pivot_axis_leaves_the_other_centred` fails, and so does `present_without_them_is_unchanged`.
- Read `pivot_y` into `pivot.0` and `pivot_x` into `pivot.1` → `present_reads_depth_and_pivot` fails.
- Default `z` to `1.0` → `present_without_them_is_unchanged` fails.

- [ ] **Step 6: Document it**

In `docs/ricing.md`, beside the other `sol.present` options:

```markdown
### Depth and pivot

    sol.present(id, { z = 2 })                       -- drawn in front
    sol.present(id, { rotate_y = 20, pivot_x = 0 })  -- turns about its left edge

`z` is draw order and nothing else. Equal values keep the order the stack gave
them, so the default costs nothing — and a window raised above its neighbour is
still **clicked where the layout put it**, because `rect` stays the truth for
input.

`pivot` is a fraction of the window, not pixels: `(0.5, 0.5)` is the centre and
is the default, `(0, 0)` the top-left corner. Each axis defaults on its own.
```

- [ ] **Step 7: Commit**

```bash
git add crates/solium/src/script.rs docs/ricing.md
git commit -m "script: z and pivot on present, each axis defaulting on its own"
```

---

### Task 5: watch it

**Files:**
- No production change expected. If one is needed, that is the finding.

- [ ] **Step 1: Turn about a corner, and see it**

Using the nested harness (`scratchpad/shoot.sh`, or adapt it), with a `SOLIUM_LUA_INIT` script that presents one window twice — once `rotate_z = 20` with the default pivot, once with `pivot_x = 0, pivot_y = 0` — capture a frame of each.

Expected, and measure it rather than eyeball it: **the window's top-left corner is at the same pixel in the second capture as in an untransformed one, and is not in the first.** Report the two coordinates.

- [ ] **Step 2: Raise one window over another, and see it**

Two overlapping windows; present the lower one with `z = 1`. Capture before and after.

Expected: the pixels where they overlap come from the other window after. Report the colour at one overlapping point in each.

- [ ] **Step 3: Confirm clicks did not move**

With the raised window still raised, drive `SOLIUM_DRAG_AT` into the overlap and confirm focus goes to the window the **layout** has on top, not the one now drawn on top. This is the spec's *Hit-testing does not move*, and it is the half a screenshot cannot show.

If it does not hold, that is the result — report it rather than changing the hit-test, because `rect` staying the truth for input is a decision the spec makes and not a bug to fix here.

- [ ] **Step 4: Commit whatever the looking produced**

```bash
git add -A
git commit -m "present: z and pivot, seen on a screen"
```

---

## Self-Review

**Spec coverage.** Item 7 names three things; two are built here and the third — node alpha — was verified as already present before the plan was written, with the sites listed. *What every node carries* is fully covered: `rect`, `opacity`, `matrix`, `deform` existed; `z` and `pivot` are Tasks 1–4. *Hit-testing does not move* is a global constraint and Task 5 Step 3 checks it. The spec's note that `pivot` "belongs at the two lines in `warp.rs` that compute the centre today" is Task 2, at those lines.

**Placeholders.** Two admitted gaps, both labelled where they sit rather than hidden. Task 2's second test has a body that must be finished against `mesh`'s real return type, which I did not read closely enough to write blind — the task says so and says what the property must be. Task 4's `frame_from_options` may already exist under another name, and the task says to prefer the existing one. Everything else carries the code to write.

**Type consistency.** `z: f32` and `pivot: (f32, f32)` are the same types in `Frame`, in `by_depth`, in `mesh`'s new argument and in the script reader. `by_depth` is generic over the payload so the test and the compositor share it. The Lua keys are `z`, `pivot_x`, `pivot_y` in the reader, the tests and the docs.
