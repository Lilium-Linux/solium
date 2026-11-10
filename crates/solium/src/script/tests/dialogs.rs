//! The layouts, against the real `tiling.lua` and `scrolling.lua`.
//!
//! Issue #72's harder half is not the protocol plumbing -- that is one state, a
//! handler and a delegate macro, and it is exercised by the compositor starting
//! at all. It is what the *layouts* do with a modal dialog, and that is Lua, so
//! a test of it has to run Lua.
//!
//! So this runs the shipped scripts, unmodified, resolved through the same
//! `package.path` the compositor sets, against a snapshot of synthetic windows
//! -- and reads the `sol.place` commands that come back. That is the whole of
//! what a layout does, so it is the whole of what there is to check.
//!
//! **Both layouts are driven by the same assertions, from one list.** A modal
//! that is right in tiling and wrong in scrolling is the failure this is most
//! likely to have, and two near-identical test functions is how it would be
//! missed: somebody fixes one and copies the test, and the copy asserts about
//! the layout it was copied from.
//!
//! What is *not* here: `set_modal` arriving over the wire. That needs a Wayland
//! client, a display and a socket, and there is nothing in this crate's tests
//! that can build one -- so the step from "a client called `set_modal`" to
//! "`WindowInfo::modal` is true" is covered by the type system and by running
//! the compositor, and not by this. The snapshot is built by hand here, which
//! is honest about where the seam is.

use super::{Command, Drawn, MonitorInfo, Parentage, Rect, Scripts, Snapshot, WindowInfo};
use crate::present::Frame;
use smithay::utils::{Logical, Rectangle};

/// One screen, no bar, round numbers so a wrong answer reads as a wrong
/// place rather than as arithmetic.
const AREA: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 2560.0,
    h: 1440.0,
};

/// The second screen, where there are two: the same size, immediately to the
/// right of the first. See [`second_monitor`].
const BESIDE: Rect = Rect {
    x: 2560.0,
    y: 0.0,
    w: 2560.0,
    h: 1440.0,
};

/// Both layouts, by the module that defines them and the key that turns
/// them on. Every assertion below runs against both; see the module note.
const LAYOUTS: [(&str, &str); 2] = [("tiling", "super+t"), ("scrolling", "super+s")];

/// The shipped scripts, with one layout required and nothing else.
///
/// `package.path` is written by the script rather than left to
/// `Scripts::load`, which prepends the *developer's* `~/.config/solium` --
/// so on a machine with a user configuration this would silently test that
/// instead. The same trap `group.rs`'s `desk` fixture documents.
///
/// No `config.lua` of its own: the shipped defaults are what users get, and
/// a dialog that is only centred under a hand-written configuration is a
/// dialog that is not centred.
///
/// A directory per *call*, not per layout, and the serial number is not
/// decoration. `cargo test` runs these on several threads and more than one
/// test arranges the same layout, so a path derived from the layout name is
/// two threads writing and reading one `init.lua` at once. The symptom is
/// not a wrong configuration -- every test writes the same bytes -- it is a
/// truncated read, and it surfaces as "the layout key was not handled" in
/// whichever test lost the race. Observed while mutation-testing this
/// module: a change that should have failed one assertion failed a
/// different test's, in a different file. `group.rs`'s `desk` fixture
/// documents the same trap one step less far.
fn scripts(name: &str, layout: &str) -> Scripts {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "solium-dialogs-{name}-{}-{serial}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            "package.path = {shipped:?} .. \"/?.lua\"\n\
             require(\"modes\")\n\
             require({layout:?})\n",
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    )
    .expect("writing the entry point");
    Scripts::load(&entry).expect("loading the shipped layout")
}

fn monitor() -> MonitorInfo {
    MonitorInfo {
        name: "DP-1".to_owned(),
        area: AREA,
        whole: AREA,
        scale: 1.0,
        focused: true,
        primary: true,
        transform: "normal".to_owned(),
        off: false,
    }
}

/// The screen to the right of [`monitor`], in the one global coordinate
/// space the compositor has: same size, offset by its whole width.
///
/// Not focused and not primary, because the point of it is to be the screen
/// nothing falls back to. Every wrong answer about where a dialog goes lands
/// on `DP-1` — it is the monitor at the origin, where every toplevel is
/// mapped, and the one `sol.monitor()` answers — so an assertion that a
/// dialog is on `DP-2` cannot pass by accident.
fn second_monitor() -> MonitorInfo {
    MonitorInfo {
        name: "DP-2".to_owned(),
        area: BESIDE,
        whole: BESIDE,
        scale: 1.0,
        focused: false,
        primary: false,
        transform: "normal".to_owned(),
        off: false,
    }
}

/// A window the compositor has decided is on this monitor.
///
/// Both halves, because the compositor sets both and a layout reads them
/// from different places: `monitor` is what `monitors.each` groups by, and
/// the rect is what decides the answer again on the next pass
/// (`Solium::output_of`). A fixture that moved one without the other would
/// be a state the compositor never produces.
fn on(monitor: &MonitorInfo, mut window: WindowInfo) -> WindowInfo {
    window.monitor.clone_from(&monitor.name);
    window.rect.x = monitor.area.x;
    window.rect.y = monitor.area.y;
    window
}

/// A window as it is before any layout has had a say: the size its client
/// chose, in the corner a Wayland toplevel is mapped at.
fn window(id: u64, w: f64, h: f64) -> WindowInfo {
    WindowInfo {
        icon: None,
        client_side: false,
        blur_region: false,
        insets: crate::decoration::Insets::default(),
        id,
        rect: Rect {
            x: 0.0,
            y: 0.0,
            w,
            h,
        },
        drawn: Drawn::default(),
        title: format!("window {id}"),
        focused: false,
        monitor: "DP-1".to_owned(),
        modal: false,
        parent: Parentage::None,
        leaving: false,
        minimized: false,
        above: false,
        app_id: String::new(),
        min: None,
        max: None,
        cramped: false,
        shown: true,
        fullscreen: false,
        maximized: false,
        style: String::new(),
        x11_type: None,
        accepts_input: true,
        class: None,
        instance: None,
        content_type: "none",
        pid: None,
    }
}

/// Drawn at `rect`, where it lives: a window at rest, as the compositor
/// hands it to a script that has just placed it -- so `sol.window_at`
/// finds it there, which is what makes
/// `a_dropped_window_goes_to_the_tile_it_was_let_go_over` a drop onto
/// window 1. No screens in these snapshots, so no screen refuses it.
fn drawn(rect: Rect) -> Drawn {
    let rect = Rectangle::<f64, Logical>::new((rect.x, rect.y).into(), (rect.w, rect.h).into());
    let slot = rect.to_i32_round();
    Drawn {
        slot,
        frame: Frame {
            rect,
            ..Frame::real(slot)
        },
    }
}

fn modal(id: u64, w: f64, h: f64, parent: Parentage) -> WindowInfo {
    WindowInfo {
        icon: None,
        client_side: false,
        blur_region: false,
        insets: crate::decoration::Insets::default(),
        modal: true,
        parent,
        ..window(id, w, h)
    }
}

/// What the compositor looked like, with these monitors and these windows.
///
/// The monitor list is a parameter rather than a constant because it was a
/// constant, and a one-screen fixture cannot see a whole class of bug: every
/// window is mapped at (0, 0), so with one monitor every window is on the
/// right monitor no matter what a layout does with the answer. `work_area`
/// follows the focused screen, which is what `sol.monitor()` reports.
fn snapshot_on(monitors: Vec<MonitorInfo>, windows: Vec<WindowInfo>) -> Snapshot {
    let work_area = monitors
        .iter()
        .find(|monitor| monitor.focused)
        .or_else(|| monitors.first())
        .map_or(AREA, |monitor| monitor.area);
    Snapshot {
        windows,
        monitors,
        work_area,
        ..Snapshot::default()
    }
}

fn snapshot(windows: Vec<WindowInfo>) -> Snapshot {
    snapshot_on(vec![monitor()], windows)
}

/// Where each window was told to go, by id.
///
/// A layout may place the same window twice in one pass; the last one is
/// what the compositor applies, so it is what this reports.
fn placed(commands: &[Command]) -> std::collections::HashMap<u64, Rect> {
    let mut out = std::collections::HashMap::new();
    for command in commands {
        if let Command::Place { id, rect, .. } = command {
            out.insert(*id, *rect);
        }
    }
    out
}

/// **`sol.windows()` says what a window's frame takes on each side**
/// (#435): what a configuration putting a window back needs to tell the
/// frame from the client.
#[test]
fn sol_windows_says_what_a_windows_frame_takes() {
    let directory = std::env::temp_dir().join("solium-script-test-window-insets");
    let _ = std::fs::create_dir_all(&directory);
    let config = directory.join("init.lua");
    std::fs::write(
        &config,
        r#"
        sol.bind("Super+I", function()
            local w = sol.windows()[1]
            sol.place(w.id, {
                x = w.insets.left, y = w.insets.top,
                w = w.insets.right, h = w.insets.bottom,
            })
        end)
        "#,
    )
    .expect("writing the test script");
    let mut scripts = Scripts::load(&config).expect("loading the test script");
    let framed = WindowInfo {
        icon: None,
        client_side: false,
        blur_region: false,
        insets: crate::decoration::Insets {
            top: 32,
            right: 1,
            bottom: 2,
            left: 3,
        },
        ..window(7, 400.0, 300.0)
    };
    let outcome = scripts.key("super+i", snapshot(vec![framed]));
    assert!(outcome.handled);
    let rect = placed(&outcome.commands).get(&7).copied().expect("placed");
    assert_eq!(
        (rect.x, rect.y, rect.w, rect.h),
        (3.0, 32.0, 1.0, 2.0),
        "left, top, right and bottom, as the compositor has them"
    );
}

/// Rects out of Lua are `f64` through a divide or two, so they are compared
/// with a tolerance rather than exactly. A pixel is far below anything
/// these assertions are about and far above the error.
fn about(left: f64, right: f64) -> bool {
    (left - right).abs() < 1.0
}

fn centre(rect: Rect) -> (f64, f64) {
    (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0)
}

fn overlap(left: Rect, right: Rect) -> bool {
    left.x < right.x + right.w
        && right.x < left.x + left.w
        && left.y < right.y + right.h
        && right.y < left.y + left.h
}

/// Turn a layout on with these windows, and report where everything went.
///
/// Switching the mode on runs `started`, which adopts the windows and lays
/// them out -- the same path a user pressing the key takes.
fn arrange_on(
    name: &str,
    monitors: Vec<MonitorInfo>,
    windows: Vec<WindowInfo>,
) -> std::collections::HashMap<u64, Rect> {
    let (module, key) = LAYOUTS
        .iter()
        .find(|(module, _)| *module == name)
        .copied()
        .expect("a layout this module knows");
    let mut scripts = scripts(name, module);
    let outcome = scripts.key(key, snapshot_on(monitors, windows));
    assert!(outcome.handled, "{name}: the layout key was not handled");
    placed(&outcome.commands)
}

fn arrange(name: &str, windows: Vec<WindowInfo>) -> std::collections::HashMap<u64, Rect> {
    arrange_on(name, vec![monitor()], windows)
}

/// Whether each window's placements in `commands` were tiles, by id, in
/// the order they were sent. See `Command::Place::tile`.
fn tiles(commands: &[Command]) -> std::collections::HashMap<u64, Vec<bool>> {
    let mut out: std::collections::HashMap<u64, Vec<bool>> = std::collections::HashMap::new();
    for command in commands {
        if let Command::Place { id, tile, .. } = command {
            out.entry(*id).or_default().push(*tile);
        }
    }
    out
}

/// **#133: a layout's arrangement is a tile, and a dialog it centres is
/// not.**
///
/// The compositor holds a tile's client inside it. That is what an
/// arrangement wants -- a window that will not shrink as far as its slot
/// is cut to the slot rather than drawn over its neighbour -- and it is
/// wrong for a modal, which `dialogs.lua` centres over its parent at the
/// size it had and which may grow afterwards. So `sol.place` defaults to a
/// tile and `dialogs.place` says `tile = false`. Both layouts, since both
/// go through `dialogs.place`.
#[test]
fn a_layouts_arrangement_is_a_tile_and_a_dialog_it_centres_is_not() {
    for (name, key) in LAYOUTS {
        let mut scripts = scripts(name, name);
        let outcome = scripts.key(
            key,
            snapshot(vec![
                window(1, 800.0, 600.0),
                window(2, 800.0, 600.0),
                modal(3, 600.0, 400.0, Parentage::Window(1)),
            ]),
        );
        assert!(outcome.handled, "{name}: the layout key was not handled");
        let placed = tiles(&outcome.commands);
        for id in [1, 2] {
            assert!(
                placed
                    .get(&id)
                    .is_some_and(|each| !each.is_empty() && each.iter().all(|tile| *tile)),
                "{name}: window {id} is in the arrangement and was not placed as a tile: {placed:?}"
            );
        }
        assert!(
            placed
                .get(&3)
                .is_some_and(|each| !each.is_empty() && each.iter().all(|tile| !*tile)),
            "{name}: the dialog was placed as a tile, so a dialog that grows is cut to \
             the size it was centred at: {placed:?}"
        );
    }
}

/// **A layout that stops lets every window out of its tile.**
///
/// The mode is the script's, so the compositor cannot see a switch to
/// floating happen: `modes.use` tells it, with `sol.unplace` for every
/// window. Pressing a layout's key a second time is the switch to
/// floating, and asserted here as the one that must send it for every
/// window and place none. Turning a layout on sends it too -- the layout
/// being replaced may have been another -- and then places everything,
/// which the first half holds to: each window's last word is a tile.
#[test]
fn a_layout_that_stops_lets_every_window_out_of_its_tile() {
    for (name, key) in LAYOUTS {
        let mut scripts = scripts(name, name);
        let windows = || snapshot(vec![window(1, 800.0, 600.0), window(2, 800.0, 600.0)]);

        let on = scripts.key(key, windows());
        assert!(on.handled, "{name}: the layout key was not handled");
        for id in [1_u64, 2] {
            let last = on.commands.iter().rev().find_map(|command| match command {
                Command::Place {
                    id: placed, tile, ..
                } if *placed == id => Some(*tile),
                Command::Unplace { id: unplaced } if *unplaced == id => Some(false),
                _ => None,
            });
            assert_eq!(
                last,
                Some(true),
                "{name}: turning the layout on left window {id} out of a tile"
            );
        }

        let off = scripts.key(key, windows());
        let mut unplaced: Vec<u64> = off
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Unplace { id } => Some(*id),
                _ => None,
            })
            .collect();
        unplaced.sort_unstable();
        assert_eq!(
            unplaced,
            vec![1, 2],
            "{name}: switching to floating did not let every window go"
        );
        assert!(
            tiles(&off.commands).is_empty(),
            "{name}: nothing is laid out once the layout has stopped"
        );
    }
}

/// **A modal dialog takes no share of the screen.**
///
/// Stated as "the other windows are laid out exactly as they would be if
/// the dialog were not there", which is the strongest form of it and the
/// one that cannot pass by accident: a dialog that is merely *also* placed
/// on top of its parent, while still holding a slot in the tree, gives the
/// other windows different rects and fails here.
#[test]
fn a_modal_does_not_take_a_share_of_the_screen() {
    for (name, _) in LAYOUTS {
        let without = arrange(name, vec![window(1, 800.0, 600.0), window(2, 800.0, 600.0)]);
        let with = arrange(
            name,
            vec![
                window(1, 800.0, 600.0),
                window(2, 800.0, 600.0),
                modal(3, 600.0, 400.0, Parentage::Window(1)),
            ],
        );

        for id in [1, 2] {
            let before = without.get(&id).copied().unwrap_or_default();
            let after = with.get(&id).copied().unwrap_or_default();
            assert!(
                about(before.x, after.x)
                    && about(before.y, after.y)
                    && about(before.w, after.w)
                    && about(before.h, after.h),
                "{name}: window {id} was given {after:?} with a dialog on screen and \
                 {before:?} without one -- the dialog is taking a share of the screen"
            );
        }
        assert!(
            with.contains_key(&3),
            "{name}: the dialog was left out of the arrangement and then never \
             placed at all, which for a Wayland toplevel is the top-left corner"
        );
    }
}

/// **And it sits over the window waiting on it.**
///
/// Centred on its parent's *new* slot, not on the rect the snapshot
/// carried: the same pass that places the dialog has just moved the parent,
/// so the snapshot rect is stale by the time it is read. Asserted against
/// `placed[1]` for exactly that reason -- comparing with the input rect
/// would pass whichever answer the script gave.
#[test]
fn a_modal_is_centred_on_its_parent() {
    for (name, _) in LAYOUTS {
        let out = arrange(
            name,
            vec![
                window(1, 800.0, 600.0),
                window(2, 800.0, 600.0),
                modal(3, 600.0, 400.0, Parentage::Window(1)),
            ],
        );
        let parent = out.get(&1).copied().expect("the parent was placed");
        let dialog = out.get(&3).copied().expect("the dialog was placed");

        assert!(
            about(centre(parent).0, centre(dialog).0) && about(centre(parent).1, centre(dialog).1),
            "{name}: the dialog is at {dialog:?}, centred on {:?}, but its parent is \
             at {parent:?}, centred on {:?}",
            centre(dialog),
            centre(parent)
        );
        // Its own size, kept. A dialog knows how big it needs to be and a
        // layout that resizes it wraps the question onto four lines.
        assert!(
            about(dialog.w, 600.0) && about(dialog.h, 400.0),
            "{name}: the dialog was resized to {}x{}",
            dialog.w,
            dialog.h
        );
        // The other window is elsewhere, so this is a dialog over its own
        // parent rather than a dialog in the middle of the screen that
        // happens to be over everything.
        let other = out.get(&2).copied().expect("the other window was placed");
        assert!(
            !about(centre(parent).0, centre(other).0) || !about(centre(parent).1, centre(other).1),
            "{name}: both windows are in the same place, so centring on either \
             proves nothing"
        );
    }
}

