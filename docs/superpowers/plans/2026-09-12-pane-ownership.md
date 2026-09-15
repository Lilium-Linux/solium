# Pane ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A pane owns its own frame state, so "does this pane have a frame?" has exactly one answer and the illegal states stop being representable.

**Architecture:** Five collections keyed by `PaneId` — two in `Decorations`, three in `Solium` — are reconciled by hand against a set of live panes. They collapse into one `Frame` enum on `Pane`, plus two timers that move in with it. Done by strangler: the enum is added alongside the tables and shadows them, every reader moves over under a debug assertion that the two agree, and only then are the tables deleted.

**Tech Stack:** Rust 2024, Smithay 0.7.

**Spec:** `docs/superpowers/specs/2026-09-12-panes-and-effects-design.md`, and the "Pane ownership" section of `docs/superpowers/specs/2026-09-08-pane-styles-design.md`, which this implements.

## Global Constraints

- Rust edition 2024. Workspace lints **deny** `unwrap_used`, `expect_used`, `panic`, `todo`. `unsafe_code` is `warn` — every `unsafe` block needs `#[expect(unsafe_code, reason = "…")]`.
- **All builds run in podman**, never on the host: `dev/gate.sh` wraps it and takes no arguments. The host has no Qt 6 development files.
- `dev/wirecheck`'s six negative controls must keep biting. `dev/wirecheck/README.md` says how to generate and run them.
- **This is a refactor. It must change no behaviour.** Every stage is verified by the desktop looking and behaving identically, not by tests alone.
- Never launch `claude-desktop`. Never start the compositor — there is no free VT.

---

## Amended after Task 1 — measured, not assumed

**Drop the `Box`.** `size_of::<Decoration>()` is **248**, and
`Frame::Styled(Decoration)` inline is **also 248** — the discriminant lands in
a niche, so inlining is free relative to the decoration itself. Boxing costs an
allocation to save nothing. Task 4 uses `Styled(Decoration)`.

**`Frame::Styled` carries `Insets` until Task 4, not a `Decoration`.** A
`Decoration` owns a live Qt scene, is not `Clone`, and `frames` holds the only
one there is — so while the tables stay authoritative the pane cannot hold one
without building a second Qt scene per window, which is a behaviour change
wearing a refactor's clothes. `Insets` is `Copy`, written once at construction,
and is the only thing `insets_of` ever asks a decoration for.

**The consequence for Task 2:** only the readers that want *insets* can move in
Task 2. The ones that want the `Decoration` itself — `frame`, `pointer`,
`on_button`, `take_action`, `restore` — wait for Task 4, when `frames` is
deleted and the decoration is **moved** in rather than cloned.

**Writers are not at call sites.** All six mutators are methods on
`Decorations`, which holds `&mut self` and has no access to `Panes`, and
`insert` alone picks between three outcomes internally. Task 1 therefore
*derives* the pane's value from the authority afterwards (`shadow_frame` reads
`frame_of`) rather than reasoning it out a second time beside each writer. Keep
that shape.

**`Pane::loading` takes six arguments.** Every test snippet below that calls it
with one is wrong; use the real signature.

**The file lists below omit `state.rs` and `xwayland.rs`,** where all ten
writer sites live.

---

## Why

`Decorations` holds `frames: HashMap<PaneId, Decoration>` and `bare:
HashSet<PaneId>`. Two tables answer one question, so they can disagree:
`Solium::insets_of` (`state.rs:2392`) checks `frames` first, so a pane in both
has `bare` silently ignored. Nothing tests that.

Three more collections key on the same id — `closing` (`state.rs:248`),
`asked` (`state.rs:254`) and `hovered_frame` (`state.rs:246`) — and
`sync_panes` (`state.rs:1255`) reconciles them by hand in three places
(`state.rs:1306`, `state.rs:1307`, `decoration.rs:726-727`). A pane removed
without one of those retains running is a leak that nothing reports.

Pane styles turns one entry per pane into three. This lands first so that it
does not.

`hovered_frame` **stays where it is.** It is a property of the pointer — which
pane is hovered — not of a pane. Moving it in would mean a bool on every pane
and a scan to find the one that is true.

---

### Task 1: `Frame` on `Pane`, shadowing the tables

**Files:**
- Modify: `crates/solium/src/pane.rs`
- Modify: `crates/solium/src/decoration.rs`

**Interfaces:**
- Produces: `pane::Frame`, `Pane::frame`, `Pane::frame_mut`, `Pane::set_frame`.