/// **And it follows its parent onto the other screen.**
///
/// The case the single-monitor fixture could not see, and the reason the
/// fixture takes a monitor list now. Every toplevel is mapped at (0, 0) and
/// the compositor decides which screen a window is on from the centre of its
/// rect, so a dialog for a window on `DP-2` *arrives* belonging to `DP-1` —
/// this is the state the compositor really hands a layout, not a contrived
/// one. Clamp it into its own screen's work area and it is pinned to that
/// screen's edge; its centre stays there, so the next pass reads back the
/// same monitor and clamps it identically. It never converges, which is why
/// a second pass could exist for exactly this case and not deliver it.
///
/// Asserted two ways on purpose. Centred on its parent is the property; on
/// `DP-2` at all is the one that fails loudly when the clamp is taken from
/// the wrong screen, because the wrong answer is not a few pixels out — it
/// is a whole monitor away, against the edge nearest the parent.
#[test]
fn a_modal_is_centred_on_its_parents_monitor_and_not_its_own() {
    for (name, _) in LAYOUTS {
        let screens = vec![monitor(), second_monitor()];
        let out = arrange_on(
            name,
            screens,
            vec![
                // The parent, on the second screen.
                on(&second_monitor(), window(1, 800.0, 600.0)),
                // An unrelated window on the first, so the dialog cannot
                // land on an empty screen and look right by default.
                on(&monitor(), window(2, 800.0, 600.0)),
                // The dialog, where the compositor puts a new toplevel: the
                // origin, which is the *first* screen.
                modal(3, 600.0, 400.0, Parentage::Window(1)),
            ],
        );

        let parent = out.get(&1).copied().expect("the parent was placed");
        let dialog = out.get(&3).copied().expect("the dialog was placed");

        assert!(
            parent.x >= BESIDE.x,
            "{name}: the parent was laid out at {parent:?}, which is not on DP-2 \
             ({BESIDE:?}) -- the fixture is wrong before the dialog is even asked \
             about"
        );
        assert!(
            about(centre(parent).0, centre(dialog).0) && about(centre(parent).1, centre(dialog).1),
            "{name}: the dialog is at {dialog:?}, centred on {:?}, and its parent is \
             on the other screen at {parent:?}, centred on {:?}",
            centre(dialog),
            centre(parent)
        );
        assert!(
            dialog.x >= BESIDE.x
                && dialog.y >= BESIDE.y
                && dialog.x + dialog.w <= BESIDE.x + BESIDE.w
                && dialog.y + dialog.h <= BESIDE.y + BESIDE.h,
            "{name}: the dialog at {dialog:?} is not on DP-2 ({BESIDE:?}) at all -- it \
             was clamped into the work area of the screen it was mapped on rather \
             than its parent's"
        );
    }
}

/// **A modal on a modal is centred on where that modal ended up.**
///
/// A file chooser is modal for the document and its "Replace?" prompt is
/// modal for the chooser: two dialogs, one waiting on the other, and nothing
/// arranges either of them. So the chooser's rect in this pass is one only
/// this pass knows — which is what `placed` is for, and a dialog has to be
/// written into it like any other placement.
///
/// The prompt is listed *first*, which is the order that matters: the
/// snapshot is topmost first and a prompt is above the chooser that opened
/// it, so the naive pass reaches the child before the parent exists
/// anywhere. Left out of `placed`, the child falls through to the snapshot
/// — where its parent is still at the (0, 0) every toplevel is mapped at —
/// and lands in the top-left corner.
#[test]
fn a_modal_waiting_on_a_modal_is_centred_on_it() {
    for (name, _) in LAYOUTS {
        let out = arrange_on(
            name,
            vec![monitor()],
            vec![
                modal(3, 300.0, 200.0, Parentage::Window(2)),
                modal(2, 600.0, 400.0, Parentage::Window(1)),
                window(1, 800.0, 600.0),
                window(4, 800.0, 600.0),
            ],
        );

        let document = out.get(&1).copied().expect("the document was placed");
        let chooser = out.get(&2).copied().expect("the chooser was placed");
        let prompt = out.get(&3).copied().expect("the prompt was placed");

        assert!(
            about(centre(document).0, centre(chooser).0)
                && about(centre(document).1, centre(chooser).1),
            "{name}: the chooser at {chooser:?} is not over the document at \
             {document:?}, so nothing below proves anything"
        );
        assert!(
            about(centre(chooser).0, centre(prompt).0)
                && about(centre(chooser).1, centre(prompt).1),
            "{name}: the prompt is at {prompt:?}, centred on {:?}, and the chooser it \
             is waiting on is at {chooser:?}, centred on {:?} -- the prompt was \
             centred on where the chooser was in the snapshot rather than on where \
             this pass put it",
            centre(prompt),
            centre(chooser)
        );
    }
}

/// **A modal whose parent cannot be found still floats.**
///
/// Both ways of having nothing to centre on: the client named a parent this
/// compositor cannot point at (`Unknown` -- not mapped, not ours, or gone
/// while the dialog was up) and the client named none at all (`None`).
///
/// The two things that must not happen are "tiled" and "lost": it keeps its
/// size, it does not push the other windows around, and every edge of it is
/// on the screen. A dialog at the origin passes "not tiled" and is still
/// under the panel in the corner, so the second half is the one that
/// matters.
#[test]
fn a_modal_with_no_parent_to_find_is_centred_on_the_screen() {
    for (name, _) in LAYOUTS {
        for parent in [Parentage::Unknown, Parentage::None] {
            let without = arrange(name, vec![window(1, 800.0, 600.0)]);
            let out = arrange(
                name,
                vec![window(1, 800.0, 600.0), modal(3, 600.0, 400.0, parent)],
            );
            let dialog = out.get(&3).copied().expect("the dialog was placed");

            let before = without.get(&1).copied().unwrap_or_default();
            let after = out.get(&1).copied().unwrap_or_default();
            assert!(
                about(before.w, after.w) && about(before.h, after.h),
                "{name}/{parent:?}: the window lost room to a dialog with no parent"
            );

            assert!(
                about(centre(dialog).0, centre(AREA).0) && about(centre(dialog).1, centre(AREA).1),
                "{name}/{parent:?}: a parentless dialog went to {dialog:?} rather than \
                 the middle of the screen"
            );
            assert!(
                dialog.x >= AREA.x
                    && dialog.y >= AREA.y
                    && dialog.x + dialog.w <= AREA.x + AREA.w
                    && dialog.y + dialog.h <= AREA.y + AREA.h,
                "{name}/{parent:?}: the dialog at {dialog:?} hangs off the work area \
                 {AREA:?}"
            );
        }
    }
}

/// **`unset_modal` puts it back in the arrangement.**
///
/// The compositor turns `unset_modal` into a relayout -- see
/// `XdgDialogHandler::modal_changed` -- so the second pass here is exactly
/// what the compositor does: the same scripts, the same windows, one of
/// them no longer modal.
///
/// Rejoining is asserted two ways, because either alone is weak. The
/// arrangement resizes it -- a floating dialog keeps the size its client
/// chose, and this one no longer has it; and nothing overlaps anything,
/// which is a property an arrangement has and a floating window does not,
/// since a modal overlaps its parent by construction.
///
/// What is deliberately *not* asserted is "the other windows gave up room".
/// It is true of tiling and false of scrolling on purpose: a column's width
/// is a share of the view, so a third column makes the strip longer rather
/// than the first two thinner. That is the first paragraph of
/// `scrolling.lua` and the whole reason a scroller is not a row of windows
/// squeezed to fit, so a test that demanded it would be a test demanding
/// the wrong layout.
#[test]
fn unset_modal_returns_a_dialog_to_the_arrangement() {
    for (name, key) in LAYOUTS {
        let mut scripts = scripts(&format!("{name}-unset"), name);
        let outcome = scripts.key(
            key,
            snapshot(vec![
                window(1, 800.0, 600.0),
                window(2, 800.0, 600.0),
                modal(3, 600.0, 400.0, Parentage::Window(1)),
            ]),
        );
        assert!(outcome.handled, "{name}: the layout key was not handled");
        let floating = placed(&outcome.commands);

        // The client called `unset_modal`; everything else is unchanged.
        let outcome = scripts.relayout(snapshot(vec![
            window(1, 800.0, 600.0),
            window(2, 800.0, 600.0),
            window(3, 600.0, 400.0),
        ]));
        let tiled = placed(&outcome.commands);

        let was = floating.get(&3).copied().expect("the dialog was placed");
        let now = tiled.get(&3).copied().expect("the dialog is still placed");
        assert!(
            about(was.w, 600.0) && about(was.h, 400.0),
            "{name}: the dialog was {was:?} while modal, so it was never floating \
             at its own size and this test proves nothing about it stopping"
        );
        assert!(
            !about(now.w, 600.0) || !about(now.h, 400.0),
            "{name}: the dialog still has the size its client chose ({now:?}) after \
             `unset_modal`, so it is floating rather than arranged"
        );

        let rects: Vec<(u64, Rect)> = [1, 2, 3]
            .into_iter()
            .map(|id| {
                (
                    id,
                    tiled
                        .get(&id)
                        .copied()
                        .unwrap_or_else(|| panic!("{name}: window {id} was not placed")),
                )
            })
            .collect();
        for (index, (id, rect)) in rects.iter().enumerate() {
            for (other, against) in rects.iter().skip(index + 1) {
                assert!(
                    !overlap(*rect, *against),
                    "{name}: {id} at {rect:?} overlaps {other} at {against:?}, so one \
                     of them is still floating rather than arranged"
                );
            }
        }
    }
}

/// **#128: a layout reflows the moment a close is asked for.**
///
/// Against the shipped `tiling.lua` and `scrolling.lua`, the same way the
/// dialog tests above run them: synthetic snapshots in, `sol.place`
/// commands out. What the compositor does around these events -- when it
/// sends them, and what a window being closed looks like meanwhile -- is
/// `state.rs`'s `reflow_on_close` module; this is what the layouts do with
/// them.
mod reflow_on_close {
    use super::*;
    use std::collections::HashMap;