- [ ] **Step 1: Write the failing test**

In `pane.rs`:

```rust
#[test]
fn a_pane_is_pending_until_it_is_told_otherwise() {
    let mut pane = Pane::loading("kitty".into(), Duration::ZERO);
    assert!(matches!(pane.frame(), Frame::Pending));
    pane.set_frame(Frame::None);
    assert!(matches!(pane.frame(), Frame::None));
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
dev/gate.sh
```

Expected: `Frame` does not exist.

- [ ] **Step 3: Add the enum**

```rust
/// What the compositor draws around this pane's client.
///
/// One value rather than two tables. `Decorations` held `frames` and `bare`
/// as parallel collections keyed by `PaneId`, and two tables answering one
/// question can disagree — `insets_of` checked `frames` first, so a pane in
/// both had `bare` silently ignored.
#[derive(Debug, Default)]
pub(crate) enum Frame {
    /// A client is still coming. Keep reserving the insets a frame will want,
    /// or the window jumps when it finally arrives.
    #[default]
    Pending,
    /// There will never be a frame: the client draws its own, or this is an
    /// override-redirect menu. Not the same as "not yet".
    None,
    /// Drawn by the compositor.
    Styled(Box<crate::decoration::Decoration>),
}
```

`Box` because `Decoration` is large and every pane carries this.

- [ ] **Step 4: Add the accessors, and populate alongside the tables**

`Pane` gains `frame: Frame`. Everywhere `Decorations` inserts into `frames`
or `bare`, set the pane's `Frame` to match. **Do not remove anything yet** —
both live, and the tables stay authoritative.

- [ ] **Step 5: Run the gate**

```bash
dev/gate.sh
```

Expected: passes. Nothing reads the new field, so nothing can have changed.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/pane.rs crates/solium/src/decoration.rs
git commit -m "pane: a frame is one value, kept alongside the tables that hold it"
```

---

### Task 2: Every reader moves over, under an assertion

**Files:**
- Modify: `crates/solium/src/state.rs`
- Modify: `crates/solium/src/decoration.rs`
- Modify: `crates/solium/src/render.rs`

**Interfaces:**
- Consumes: `Pane::frame` from Task 1.

- [ ] **Step 1: Add the agreement assertion**

In `Decorations`, one helper used by every reader during the transition:

```rust
/// The tables and the pane must agree. This exists for exactly as long as
/// both do — Task 4 deletes it with them.
#[cfg(debug_assertions)]
fn agree(&self, id: PaneId, pane: &crate::pane::Pane) {
    let table = if self.frames.contains_key(&id) {
        "styled"
    } else if self.bare.contains(&id) {
        "none"
    } else {
        "pending"
    };
    let owned = match pane.frame() {
        Frame::Styled(_) => "styled",
        Frame::None => "none",
        Frame::Pending => "pending",
    };
    debug_assert_eq!(table, owned, "pane {id:?} disagrees with the tables");
}
```

- [ ] **Step 2: Move `insets_of` to the pane**

`state.rs:2392`. It becomes a pure function of the pane:

```rust
    pub(crate) fn insets_of(&self, id: crate::pane::PaneId) -> Insets {
        let Some(pane) = self.panes.get(id) else {
            return Insets::default();
        };
        #[cfg(debug_assertions)]
        self.decorations.agree(id, pane);
        match pane.frame() {
            // A frame that has not arrived still reserves what one will want,
            // or the window jumps when it does.
            Frame::Pending => self.decorations.insets_for_style(),
            Frame::None => Insets::default(),
            Frame::Styled(frame) => frame.insets(),
        }
    }