    /// The shipped layout, with `before` run ahead of it -- a setting,
    /// usually -- and the entry point, which a reload needs to read again.
    ///
    /// A directory per call, for the reason [`scripts`] gives.
    fn layout_with(layout: &str, before: &str) -> (Scripts, std::path::PathBuf) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "solium-reflow-{layout}-{}-{serial}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\n\
                 {before}\n\
                 require(\"modes\")\n\
                 require({layout:?})\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        )
        .expect("writing the entry point");
        let scripts = Scripts::load(&entry).expect("loading the shipped layout");
        (scripts, entry)
    }

    fn when_gone(layout: &str) -> String {
        format!("require(\"config\").{layout}.reflow_on_close = \"when_gone\"")
    }

    /// Windows `ids`, topmost first, each where `placed` put it -- so a
    /// layout reading the snapshot sees the arrangement it made -- and the
    /// ones in `leaving` marked as closing.
    fn desk(placed: &HashMap<u64, Rect>, ids: &[u64], leaving: &[u64]) -> Snapshot {
        snapshot(
            ids.iter()
                .map(|&id| {
                    let rect = placed.get(&id).copied().unwrap_or_default();
                    WindowInfo {
                        icon: None,
                        client_side: false,
                        blur_region: false,
                        insets: crate::decoration::Insets::default(),
                        rect,
                        drawn: drawn(rect),
                        leaving: leaving.contains(&id),
                        ..window(id, rect.w, rect.h)
                    }
                })
                .collect(),
        )
    }

    fn key_of(layout: &str) -> &'static str {
        LAYOUTS
            .iter()
            .find(|(module, _)| *module == layout)
            .map(|(_, key)| *key)
            .expect("a layout this module knows")
    }

    /// The layout switched on over three windows, and where it put them.
    fn arranged(scripts: &mut Scripts, layout: &str) -> HashMap<u64, Rect> {
        arranged_over(scripts, layout, 3)
    }

    /// The same over windows `1..=count`.
    fn arranged_over(scripts: &mut Scripts, layout: &str, count: u64) -> HashMap<u64, Rect> {
        let outcome = scripts.key(
            key_of(layout),
            snapshot((1..=count).map(|id| window(id, 800.0, 600.0)).collect()),
        );
        assert!(outcome.handled, "{layout}: the layout key was not handled");
        let out = placed(&outcome.commands);
        assert_eq!(
            out.len(),
            usize::try_from(count).unwrap_or_default(),
            "{layout}: not every window was placed: {out:?}"
        );
        out
    }

    /// A monitor arriving, as the compositor announces one: `monitors`,
    /// which is where a layout adopts, then `layout`, which is where it
    /// places -- `Solium::settle_monitors`, in that order. What comes back
    /// is where the second put everything.
    fn adopt(scripts: &mut Scripts, snapshot: Snapshot) -> HashMap<u64, Rect> {
        let _ = scripts.monitors_changed(snapshot.clone());
        placed(&scripts.relayout(snapshot).commands)
    }

    fn same(left: Rect, right: Rect) -> bool {
        about(left.x, right.x)
            && about(left.y, right.y)
            && about(left.w, right.w)
            && about(left.h, right.h)
    }

    /// **`closing` gives the survivors exactly what `close` would have, a
    /// whole client's lifetime sooner.**
    ///
    /// Compared against the same layout set to `"when_gone"` and told
    /// `close`, which is what every close did before #128 -- so the claim is
    /// not "something moved" but "the arrangement the user would have got
    /// is the one they get, at the moment they asked". And the window being
    /// closed is placed by neither: it is fading where it stood, and a slot
    /// for it would be a slot for nothing.
    #[test]
    fn closing_reflows_the_survivors_as_close_would_have() {
        for (layout, _) in LAYOUTS {
            let mut moved = false;
            for dying in [1, 2, 3] {
                let survivors: Vec<u64> = [1, 2, 3].into_iter().filter(|id| *id != dying).collect();

                let (mut immediate, _) = layout_with(layout, "");
                let before = arranged(&mut immediate, layout);
                let early = placed(
                    &immediate
                        .closing(dying, desk(&before, &[1, 2, 3], &[dying]))
                        .commands,
                );

                let (mut waiting, _) = layout_with(layout, &when_gone(layout));
                let was = arranged(&mut waiting, layout);
                let told = waiting.closing(dying, desk(&was, &[1, 2, 3], &[dying]));
                assert!(
                    placed(&told.commands).is_empty(),
                    "{layout}/{dying}: waiting for the client, `closing` placed something: {:?}",
                    told.commands
                );
                let late = placed(
                    &waiting
                        .closed(dying, desk(&was, &[1, 2, 3], &[dying]))
                        .commands,
                );

                assert!(
                    !early.contains_key(&dying),
                    "{layout}/{dying}: the window being closed was given a slot: {:?}",
                    early.get(&dying)
                );
                for id in &survivors {
                    let now = early.get(id).copied();
                    let then = late.get(id).copied();
                    assert!(
                        now.zip(then).is_some_and(|(now, then)| same(now, then)),
                        "{layout}/{dying}: window {id} got {now:?} when the close was asked \
                         for, and {then:?} when the client went -- the reflow is not the \
                         same one"
                    );
                }
                moved |= survivors.iter().any(|id| {
                    early
                        .get(id)
                        .zip(before.get(id))
                        .is_some_and(|(now, was)| !same(*now, *was))
                });
            }
            // Rules out a layout that placed everything where it was: equal
            // to `close`'s answer only means something if that answer moves
            // a window. Across the three rather than for each, because this
            // is only here to keep the comparison from being empty; which
            // closes move which windows is the layout's own business.
            assert!(
                moved,
                "{layout}: no close moved anything, so this compared nothing"
            );
        }
    }

    /// **A refused close in tiling comes back where it was.**
    ///
    /// `closing` keeps the centre of where the window stood, and `refused`
    /// splits whichever window covers it now. For a window whose sibling was
    /// a single window, that is the sibling, grown into the whole of their
    /// parent's box -- so the axis and the side both come back and the
    /// arrangement is the one it was, rect for rect. Windows 1 and 3 are
    /// that case in the three-window arrangement `adopt` builds (1 over 3,
    /// both beside 2).
    ///
    /// Window 2's sibling was the pair, so it comes back splitting one of
    /// them: accepted in the design, and asserted only as "back in the
    /// arrangement, overlapping nothing".
    #[test]
    fn a_refused_close_in_tiling_comes_back_where_it_was() {
        for dying in [1, 3] {
            let (mut scripts, _) = layout_with("tiling", "");
            let before = arranged(&mut scripts, "tiling");
            let closed = placed(
                &scripts
                    .closing(dying, desk(&before, &[1, 2, 3], &[dying]))
                    .commands,
            );
            let mut now = before.clone();
            now.extend(closed);
            let back = placed(&scripts.refused(dying, desk(&now, &[1, 2, 3], &[])).commands);
            for id in [1, 2, 3] {
                let was = before.get(&id).copied();
                let is = back.get(&id).copied();
                assert!(
                    was.zip(is).is_some_and(|(was, is)| same(was, is)),
                    "window {id} was at {was:?} before {dying} was closed, and is at {is:?} \
                     after the close was refused"
                );
            }
        }

        let (mut scripts, _) = layout_with("tiling", "");
        let before = arranged(&mut scripts, "tiling");
        let closed = placed(&scripts.closing(2, desk(&before, &[1, 2, 3], &[2])).commands);
        let mut now = before.clone();
        now.extend(closed);
        let back = placed(&scripts.refused(2, desk(&now, &[1, 2, 3], &[])).commands);
        let rects: Vec<Rect> = [1, 2, 3]
            .iter()
            .filter_map(|id| back.get(id).copied())
            .collect();
        assert_eq!(
            rects.len(),
            3,
            "a refused window is back in the arrangement: {back:?}"
        );
        for (index, rect) in rects.iter().enumerate() {
            for other in rects.iter().skip(index + 1) {
                assert!(!overlap(*rect, *other), "{rect:?} overlaps {other:?}");
            }
        }

        // And a refusal for a window the tree still holds -- one whose
        // `closing` never reached this layout -- inserts nothing a second
        // time, which is the insert the handler exists to guard.
        let told = scripts.refused(1, desk(&back, &[1, 2, 3], &[]));
        assert!(
            placed(&told.commands).is_empty(),
            "a window that is in the tree was put in it again: {:?}",
            told.commands
        );
    }

    /// **A refused close in scrolling comes back in its place in the
    /// strip.**
    ///
    /// Order and adjacency rather than exact rects, because the strip's
    /// *view* follows its focused column, and putting a window back focuses
    /// it: the columns are where they were relative to each other, and the
    /// whole strip may be scrolled. Every position is asserted, the first
    /// included, because the first has no window on its left to go after
    /// and is where a plain `insert` would put it on the far side of the
    /// focused column instead.
    #[test]
    fn a_refused_close_in_scrolling_comes_back_in_its_place() {
        for dying in [1, 2, 3] {
            let (mut scripts, _) = layout_with("scrolling", "");
            let before = arranged(&mut scripts, "scrolling");
            let mut order: Vec<u64> = vec![1, 2, 3];
            order.sort_by(|a, b| before[a].x.total_cmp(&before[b].x));

            let closed = placed(
                &scripts
                    .closing(dying, desk(&before, &[1, 2, 3], &[dying]))
                    .commands,
            );
            let mut now = before.clone();
            now.extend(closed);
            let back = placed(&scripts.refused(dying, desk(&now, &[1, 2, 3], &[])).commands);

            let mut returned: Vec<u64> = vec![1, 2, 3];
            assert!(
                returned.iter().all(|id| back.contains_key(id)),
                "{dying}: not every window was placed after the refusal: {back:?}"
            );
            returned.sort_by(|a, b| back[a].x.total_cmp(&back[b].x));
            assert_eq!(
                returned, order,
                "the strip read {order:?} before {dying} was closed and {returned:?} after the \
                 close was refused"
            );
            for pair in returned.windows(2) {
                let (left, right) = (back[&pair[0]], back[&pair[1]]);
                assert!(
                    about(left.x + left.w + 12.0, right.x),
                    "{dying}: {} ends at {} and {} starts at {}, so they are not neighbours",
                    pair[0],
                    left.x + left.w,
                    pair[1],
                    right.x
                );
            }
        }

        // The window after it closes too, before the refusal: the left
        // neighbour is the only one left to go after, and a strip that
        // looked for the right one would fall through to a plain `insert`
        // and put the window after the focused column instead.
        let (mut scripts, _) = layout_with("scrolling", "");
        let before = arranged_over(&mut scripts, "scrolling", 4);
        let mut order: Vec<u64> = vec![1, 2, 3, 4];
        order.sort_by(|a, b| before[a].x.total_cmp(&before[b].x));
        let (dying, next) = (order[1], order[2]);
        let mut now = before.clone();
        now.extend(placed(
            &scripts
                .closing(dying, desk(&now, &[1, 2, 3, 4], &[dying]))
                .commands,
        ));
        now.extend(placed(
            &scripts
                .closing(next, desk(&now, &[1, 2, 3, 4], &[dying, next]))
                .commands,
        ));
        let back = placed(
            &scripts
                .refused(dying, desk(&now, &[1, 2, 3, 4], &[next]))
                .commands,
        );
        let mut returned: Vec<u64> = order.iter().copied().filter(|id| *id != next).collect();
        returned.sort_by(|a, b| back[a].x.total_cmp(&back[b].x));
        let expected: Vec<u64> = order.iter().copied().filter(|id| *id != next).collect();
        assert_eq!(
            returned, expected,
            "with the window after it gone as well, {dying} did not come back after the one                  before it"
        );
    }

    /// **Set to `"when_gone"`, `closing` and `refused` do nothing, and
    /// `adopt` treats a window being closed as the ordinary window it
    /// always was.**
    ///
    /// The first half is in `closing_reflows_the_survivors_as_close_would_have`,
    /// which compares against this very setting's `close`. This is the rest:
    /// a refusal has nothing to undo, and a `monitors` event during the
    /// close still places the window being closed -- its tile stays reserved
    /// until `close`, which is what waiting means.
    #[test]
    fn when_gone_waits_for_the_client_as_every_close_used_to() {
        for (layout, _) in LAYOUTS {
            let (mut scripts, _) = layout_with(layout, &when_gone(layout));
            let before = arranged(&mut scripts, layout);
            let told = scripts.closing(3, desk(&before, &[1, 2, 3], &[3]));
            assert!(
                told.commands.is_empty(),
                "{layout}: `closing` asked for {:?}",
                told.commands
            );
            let adopted = adopt(&mut scripts, desk(&before, &[1, 2, 3], &[3]));
            assert!(
                adopted.contains_key(&3),
                "{layout}: waiting for the client, a monitors event dropped the window being \
                 closed out of the arrangement: {adopted:?}"
            );
            let told = scripts.refused(3, desk(&before, &[1, 2, 3], &[]));
            assert!(
                told.commands.is_empty(),
                "{layout}: `refused` asked for {:?}",
                told.commands
            );
        }
    }

    /// **A setting spelt some other way reads as the default.** A typo in a
    /// value is not something `config.lua`'s unknown-key report can see, so
    /// what it costs is decided here: the default, rather than the old
    /// behaviour.
    #[test]
    fn a_misspelt_reflow_setting_reads_as_immediate() {
        for (layout, _) in LAYOUTS {
            let (mut scripts, _) = layout_with(
                layout,
                &format!("require(\"config\").{layout}.reflow_on_close = \"when-gone\""),
            );
            let before = arranged(&mut scripts, layout);
            let early = placed(&scripts.closing(3, desk(&before, &[1, 2, 3], &[3])).commands);
            assert!(
                !early.is_empty() && !early.contains_key(&3),
                "{layout}: a misspelt setting did not reflow at once: {early:?}"
            );
        }
    }

    /// **`adopt` during a close does not put the window being closed back.**
    ///
    /// `monitors` and `restore` are the two events that run `adopt` on a
    /// live session -- a screen plugged in, `super+shift+r` -- and `adopt`
    /// exists to put back whatever the arrangement is missing. A window
    /// `closing` took out is missing on purpose.
    ///
    /// Three cases. After `closing`, it stays out. With no `closing` at all
    /// -- an event some route failed to deliver -- `adopt` takes it out
    /// itself, which is the backstop `adopt` has always been for a missed
    /// event. And after a reload, which carries the arrangement across
    /// without it, it is not put back in.
    #[test]
    fn adopt_during_a_close_leaves_the_window_out() {
        for (layout, _) in LAYOUTS {
            let (mut scripts, entry) = layout_with(layout, "");
            let before = arranged(&mut scripts, layout);
            let closed = placed(&scripts.closing(3, desk(&before, &[1, 2, 3], &[3])).commands);
            let adopted = adopt(&mut scripts, desk(&before, &[1, 2, 3], &[3]));
            assert!(
                !adopted.contains_key(&3),
                "{layout}: a monitors event during the close put the window being closed \
                 back: {adopted:?}"
            );
            for id in [1, 2] {
                assert!(
                    adopted
                        .get(&id)
                        .zip(closed.get(&id))
                        .is_some_and(|(now, then)| same(*now, *then)),
                    "{layout}: window {id} moved on a monitors event that changed nothing"
                );
            }

            // A reload: a fresh Lua state, the layout still current.
            let mut reloaded = scripts;
            reloaded
                .reload(&entry)
                .expect("reloading the shipped layout");
            let restored = placed(&reloaded.restored(desk(&before, &[1, 2, 3], &[3])).commands);
            assert!(
                restored.contains_key(&1) && !restored.contains_key(&3),
                "{layout}: a reload during the close put the window being closed back \
                 into the arrangement: {restored:?}"
            );

            // `closing` never arrived.
            let (mut missed, _) = layout_with(layout, "");
            let before = arranged(&mut missed, layout);
            let adopted = adopt(&mut missed, desk(&before, &[1, 2, 3], &[3]));
            assert!(
                !adopted.contains_key(&3) && adopted.contains_key(&1),
                "{layout}: with no `closing`, a monitors event kept a slot for the window \
                 being closed: {adopted:?}"
            );
        }
    }

    /// **`view:windows()` is the strip in reading order**: columns left to
    /// right, and top to bottom inside a column. What `scrolling.lua` finds
    /// a closing window's neighbours in.
    #[test]
    fn a_strip_lists_its_windows_in_reading_order() {
        let (scripts, _) = layout_with("scrolling", "");
        let order = scripts.evaluate(
            "local view = sol.layout.scroller()\n\
             local area = { x = 0, y = 0, w = 2560, h = 1440, gap = 12 }\n\
             for id = 1, 4 do view:insert(id, area) end\n\
             -- Focus the first column and pull the second's window into it.\n\
             view:focus_window(1, area)\n\
             view:consume()\n\
             -- Then carry the last column to the front.\n\
             view:focus_window(4, area)\n\
             view:move_column(-1, area)\n\
             view:move_column(-1, area)\n\
             return table.concat(view:windows(), \",\")",
        );
        assert_eq!(
            order, "4,1,2,3",
            "the strip is [4] [1 over 2] [3], and read as {order}"
        );
    }

    /// Everything a layout holds, as text: every tree's or strip's
    /// arrangement of one screen, in key order, and for a strip which
    /// column it has focused. What a test compares to say a layout was
    /// left exactly as it was, since a layout that is not in charge places
    /// nothing and its commands say nothing.
    fn structure(scripts: &Scripts, layout: &str) -> String {
        let (table, extra) = if layout == "tiling" {
            ("trees", "")
        } else {
            ("views", " .. \" focus=\" .. tostring(each:focused())")
        };
        scripts.evaluate(&format!(
            "local layout = require({layout:?})\n\
             local keys = {{}}\n\
             for key in pairs(layout.{table}) do keys[#keys + 1] = key end\n\
             table.sort(keys)\n\
             local area = {{ x = 0, y = 0, w = 2560, h = 1440, gap = 12, split = 0.5 }}\n\
             local out = {{}}\n\
             for _, key in ipairs(keys) do\n\
                 local each = layout.{table}[key]\n\
                 local slots = {{}}\n\
                 for _, slot in ipairs(each:layout(area)) do\n\
                     slots[#slots + 1] = string.format(\"%d@%.0f,%.0f,%.0fx%.0f\",\n\
                         slot.id, slot.x, slot.y, slot.w, slot.h)\n\
                 end\n\
                 out[#out + 1] = key .. \"=\" .. table.concat(slots, \" \"){extra}\n\
             end\n\
             return table.concat(out, \" | \")"
        ))
    }

    /// Three windows opened while `layout` is not in charge, which is
    /// every session's start: floating is the default mode. Each layout
    /// hears `open` whether it is in charge or not and keeps the window in
    /// its structure, so there is something there to be disturbed.
    fn opened_while_floating(scripts: &mut Scripts) -> Vec<WindowInfo> {
        let windows: Vec<WindowInfo> = (1..=3_u32)
            .map(|n| {
                let mut each = window(u64::from(n), 700.0, 500.0);
                each.rect.x = f64::from(n) * 300.0;
                each.rect.y = f64::from(n) * 150.0;
                each
            })
            .collect();
        for count in 1..=windows.len() {
            let id = windows[count - 1].id;
            let _ = scripts.opened(id, snapshot(windows[..count].to_vec()));
        }
        windows
    }

    fn leaving(windows: &[WindowInfo], ids: &[u64]) -> Vec<WindowInfo> {
        windows
            .iter()
            .cloned()
            .map(|window| WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                leaving: ids.contains(&window.id),
                ..window
            })
            .collect()
    }

    /// **A layout that is not in charge is left exactly as it was by a
    /// close and its refusal.**
    ///
    /// Floating is the default mode, and both layouts listen all the same.
    /// `closing` used to take the window out of a tree or strip nobody was
    /// using and record where it stood in some other mode's geometry, and
    /// `refused` put it back from that: at the configured split rather
    /// than its old one, or with the inactive strip's focus moved onto it
    /// -- which `scrolling.started` then hands the keyboard to when the
    /// user switches to it. Whichever layout the user had set to
    /// `"when_gone"`, the other still did it.
    #[test]
    fn a_layout_not_in_charge_is_left_as_it_was_by_a_refused_close() {
        for (layout, _) in LAYOUTS {
            let (mut scripts, _) = layout_with(layout, "");
            let windows = opened_while_floating(&mut scripts);
            let before = structure(&scripts, layout);
            assert!(
                before.contains("2@"),
                "{layout}: the premise is a structure holding the window: {before}"
            );

            let told = scripts.closing(2, snapshot(leaving(&windows, &[2])));
            assert!(
                placed(&told.commands).is_empty(),
                "{layout}: a layout not in charge placed something: {:?}",
                told.commands
            );
            assert_eq!(
                structure(&scripts, layout),
                before,
                "{layout}: `closing` rearranged a layout that is not in charge"
            );
            let _ = scripts.refused(2, snapshot(windows.clone()));
            assert_eq!(
                structure(&scripts, layout),
                before,
                "{layout}: `refused` rearranged a layout that is not in charge"
            );

            // And a window it never held -- one open before the scripts
            // were -- is not put into it by a refusal either.
            let mut more = windows.clone();
            more.push(window(4, 700.0, 500.0));
            let _ = scripts.closing(4, snapshot(leaving(&more, &[4])));
            let _ = scripts.refused(4, snapshot(more));
            assert_eq!(
                structure(&scripts, layout),
                before,
                "{layout}: `refused` put a window into a layout that is not in charge"
            );
        }
    }

    /// **Switched on during a close, a layout still puts a refused window
    /// back.** Not in charge at `closing`, it took nothing out -- but
    /// `adopt`, when the user switches to it, leaves a window being closed
    /// out of the arrangement, so `refused` has to put back a window it did
    /// not itself take out. The rule is "missing from a layout that is in
    /// charge", not only "taken out by `closing`".
    #[test]
    fn a_layout_switched_on_during_a_close_puts_the_refused_window_back() {
        for (layout, key) in LAYOUTS {
            let (mut scripts, _) = layout_with(layout, "");
            let windows = opened_while_floating(&mut scripts);
            let _ = scripts.closing(3, snapshot(leaving(&windows, &[3])));
            let on = placed(&scripts.key(key, snapshot(leaving(&windows, &[3]))).commands);
            assert!(
                on.contains_key(&1) && !on.contains_key(&3),
                "{layout}: switched on during the close, the layout placed the window being \
                 closed, or nothing: {on:?}"
            );
            let back = placed(&scripts.refused(3, snapshot(windows.clone())).commands);
            assert!(
                back.contains_key(&3),
                "{layout}: the refused window was left out of the arrangement: {back:?}"
            );
        }
    }

    /// **A refused window goes back whatever the setting says now.**
    ///
    /// The setting is read at each event, so a script can change it while
    /// the session runs -- including between a `closing` that took the
    /// window out and the `refused` that has to put it back. `refused`
    /// decided from the setting as it was then, and returned: a live
    /// window, out of every tree and strip, drawn over the neighbour that
    /// had grown into its space until some later event rebuilt the layout.
    #[test]
    fn a_refused_window_goes_back_whatever_the_setting_says_now() {
        for (layout, _) in LAYOUTS {
            let (mut scripts, _) = layout_with(layout, "");
            let before = arranged(&mut scripts, layout);
            let closed = placed(&scripts.closing(3, desk(&before, &[1, 2, 3], &[3])).commands);
            assert!(
                !closed.is_empty() && !closed.contains_key(&3),
                "{layout}: the premise is a layout that closed up at once: {closed:?}"
            );
            let _ = scripts.evaluate(&format!("{}\nreturn \"\"", when_gone(layout)));
            let mut now = before.clone();
            now.extend(closed);
            let back = placed(&scripts.refused(3, desk(&now, &[1, 2, 3], &[])).commands);
            assert!(
                back.contains_key(&3),
                "{layout}: the setting changed during the close and the refused window was \
                 left out of the arrangement: {back:?}"
            );
        }
    }

    /// **A refused window goes back onto the screen it is on now.**
    ///
    /// A monitor can go during the grace period. Its windows are on one
    /// that is still there, `monitors` is announced, and `adopt` re-homes
    /// them -- except the one being closed, which it leaves out on purpose.
    /// `refused` then put that one back into the tree or strip it had been
    /// in, which is keyed by a monitor that no longer exists and which
    /// `apply` never walks: a window on screen that nothing lays out.
    #[test]
    fn a_refused_window_goes_back_onto_the_screen_it_is_on_now() {
        for (layout, _) in LAYOUTS {
            let (mut scripts, _) = layout_with(layout, "");
            let before = arranged(&mut scripts, layout);
            let _ = scripts.closing(3, desk(&before, &[1, 2, 3], &[3]));

            let moved = |leaving: bool| {
                snapshot_on(
                    vec![second_monitor()],
                    (1..=3)
                        .map(|id| WindowInfo {
                            icon: None,
                            client_side: false,
                            blur_region: false,
                            insets: crate::decoration::Insets::default(),
                            leaving: leaving && id == 3,
                            ..on(&second_monitor(), window(id, 800.0, 600.0))
                        })
                        .collect(),
                )
            };
            let adopted = adopt(&mut scripts, moved(true));
            assert!(
                adopted.contains_key(&1) && !adopted.contains_key(&3),
                "{layout}: the premise is the other windows re-homed onto the screen that is \
                 left, without the window being closed: {adopted:?}"
            );

            let back = placed(&scripts.refused(3, moved(false)).commands);
            assert!(
                back.get(&3).is_some_and(|rect| rect.x >= BESIDE.x),
                "{layout}: the refused window was not put back onto the screen it is on: \
                 {back:?}"
            );
        }
    }
}

/// **#134: a configurable minimum tile, and a new window with no room
/// overflowing -- by default to the next empty workspace.**
///
/// Against the shipped `tiling.lua` and `workspaces.lua`, as the modules
/// above run them. The chain is `tiling.lua`'s and the tree steps it calls
/// are `solium_layout::tree::minimum_tests`'; this is the chain.
///
/// Most tests set a minimum of 1300x800 on a 2560x1440 screen at the
/// shipped 12px gap. The first window takes the whole 2536x1416 tile, and
/// no split of that has room -- side by side is 1262 wide, one above the
/// other is 702 high -- so a workspace holds one window unless something
/// allows a second, and where each window goes is never in doubt.
mod overflow {
    use super::super::Outcome;
    use super::*;

    /// [`AREA`] less the shipped gap all round: the one tile a workspace
    /// with one window has.
    const FULL: Rect = Rect {
        x: 12.0,
        y: 12.0,
        w: 2536.0,
        h: 1416.0,
    };

    /// The shipped layouts in the order `init.lua` requires them, `before`
    /// run ahead of them, and the entry point for a reload. A directory
    /// per call, for the reason [`scripts`] gives.
    ///
    /// `scrolling.lua` too, although no test here switches it on: every
    /// layout hears `open` whether or not it is in charge, and until #134's
    /// review this harness left it out -- which is how a strip that was
    /// not in charge handing the keyboard to a window parked on another
    /// workspace went unseen. See
    /// `with_follow_overflow_off_the_window_opens_there_and_the_view_stays`.
    fn tiling_with(before: &str) -> (Scripts, std::path::PathBuf) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("solium-overflow-{}-{serial}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\n\
                 {before}\n\
                 require(\"modes\")\n\
                 require(\"workspaces\")\n\
                 require(\"tiling\")\n\
                 require(\"scrolling\")\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        )
        .expect("writing the entry point");
        let scripts = Scripts::load(&entry).expect("loading the shipped layout");
        (scripts, entry)
    }

    /// Everything `sol.log` is handed, kept in `_G.logged` as well as
    /// logged. `tiling.lua` looks `sol.log` up when it speaks, so a
    /// replacement installed before it loads is the one it calls.
    const CAPTURE: &str = "_G.logged = {}\n\
         local log = sol.log\n\
         sol.log = function(message)\n\
             _G.logged[#_G.logged + 1] = message\n\
             log(message)\n\
         end";

    fn logged(scripts: &Scripts) -> String {
        scripts.evaluate("return table.concat(_G.logged or {}, \"\\n\")")
    }

    fn minimum(w: u32, h: u32) -> String {
        format!("require(\"config\").tiling.minimum = {{ w = {w}, h = {h} }}")
    }

    /// The windows `ids`, where a toplevel maps, with the pointer at
    /// `cursor`. Nothing here is drawn anywhere, so `sol.window_at`
    /// finds nothing and the tree's own hit test is what picks the tile
    /// under the pointer -- the same tile, from the same rectangles, that
    /// the layout placed.
    fn desk_at(ids: &[u64], cursor: (f64, f64)) -> Snapshot {
        Snapshot {
            cursor,
            ..snapshot(ids.iter().map(|&id| window(id, 800.0, 600.0)).collect())
        }
    }

    /// Windows `ids`, each drawn and placed where `placed` says, the ones
    /// in `leaving` being closed. What a layout reading the snapshot after
    /// its own pass would see.
    fn desk(placed: &HashMap<u64, Rect>, ids: &[u64], leaving: &[u64]) -> Snapshot {
        snapshot(
            ids.iter()
                .map(|&id| {
                    let rect = placed.get(&id).copied().unwrap_or_default();
                    WindowInfo {
                        icon: None,
                        client_side: false,
                        blur_region: false,
                        insets: crate::decoration::Insets::default(),
                        rect,
                        drawn: drawn(rect),
                        leaving: leaving.contains(&id),
                        ..window(id, rect.w, rect.h)
                    }
                })
                .collect(),
        )
    }

    type HashMap<K, V> = std::collections::HashMap<K, V>;

    /// Tiling switched on over an empty desk, so every window after this
    /// arrives through `open`, as a window does in a session.
    fn switched_on(scripts: &mut Scripts) {
        let outcome = scripts.key("super+t", desk_at(&[], (0.0, 0.0)));
        assert!(outcome.handled, "the tiling key was not handled");
    }

    /// The workspace a window belongs to, as `workspaces.at` answers.
    fn workspace_of(scripts: &Scripts, id: u64) -> String {
        scripts.evaluate(&format!(
            "return tostring(require(\"workspaces\").at({id}, \"DP-1\"))"
        ))
    }

    /// The workspace `DP-1` is showing.
    fn showing(scripts: &Scripts) -> String {
        scripts.evaluate("return tostring(require(\"workspaces\").on(\"DP-1\"))")
    }

    fn last_focus(commands: &[Command]) -> Option<u64> {
        commands.iter().rev().find_map(|command| match command {
            Command::Focus { id, .. } => Some(*id),
            _ => None,
        })
    }

    fn same(left: Rect, right: Rect) -> bool {
        about(left.x, right.x)
            && about(left.y, right.y)
            && about(left.w, right.w)
            && about(left.h, right.h)
    }