```

- [ ] **Step 3: Move the readers that want insets**

Find them: `grep -rn "frames\|\.bare" crates/solium/src/`. Each becomes a match
on `pane.frame()` with the assertion above it. Do not change what any of them
decides — only where it reads it from.

**Only the insets readers move in this task.** `Frame::Styled` carries `Insets`
until Task 4 (see *Amended* above), so `frame`, `pointer`, `on_button`,
`take_action` and `restore` — which want the `Decoration` itself — stay on the
tables for now. Moving them would mean a second Qt scene per window.

**The assertion checks agreement, not correctness.** Task 1 found two
pre-existing bugs the shadow reproduces faithfully: a decoration whose QML
fails to load keeps reserving a titlebar for ever, and `decoration = "none"`
only takes effect for windows opened afterwards. The tables and the pane agree
in both cases, and both are wrong together. Do **not** fix them here — a
refactor that changes no behaviour cannot also change behaviour, or nobody can
tell which change caused what.

- [ ] **Step 4: Run the gate**

```bash
dev/gate.sh
```

Expected: passes, with no assertion firing. A firing assertion here is a real
disagreement the tables were hiding, and is worth reporting rather than
silencing.

- [ ] **Step 5: Commit**

```bash
git add -A crates/solium/src
git commit -m "pane: read the frame from the pane, and assert the tables agree"
```

---

### Task 3: `closing` and `asked` move in

**Files:**
- Modify: `crates/solium/src/pane.rs`
- Modify: `crates/solium/src/state.rs`

**Interfaces:**
- Consumes: `Pane::frame`.
- Produces: `Pane::closing_since`, `Pane::asked_at` and their setters.

`closing` (`state.rs:248`) and `asked` (`state.rs:254`) are `HashMap<PaneId,
Duration>` retained in `sync_panes` for the same reason the frame tables are.
They are per-pane timers and belong on the pane.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_closing_timer_goes_when_the_pane_does() {
    let mut panes = Panes::default();
    let id = panes.open(Pane::loading("kitty".into(), Duration::ZERO));
    if let Some(pane) = panes.get_mut(id) {
        pane.begin_closing(Duration::from_millis(10));
    }
    panes.remove(id);
    assert!(panes.get(id).is_none());
    // Nothing to retain: the timer left with its pane.
}
```

- [ ] **Step 2: Move the fields**

`Option<Duration>` on `Pane`, not a map. Move every read and write. Delete the
two `retain` lines at `state.rs:1306-1307`.

- [ ] **Step 3: Run the gate, then commit**

```bash
dev/gate.sh
git add -A crates/solium/src
git commit -m "pane: a pane's own timers live on it, and leave with it"
```

---

### Task 4: Delete the tables

**Files:**
- Modify: `crates/solium/src/decoration.rs`
- Modify: `crates/solium/src/state.rs`

- [ ] **Step 1: Remove `frames`, `bare`, `Decorations::retain` and `agree`**

`Decorations` keeps only `style`. Everything that inserted into a table now
sets the pane's `Frame` and nothing else.

- [ ] **Step 2: Confirm the reconciliation is gone**

```bash
grep -n "retain" crates/solium/src/state.rs crates/solium/src/decoration.rs
```

Expected: the screencopy `retain` at `state.rs:896` and nothing about panes.
`sync_panes` no longer reconciles anything keyed by `PaneId` except the panes
themselves.

- [ ] **Step 3: Run the gate, then commit**

```bash
dev/gate.sh
git add -A crates/solium/src
git commit -m "pane: delete the tables the pane now owns"
```

---

### Task 5: Make the illegal state unrepresentable, and prove it

**Files:**
- Modify: `crates/solium/src/pane.rs`
- Modify: `crates/solium/src/decoration.rs`

- [ ] **Step 1: Tests for what used to be possible**

```rust
#[test]
fn a_pane_cannot_be_both_styled_and_bare() {
    // Not a runtime check — `Frame` is an enum, so this is the type system.
    // The test exists to say so, and to fail loudly if anyone reintroduces a
    // second source of truth.
    let mut pane = Pane::loading("kitty".into(), Duration::ZERO);
    pane.set_frame(Frame::None);
    assert!(pane.frame_mut().styled().is_none());
}

#[test]
fn reloading_a_style_keeps_a_window_framed() {
    // The pre-refactor comment at decoration.rs:665 records why: rebuilding
    // rather than dropping is what stops a reload leaving every open window
    // bare until it is reopened.
}
```

Fill the second one in against the real reload path.

- [ ] **Step 2: Run the gate**

```bash
dev/gate.sh
```

- [ ] **Step 3: Verify the six controls still bite**

Per `dev/wirecheck/README.md`. None of them is near this code, so unchanged is
the expected result — confirm rather than assume.

- [ ] **Step 4: Commit**

```bash
git add -A crates/solium/src
git commit -m "pane: the illegal state is gone, and a test says so"
```

---

## What this does not do

No layers, no bleed, no `PaneStyle` type. `Frame::Styled` holds today's
`Decoration`, and pane styles replaces what is inside it without changing what
a pane owns — which is why this lands first and safely.

`hovered_frame` stays on `Solium`, for the reason given above.