    /// Every rect `commands` placed window `id` at, in order.
    fn places_of(commands: &[Command], id: u64) -> Vec<Rect> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Place {
                    id: placed, rect, ..
                } if *placed == id => Some(*rect),
                _ => None,
            })
            .collect()
    }

    /// **A window with no room opens on the next empty workspace, and the
    /// view goes with it, keyboard included.**
    ///
    /// Window 2 has no room beside window 1 (see the module note), and the
    /// default list is largest, workspace, allow: no tile has room, so it
    /// is workspace 2. The view switches there as `super+2` would -- the
    /// status line says so -- the keyboard is handed to the new window,
    /// and the window is placed once, at its final tile. Window 1 stays
    /// where it was, on workspace 1.
    #[test]
    fn a_window_with_no_room_opens_on_the_next_empty_workspace_and_the_view_follows() {
        let (mut scripts, _) = tiling_with(&minimum(1300, 800));
        switched_on(&mut scripts);
        let first = open(&mut scripts, 1, &[1]);
        assert!(
            places_of(&first.commands, 1)
                .last()
                .is_some_and(|rect| same(*rect, FULL)),
            "the premise: window 1 takes the whole screen: {:?}",
            first.commands
        );

        let told = open(&mut scripts, 2, &[1, 2]);
        assert_eq!(workspace_of(&scripts, 2), "2", "window 2 was not sent on");
        assert_eq!(workspace_of(&scripts, 1), "1", "window 1 moved workspace");
        assert_eq!(showing(&scripts), "2", "the view did not follow");
        assert_eq!(told.status.as_deref(), Some("workspace 2"));
        assert_eq!(
            last_focus(&told.commands),
            Some(2),
            "the keyboard was left on a workspace nobody is looking at"
        );
        let placed = places_of(&told.commands, 2);
        assert!(
            !placed.is_empty() && placed.iter().all(|rect| same(*rect, FULL)),
            "window 2 was placed somewhere on its way to its tile: {placed:?}"
        );
        assert!(
            places_of(&told.commands, 1).is_empty(),
            "window 1 was placed again, from a workspace that is not in view"
        );
    }

    /// **`follow_overflow = false`: the window opens over there, and the
    /// view stays.** It is still placed in its tile, and grouped onto its
    /// desk in the same dispatch, so it is never drawn over this one.
    ///
    /// And no script hands it the keyboard. `scrolling.lua`, which was not
    /// in charge, did: its `open` handler inserted the window into a strip
    /// and focused the strip's new column, so the keyboard followed the
    /// window to a desk nobody could see (#134 review). This checks the
    /// scripts; that the compositor does not give it the keyboard either is
    /// `a_window_that_overflows_to_a_hidden_workspace_does_not_take_the_keyboard`
    /// in `state.rs`, which types and sees where the key lands.
    #[test]
    fn with_follow_overflow_off_the_window_opens_there_and_the_view_stays() {
        let (mut scripts, _) = tiling_with(&format!(
            "{}\nrequire(\"config\").tiling.follow_overflow = false",
            minimum(1300, 800)
        ));
        switched_on(&mut scripts);
        let _ = open(&mut scripts, 1, &[1]);
        let told = open(&mut scripts, 2, &[1, 2]);

        assert_eq!(workspace_of(&scripts, 2), "2", "window 2 was not sent on");
        assert_eq!(showing(&scripts), "1", "the view went with it");
        assert_eq!(
            last_focus(&told.commands),
            None,
            "the keyboard was moved: {:?}",
            told.commands
        );
        assert!(
            places_of(&told.commands, 2)
                .last()
                .is_some_and(|rect| same(*rect, FULL)),
            "window 2 was not placed in its tile over there: {:?}",
            told.commands
        );
        let grouped = told.commands.iter().any(|command| {
            matches!(
                command,
                Command::Group { name, selection: Some(selection), .. }
                    if name == "desk-2@DP-1" && selection.windows.contains(&2)
            )
        });
        assert!(
            grouped,
            "window 2 was not put on its own desk in the same dispatch: {:?}",
            told.commands
        );
    }

    /// **With every workspace taken, `"workspace"` does nothing and
    /// `"allow"` places the window here, under the minimum.** The
    /// workspaces are a fixed four, so there is none to make. `"allow"`
    /// is a step the list names, so this is not a fall-through and is not
    /// logged as one.
    #[test]
    fn with_every_workspace_taken_the_window_is_allowed_under_the_minimum() {
        let (mut scripts, _) = tiling_with(&format!("{CAPTURE}\n{}", minimum(1300, 800)));
        switched_on(&mut scripts);
        let _ = scripts.evaluate(
            "local workspaces = require(\"workspaces\")\n\
             workspaces.of[12] = 2\n\
             workspaces.of[13] = 3\n\
             workspaces.of[14] = 4\n\
             return \"\"",
        );
        assert_eq!(
            scripts.evaluate("return tostring(require(\"workspaces\").count())"),
            "4"
        );
        let _ = open(&mut scripts, 1, &[1, 12, 13, 14]);
        let told = open(&mut scripts, 2, &[1, 2, 12, 13, 14]);

        assert_eq!(workspace_of(&scripts, 2), "1");
        assert_eq!(showing(&scripts), "1");
        let placed = placed(&told.commands);
        let (one, two) = (placed.get(&1).copied(), placed.get(&2).copied());
        assert!(
            one.zip(two)
                .is_some_and(|(one, two)| !overlap(one, two) && one.w < 1300.0 && two.w < 1300.0),
            "the two were not split under the minimum: {one:?} {two:?}"
        );
        assert!(
            !logged(&scripts).contains("below tiling.minimum"),
            "an \"allow\" the list names was reported as a fall-through: {}",
            logged(&scripts)
        );
    }

    /// **A list with no `"allow"` still places the window, and says so.**
    /// `{ "largest" }` finds no tile with room, and the list is over; the
    /// window splits the tile under the pointer all the same, on this
    /// workspace, and the log says it went under the minimum.
    #[test]
    fn a_list_without_allow_still_places_the_window_and_says_so() {
        let (mut scripts, _) = tiling_with(&format!(
            "{CAPTURE}\n{}\nrequire(\"config\").tiling.overflow = {{ \"largest\" }}",
            minimum(1300, 800)
        ));
        switched_on(&mut scripts);
        let _ = open(&mut scripts, 1, &[1]);
        let told = open(&mut scripts, 2, &[1, 2]);

        assert_eq!(
            workspace_of(&scripts, 2),
            "1",
            "a list with no workspace step moved it"
        );
        let placed = placed(&told.commands);
        assert!(
            placed.get(&2).is_some_and(|rect| rect.w < 1300.0) && placed.contains_key(&1),
            "window 2 was not placed beside window 1: {placed:?}"
        );
        let said = logged(&scripts);
        assert!(
            said.contains("window 2") && said.contains("below tiling.minimum"),
            "the fall-through was silent: {said:?}"
        );
    }

    /// Four windows at 600x750, each opened with the pointer at the left
    /// edge. 2 goes left of 1, 1262 wide each. 3 has no room one above the
    /// other in 2 -- 702 high -- and so turns, and goes beside it at 625.
    /// 4 has room in 3 neither way, while 1, the largest tile, still has
    /// room across.
    fn four_at_the_left_edge(before: &str) -> (Scripts, Outcome) {
        let (mut scripts, _) = tiling_with(&format!("{}\n{before}", minimum(600, 750)));
        switched_on(&mut scripts);
        let at = (100.0, 700.0);
        let _ = scripts.opened(1, desk_at(&[1], at));
        let _ = scripts.opened(2, desk_at(&[1, 2], at));
        let third = scripts.opened(3, desk_at(&[1, 2, 3], at));
        let placed = placed(&third.commands);
        assert!(
            placed
                .get(&3)
                .is_some_and(|rect| about(rect.w, 625.0) && about(rect.h, 1416.0))
                && placed.get(&2).is_some_and(|rect| about(rect.w, 625.0)),
            "the premise: window 3 turned and went beside window 2: {placed:?}"
        );
        let fourth = scripts.opened(4, desk_at(&[1, 2, 3, 4], at));
        (scripts, fourth)
    }

    /// **The steps are tried in the order written.** Window 4 has no room
    /// under the pointer and window 1 has room across (see
    /// [`four_at_the_left_edge`]): the shipped order puts it there, and
    /// `workspace` ahead of `largest` sends it to workspace 2 instead.
    #[test]
    fn the_steps_are_tried_in_the_order_written() {
        let (shipped, told) = four_at_the_left_edge("");
        assert_eq!(
            workspace_of(&shipped, 4),
            "1",
            "largest comes first by default"
        );
        let one = placed(&told.commands).get(&1).copied();
        let four = placed(&told.commands).get(&4).copied();
        assert!(
            one.zip(four).is_some_and(|(one, four)| about(one.w, 625.0)
                && about(four.w, 625.0)
                && four.x > one.x
                && one.x > 1200.0),
            "window 4 did not split window 1, the largest tile with room: {one:?} {four:?}"
        );

        let (reordered, told) = four_at_the_left_edge(
            "require(\"config\").tiling.overflow = { \"workspace\", \"largest\", \"allow\" }",
        );
        assert_eq!(
            workspace_of(&reordered, 4),
            "2",
            "workspace was written first"
        );
        assert!(
            placed(&told.commands)
                .get(&4)
                .is_some_and(|rect| same(*rect, FULL)),
            "window 4 is alone on workspace 2 and has the whole of it: {:?}",
            told.commands
        );
    }

    /// **A window being closed is room for the next one** (#128). The
    /// layout takes it out of its tree at `closing`, and its neighbour
    /// grows into the space -- so the next window has room there and stays
    /// on this workspace, where a tile still held for the closing window
    /// would have sent it to the next.
    ///
    /// And a workspace whose only window is being closed is empty: the
    /// next window with no room goes there, and it is that window the
    /// keyboard goes to, not the one fading out ahead of it in the list.
    #[test]
    fn a_window_being_closed_is_room_for_the_next_one() {
        let (mut scripts, _) = tiling_with(&minimum(1200, 800));
        switched_on(&mut scripts);
        let _ = open(&mut scripts, 1, &[1]);
        let second = open(&mut scripts, 2, &[1, 2]);
        let before = placed(&second.commands);
        assert!(
            before
                .get(&1)
                .zip(before.get(&2))
                .is_some_and(|(one, two)| about(one.w, 1262.0) && about(two.w, 1262.0)),
            "the premise: 1 and 2 side by side, with no room left: {before:?}"
        );
        let _ = scripts.closing(2, desk(&before, &[1, 2], &[2]));
        let _ = scripts.opened(
            3,
            Snapshot {
                cursor: (500.0, 500.0),
                ..desk(&before, &[1, 2, 3], &[2])
            },
        );
        assert_eq!(
            workspace_of(&scripts, 3),
            "1",
            "the closing window's tile was counted as taken"
        );

        let (mut scripts, _) = tiling_with(&minimum(1300, 800));
        switched_on(&mut scripts);
        let _ = scripts.evaluate("require(\"workspaces\").of[5] = 2\nreturn \"\"");
        let _ = open(&mut scripts, 1, &[5, 1]);
        let mut windows = vec![
            window(5, 800.0, 600.0),
            window(1, 800.0, 600.0),
            window(3, 800.0, 600.0),
        ];
        windows[0].leaving = true;
        let told = scripts.opened(
            3,
            Snapshot {
                cursor: (500.0, 500.0),
                ..snapshot(windows)
            },
        );
        assert_eq!(
            workspace_of(&scripts, 3),
            "2",
            "a workspace holding only a closing window was counted as taken"
        );
        assert_eq!(last_focus(&told.commands), Some(3));
    }

    /// **A layout not in charge sends nothing to another workspace.**
    /// Tiling hears `open` while floating is the mode and keeps the window
    /// in its tree for later. That tree is full, and the window goes into
    /// it anyway -- under the minimum -- rather than off to workspace 2
    /// with the view behind it, on a desktop tiling is not arranging.
    #[test]
    fn a_layout_not_in_charge_sends_nothing_to_another_workspace() {
        let (mut scripts, _) = tiling_with(&minimum(1300, 800));
        let _ = open(&mut scripts, 1, &[1]);
        let told = open(&mut scripts, 2, &[1, 2]);
        assert_eq!(workspace_of(&scripts, 2), "1");
        assert_eq!(showing(&scripts), "1");
        assert_eq!(last_focus(&told.commands), None);
        let held = scripts.evaluate(
            "local trees = require(\"tiling\").trees\n\
             local out = {}\n\
             for key, tree in pairs(trees) do\n\
                 local ids = tree:windows()\n\
                 table.sort(ids)\n\
                 if #ids > 0 then out[#out + 1] = key .. \"=\" .. table.concat(ids, \",\") end\n\
             end\n\
             table.sort(out)\n\
             return table.concat(out, \" \")",
        );
        assert_eq!(held, "1@DP-1=1,2", "the trees tiling keeps for later");
    }

    /// **Nothing already open is sent to another workspace**, by any route
    /// that puts one back into a tree -- only a window being opened
    /// overflows.
    ///
    /// Four windows at a minimum where a workspace holds one, so every
    /// route below has three windows with no room and three empty
    /// workspaces to send them to. Each must keep every window on
    /// workspace 1 of its screen and place it there:
    ///
    ///   * `adopt`, by switching tiling on;
    ///   * `adopt`, on a monitor change -- every window now on `DP-2`,
    ///     missing from that screen's tree;
    ///   * `restore`, a reload, with every window back on `DP-1` -- whose
    ///     tree the monitor change emptied, so it is built again from
    ///     nothing;
    ///   * `refused`, a close the application declined;
    ///   * a dialog that stops being modal and rejoins its tree.
    #[test]
    fn nothing_already_open_is_sent_to_another_workspace() {
        let (mut scripts, entry) = tiling_with(&minimum(1300, 800));
        let ids = [1, 2, 3, 4];
        let everyone_home = |scripts: &Scripts, monitor: &str, route: &str| {
            let away = scripts.evaluate(&format!(
                "local workspaces = require(\"workspaces\")\n\
                 local out = {{}}\n\
                 for id = 1, 5 do\n\
                     local at = workspaces.at(id, {monitor:?})\n\
                     if at ~= 1 then out[#out + 1] = id .. \"@\" .. tostring(at) end\n\
                 end\n\
                 out[#out + 1] = \"showing=\" .. tostring(workspaces.on({monitor:?}))\n\
                 return table.concat(out, \" \")"
            ));
            assert_eq!(
                away, "showing=1",
                "{route} sent a window to another workspace"
            );
        };

        let switched = scripts.key("super+t", desk_at(&ids, (500.0, 500.0)));
        let placed_on = placed(&switched.commands);
        assert!(
            ids.iter().all(|id| placed_on.contains_key(id)),
            "switching tiling on left a window out: {placed_on:?}"
        );
        everyone_home(&scripts, "DP-1", "switching tiling on");

        let moved = snapshot_on(
            vec![second_monitor()],
            ids.iter()
                .map(|&id| on(&second_monitor(), window(id, 800.0, 600.0)))
                .collect(),
        );
        let _ = scripts.monitors_changed(moved.clone());
        let after = placed(&scripts.relayout(moved).commands);
        assert!(
            ids.iter()
                .all(|id| after.get(id).is_some_and(|rect| rect.x >= BESIDE.x)),
            "a monitor change left a window out of the screen it is on: {after:?}"
        );
        everyone_home(&scripts, "DP-2", "a monitor change");

        scripts.reload(&entry).expect("reloading");
        let restored = placed(&scripts.restored(desk_at(&ids, (500.0, 500.0))).commands);
        assert!(
            ids.iter().all(|id| restored.contains_key(id)),
            "a reload left a window out: {restored:?}"
        );
        everyone_home(&scripts, "DP-1", "a reload");

        let _ = scripts.closing(4, desk(&restored, &ids, &[4]));
        let back = placed(&scripts.refused(4, desk(&restored, &ids, &[])).commands);
        assert!(
            back.contains_key(&4),
            "the refused window was not put back: {back:?}"
        );
        everyone_home(&scripts, "DP-1", "a refused close");

        let mut with_dialog: Vec<WindowInfo> =
            ids.iter().map(|&id| window(id, 800.0, 600.0)).collect();
        with_dialog.push(modal(5, 600.0, 400.0, Parentage::Window(1)));
        let _ = scripts.relayout(snapshot(with_dialog.clone()));
        if let Some(dialog) = with_dialog.last_mut() {
            dialog.modal = false;
            dialog.parent = Parentage::None;
        }
        let rejoined = placed(&scripts.relayout(snapshot(with_dialog)).commands);
        assert!(
            rejoined.get(&5).is_some_and(|rect| rect.w < 1300.0),
            "the dialog that stopped being modal did not rejoin the arrangement: {rejoined:?}"
        );
        everyone_home(&scripts, "DP-1", "a dialog rejoining");
    }

    /// 1 over 3 on the left, 2 on the right, at a 700x800 minimum; then
    /// the seam between them dragged left to 1136, through the `resize`
    /// event, so the left column is 1112 wide and window 2 is 1412.
    ///
    /// `adopt` builds it: 2 has room beside 1, and 3 has none anywhere --
    /// window 1 splits to 702 high or 625 wide -- so it is allowed in over
    /// window 1. After the drag the left column has no room either way,
    /// and window 2 has room across: two of 700.
    fn lopsided() -> (Scripts, HashMap<u64, Rect>) {
        let (mut scripts, _) = tiling_with(&minimum(700, 800));
        let switched = scripts.key("super+t", desk_at(&[1, 2, 3], (500.0, 500.0)));
        let first = placed(&switched.commands);
        let dragged = scripts.resized(
            2,
            (1136.0, 700.0),
            (Some("left"), None),
            desk(&first, &[1, 2, 3], &[]),
        );
        let mut now = first;
        now.extend(placed(&dragged.commands));
        assert!(
            now.get(&1)
                .is_some_and(|rect| about(rect.w, 1112.0) && about(rect.h, 702.0))
                && now.get(&3).is_some_and(|rect| about(rect.w, 1112.0))
                && now.get(&2).is_some_and(|rect| about(rect.w, 1412.0)),
            "the fixture is not the one described: {now:?}"
        );
        (scripts, now)
    }

    /// **A refused window under the minimum comes back where it was**,
    /// not into the largest tile. Window 3's tile was under the minimum
    /// before it was closed, so the tile it goes back to has no room
    /// either -- and window 2's does. Taking that would move a window the
    /// user never moved; the arrangement comes back rect for rect.
    #[test]
    fn a_refused_window_under_the_minimum_comes_back_where_it_was() {
        let (mut scripts, before) = lopsided();
        let closed = placed(&scripts.closing(3, desk(&before, &[1, 2, 3], &[3])).commands);
        let mut now = before.clone();
        now.extend(closed);
        let back = placed(&scripts.refused(3, desk(&now, &[1, 2, 3], &[])).commands);
        for id in [1, 2, 3] {
            let was = before.get(&id).copied();
            let is = back.get(&id).copied();
            assert!(
                was.zip(is).is_some_and(|(was, is)| same(was, is)),
                "window {id} was at {was:?} before 3 was closed and is at {is:?} after the \
                 close was refused"
            );
        }
        assert_eq!(workspace_of(&scripts, 3), "1");
    }

    /// **A dropped window goes to the tile it was let go over**, under the
    /// minimum if it must, and not to the largest tile. Window 3 is dropped
    /// on window 1, whose tile has no room; window 2's has.
    #[test]
    fn a_dropped_window_goes_to_the_tile_it_was_let_go_over() {
        let (mut scripts, before) = lopsided();
        let one = before.get(&1).copied().unwrap_or_default();
        let (x, y) = (one.x + one.w / 2.0, one.y + one.h * 0.75);
        let dropped = placed(
            &scripts
                .dropped(3, x, y, desk(&before, &[1, 2, 3], &[]))
                .commands,
        );
        let (one, two, three) = (
            dropped.get(&1).copied(),
            dropped.get(&2).copied(),
            dropped.get(&3).copied(),
        );
        assert!(
            two.zip(before.get(&2).copied())
                .is_some_and(|(now, was)| same(now, was)),
            "window 2's tile was split for a window dropped somewhere else: {two:?}"
        );
        assert!(
            one.zip(three)
                .is_some_and(|(one, three)| about(one.x + one.w, 1124.0)
                    && about(three.x + three.w, 1124.0)
                    && !overlap(one, three)),
            "window 3 did not go back into the left column it was dropped on: {one:?} \
             {three:?}"
        );
        assert_eq!(workspace_of(&scripts, 3), "1");
    }

    /// **The seams stop at the minimum, by the drag and by the keyboard.**
    /// The shipped 160: window 1's right edge dragged to x=50 stops where
    /// window 1 is 160 wide, where 0.05..0.95 alone stopped it at 126; and
    /// `super+minus` pressed until nothing changes stops at 160 too.
    #[test]
    fn the_shipped_seams_stop_at_the_minimum() {
        let (mut scripts, _) = tiling_with("");
        let switched = scripts.key("super+t", desk_at(&[1, 2], (500.0, 500.0)));
        let first = placed(&switched.commands);
        let dragged = placed(
            &scripts
                .resized(
                    1,
                    (50.0, 700.0),
                    (Some("right"), None),
                    desk(&first, &[1, 2], &[]),
                )
                .commands,
        );
        assert!(
            dragged.get(&1).is_some_and(|rect| about(rect.w, 160.0)),
            "the drag went under the minimum: {dragged:?}"
        );

        let (mut scripts, _) = tiling_with("");
        let switched = scripts.key("super+t", desk_at(&[1, 2], (500.0, 500.0)));
        let mut now = placed(&switched.commands);
        for _ in 0..12 {
            let mut focused = desk(&now, &[1, 2], &[]);
            for window in &mut focused.windows {
                window.focused = window.id == 1;
            }
            now.extend(placed(&scripts.key("super+minus", focused).commands));
        }
        assert!(
            now.get(&1).is_some_and(|rect| about(rect.w, 160.0)),
            "the keyboard went under the minimum: {now:?}"
        );
    }

    /// **A minimum that is not a size is named once, and ignored on that
    /// side only.** `w = "wide"` is no minimum across, so window 3 turns
    /// and goes beside window 1 when one above the other would take it
    /// under the 800 that `h` still asks for. Three dispatches, one line.
    #[test]
    fn a_minimum_that_is_not_a_size_is_named_once_and_ignored() {
        let (mut scripts, _) = tiling_with(&format!(
            "{CAPTURE}\nrequire(\"config\").tiling.minimum = {{ w = \"wide\", h = 800 }}"
        ));
        let switched = scripts.key("super+t", desk_at(&[1, 2, 3], (500.0, 500.0)));
        let _ = scripts.relayout(desk_at(&[1, 2, 3], (500.0, 500.0)));
        let _ = scripts.relayout(desk_at(&[1, 2, 3], (500.0, 500.0)));
        let placed = placed(&switched.commands);
        assert!(
            placed
                .get(&1)
                .zip(placed.get(&3))
                .is_some_and(|(one, three)| about(one.y, three.y)
                    && about(one.h, 1416.0)
                    && about(three.h, 1416.0)),
            "window 3 was not put beside window 1: {placed:?}"
        );
        let said = logged(&scripts);
        assert_eq!(
            said.matches("tiling.minimum.w").count(),
            1,
            "the bad side was not named exactly once: {said:?}"
        );
        assert!(
            !said.contains("tiling.minimum.h"),
            "the good side was named: {said:?}"
        );
    }

    /// **An empty workspace is given whole once the window on it was sent
    /// away.** Window 7, opened on workspace 2 and sent to 3, leaves
    /// workspace 2 empty, and window 2, overflowing to it, gets the whole
    /// of it rather than half.
    #[test]
    fn a_workspace_left_empty_by_a_send_is_given_whole() {
        let (mut scripts, _) = tiling_with(&minimum(1300, 800));
        switched_on(&mut scripts);
        let _ = scripts.key("super+2", desk_at(&[], (0.0, 0.0)));
        let _ = open(&mut scripts, 7, &[7]);
        let mut sending = desk_at(&[7], (0.0, 0.0));
        for window in &mut sending.windows {
            window.focused = true;
        }
        let _ = scripts.key("super+shift+3", sending);
        let _ = scripts.key("super+1", desk_at(&[7], (0.0, 0.0)));
        assert_eq!(workspace_of(&scripts, 7), "3");
        assert_eq!(showing(&scripts), "1");

        let _ = open(&mut scripts, 1, &[7, 1]);
        let told = open(&mut scripts, 2, &[7, 1, 2]);
        assert_eq!(workspace_of(&scripts, 2), "2");
        assert!(
            places_of(&told.commands, 2)
                .last()
                .is_some_and(|rect| same(*rect, FULL)),
            "window 2 shared workspace 2 with a window that is not there: {:?}",
            told.commands
        );
    }

    /// **The next empty workspace is after the one in view, and round
    /// again**; a window being closed does not take one; and with
    /// workspaces not per monitor, a window on any screen does.
    #[test]
    fn the_next_empty_workspace_is_after_the_one_in_view_and_round_again() {
        let vacant = |per_monitor: bool, windows: Vec<WindowInfo>| {
            let (mut scripts, _) = tiling_with(&format!(
                "require(\"config\").workspaces.per_monitor = {per_monitor}"
            ));
            let _ = scripts.evaluate(
                "local workspaces = require(\"workspaces\")\n\
                 workspaces.showing[\"DP-1\"] = 3\n\
                 workspaces.showing[\"*all*\"] = 3\n\
                 workspaces.of[21] = 4\n\
                 workspaces.of[22] = 2\n\
                 workspaces.of[23] = 1\n\
                 workspaces.of[24] = 1\n\
                 return \"\"",
            );
            let _ = scripts.relayout(snapshot_on(vec![monitor(), second_monitor()], windows));
            scripts.evaluate("return tostring(require(\"workspaces\").vacant(\"DP-1\"))")
        };
        let closing = |id| WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets::default(),
            leaving: true,
            ..window(id, 800.0, 600.0)
        };
        let taken = |id| window(id, 800.0, 600.0);

        // Showing 3, with 4 and 2 taken: round to 1, whose only window is
        // being closed.
        assert_eq!(vacant(true, vec![taken(21), taken(22), closing(23)]), "1");
        // The same with 1 taken for real: nothing left.
        assert_eq!(vacant(true, vec![taken(21), taken(22), taken(23)]), "nil");
        // Per monitor, a window on the other screen is its own screen's.
        let elsewhere = on(&second_monitor(), window(24, 800.0, 600.0));
        assert_eq!(
            vacant(true, vec![taken(21), taken(22), elsewhere.clone()]),
            "1"
        );
        // Together, the same window takes workspace 1 on every screen.
        assert_eq!(vacant(false, vec![taken(21), taken(22), elsewhere]), "nil");
        // And a window that belongs to no workspace in particular is on
        // every one, since the view carries it along: 25 was never given
        // one, so no workspace is empty (#134 review).
        assert_eq!(
            vacant(true, vec![taken(21), window(25, 800.0, 600.0)]),
            "nil"
        );
    }

    /// **A tree handed a minimum that is not a size has none on that
    /// side.** `tiling.lua` cleans `config.tiling.minimum` before handing
    /// it over, so this is the binding's own answer for a script that does
    /// not: `0/0` and a string are no floor, where a real 160x150 on the
    /// same 300x200 tile refuses the split both ways.
    #[test]
    fn a_tree_handed_a_minimum_that_is_not_a_size_has_none_on_that_side() {
        let (scripts, _) = tiling_with("");
        let answer = scripts.evaluate(
            "local function try(minimum)\n\
                 local area = { x = 0, y = 0, w = 300, h = 200, gap = 0, split = 0.5,\n\
                     minimum = minimum }\n\
                 local tree = sol.layout.tree()\n\
                 tree:insert(1, nil, nil, nil, area)\n\
                 return tostring(tree:insert_fitting(2, nil, 150, 100, area))\n\
             end\n\
             return try({ w = 0/0, h = \"tall\" }) .. \" \" .. try({ w = 160, h = 150 })",
        );
        assert_eq!(answer, "true false");
    }

    /// **With `follow_new_windows` off, no workspace is empty.** A window
    /// that belongs to no workspace in particular is on every one -- the
    /// view carries it along, which is what `workspaces.at` says -- so
    /// window 1 is on workspace 2 as much as on this one. Sending window 2
    /// there moved the view, left window 1 on screen, and laid window 2
    /// over the whole of it from workspace 2's empty tree (#134 review).
    /// So the step does nothing and `"allow"` puts window 2 beside window
    /// 1, here.
    #[test]
    fn with_follow_new_windows_off_no_workspace_is_empty() {
        let (mut scripts, _) = tiling_with(&format!(
            "{}\nrequire(\"config\").workspaces.follow_new_windows = false",
            minimum(1300, 800)
        ));
        switched_on(&mut scripts);
        let _ = open(&mut scripts, 1, &[1]);
        let told = open(&mut scripts, 2, &[1, 2]);

        assert_eq!(
            showing(&scripts),
            "1",
            "the view went to a workspace window 1 is on as well"
        );
        assert_eq!(
            scripts.evaluate("return tostring(require(\"workspaces\").of[2])"),
            "nil",
            "window 2 was given a workspace of its own"
        );
        let placed = placed(&told.commands);
        let (one, two) = (placed.get(&1).copied(), placed.get(&2).copied());
        assert!(
            one.zip(two).is_some_and(|(one, two)| !overlap(one, two)),
            "window 2 was not placed beside window 1: {one:?} {two:?}"
        );
    }

    /// **With `reflow_on_close = "when_gone"` or `"when_landed"`, a
    /// window being closed still takes its workspace.** Those settings
    /// keep a closing window's tile while it fades, and "when_gone" until
    /// its application has gone, so a workspace whose only window is
    /// closing is not empty to them: window 3 goes on to workspace 3,
    /// rather than into a fresh tree on workspace 2 that has it cover
    /// window 5 while window 5 fades (#134 review). Closing up at once,
    /// the same workspace is empty -- that half is
    /// `a_window_being_closed_is_room_for_the_next_one`. Against `0ce2ab8`
    /// `"when_landed"` fails: it was read as "immediate" (#128).
    #[test]
    fn waiting_to_reflow_a_closing_window_still_takes_its_workspace() {
        for setting in ["when_gone", "when_landed"] {
            let (mut scripts, _) = tiling_with(&format!(
                "{}\nrequire(\"config\").tiling.reflow_on_close = {setting:?}",
                minimum(1300, 800)
            ));
            switched_on(&mut scripts);
            let _ = scripts.evaluate("require(\"workspaces\").of[5] = 2\nreturn \"\"");
            let _ = open(&mut scripts, 1, &[5, 1]);
            let mut windows = vec![
                window(5, 800.0, 600.0),
                window(1, 800.0, 600.0),
                window(3, 800.0, 600.0),
            ];
            windows[0].leaving = true;
            let _ = scripts.opened(
                3,
                Snapshot {
                    cursor: (500.0, 500.0),
                    ..snapshot(windows)
                },
            );
            assert_eq!(
                workspace_of(&scripts, 3),
                "3",
                "{setting}: a workspace whose only window keeps its tile while it closes was \
             counted as empty"
            );
        }
    }

    /// **With `follow_overflow` off, each window with no room takes
    /// another empty workspace**, whether or not the one the window
    /// before went to has room.
    ///
    /// At 1000x1000, 1 and 2 share workspace 1 side by side at 1262x1416,
    /// and neither of those halves has room either way. The view stays on
    /// workspace 1, so 3, 4 and 5 each open against those tiles, and each
    /// finds the workspace the last one went to no longer empty: 3 goes to
    /// 2, 4 to 3, 5 to 4. With none left empty, 6 is allowed in on
    /// workspace 1 under the minimum -- although workspace 2, holding
    /// window 3 alone, could still have taken it beside window 3. That is
    /// the step as written, "the next empty workspace"; `config.lua` says
    /// so beside `follow_overflow`.
    #[test]
    fn with_follow_overflow_off_each_window_with_no_room_takes_another_empty_workspace() {
        let (mut scripts, _) = tiling_with(&format!(
            "{}\nrequire(\"config\").tiling.follow_overflow = false",
            minimum(1000, 1000)
        ));
        switched_on(&mut scripts);
        for id in 1..=6 {
            let ids: Vec<u64> = (1..=id).collect();
            let _ = open(&mut scripts, id, &ids);
        }
        let went: Vec<String> = (1..=6).map(|id| workspace_of(&scripts, id)).collect();
        assert_eq!(went, ["1", "1", "2", "3", "4", "1"]);
        assert_eq!(showing(&scripts), "1");
        assert_eq!(
            scripts.evaluate(
                "local area = { x = 0, y = 0, w = 2560, h = 1440, gap = 12, split = 0.5,\n\
                     minimum = { w = 1000, h = 1000 } }\n\
                 local tree = require(\"tiling\").trees[\"2@DP-1\"]\n\
                 return tostring(tree:insert_fitting(99, nil, nil, nil, area))"
            ),
            "true",
            "the premise: workspace 2 had room for window 6"
        );
    }

    fn open(scripts: &mut Scripts, id: u64, ids: &[u64]) -> Outcome {
        scripts.opened(id, desk_at(ids, (500.0, 500.0)))
    }
}

/// **#115: what a window's own application says about its size**, and
/// what the shipped layouts do with it.
///
/// Against the shipped `tiling.lua`, `scrolling.lua` and `sizes.lua`, as
/// `init.lua` loads them, on one 2560x1440 screen at the shipped 12px gap
/// and 160x96 minimum. Windows 1 and 2 side by side take 1262 each of the
/// 2536 inside the gap, which is the arrangement most of these start from.
/// The limits are what `sol.windows()` carries -- frame included, as the
/// compositor adds it -- so nothing here converts anything.
mod client_sizes {
    use super::super::Outcome;
    use super::*;
    use smithay::utils::Size;

    /// The shipped layouts with `before` run ahead of them, and every
    /// `sol.log` line kept in `_G.logged`. A directory per call, for the
    /// reason [`scripts`] gives.
    fn layouts_with(before: &str) -> Scripts {
        Scripts::load(&entry_with(before)).expect("loading the shipped layouts")
    }

    /// The entry point [`layouts_with`] loads, for a test that loads it
    /// again.
    fn entry_with(before: &str) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("solium-sizes-{}-{serial}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\n\
                 _G.logged = {{}}\n\
                 local log = sol.log\n\
                 sol.log = function(message)\n\
                     _G.logged[#_G.logged + 1] = message\n\
                     log(message)\n\
                 end\n\
                 {before}\n\
                 require(\"modes\")\n\
                 require(\"workspaces\")\n\
                 require(\"tiling\")\n\
                 require(\"scrolling\")\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        )
        .expect("writing the entry point");
        entry
    }

    fn logged(scripts: &Scripts) -> String {
        scripts.evaluate("return table.concat(_G.logged or {}, \"\\n\")")
    }

    fn size(w: i32, h: i32) -> Option<Size<i32, Logical>> {
        Some(Size::from((w, h)))
    }

    /// Window `id` with nothing to say about its size.
    fn plain(id: u64) -> WindowInfo {
        window(id, 800.0, 600.0)
    }

    /// Window `id`, whose application says it cannot go under `min`.
    fn at_least(id: u64, w: i32, h: i32) -> WindowInfo {
        WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets::default(),
            min: size(w, h),
            ..plain(id)
        }
    }

    /// Window `id`, whose application says it cannot go over `max`.
    fn at_most(id: u64, w: i32, h: i32) -> WindowInfo {
        WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets::default(),
            max: size(w, h),
            ..plain(id)
        }
    }

    fn with_app(app_id: &str, window: WindowInfo) -> WindowInfo {
        WindowInfo {
            icon: None,
            client_side: false,
            blur_region: false,
            insets: crate::decoration::Insets::default(),
            app_id: app_id.to_owned(),
            ..window
        }
    }

    /// These windows, with the pointer at `cursor`.
    fn desk(windows: Vec<WindowInfo>, cursor: (f64, f64)) -> Snapshot {
        Snapshot {
            cursor,
            ..snapshot(windows)
        }
    }

    /// A layout switched on over an empty desk, so every window after
    /// this arrives through `open`.
    fn switched_on(scripts: &mut Scripts, key: &str) {
        assert!(
            scripts.key(key, desk(Vec::new(), (0.0, 0.0))).handled,
            "{key} was not handled"
        );
    }

    /// Windows 1 and 2 side by side under tiling: 2 opened with the
    /// pointer over the right half of 1, so it takes the right.
    fn side_by_side(scripts: &mut Scripts) {
        switched_on(scripts, "super+t");
        let _ = scripts.opened(1, desk(vec![plain(1)], (2000.0, 700.0)));
        let _ = scripts.opened(2, desk(vec![plain(1), plain(2)], (2000.0, 700.0)));
    }

    /// The last placement of window `id` in `commands`: the rect, the tile
    /// it was placed inside when that is not the rect, and whether the
    /// layout said it is cramped.
    fn last_place(commands: &[Command], id: u64) -> Option<(Rect, Option<Rect>, bool)> {
        commands.iter().rev().find_map(|command| match command {
            Command::Place {
                id: placed,
                rect,
                inside,
                cramped,
                ..
            } if *placed == id => Some((*rect, *inside, *cramped)),
            _ => None,
        })
    }

    fn width_of(outcome: &Outcome, id: u64) -> f64 {
        last_place(&outcome.commands, id)
            .map(|(rect, _, _)| rect.w)
            .unwrap_or_else(|| panic!("window {id} was not placed: {:?}", outcome.commands))
    }

    /// **`sol.windows()` says how small and how large each window can
    /// be**, which application it is, and whether the layout said it is
    /// cramped -- the four as a layout reads them, in one pass. Absent is
    /// `nil`, so `if window.min then` is the question to ask.
    #[test]
    fn sol_windows_says_how_small_and_how_large_each_window_can_be() {
        let mut scripts = layouts_with(
            "_G.seen = {}\n\
             sol.on(\"layout\", function()\n\
                 for _, window in ipairs(sol.windows()) do\n\
                     local function size(of)\n\
                         return of and string.format(\"%dx%d\", of.w, of.h) or \"nil\"\n\
                     end\n\
                     _G.seen[#_G.seen + 1] = string.format(\"%d %s min=%s max=%s cramped=%s\",\n\
                         window.id, window.app_id, size(window.min), size(window.max),\n\
                         tostring(window.cramped))\n\
                 end\n\
             end)",
        );
        let windows = vec![
            WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                max: size(1200, 0),
                cramped: true,
                ..with_app("firefox", at_least(1, 820, 0))
            },
            with_app("kitty", plain(2)),
        ];
        let _ = scripts.relayout(desk(windows, (0.0, 0.0)));
        assert_eq!(
            scripts.evaluate("return table.concat(_G.seen, \", \")"),
            "1 firefox min=820x0 max=1200x0 cramped=true, \
             2 kitty min=nil max=nil cramped=false"
        );
    }

    /// **A window whose minimum is wider than its tile is given the room**,
    /// taken from the window beside it -- which is the whole of #115: the
    /// tile no longer lies about what the client will draw.
    ///
    /// Window 1 says 1600 wide; it had 1262. Window 2 gives up the
    /// difference and is left 924, well over the minimum. Neither is
    /// cramped: the room was there.
    #[test]
    fn a_window_with_a_minimum_wider_than_its_tile_is_given_the_room() {
        let mut scripts = layouts_with("");
        side_by_side(&mut scripts);
        let told = scripts.relayout(desk(vec![at_least(1, 1600, 0), plain(2)], (0.0, 0.0)));
        let (one, _, cramped) = last_place(&told.commands, 1).expect("window 1 placed");
        let (two, _, _) = last_place(&told.commands, 2).expect("window 2 placed");
        assert!(
            about(one.w, 1600.0),
            "window 1 was not given its minimum: {one:?}"
        );
        assert!(
            about(two.w, 924.0) && about(two.x, 12.0 + 1600.0 + 12.0),
            "{two:?}"
        );
        assert!(!cramped, "a window with the room it needs is not cramped");
    }

    /// **`"ignore"` and `client_size_ignore` are the arrangement before
    /// #115**, for the minimum and the maximum alike: window 1 keeps its
    /// 1262 however wide it says it must be, and a window no wider than
    /// 800 fills its tile from the corner rather than being centred.
    ///
    /// `client_size_ignore` by the application's name, and only that
    /// application: `firefox` is not believed, `kitty` beside it still is.
    #[test]
    fn ignore_and_client_size_ignore_lay_out_what_was_laid_out_before() {
        let wide = |app: &str| with_app(app, at_least(1, 1600, 0));
        for (before, app, says) in [
            (
                "require(\"config\").tiling.client_minimum = \"ignore\"",
                "kitty",
                "ignore",
            ),
            (
                "require(\"config\").tiling.client_size_ignore = { \"firefox\" }",
                "firefox",
                "client_size_ignore",
            ),
        ] {
            let mut scripts = layouts_with(before);
            side_by_side(&mut scripts);
            let told = scripts.relayout(desk(vec![wide(app), plain(2)], (0.0, 0.0)));
            assert!(
                about(width_of(&told, 1), 1262.0),
                "{says}: the minimum was still heard: {:?}",
                told.commands
            );
        }
        let mut scripts =
            layouts_with("require(\"config\").tiling.client_size_ignore = { \"firefox\" }");
        side_by_side(&mut scripts);
        let told = scripts.relayout(desk(vec![wide("kitty"), plain(2)], (0.0, 0.0)));
        assert!(
            about(width_of(&told, 1), 1600.0),
            "an application not on the list was not heard: {:?}",
            told.commands
        );

        for before in [
            "require(\"config\").tiling.client_maximum = \"ignore\"",
            "require(\"config\").tiling.client_size_ignore = { \"firefox\" }",
        ] {
            let mut scripts = layouts_with(before);
            switched_on(&mut scripts, "super+t");
            let told = scripts.opened(
                1,
                desk(vec![with_app("firefox", at_most(1, 800, 600))], (0.0, 0.0)),
            );
            let (rect, inside, _) = last_place(&told.commands, 1).expect("placed");
            assert!(
                about(rect.w, 2536.0) && inside.is_none(),
                "{before}: the maximum was still heard: {rect:?} in {inside:?}"
            );
        }
    }

    /// **A setting that is not one of its words is read as the default**,
    /// and a `client_size_ignore` that is not a list believes every
    /// application -- so a typo in either is the shipped behaviour and not
    /// the old one.
    #[test]
    fn a_setting_that_is_not_one_of_its_words_is_read_as_the_default() {
        for before in [
            "require(\"config\").tiling.client_minimum = \"respekt\"",
            "require(\"config\").tiling.client_size_ignore = \"firefox\"",
        ] {
            let mut scripts = layouts_with(before);
            side_by_side(&mut scripts);
            let told = scripts.relayout(desk(
                vec![with_app("firefox", at_least(1, 1600, 0)), plain(2)],
                (0.0, 0.0),
            ));
            assert!(
                about(width_of(&told, 1), 1600.0),
                "{before}: the minimum was not heard: {:?}",
                told.commands
            );
        }
        let mut scripts = layouts_with("require(\"config\").tiling.client_maximum = \"centre\"");
        switched_on(&mut scripts, "super+t");
        let told = scripts.opened(1, desk(vec![at_most(1, 800, 600)], (0.0, 0.0)));
        let (rect, inside, _) = last_place(&told.commands, 1).expect("placed");
        assert!(
            about(rect.w, 800.0) && inside.is_some(),
            "\"centre\" was not read as \"center\": {rect:?} in {inside:?}"
        );
    }

    /// **`options.floors` takes sizes, and anything else is no floor**: a
    /// side that is not a size is none on that side, an entry that is not
    /// a table is none at all. The one real floor still moves the seam,
    /// and one the tree cannot meet comes back `cramped`.
    #[test]
    fn floors_that_are_not_sizes_are_no_floors() {
        let scripts = layouts_with("");
        let lay_out = |floors: &str| {
            scripts.evaluate(&format!(
                "local area = {{ x = 0, y = 0, w = 2000, h = 1000, gap = 0, split = 0.5,\n\
                     floors = {floors} }}\n\
                 local tree = sol.layout.tree()\n\
                 tree:insert(1, nil, nil, nil, area)\n\
                 tree:insert(2, 1, nil, nil, area)\n\
                 local out = {{}}\n\
                 for _, slot in ipairs(tree:layout(area)) do\n\
                     out[#out + 1] = string.format(\"%d=%.0f%s\", slot.id, slot.w,\n\
                         slot.cramped and \" cramped\" or \"\")\n\
                 end\n\
                 return table.concat(out, \",\")"
            ))
        };
        assert_eq!(
            lay_out("{ [1] = { w = 1500, h = \"tall\" }, [2] = \"wide\", [3] = { w = 0/0 } }"),
            "1=1500,2=500"
        );
        assert_eq!(lay_out("{ [1] = { w = 2500 } }"), "1=1900 cramped,2=100");
    }

    /// **A window whose maximum is smaller than its tile is its maximum,
    /// in the middle of the tile** -- and the tile goes with it as `tile`,
    /// so the compositor holds the client inside the tile and a dragged
    /// edge moves the tile's seam.
    #[test]
    fn a_window_whose_maximum_is_smaller_than_its_tile_is_centred_in_it() {
        let mut scripts = layouts_with("");
        switched_on(&mut scripts, "super+t");
        let told = scripts.opened(1, desk(vec![at_most(1, 800, 600)], (0.0, 0.0)));
        let (rect, inside, _) = last_place(&told.commands, 1).expect("window 1 placed");
        let tile = inside.expect("the tile was not handed over with the window");
        assert!(
            about(tile.x, 12.0) && about(tile.w, 2536.0) && about(tile.h, 1416.0),
            "{tile:?}"
        );
        assert!(about(rect.w, 800.0) && about(rect.h, 600.0), "{rect:?}");
        assert!(
            about(rect.x, 12.0 + (2536.0 - 800.0) / 2.0)
                && about(rect.y, 12.0 + (1416.0 - 600.0) / 2.0),
            "not in the middle of its tile: {rect:?}"
        );
    }

    /// **A window that would crowd a minimum goes where `overflow`
    /// says.** Window 1 needs 2450x1350 of the 2536x1416 it has: beside it
    /// is 74 wide and below it 54 high, both under the minimum, so window 2
    /// has no room either way and the shipped chain sends it to workspace
    /// 2. Before #115 it split window 1's tile, and window 1 drew over it.
    #[test]
    fn a_window_that_would_crowd_a_minimum_goes_where_overflow_says() {
        let mut scripts = layouts_with("");
        switched_on(&mut scripts, "super+t");
        let _ = scripts.opened(1, desk(vec![at_least(1, 2450, 1350)], (2000.0, 700.0)));
        let _ = scripts.opened(
            2,
            desk(vec![at_least(1, 2450, 1350), plain(2)], (2000.0, 700.0)),
        );
        assert_eq!(
            scripts.evaluate("return tostring(require(\"workspaces\").at(2, \"DP-1\"))"),
            "2",
            "window 2 was squeezed in beside a window that has no room to give"
        );
    }

    /// **A window that cannot have its minimum is cramped, and the log
    /// says so once, with the numbers.** Window 1 says 2500 wide; the most
    /// window 2 can give leaves it 2364, window 2 at the 160 minimum. The
    /// layout says `cramped` on every placement and names it in the log on
    /// the first; a window that has room again stops being cramped, and
    /// one that loses it again is named again.
    #[test]
    fn a_window_that_cannot_have_its_minimum_is_cramped_and_said_once() {
        let mut scripts = layouts_with("");
        side_by_side(&mut scripts);
        let crowded = || desk(vec![at_least(1, 2500, 0), plain(2)], (0.0, 0.0));

        let told = scripts.relayout(crowded());
        let (one, _, cramped) = last_place(&told.commands, 1).expect("window 1 placed");
        assert!(about(one.w, 2364.0), "{one:?}");
        assert!(cramped, "window 1 is under its minimum and not said to be");
        assert!(
            !last_place(&told.commands, 2).is_some_and(|(_, _, cramped)| cramped),
            "window 2, at the minimum, is not cramped"
        );
        let said = logged(&scripts);
        assert!(
            said.contains("window 1 needs at least 2500 wide")
                && said.contains("its tile is 2364x1416"),
            "{said}"
        );

        let told = scripts.relayout(crowded());
        assert!(last_place(&told.commands, 1).is_some_and(|(_, _, cramped)| cramped));
        assert_eq!(
            logged(&scripts).matches("is cramped").count(),
            1,
            "said again for a window that was cramped already"
        );

        let told = scripts.relayout(desk(vec![plain(1), plain(2)], (0.0, 0.0)));
        assert!(!last_place(&told.commands, 1).is_some_and(|(_, _, cramped)| cramped));
        let _ = scripts.relayout(crowded());
        assert_eq!(logged(&scripts).matches("is cramped").count(), 2);
    }

    /// **A seam dragged towards a window stops at its minimum.** Window 2
    /// needs 1500, so window 1 has 1024; dragging window 1's right edge out
    /// to 2000 leaves both where they are.
    #[test]
    fn a_seam_dragged_towards_a_window_stops_at_its_minimum() {
        let mut scripts = layouts_with("");
        side_by_side(&mut scripts);
        let desk_now = || desk(vec![plain(1), at_least(2, 1500, 0)], (0.0, 0.0));
        let told = scripts.relayout(desk_now());
        assert!(
            about(width_of(&told, 1), 1024.0),
            "the premise: {:?}",
            told.commands
        );

        let told = scripts.resized(1, (2000.0, 700.0), (Some("right"), None), desk_now());
        assert!(
            about(width_of(&told, 1), 1024.0) && about(width_of(&told, 2), 1500.0),
            "the drag went past window 2's minimum: {:?}",
            told.commands
        );
    }

    /// **A launched window whose minimum does not fit the tile it was
    /// given is placed again, once, before it is shown -- and never
    /// after.**
    ///
    /// Window 1 fills the screen and window 2 is launched beside it, with
    /// nothing said yet. Its application then says 2450x1350 before its
    /// first frame: beside window 1 there is no room either way, so it goes
    /// where the shipped chain sends it, workspace 2, as it would have at
    /// an `open` that knew. The same minimum arriving after the first
    /// frame is a window already on screen, which is rebalanced and left
    /// cramped where it is. And a window whose desk the user has switched
    /// away from is left on it, in its own desk's tree and no other.
    #[test]
    fn a_launched_window_is_placed_again_by_its_minimum_only_before_it_is_shown() {
        for (shown, away) in [(false, false), (true, false), (false, true)] {
            let mut scripts = layouts_with("");
            switched_on(&mut scripts, "super+t");
            let _ = scripts.opened(1, desk(vec![plain(1)], (2000.0, 700.0)));
            let launched = WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                shown: false,
                ..plain(2)
            };
            let _ = scripts.opened(2, desk(vec![plain(1), launched], (2000.0, 700.0)));
            let workspace = |scripts: &Scripts| {
                scripts.evaluate("return tostring(require(\"workspaces\").of[2])")
            };
            assert_eq!(workspace(&scripts), "1", "the premise");

            let arrived = WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                shown,
                ..at_least(2, 2450, 1350)
            };
            if away {
                assert!(
                    scripts
                        .key("super+2", desk(vec![plain(1), arrived.clone()], (0.0, 0.0)))
                        .handled,
                    "super+2 was not handled"
                );
            }
            let told = scripts.relayout(desk(vec![plain(1), arrived], (0.0, 0.0)));
            if away {
                assert_eq!(
                    workspace(&scripts),
                    "1",
                    "a window on a desk out of view was moved"
                );
                assert_eq!(
                    scripts.evaluate(
                        "local tree = require(\"tiling\").trees[require(\"monitors\").key(2, \"DP-1\")]\n\
                         return tostring(tree ~= nil and tree:contains(2))"
                    ),
                    "false",
                    "the window is in the tree of a desk it is not on"
                );
            } else if shown {
                assert_eq!(workspace(&scripts), "1", "a window on screen was sent away");
                assert!(
                    last_place(&told.commands, 2).is_some_and(|(_, _, cramped)| cramped),
                    "{:?}",
                    told.commands
                );
            } else {
                assert_eq!(
                    workspace(&scripts),
                    "2",
                    "the launched window stayed in a tile that cannot hold it"
                );
                let again = WindowInfo {
                    icon: None,
                    client_side: false,
                    blur_region: false,
                    insets: crate::decoration::Insets::default(),
                    shown: false,
                    ..at_least(2, 2450, 1350)
                };
                let _ = scripts.relayout(desk(vec![plain(1), again], (0.0, 0.0)));
                assert_eq!(workspace(&scripts), "2", "placed a second time");
            }
        }
    }

    /// **What a floating drag believes reaches the compositor**:
    /// `sizes.lua` hands `floating.client_limits` and
    /// `tiling.client_size_ignore` over as it loads, and the shipped
    /// settings are the compositor's own default.
    #[test]
    fn the_floating_setting_and_the_ignored_applications_reach_the_compositor() {
        let handed = |before: &str| {
            layouts_with(before)
                .startup()
                .commands
                .into_iter()
                .find_map(|command| match command {
                    Command::ClientSizes(sizes) => Some(sizes),
                    _ => None,
                })
        };
        assert_eq!(handed(""), Some(super::super::ClientSizes::default()));
        for before in [
            "require(\"config\").floating.client_limits = \"respekt\"",
            "require(\"config\").tiling.client_size_ignore = \"firefox\"",
        ] {
            assert_eq!(
                handed(before),
                Some(super::super::ClientSizes::default()),
                "{before}: not read as the default"
            );
        }
        assert_eq!(
            handed(
                "local config = require(\"config\")\n\
                 config.floating.client_limits = \"ignore\"\n\
                 config.tiling.client_size_ignore = { \"firefox\", 7 }"
            ),
            Some(super::super::ClientSizes {
                floating: false,
                ignored: vec!["firefox".to_owned()],
            })
        );
    }

    /// **A reload does not name a cramped window again**: it is the same
    /// window in the same tile, and the log said so once already.
    #[test]
    fn a_reload_does_not_name_a_cramped_window_again() {
        let entry = entry_with("");
        let mut scripts = Scripts::load(&entry).expect("loading the shipped layouts");
        side_by_side(&mut scripts);
        let crowded = || desk(vec![at_least(1, 2500, 0), plain(2)], (0.0, 0.0));
        let _ = scripts.relayout(crowded());
        assert_eq!(
            logged(&scripts).matches("is cramped").count(),
            1,
            "the premise"
        );

        let mut fresh = scripts;
        fresh.reload(&entry).expect("reloading the scripts");
        let _ = fresh.restored(crowded());
        let _ = fresh.monitors_changed(crowded());
        let told = fresh.relayout(crowded());
        assert!(
            last_place(&told.commands, 1).is_some_and(|(_, _, cramped)| cramped),
            "the premise: still cramped after the reload"
        );
        assert_eq!(
            logged(&fresh).matches("is cramped").count(),
            0,
            "named again after a reload"
        );
    }

    /// **A scrolling column is never narrower than its window's
    /// minimum.** A column opens at a third of the 2536 inside the gap,
    /// 845; a window that needs 1500 has a 1500 column.
    #[test]
    fn a_scrolling_column_is_as_wide_as_its_windows_minimum() {
        let mut scripts = layouts_with("");
        switched_on(&mut scripts, "super+s");
        let told = scripts.opened(1, desk(vec![at_least(1, 1500, 0)], (0.0, 0.0)));
        assert!(
            about(width_of(&told, 1), 1500.0),
            "the column is narrower than its window: {:?}",
            told.commands
        );
    }
}

/// **#118 and #129: a reload is a change of configuration, not of
/// arrangement.**
///
/// Every layout on every desk comes back from `super+shift+r` exactly as it
/// was: the same windows in the same tiles at the same split ratios, and the
/// same columns in the same order at the same widths with the view where it
/// was. Driven through the sequence `Solium::reload` runs -- the keep read
/// off the running scripts, the configuration read again, then `restore`,
/// `monitors` and `layout` -- against the shipped layouts, as the modules
/// above run them.
mod reload {
    use super::*;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// [`AREA`] less the shipped gap all round: the one tile a workspace
    /// with one window has.
    const FULL: Rect = Rect {
        x: 12.0,
        y: 12.0,
        w: 2536.0,
        h: 1416.0,
    };

    /// The shipped layouts in the order `init.lua` requires them, with
    /// `before` run ahead of them. Written again between two loads, it is
    /// a configuration edited between them -- which is what a reload is
    /// for.
    fn write(entry: &Path, before: &str) {
        std::fs::write(
            entry,
            format!(
                "package.path = {shipped:?} .. \"/?.lua\"\n\
                 {before}\n\
                 require(\"modes\")\n\
                 require(\"workspaces\")\n\
                 require(\"tiling\")\n\
                 require(\"scrolling\")\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        )
        .expect("writing the entry point");
    }

    /// The session, and the entry point a reload reads again. A directory
    /// per call, for the reason [`scripts`] gives.
    fn session(before: &str) -> (Scripts, PathBuf) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("solium-reload-{}-{serial}", std::process::id()));
        let _ = std::fs::create_dir_all(&directory);
        let entry = directory.join("init.lua");
        write(&entry, before);
        let scripts = Scripts::load(&entry).expect("loading the shipped layouts");
        (scripts, entry)
    }

    /// Exactly what `Solium::reload` does, in its order, with the world
    /// looking like `now` throughout. The new scripts, and every command
    /// the three announcements produced.
    fn reload(mut scripts: Scripts, entry: &Path, now: &Snapshot) -> (Scripts, Vec<Command>) {
        scripts.reload(entry).expect("reloading the scripts");
        let mut commands = scripts.restored(now.clone()).commands;
        commands.extend(scripts.monitors_changed(now.clone()).commands);
        commands.extend(scripts.relayout(now.clone()).commands);
        (scripts, commands)
    }

    /// The windows `ids` on `DP-1`, topmost first as the compositor lists
    /// them, each where `rects` says it was placed, with the pointer at
    /// `cursor`. A window `rects` has nothing for is one being mapped: the
    /// size its client chose, at the origin.
    fn desk(rects: &HashMap<u64, Rect>, ids: &[u64], cursor: (f64, f64)) -> Snapshot {
        Snapshot {
            cursor,
            ..snapshot(ids.iter().map(|&id| at_rest(rects, id)).collect())
        }
    }

    fn at_rest(rects: &HashMap<u64, Rect>, id: u64) -> WindowInfo {
        match rects.get(&id) {
            Some(rect) => WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                rect: *rect,
                ..window(id, rect.w, rect.h)
            },
            None => window(id, 800.0, 600.0),
        }
    }

    /// Window `id` opens on top of `below` with the pointer at `cursor`,
    /// and wherever that placed anything is recorded in `rects`.
    fn open(
        scripts: &mut Scripts,
        rects: &mut HashMap<u64, Rect>,
        id: u64,
        below: &[u64],
        cursor: (f64, f64),
    ) {
        let mut ids = vec![id];
        ids.extend_from_slice(below);
        let outcome = scripts.opened(id, desk(rects, &ids, cursor));
        rects.extend(placed(&outcome.commands));
    }

    /// `combo`, pressed over the windows `ids`, with what it placed
    /// recorded in `rects`.
    fn press(
        scripts: &mut Scripts,
        rects: &mut HashMap<u64, Rect>,
        ids: &[u64],
        combo: &str,
        cursor: (f64, f64),
    ) {
        let outcome = scripts.key(combo, desk(rects, ids, cursor));
        assert!(outcome.handled, "{combo} was not handled");
        rects.extend(placed(&outcome.commands));
    }

    /// One tree's or strip's arrangement of a 2560x1440 screen, as text:
    /// every slot it lays out, in its own order, and for a strip the window
    /// it has focused. The columns' order and widths and where the view sits
    /// are all in the slots. `nil` when the layout holds nothing under
    /// `key`, which is what a hidden desk's tree was after a reload before
    /// #129.
    fn arrangement(scripts: &Scripts, layout: &str, key: &str) -> String {
        let (table, focus) = if layout == "tiling" {
            ("trees", "")
        } else {
            (
                "views",
                "out[#out + 1] = \"focus=\" .. tostring(each:focused())",
            )
        };
        scripts.evaluate(&format!(
            "local each = require({layout:?}).{table}[{key:?}]\n\
             if not each then return \"nil\" end\n\
             local area = {{ x = 0, y = 0, w = 2560, h = 1440, gap = 12 }}\n\
             local out = {{}}\n\
             for _, slot in ipairs(each:layout(area)) do\n\
                 out[#out + 1] = string.format(\"%d@%.2f,%.2f,%.2fx%.2f\",\n\
                     slot.id, slot.x, slot.y, slot.w, slot.h)\n\
             end\n\
             {focus}\n\
             return table.concat(out, \" \")"
        ))
    }

    /// The windows tiling holds under `key`, sorted, or `nil` for no tree.
    fn held(scripts: &Scripts, key: &str) -> String {
        scripts.evaluate(&format!(
            "local tree = require(\"tiling\").trees[{key:?}]\n\
             if not tree then return \"nil\" end\n\
             local ids = tree:windows()\n\
             table.sort(ids)\n\
             return table.concat(ids, \",\")"
        ))
    }

    fn same(left: Rect, right: Rect) -> bool {
        about(left.x, right.x)
            && about(left.y, right.y)
            && about(left.w, right.w)
            && about(left.h, right.h)
    }

    /// Every place `commands` sent, in order.
    fn places(commands: &[Command]) -> Vec<(u64, Rect)> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Place { id, rect, .. } => Some((*id, *rect)),
                _ => None,
            })
            .collect()
    }

    /// Every place a reload sent is where that window already was, and
    /// every window in `ids` was sent one: the reload re-stated the
    /// arrangement rather than making another.
    fn nothing_moved(commands: &[Command], rects: &HashMap<u64, Rect>, ids: &[u64]) {
        let sent = places(commands);
        for id in ids {
            assert!(
                sent.iter().any(|(placed, _)| placed == id),
                "the reload did not lay out window {id}: {sent:?}"
            );
        }
        for (id, rect) in sent {
            let was = rects.get(&id).copied().unwrap_or_default();
            assert!(
                same(rect, was),
                "the reload moved window {id} from {was:?} to {rect:?}"
            );
        }
    }

    /// **#118: a reload leaves a tiled desk exactly as it was, dragged
    /// seam and all.**
    ///
    /// Three windows opened in order -- 2 beside 1 with the pointer on 1's
    /// right half, 3 under 2 with it on 2's lower half -- and then the seam
    /// between the two columns dragged left to x=900, a split nothing would
    /// give a window by itself. The reload's snapshot lists the windows as
    /// the compositor does, topmost first, and 1 is on top, having just
    /// been dragged: not the order they were opened in.
    ///
    /// Before #118 the tree died with the Lua state and `adopt` built
    /// another from that list: 1, then 3 beside it, then 2 under 1, every
    /// split at the configured 0.5.
    #[test]
    fn a_reload_leaves_a_tiled_desk_exactly_as_it_was() {
        let (mut scripts, entry) = session("");
        let mut rects = HashMap::new();
        press(&mut scripts, &mut rects, &[], "super+t", (1280.0, 720.0));
        open(&mut scripts, &mut rects, 1, &[], (1280.0, 720.0));
        open(&mut scripts, &mut rects, 2, &[1], (1900.0, 720.0));
        open(&mut scripts, &mut rects, 3, &[2, 1], (1900.0, 1100.0));
        let dragged = scripts.resized(
            1,
            (900.0, 720.0),
            (Some("right"), None),
            desk(&rects, &[1, 3, 2], (900.0, 720.0)),
        );
        rects.extend(placed(&dragged.commands));
        assert!(
            rects.get(&1).is_some_and(|rect| about(rect.w, 888.0))
                && rects
                    .get(&2)
                    .zip(rects.get(&3))
                    .is_some_and(|(two, three)| about(two.x, 912.0) && three.y > two.y),
            "the fixture is not the one described: {rects:?}"
        );
        let tree = arrangement(&scripts, "tiling", "1@DP-1");

        // A reload that fails goes on running the scripts it read the keep
        // from, so reading it copies.
        assert!(
            scripts
                .reload(&entry.with_file_name("missing.lua"))
                .is_err()
        );
        assert_eq!(
            arrangement(&scripts, "tiling", "1@DP-1"),
            tree,
            "reading the keep took the tree away from the running scripts"
        );
        let (reloaded, commands) =
            reload(scripts, &entry, &desk(&rects, &[1, 3, 2], (900.0, 720.0)));
        assert_eq!(
            arrangement(&reloaded, "tiling", "1@DP-1"),
            tree,
            "the reload came back with a different tree"
        );
        nothing_moved(&commands, &rects, &[1, 2, 3]);
    }

    /// **And so does a window manager started in place of one that stopped.**
    ///
    /// The compositor asks for what the window manager keeps with its
    /// stores, holds it as documents, and hands it to the next one in
    /// `hello`: the tree crosses a socket twice and comes back the tree it
    /// was, where a new window manager would otherwise build one from the
    /// window list, as a reload did before #118.
    #[test]
    fn a_window_manager_started_in_its_place_finds_the_tiled_desk_exactly_as_it_was() {
        let (mut scripts, entry) = session("");
        let mut rects = HashMap::new();
        press(&mut scripts, &mut rects, &[], "super+t", (1280.0, 720.0));
        open(&mut scripts, &mut rects, 1, &[], (1280.0, 720.0));
        open(&mut scripts, &mut rects, 2, &[1], (1900.0, 720.0));
        open(&mut scripts, &mut rects, 3, &[2, 1], (1900.0, 1100.0));
        let dragged = scripts.resized(
            1,
            (900.0, 720.0),
            (Some("right"), None),
            desk(&rects, &[1, 3, 2], (900.0, 720.0)),
        );
        rects.extend(placed(&dragged.commands));
        let tree = arrangement(&scripts, "tiling", "1@DP-1");
        let now = desk(&rects, &[1, 3, 2], (900.0, 720.0));

        let _ = scripts.save_stores(now.clone());
        let keeps = scripts.take_keeps();
        assert!(
            keeps.iter().any(|(name, _)| name == "tiling"),
            "the compositor was not sent the tiling keep: {:?}",
            keeps.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        // Unchanged, it is not sent again.
        let _ = scripts.save_stores(now.clone());
        assert!(scripts.take_keeps().is_empty());
        drop(scripts);

        let mut again = Scripts::start_again(&entry, false, keeps).expect("started again");
        let _ = again.startup();
        let mut commands = again.restored(now.clone()).commands;
        commands.extend(again.monitors_changed(now.clone()).commands);
        commands.extend(again.relayout(now).commands);
        assert_eq!(
            arrangement(&again, "tiling", "1@DP-1"),
            tree,
            "the window manager started in its place has a different tree"
        );
        nothing_moved(&commands, &rects, &[1, 2, 3]);
    }

    /// **#118 as it was reported: on workspace 3, Firefox and then kitty,
    /// a reload, and the two traded places.**
    ///
    /// Firefox takes the screen; kitty opens with the pointer on Firefox's
    /// right half and takes that half. kitty is the window in use, so it is
    /// topmost when the reload comes and the compositor lists it first.
    /// Rebuilt from that list, kitty went in first and took the screen, and
    /// Firefox split it with no pointer to go by -- onto the far side, the
    /// right.
    #[test]
    fn firefox_and_kitty_on_workspace_3_keep_their_places_through_a_reload() {
        let (firefox, kitty) = (1, 2);
        let (mut scripts, entry) = session("");
        let mut rects = HashMap::new();
        press(&mut scripts, &mut rects, &[], "super+t", (1280.0, 720.0));
        press(&mut scripts, &mut rects, &[], "super+3", (1280.0, 720.0));
        open(&mut scripts, &mut rects, firefox, &[], (1280.0, 720.0));
        open(&mut scripts, &mut rects, kitty, &[firefox], (1900.0, 720.0));
        assert!(
            rects
                .get(&firefox)
                .zip(rects.get(&kitty))
                .is_some_and(|(left, right)| left.x < right.x && about(left.w, right.w)),
            "the fixture is Firefox on the left and kitty on the right: {rects:?}"
        );

        let (scripts, commands) = reload(
            scripts,
            &entry,
            &desk(&rects, &[kitty, firefox], (1900.0, 720.0)),
        );
        nothing_moved(&commands, &rects, &[firefox, kitty]);
        let desks = scripts.evaluate(
            "local workspaces = require(\"workspaces\")\n\
             return string.format(\"%d %d %d\", workspaces.at(1, \"DP-1\"),\n\
                 workspaces.at(2, \"DP-1\"), workspaces.on(\"DP-1\"))",
        );
        assert_eq!(desks, "3 3 3", "Firefox, kitty, and the workspace in view");
    }

    /// **#129: a desk nobody is looking at keeps its arrangement through a
    /// reload, and through a `monitors` event**, and a window opened there
    /// afterwards splits it rather than covering the screen.
    ///
    /// Two windows side by side on workspace 1, and the view on workspace
    /// 2 when it happens. `adopt` runs on both -- `restore` switches tiling
    /// back on, and `monitors` follows on every reload as on every hotplug
    /// -- and it measured every tree against the windows on the desks *in
    /// view*. So workspace 1's tree lost both windows to a `monitors`
    /// event, and after a reload did not exist at all. Back on workspace 1
    /// the windows were still in their tiles, where the compositor had left
    /// them, and the layout did not know they were there: the next window
    /// took the whole screen, over the top of both.
    #[test]
    fn a_hidden_desk_keeps_its_arrangement_through_a_reload_and_a_monitors_event() {
        for route in ["a reload", "a monitors event"] {
            let (mut scripts, entry) = session("");
            let mut rects = HashMap::new();
            press(&mut scripts, &mut rects, &[], "super+t", (1280.0, 720.0));
            open(&mut scripts, &mut rects, 1, &[], (1280.0, 720.0));
            open(&mut scripts, &mut rects, 2, &[1], (1900.0, 720.0));
            let tree = arrangement(&scripts, "tiling", "1@DP-1");
            press(
                &mut scripts,
                &mut rects,
                &[2, 1],
                "super+2",
                (1280.0, 720.0),
            );

            let away = desk(&rects, &[2, 1], (1280.0, 720.0));
            let mut scripts = if route == "a reload" {
                reload(scripts, &entry, &away).0
            } else {
                let _ = scripts.monitors_changed(away.clone());
                let _ = scripts.relayout(away);
                scripts
            };
            press(
                &mut scripts,
                &mut rects,
                &[2, 1],
                "super+1",
                (1900.0, 1100.0),
            );
            assert_eq!(
                arrangement(&scripts, "tiling", "1@DP-1"),
                tree,
                "after {route} on workspace 2, workspace 1's arrangement is not the one it had"
            );

            let (left, right) = (
                rects.get(&1).copied().unwrap_or_default(),
                rects.get(&2).copied().unwrap_or_default(),
            );
            open(&mut scripts, &mut rects, 3, &[2, 1], (1900.0, 1100.0));
            let third = rects.get(&3).copied().unwrap_or_default();
            assert!(
                !same(third, FULL),
                "after {route}, window 3 was given the whole screen, over windows 1 and 2"
            );
            let now: Vec<Rect> = [1, 2, 3]
                .iter()
                .filter_map(|id| rects.get(id).copied())
                .collect();
            assert!(
                rects.get(&1).is_some_and(|rect| same(*rect, left))
                    && [2, 3].iter().all(|id| {
                        rects.get(id).is_some_and(|rect| {
                            rect.x >= right.x - 1.0 && rect.x + rect.w <= right.x + right.w + 1.0
                        })
                    })
                    && now.iter().enumerate().all(|(index, one)| {
                        now.iter()
                            .skip(index + 1)
                            .all(|other| !overlap(*one, *other))
                    }),
                "after {route}, window 3 did not split window 2's tile: {rects:?}"
            );
        }
    }

    /// **A reload leaves every scrolling strip as it was, in view or not**:
    /// which windows share a column, the columns' order and widths, which
    /// one is focused and where the view sits. And so does a `monitors`
    /// event, which every reload also announces.
    ///
    /// Workspace 1: three columns, the third widened with `super+r` and
    /// carried to the middle, then focus back on the first, which scrolls
    /// the view. Workspace 2, in view throughout: two windows stacked in
    /// one column, widened. Before, `monitors` emptied the strip out of
    /// view (#129); and a reload rebuilt each strip in view from the
    /// snapshot -- a column per window, in stacking order, at the width a
    /// column opens at -- and a strip out of view not at all (#118).
    #[test]
    fn a_reload_leaves_every_scrolling_strip_as_it_was() {
        let cursor = (1280.0, 720.0);
        let (mut scripts, entry) = session("");
        let mut rects = HashMap::new();
        press(&mut scripts, &mut rects, &[], "super+s", cursor);
        open(&mut scripts, &mut rects, 1, &[], cursor);
        open(&mut scripts, &mut rects, 2, &[1], cursor);
        open(&mut scripts, &mut rects, 3, &[2, 1], cursor);
        let first = [3, 2, 1];
        press(&mut scripts, &mut rects, &first, "super+r", cursor);
        press(
            &mut scripts,
            &mut rects,
            &first,
            "super+ctrl+bracketleft",
            cursor,
        );
        press(
            &mut scripts,
            &mut rects,
            &first,
            "super+bracketleft",
            cursor,
        );
        let hidden = arrangement(&scripts, "scrolling", "1@DP-1");
        assert!(
            hidden.ends_with("focus=1")
                && rects
                    .get(&3)
                    .zip(rects.get(&2))
                    .is_some_and(|(three, two)| three.x < two.x && three.w > two.w),
            "the fixture is [1] [3, widened] [2] with 1 focused: {hidden}"
        );

        press(&mut scripts, &mut rects, &first, "super+2", cursor);
        open(&mut scripts, &mut rects, 4, &[3, 2, 1], cursor);
        open(&mut scripts, &mut rects, 5, &[4, 3, 2, 1], cursor);
        let all = [5, 4, 3, 2, 1];
        press(&mut scripts, &mut rects, &all, "super+bracketleft", cursor);
        press(&mut scripts, &mut rects, &all, "super+comma", cursor);
        press(&mut scripts, &mut rects, &all, "super+r", cursor);
        let shown = arrangement(&scripts, "scrolling", "2@DP-1");
        assert!(
            rects
                .get(&4)
                .zip(rects.get(&5))
                .is_some_and(|(four, five)| about(four.x, five.x)
                    && about(four.w, 1268.0)
                    && five.y > four.y),
            "the fixture is 4 over 5 in one column, half the view wide: {rects:?}"
        );

        let now = desk(&rects, &all, cursor);
        let _ = scripts.monitors_changed(now.clone());
        let _ = scripts.relayout(now.clone());
        assert_eq!(
            [
                arrangement(&scripts, "scrolling", "1@DP-1"),
                arrangement(&scripts, "scrolling", "2@DP-1")
            ],
            [hidden.clone(), shown.clone()],
            "a monitors event changed a strip, out of view or in it"
        );

        let (scripts, commands) = reload(scripts, &entry, &now);
        assert_eq!(
            arrangement(&scripts, "scrolling", "2@DP-1"),
            shown,
            "the strip in view came back different"
        );
        assert_eq!(
            arrangement(&scripts, "scrolling", "1@DP-1"),
            hidden,
            "the strip out of view came back different"
        );
        nothing_moved(&commands, &rects, &[4, 5]);
    }

    /// **And a reload still changes the configuration**: a strip keeps its
    /// columns, and takes the widths the file now names.
    ///
    /// `scrolling.widths` is read where a strip is made (#117), and a strip
    /// that is carried across a reload is not made again. Left at that, it
    /// would go on opening columns at, and cycling through, the widths the
    /// *last* file named: an edited setting that nothing reads. So the
    /// columns already there keep the widths they have, a new one opens at
    /// the new list's first, and `super+r` walks the new list.
    #[test]
    fn a_reload_keeps_the_strip_and_takes_the_widths_the_file_now_names() {
        let cursor = (1280.0, 720.0);
        let (mut scripts, entry) = session("");
        let mut rects = HashMap::new();
        press(&mut scripts, &mut rects, &[], "super+s", cursor);
        open(&mut scripts, &mut rects, 1, &[], cursor);
        open(&mut scripts, &mut rects, 2, &[1], cursor);
        press(&mut scripts, &mut rects, &[2, 1], "super+r", cursor);
        let before = arrangement(&scripts, "scrolling", "1@DP-1");
        assert!(
            rects
                .get(&1)
                .is_some_and(|rect| about(rect.w, 2536.0 / 3.0))
                && rects.get(&2).is_some_and(|rect| about(rect.w, 1268.0)),
            "the fixture is a third and a half: {rects:?}"
        );

        write(
            &entry,
            "require(\"config\").scrolling.widths = { 0.25, 0.75 }",
        );
        let (mut scripts, _) = reload(scripts, &entry, &desk(&rects, &[2, 1], cursor));
        assert_eq!(
            arrangement(&scripts, "scrolling", "1@DP-1"),
            before,
            "the columns did not keep their widths through the reload"
        );
        open(&mut scripts, &mut rects, 3, &[2, 1], cursor);
        assert!(
            rects.get(&3).is_some_and(|rect| about(rect.w, 634.0)),
            "a column opened after the reload is not the new first width, a quarter: {rects:?}"
        );
        press(&mut scripts, &mut rects, &[3, 2, 1], "super+r", cursor);
        assert!(
            rects.get(&3).is_some_and(|rect| about(rect.w, 1902.0)),
            "super+r after the reload did not go to the new list's next width: {rects:?}"
        );
    }

    /// **A monitor that goes away still hands its windows to the one that
    /// is left** -- the job the `monitors` event was added for.
    ///
    /// `DP-2` holds 3 and 4 on the workspace it shows and 5 on its
    /// workspace 2, out of view. It is unplugged: every window is on `DP-1`
    /// now, and `monitors` is announced. 3 and 4 join workspace 1's tree
    /// there beside 1 and 2, and are laid out on `DP-1`. 5 joins workspace
    /// 2's -- the workspace it was on, on the screen it is on now -- where
    /// before #129 it was taken out of `DP-2`'s tree and put into none,
    /// because `adopt` only put back a window on a desk in view. No tree of
    /// the screen that went holds anything.
    #[test]
    fn a_monitor_that_goes_away_still_hands_its_windows_on() {
        let (mut scripts, _) = session("");
        let both = || vec![monitor(), second_monitor()];
        let two = |rects: &HashMap<u64, Rect>, ids: &[u64], cursor: (f64, f64)| Snapshot {
            cursor,
            ..snapshot_on(
                both(),
                ids.iter()
                    .map(|&id| {
                        let each = at_rest(rects, id);
                        if id >= 3 && !rects.contains_key(&id) {
                            on(&second_monitor(), each)
                        } else if id >= 3 {
                            WindowInfo {
                                icon: None,
                                client_side: false,
                                blur_region: false,
                                insets: crate::decoration::Insets::default(),
                                monitor: "DP-2".to_owned(),
                                ..each
                            }
                        } else {
                            each
                        }
                    })
                    .collect(),
            )
        };
        let mut rects: HashMap<u64, Rect> = HashMap::new();
        let outcome = scripts.key("super+t", two(&rects, &[], (1280.0, 720.0)));
        assert!(outcome.handled, "the tiling key was not handled");
        let opening = |scripts: &mut Scripts,
                       rects: &mut HashMap<u64, Rect>,
                       id: u64,
                       below: &[u64],
                       cursor: (f64, f64)| {
            let mut ids = vec![id];
            ids.extend_from_slice(below);
            let outcome = scripts.opened(id, two(rects, &ids, cursor));
            rects.extend(placed(&outcome.commands));
        };
        opening(&mut scripts, &mut rects, 1, &[], (1280.0, 720.0));
        opening(&mut scripts, &mut rects, 2, &[1], (1900.0, 720.0));
        opening(&mut scripts, &mut rects, 3, &[2, 1], (3840.0, 720.0));
        opening(&mut scripts, &mut rects, 4, &[3, 2, 1], (4460.0, 720.0));
        let _ = scripts.evaluate_in(
            two(&rects, &[4, 3, 2, 1], (4460.0, 720.0)),
            "require(\"workspaces\").go(2, \"DP-2\") return \"\"",
        );
        opening(&mut scripts, &mut rects, 5, &[4, 3, 2, 1], (3840.0, 720.0));
        let _ = scripts.evaluate_in(
            two(&rects, &[5, 4, 3, 2, 1], (3840.0, 720.0)),
            "require(\"workspaces\").go(1, \"DP-2\") return \"\"",
        );
        assert_eq!(
            [
                held(&scripts, "1@DP-1"),
                held(&scripts, "1@DP-2"),
                held(&scripts, "2@DP-2")
            ],
            ["1,2", "3,4", "5"],
            "the fixture is two screens, and 5 on DP-2's workspace 2"
        );

        // Unplugged: the compositor has already moved every window onto
        // the screen that is left, by the width of the one that went.
        let gone = snapshot(
            [5, 4, 3, 2, 1]
                .iter()
                .map(|&id| {
                    let mut each = at_rest(&rects, id);
                    if id >= 3 {
                        each.rect.x -= BESIDE.x;
                    }
                    each
                })
                .collect(),
        );
        let _ = scripts.monitors_changed(gone.clone());
        let after = placed(&scripts.relayout(gone).commands);
        let shown: Vec<Rect> = [1, 2, 3, 4]
            .iter()
            .filter_map(|id| after.get(id).copied())
            .collect();
        assert!(
            shown.len() == 4
                && shown.iter().all(|rect| rect.x + rect.w <= AREA.w + 1.0)
                && shown.iter().enumerate().all(|(index, one)| {
                    shown
                        .iter()
                        .skip(index + 1)
                        .all(|other| !overlap(*one, *other))
                }),
            "the windows on the workspace in view are not all laid out on DP-1: {after:?}"
        );
        assert_eq!(held(&scripts, "1@DP-1"), "1,2,3,4");
        assert_eq!(
            held(&scripts, "2@DP-1"),
            "5",
            "the window on DP-2's hidden workspace is in no tree of the screen it is on"
        );
        assert_eq!(
            [held(&scripts, "1@DP-2"), held(&scripts, "2@DP-2")],
            ["", ""],
            "a tree of the screen that went still holds a window"
        );
    }
}
