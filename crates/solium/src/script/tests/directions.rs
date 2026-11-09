//! Focus and move by direction (#150), against the shipped scripts.
//!
//! A compositor small enough to read: each window's rect and which one has the
//! keyboard. `sol.place` moves a window, and its monitor with it by where its
//! centre lands, as `Solium::output_of` decides; `sol.focus` moves the keyboard
//! and raises the window. Every press is dispatched from the desk as the last
//! one left it, which is what the compositor hands a script.

use super::{
    AnimationSpec, Command, Drawn, FocusSource, Interrupt, MonitorInfo, Parentage, Rect, Scripts,
    Snapshot, Warp, WarpTo, WindowInfo,
};
use crate::present::Curve;
use std::path::PathBuf;

/// A landscape screen at the origin.
const WIDE: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 2560.0,
    h: 1440.0,
};

/// A portrait screen to its right, so a column beside it is taller than
/// it is wide and splits one above the other.
const TALL: Rect = Rect {
    x: 2560.0,
    y: 0.0,
    w: 1440.0,
    h: 2560.0,
};

fn screen(name: &str, area: Rect) -> MonitorInfo {
    MonitorInfo {
        name: name.to_owned(),
        area,
        whole: area,
        scale: 1.0,
        focused: name == "DP-1",
        primary: name == "DP-1",
        transform: "normal".to_owned(),
        off: false,
    }
}

fn one_screen() -> Vec<MonitorInfo> {
    vec![screen("DP-1", WIDE)]
}

fn two_screens() -> Vec<MonitorInfo> {
    vec![screen("DP-1", WIDE), screen("DP-2", TALL)]
}

/// A smaller landscape screen top-aligned to the right of `WIDE`: a
/// laptop's panel beside a monitor, the shape where the one reaches
/// lower than the other and neither is above it.
const BESIDE: Rect = Rect {
    x: 2560.0,
    y: 0.0,
    w: 1920.0,
    h: 1080.0,
};

/// A screen touching `WIDE` only at its bottom-right corner.
const CORNER: Rect = Rect {
    x: 2560.0,
    y: 1440.0,
    w: 1920.0,
    h: 1080.0,
};

/// A smaller landscape screen whose right edge meets `TALL`'s left, so
/// the bottom half of the portrait screen is level with nothing on it.
const LEFT_OF_TALL: Rect = Rect {
    x: 640.0,
    y: 0.0,
    w: 1920.0,
    h: 1080.0,
};

/// A screen under `BESIDE`, so the two stand stacked to the right of
/// `WIDE`: the upper level with its top, this one with its bottom.
const UNDER_BESIDE: Rect = Rect {
    x: 2560.0,
    y: 1080.0,
    w: 1920.0,
    h: 1080.0,
};

/// A portrait screen with `ABOVE_LEFT` and `BELOW_LEFT` stacked on its
/// left, the upper level with its top half and the lower with its bottom.
const PORTRAIT: Rect = Rect {
    x: 1920.0,
    y: 0.0,
    w: 1440.0,
    h: 2560.0,
};

const ABOVE_LEFT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1920.0,
    h: 1080.0,
};

const BELOW_LEFT: Rect = Rect {
    x: 0.0,
    y: 1080.0,
    w: 1920.0,
    h: 1080.0,
};

fn window(id: u64, monitor: &str, rect: Rect) -> WindowInfo {
    WindowInfo {
        icon: None,
        client_side: false,
        blur_region: false,
        insets: crate::decoration::Insets::default(),
        id,
        rect,
        drawn: Drawn::default(),
        title: format!("window {id}"),
        focused: false,
        monitor: monitor.to_owned(),
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

fn about(left: f64, right: f64) -> bool {
    (left - right).abs() < 1.0
}

fn same(left: Rect, right: Rect) -> bool {
    about(left.x, right.x)
        && about(left.y, right.y)
        && about(left.w, right.w)
        && about(left.h, right.h)
}

fn inside(rect: Rect, area: Rect) -> bool {
    rect.x >= area.x - 1.0
        && rect.y >= area.y - 1.0
        && rect.x + rect.w <= area.x + area.w + 1.0
        && rect.y + rect.h <= area.y + area.h + 1.0
}

/// The window ids `commands` handed the keyboard to, in order.
fn focuses(commands: &[Command]) -> Vec<u64> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Focus { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

/// The window ids `commands` placed.
fn places(commands: &[Command]) -> Vec<u64> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Place { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

struct Desk {
    scripts: Scripts,
    entry: PathBuf,
    monitors: Vec<MonitorInfo>,
    windows: Vec<WindowInfo>,
    cursor: (f64, f64),
}

impl Desk {
    /// The shipped scripts in the order `init.lua` requires them, with
    /// `before` run ahead of them, on these screens and no windows.
    /// `package.path` is the entry's own and the directory is one per call,
    /// for the reasons `dialogs::scripts` gives.
    fn new(before: &str, monitors: Vec<MonitorInfo>) -> Self {
        Self::new_with(before, "", monitors)
    }

    /// As `new`, with `after` run once every layout script this harness
    /// loads has been required -- the one hook a test needing
    /// `require("bindings")` has, since the real `init.lua` requires it
    /// *last* (`bindings.lua`'s own comment) and this harness otherwise
    /// has no module after `floating` at all.
    /// `a_config_bindings_override_on_an_arrow_survives_leaving_and_returning_to_floating`.
    fn new_with(before: &str, after: &str, monitors: Vec<MonitorInfo>) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("solium-directions-{}-{serial}", std::process::id()));
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
                 require(\"scrolling\")\n\
                 require(\"direction\")\n\
                 require(\"floating\")\n\
                 {after}\n",
                shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
            ),
        )
        .expect("writing the entry point");
        let scripts = Scripts::load(&entry).expect("loading the shipped layouts");
        Self {
            scripts,
            entry,
            monitors,
            windows: Vec::new(),
            cursor: (0.0, 0.0),
        }
    }

    /// Floating windows where `windows` says, and no layout in charge.
    fn floating(monitors: Vec<MonitorInfo>, windows: &[(u64, &str, Rect)]) -> Self {
        let mut desk = Self::new("", monitors);
        for &(id, monitor, rect) in windows {
            desk.windows.push(window(id, monitor, rect));
        }
        desk
    }

    fn snapshot(&self) -> Snapshot {
        let work_area = self
            .monitors
            .iter()
            .find(|monitor| monitor.focused)
            .map_or(WIDE, |monitor| monitor.area);
        Snapshot {
            windows: self.windows.clone(),
            monitors: self.monitors.clone(),
            work_area,
            cursor: self.cursor,
            ..Snapshot::default()
        }
    }

    fn apply(&mut self, commands: &[Command]) {
        for command in commands {
            match command {
                Command::Place { id, rect, .. } => {
                    let (x, y) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                    let covering = self
                        .monitors
                        .iter()
                        .find(|monitor| {
                            let whole = monitor.whole;
                            x >= whole.x
                                && x < whole.x + whole.w
                                && y >= whole.y
                                && y < whole.y + whole.h
                        })
                        .map(|monitor| monitor.name.clone());
                    if let Some(window) = self.windows.iter_mut().find(|window| window.id == *id) {
                        window.rect = *rect;
                        if let Some(name) = covering {
                            window.monitor = name;
                        }
                    }
                }
                Command::Focus { id, .. } => self.focus(*id),
                _ => {}
            }
        }
    }

    /// Press a bound key, and apply what it asked for.
    fn press(&mut self, combo: &str) -> Vec<Command> {
        let outcome = self.scripts.key(combo, self.snapshot());
        assert!(
            outcome.handled,
            "{combo} is not bound, or its handler failed"
        );
        self.apply(&outcome.commands);
        outcome.commands
    }

    /// Window `id` opens on `monitor` with the pointer at `cursor`, and has
    /// the keyboard once it is placed, as a new window does.
    fn open(&mut self, id: u64, monitor: &str, cursor: (f64, f64)) {
        let origin = self
            .monitors
            .iter()
            .find(|each| each.name == monitor)
            .map_or(WIDE, |each| each.area);
        self.windows.insert(
            0,
            window(
                id,
                monitor,
                Rect {
                    w: 800.0,
                    h: 600.0,
                    ..origin
                },
            ),
        );
        self.cursor = cursor;
        let outcome = self.scripts.opened(id, self.snapshot());
        self.apply(&outcome.commands);
        self.focus(id);
    }

    /// The keyboard to window `id`, which comes to the front.
    fn focus(&mut self, id: u64) {
        for window in &mut self.windows {
            window.focused = window.id == id;
        }
        if let Some(at) = self.windows.iter().position(|window| window.id == id) {
            let raised = self.windows.remove(at);
            self.windows.insert(0, raised);
        }
    }

    fn rect(&self, id: u64) -> Rect {
        self.windows
            .iter()
            .find(|window| window.id == id)
            .map(|window| window.rect)
            .unwrap_or_default()
    }

    fn monitor(&self, id: u64) -> String {
        self.windows
            .iter()
            .find(|window| window.id == id)
            .map(|window| window.monitor.clone())
            .unwrap_or_default()
    }

    fn rects(&self) -> Vec<(u64, Rect)> {
        let mut out: Vec<(u64, Rect)> = self
            .windows
            .iter()
            .map(|window| (window.id, window.rect))
            .collect();
        out.sort_by_key(|(id, _)| *id);
        out
    }

    fn workspace_of(&self, id: u64) -> String {
        self.scripts.evaluate(&format!(
            "return tostring(require(\"workspaces\").of[{id}])"
        ))
    }

    /// The desks whose arrangement in `module` holds window `id`, by the
    /// key each is kept under in its `field`, sorted and joined by commas:
    /// `holding("tiling", "trees", 4)` is the trees window 4 is a leaf of.
    fn holding(&self, module: &str, field: &str, id: u64) -> String {
        self.scripts.evaluate(&format!(
            "local out = {{}}\n\
             for key, each in pairs(require(\"{module}\").{field}) do\n\
                 if each:contains({id}) then out[#out + 1] = key end\n\
             end\n\
             table.sort(out)\n\
             return table.concat(out, \",\")"
        ))
    }

    /// Window `id` dragged until its centre is at `at`, on `monitor`, and
    /// let go there with the pointer on it: the `drop` a move grab ends
    /// with, and what it asked for applied.
    fn drop(&mut self, id: u64, monitor: &str, at: (f64, f64)) {
        if let Some(window) = self.windows.iter_mut().find(|window| window.id == id) {
            window.rect.x = at.0 - window.rect.w / 2.0;
            window.rect.y = at.1 - window.rect.h / 2.0;
            window.monitor = monitor.to_owned();
        }
        self.cursor = at;
        let outcome = self.scripts.dropped(id, at.0, at.1, self.snapshot());
        self.apply(&outcome.commands);
    }

    /// What `Solium::reload` does, with the desk as it is.
    fn reload(&mut self) {
        self.scripts
            .reload(&self.entry)
            .expect("reloading the scripts");
        let mut commands = self.scripts.restored(self.snapshot()).commands;
        commands.extend(self.scripts.monitors_changed(self.snapshot()).commands);
        commands.extend(self.scripts.relayout(self.snapshot()).commands);
        self.apply(&commands);
    }
}

/// Tiling on, and four windows opened into a two-by-two grid on `DP-1`:
///
/// ```text
/// 1 | 2
/// --+--
/// 3 | 4
/// ```
///
/// Each opened with the pointer where it splits the tile it lands in the
/// way the picture says, and the picture checked, so a test reading
/// directions off it cannot pass on some other arrangement.
fn grid(before: &str, monitors: Vec<MonitorInfo>) -> Desk {
    let mut desk = Desk::new(before, monitors);
    desk.press("super+t");
    desk.open(1, "DP-1", (1280.0, 720.0));
    desk.open(2, "DP-1", (2000.0, 720.0));
    desk.open(3, "DP-1", (640.0, 1200.0));
    desk.open(4, "DP-1", (1920.0, 1200.0));
    let [one, two, three, four] = [1, 2, 3, 4].map(|id| desk.rect(id));
    assert!(
        one.x + one.w < two.x
            && three.x + three.w < four.x
            && one.y + one.h < three.y
            && two.y + two.h < four.y
            && about(one.x, three.x)
            && about(one.y, two.y),
        "the premise: a two-by-two grid, 1 2 over 3 4; it is {:?}",
        desk.rects()
    );
    desk
}

/// **In tiling, focus and move reach the neighbouring tile in each
/// direction, on the arrows and on h, j, k and l.** #150.
///
/// Every focus key from the corner that has a neighbour that way; every
/// move key trades the window with that neighbour, and the opposite move
/// puts both back exactly. At the edge of the only monitor nothing
/// happens: no focus, and nothing placed anywhere else.
#[test]
fn in_tiling_focus_and_move_reach_the_neighbour_in_each_direction() {
    let mut desk = grid("", one_screen());
    for (from, key, to) in [
        (1, "super+right", 2),
        (1, "super+down", 3),
        (4, "super+left", 3),
        (4, "super+up", 2),
        (2, "super+h", 1),
        (2, "super+j", 4),
        (3, "super+k", 1),
        (3, "super+l", 4),
    ] {
        desk.focus(from);
        let commands = desk.press(key);
        assert_eq!(focuses(&commands), [to], "{key} from window {from}");
    }

    let start = desk.rects();
    for (from, key, with, back) in [
        (1, "super+shift+right", 2, "super+shift+h"),
        (1, "super+shift+down", 3, "super+alt+k"),
        (4, "super+shift+left", 3, "super+shift+l"),
        (4, "super+shift+up", 2, "super+shift+j"),
    ] {
        desk.focus(from);
        let (was, other) = (desk.rect(from), desk.rect(with));
        desk.press(key);
        assert!(
            same(desk.rect(from), other) && same(desk.rect(with), was),
            "{key} from window {from} did not trade tiles with window {with}: {:?}",
            desk.rects()
        );
        let commands = desk.press(back);
        assert!(
            focuses(&commands).is_empty(),
            "a move does not move the keyboard"
        );
        let now = desk.rects();
        assert!(
            start
                .iter()
                .zip(&now)
                .all(|((_, before), (_, after))| same(*before, *after)),
            "{back} after {key} is not the desk it started as: {start:?} and now {now:?}"
        );
    }

    for (from, key) in [
        (1, "super+left"),
        (1, "super+up"),
        (4, "super+right"),
        (4, "super+down"),
    ] {
        desk.focus(from);
        assert!(
            focuses(&desk.press(key)).is_empty(),
            "{key} from window {from}, at the edge of the only monitor"
        );
    }
    for (from, key) in [(1, "super+shift+left"), (4, "super+shift+down")] {
        desk.focus(from);
        desk.press(key);
        let now = desk.rects();
        assert!(
            start
                .iter()
                .zip(&now)
                .all(|((_, before), (_, after))| same(*before, *after)),
            "{key} from window {from} at the edge of the only monitor moved something: {now:?}"
        );
    }
}

/// **At a monitor's edge, tiling goes on to the next monitor, and the desk
/// it is showing.** #150.
///
/// The grid on the landscape screen; on the portrait one to its right,
/// showing workspace 2, window 5 above window 6. Focus crosses both ways
/// to the tile level with the window it left, the nearer when two are.
/// A move crosses too: window 2 leaves the grid, whose other windows close
/// up, and goes in beside window 5 on the side it arrived from, on
/// workspace 2. Moved back, it splits the tile it arrives at, level with
/// where it was -- which is the grid again. Into a monitor with nothing on
/// it, a window takes the whole screen.
#[test]
fn at_a_monitors_edge_tiling_crosses_to_the_desk_the_next_monitor_shows() {
    let mut desk = grid("", two_screens());
    let start = desk.rects();
    assert_eq!(
        desk.scripts
            .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 2 return \"\""),
        ""
    );
    desk.open(5, "DP-2", (3280.0, 1280.0));
    desk.open(6, "DP-2", (3280.0, 2400.0));
    let (five, six) = (desk.rect(5), desk.rect(6));
    assert!(
        five.y + five.h < six.y && inside(five, TALL) && inside(six, TALL),
        "the premise: 5 above 6 on the portrait screen: {:?}",
        desk.rects()
    );
    assert_eq!(
        desk.workspace_of(5),
        "2",
        "the premise: DP-2 is showing workspace 2"
    );

    for (from, key, to) in [
        (2, "super+right", 5),
        (4, "super+right", 5),
        (6, "super+left", 4),
        (5, "super+left", 2),
    ] {
        desk.focus(from);
        assert_eq!(focuses(&desk.press(key)), [to], "{key} from window {from}");
    }

    desk.focus(2);
    desk.press("super+shift+right");
    let (two, beside, four) = (desk.rect(2), desk.rect(5), desk.rect(4));
    assert_eq!(
        desk.monitor(2),
        "DP-2",
        "window 2 did not cross: {:?}",
        desk.rects()
    );
    assert!(
        inside(two, TALL) && two.x + two.w < beside.x && two.y < beside.y + beside.h,
        "window 2 did not go in on the left of window 5: {:?}",
        desk.rects()
    );
    assert!(same(desk.rect(6), six), "window 6 was not beside the move");
    assert!(
        four.y < WIDE.y + 20.0 && four.h > WIDE.h - 40.0,
        "the grid did not close up over window 2's tile: {:?}",
        desk.rects()
    );
    assert_eq!(
        desk.workspace_of(2),
        "2",
        "window 2 is on the desk DP-2 shows"
    );

    desk.press("super+shift+left");
    assert_eq!(
        desk.monitor(2),
        "DP-1",
        "window 2 did not come back: {:?}",
        desk.rects()
    );
    let now = desk.rects();
    assert!(
        start.iter().all(|(id, was)| now
            .iter()
            .any(|(other, rect)| other == id && same(*rect, *was))),
        "window 2 moved back is not the grid again: {start:?} and now {now:?}"
    );
    assert!(
        same(desk.rect(5), five),
        "window 5 did not take its tile back"
    );
    assert_eq!(desk.workspace_of(2), "1");

    let mut desk = Desk::new("", two_screens());
    desk.press("super+t");
    desk.open(1, "DP-1", (1280.0, 720.0));
    desk.open(2, "DP-1", (2000.0, 720.0));
    desk.press("super+shift+right");
    let whole = |area: Rect| Rect {
        x: area.x + 12.0,
        y: area.y + 12.0,
        w: area.w - 24.0,
        h: area.h - 24.0,
    };
    assert!(
        same(desk.rect(2), whole(TALL)) && same(desk.rect(1), whole(WIDE)),
        "a window moved onto a monitor with nothing on it takes the screen, and the \
         one it left takes the other: {:?}",
        desk.rects()
    );
}

/// **With `tiling.move = "split"`, a move goes into the neighbour's split
/// instead** (#150): window 4 moved left leaves the bottom-right tile to
/// window 2 and splits window 3's, on the far side, where a swap would
/// have traded the two. And a value that is neither is named once, and
/// swaps.
#[test]
fn with_tiling_move_split_a_move_goes_into_the_neighbours_split() {
    let mut desk = grid("require(\"config\").tiling.move = \"split\"", one_screen());
    let three = desk.rect(3);
    desk.focus(4);
    desk.press("super+shift+left");
    let (two, now_three, four) = (desk.rect(2), desk.rect(3), desk.rect(4));
    assert!(
        two.h > WIDE.h - 40.0,
        "window 2 did not take the right-hand column: {:?}",
        desk.rects()
    );
    assert!(
        inside(four, three) && inside(now_three, three) && four.x + four.w < now_three.x,
        "window 4 did not go into window 3's tile on its left: {:?}",
        desk.rects()
    );

    let logged = "_G.logged = {}\n\
                  local log = sol.log\n\
                  sol.log = function(message) _G.logged[#_G.logged + 1] = message log(message) end\n\
                  require(\"config\").tiling.move = \"sideways\"";
    let mut desk = grid(logged, one_screen());
    let (three, four) = (desk.rect(3), desk.rect(4));
    desk.focus(4);
    desk.press("super+shift+left");
    desk.press("super+shift+right");
    desk.press("super+shift+left");
    assert!(
        same(desk.rect(4), three) && same(desk.rect(3), four),
        "an unknown tiling.move did not swap: {:?}",
        desk.rects()
    );
    let said = desk
        .scripts
        .evaluate("return table.concat(_G.logged, \"\\n\")");
    assert_eq!(
        said.matches("tiling.move is sideways").count(),
        1,
        "an unknown tiling.move is named once, not per move: {said:?}"
    );
}

/// The whole of `area` but the shipped gap round it: the tile a window
/// alone on a screen is given.
fn whole(area: Rect) -> Rect {
    Rect {
        x: area.x + 12.0,
        y: area.y + 12.0,
        w: area.w - 24.0,
        h: area.h - 24.0,
    }
}

/// Whether `left` and `right` share any pixel.
fn overlap(left: Rect, right: Rect) -> bool {
    left.x < right.x + right.w
        && right.x < left.x + left.w
        && left.y < right.y + right.h
        && right.y < left.y + left.h
}

/// **A window sent to another workspace leaves one grid closed up and is a
/// tile of the other** (#400), on one monitor and on two.
///
/// Window 4 sent from the grid to workspace 2: window 2 takes the right
/// column, and 4 is the whole of workspace 2, a leaf of that desk's tree
/// and of no other. Window 3 sent after it shares workspace 2 with it.
/// With a second monitor, a send from it leaves the first's grid alone
/// and its own desk closes up the same way.
#[test]
fn a_window_sent_to_another_workspace_leaves_one_grid_closed_up_and_is_a_tile_of_the_other() {
    let mut desk = grid("", one_screen());
    desk.focus(4);
    desk.press("super+shift+2");
    assert_eq!(desk.workspace_of(4), "2");
    assert_eq!(desk.holding("tiling", "trees", 4), "2@DP-1");
    let (one, two) = (desk.rect(1), desk.rect(2));
    assert!(
        about(two.y, one.y) && two.h > WIDE.h - 40.0,
        "workspace 1's grid did not close up over window 4's tile: {:?}",
        desk.rects()
    );
    assert!(
        same(desk.rect(4), whole(WIDE)),
        "window 4 is not the whole of workspace 2: {:?}",
        desk.rects()
    );

    desk.focus(3);
    desk.press("super+shift+2");
    let (three, four) = (desk.rect(3), desk.rect(4));
    assert_eq!(desk.holding("tiling", "trees", 3), "2@DP-1");
    assert!(
        inside(three, WIDE)
            && inside(four, WIDE)
            && !overlap(three, four)
            && three.w * three.h + four.w * four.h > 0.9 * WIDE.w * WIDE.h,
        "windows 3 and 4 do not share workspace 2 between them: {:?}",
        desk.rects()
    );
    assert!(
        desk.rect(1).h > WIDE.h - 40.0,
        "workspace 1's grid did not close up over window 3's tile: {:?}",
        desk.rects()
    );

    let mut desk = grid("", two_screens());
    desk.open(5, "DP-2", (3280.0, 1280.0));
    desk.open(6, "DP-2", (3280.0, 2400.0));
    let start: Vec<Rect> = [1, 2, 3, 4].map(|id| desk.rect(id)).to_vec();
    desk.focus(6);
    desk.press("super+shift+3");
    assert_eq!(desk.workspace_of(6), "3");
    assert_eq!(desk.holding("tiling", "trees", 6), "3@DP-2");
    assert_eq!(
        desk.monitor(6),
        "DP-2",
        "a send keeps the window on its monitor"
    );
    assert!(
        same(desk.rect(5), whole(TALL)) && same(desk.rect(6), whole(TALL)),
        "DP-2's desk did not close up, or window 6 is not the whole of workspace 3: {:?}",
        desk.rects()
    );
    assert!(
        [1, 2, 3, 4]
            .iter()
            .zip(&start)
            .all(|(id, was)| same(desk.rect(*id), *was)),
        "a send on DP-2 moved DP-1's grid: {:?}",
        desk.rects()
    );
}

/// **A window dropped on another workspace belongs to it, and its old
/// grid closes up** (#401): across monitors showing different
/// workspaces, and on one monitor switched to another workspace mid-drag.
/// Either way the workspace it belongs to is the one whose tree holds it.
#[test]
fn a_window_dropped_on_another_workspace_belongs_to_it_and_leaves_its_old_grid_closed_up() {
    let mut desk = grid("", two_screens());
    assert_eq!(
        desk.scripts
            .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 3 return \"\""),
        ""
    );
    desk.drop(2, "DP-2", (3280.0, 1280.0));
    assert_eq!(desk.workspace_of(2), "3");
    assert_eq!(desk.holding("tiling", "trees", 2), "3@DP-2");
    assert!(
        same(desk.rect(2), whole(TALL)),
        "window 2 is not the whole of the desk it was dropped on: {:?}",
        desk.rects()
    );
    let four = desk.rect(4);
    assert!(
        about(four.y, whole(WIDE).y) && four.h > WIDE.h - 40.0,
        "the grid did not close up over window 2's tile: {:?}",
        desk.rects()
    );

    let mut desk = grid("", one_screen());
    let three = desk.rect(3);
    desk.press("super+2");
    desk.drop(1, "DP-1", (900.0, 500.0));
    assert_eq!(desk.workspace_of(1), "2");
    assert_eq!(desk.holding("tiling", "trees", 1), "2@DP-1");
    assert!(
        same(desk.rect(1), whole(WIDE)),
        "window 1 is not the whole of the workspace it was dropped on: {:?}",
        desk.rects()
    );
    let now = desk.rect(3);
    assert!(
        about(now.x, three.x) && about(now.y, whole(WIDE).y) && now.h > WIDE.h - 40.0,
        "workspace 1's grid, out of view, did not close up over window 1's tile: {:?}",
        desk.rects()
    );
}

/// **A window sent from a scrolling strip leaves it for the other
/// workspace's** (#400): a column of its own there, and in no other strip.
#[test]
fn a_window_sent_from_a_scrolling_strip_leaves_it_for_the_other_workspaces() {
    let mut desk = Desk::new("", one_screen());
    desk.press("super+s");
    desk.open(1, "DP-1", (1280.0, 720.0));
    desk.open(2, "DP-1", (1280.0, 720.0));
    desk.focus(2);
    desk.press("super+shift+2");
    assert_eq!(desk.workspace_of(2), "2");
    assert_eq!(desk.holding("scrolling", "views", 2), "2@DP-1");
    assert_eq!(desk.holding("scrolling", "views", 1), "1@DP-1");
}

/// **A floating window dropped on the other monitor's workspace belongs
/// to it** (#401): with no layout in charge the drop moves it all the
/// same, so it is shown where it was let go.
#[test]
fn a_floating_window_dropped_on_the_other_monitors_workspace_belongs_to_it() {
    let mut desk = Desk::floating(
        two_screens(),
        &[(
            1,
            "DP-1",
            Rect {
                x: 100.0,
                y: 100.0,
                w: 800.0,
                h: 600.0,
            },
        )],
    );
    assert_eq!(
        desk.scripts.evaluate(
            "local workspaces = require(\"workspaces\")\n\
             workspaces.of[1] = 1\n\
             workspaces.showing[\"DP-2\"] = 3\n\
             return \"\""
        ),
        ""
    );
    desk.drop(1, "DP-2", (3280.0, 1280.0));
    assert_eq!(desk.workspace_of(1), "3");
}

/// **With no layout in charge, focus and move reach the nearest window
/// each way.** #150.
///
/// Window 1 in the middle, one window off each side of it. From 1, each
/// focus key finds that one. From window 4, above, nothing is level with
/// it on the left, and the nearest centre that way is window 2's. From 2
/// going right the nearer facing edge wins, 1's over 3's. A move trades
/// places with the window focus would go to, each keeping its own size,
/// and the opposite move puts both back. At the monitor's edge nothing
/// happens. Nor is a window being closed one to go to.
#[test]
fn in_floating_focus_and_move_reach_the_nearest_window_each_way() {
    let at = |x: f64, y: f64, w: f64, h: f64| Rect { x, y, w, h };
    let mut desk = Desk::floating(
        one_screen(),
        &[
            (1, "DP-1", at(1000.0, 500.0, 500.0, 400.0)),
            (2, "DP-1", at(200.0, 550.0, 400.0, 300.0)),
            (3, "DP-1", at(1800.0, 450.0, 500.0, 400.0)),
            (4, "DP-1", at(1050.0, 50.0, 400.0, 300.0)),
            (5, "DP-1", at(950.0, 1000.0, 600.0, 300.0)),
        ],
    );
    // h, j, k and l rather than the plain arrows: floating is #222's
    // desktop mode, and the arrows snap there now (`floating.lua`). h, j,
    // k and l still focus by direction, exactly as before.
    for (from, key, to) in [
        (1, "super+h", 2),
        (1, "super+l", 3),
        (1, "super+k", 4),
        (1, "super+j", 5),
        (4, "super+h", 2),
        (2, "super+l", 1),
        (5, "super+k", 1),
        (4, "super+j", 1),
    ] {
        desk.focus(from);
        assert_eq!(focuses(&desk.press(key)), [to], "{key} from window {from}");
    }

    let start = desk.rects();
    for (key, with, back) in [
        ("super+shift+right", 3, "super+shift+h"),
        ("super+shift+down", 5, "super+alt+k"),
        ("super+shift+left", 2, "super+shift+l"),
        ("super+shift+up", 4, "super+shift+j"),
    ] {
        desk.focus(1);
        let (was, other) = (desk.rect(1), desk.rect(with));
        let commands = desk.press(key);
        let (one, them) = (desk.rect(1), desk.rect(with));
        assert!(
            about(one.x, other.x)
                && about(one.y, other.y)
                && about(one.w, was.w)
                && about(them.x, was.x)
                && about(them.y, was.y)
                && about(them.w, other.w),
            "{key} did not trade places with window {with}: {:?}",
            desk.rects()
        );
        assert!(
            commands.iter().all(|command| match command {
                Command::Place { tile, .. } => !tile,
                _ => true,
            }),
            "a floating window was placed as a tile: {commands:?}"
        );
        desk.press(back);
        let now = desk.rects();
        assert!(
            start
                .iter()
                .zip(&now)
                .all(|((_, before), (_, after))| same(*before, *after)),
            "{back} after {key} is not the desk it started as: {now:?}"
        );
    }

    // A window being closed is fading where it stood, and is not one to
    // go to.
    for window in &mut desk.windows {
        window.leaving = window.id == 2;
    }
    desk.focus(1);
    assert!(
        focuses(&desk.press("super+h")).is_empty(),
        "focus went to a window being closed"
    );
    for window in &mut desk.windows {
        window.leaving = false;
    }

    desk.focus(3);
    let commands = desk.press("super+l");
    assert!(
        focuses(&commands).is_empty(),
        "nothing is right of window 3"
    );
    let commands = desk.press("super+shift+right");
    assert!(
        places(&commands).is_empty(),
        "nothing is right of window 3 and no monitor is either: {commands:?}"
    );
}

/// **At a monitor's edge, floating goes on to the next monitor.** #150.
///
/// Focus crosses both ways. A move takes the window across, keeping its
/// size, as far across the portrait screen as it was across the
/// landscape one: a half and a quarter of the way there is a half and a
/// quarter of the way here -- and kept on it, where that would not be.
#[test]
fn at_a_monitors_edge_floating_crosses_to_the_next_monitor() {
    let at = |x: f64, y: f64, w: f64, h: f64| Rect { x, y, w, h };
    let mut desk = Desk::floating(
        two_screens(),
        &[
            (1, "DP-1", at(1280.0, 360.0, 400.0, 300.0)),
            (2, "DP-2", at(2800.0, 300.0, 600.0, 500.0)),
        ],
    );
    // l and h, not the plain arrows: floating is #222's desktop mode, and
    // the arrows snap there now (`floating.lua`). l and h still focus by
    // direction, and `super+shift+right` below is still a move, which
    // the arrows keep meaning everywhere.
    desk.focus(1);
    assert_eq!(focuses(&desk.press("super+l")), [2]);
    assert_eq!(focuses(&desk.press("super+h")), [1]);

    desk.press("super+shift+right");
    assert_eq!(
        desk.monitor(1),
        "DP-2",
        "window 1 did not cross: {:?}",
        desk.rects()
    );
    assert!(
        same(desk.rect(1), at(2560.0 + 720.0, 640.0, 400.0, 300.0)),
        "window 1 is not as far across DP-2 as it was across DP-1: {:?}",
        desk.rects()
    );
    assert!(
        same(desk.rect(2), at(2800.0, 300.0, 600.0, 500.0)),
        "window 2 was not part of the move"
    );

    // As far across would put window 3 past DP-2's right edge, so it is
    // kept on the screen, against that edge.
    desk.windows
        .push(window(3, "DP-1", at(2100.0, 900.0, 400.0, 300.0)));
    desk.focus(3);
    desk.press("super+shift+right");
    assert!(
        same(desk.rect(3), at(4000.0 - 400.0, 1600.0, 400.0, 300.0)),
        "window 3 was not kept on DP-2: {:?}",
        desk.rects()
    );
}

/// **The next monitor that way is one wholly past this one's edge.**
/// #150's review.
///
/// The portrait screen stands to the right of the landscape one and
/// reaches below it, and was taken for the one below it: `super+down`
/// from the grid's bottom row focused the portrait screen's lower window,
/// and `super+shift+down` moved a window onto it. Now down from the
/// bottom row, and up from the portrait screen's top, is nothing at all.
/// The same for a smaller screen top-aligned beside a bigger one, the
/// usual laptop and monitor, where up from the big one and down from the
/// small one each reached the other -- while right and left still cross.
/// And a screen touching this one only at a corner is past its edge both
/// ways, so right and down each reach it.
#[test]
fn at_a_monitors_edge_the_next_monitor_is_one_wholly_that_way() {
    let mut desk = grid("", two_screens());
    desk.open(5, "DP-2", (3280.0, 1280.0));
    desk.open(6, "DP-2", (3280.0, 2400.0));
    let (five, six) = (desk.rect(5), desk.rect(6));
    assert!(
        five.y + five.h < six.y && inside(five, TALL) && inside(six, TALL),
        "the premise: 5 above 6 on the portrait screen: {:?}",
        desk.rects()
    );
    let start = desk.rects();
    for (from, key) in [
        (3, "super+down"),
        (4, "super+down"),
        (5, "super+up"),
        (3, "super+j"),
        (5, "super+k"),
    ] {
        desk.focus(from);
        assert!(
            focuses(&desk.press(key)).is_empty(),
            "{key} from window {from} went to the monitor beside it"
        );
    }
    for (from, key) in [
        (3, "super+shift+down"),
        (4, "super+shift+down"),
        (5, "super+shift+up"),
    ] {
        desk.focus(from);
        let commands = desk.press(key);
        assert!(
            places(&commands).is_empty(),
            "{key} from window {from} moved something onto the monitor beside it: {commands:?}"
        );
    }
    let now = desk.rects();
    assert!(
        start
            .iter()
            .zip(&now)
            .all(|((_, before), (_, after))| same(*before, *after)),
        "the desk changed: {start:?} and now {now:?}"
    );

    let at = |x: f64, y: f64, w: f64, h: f64| Rect { x, y, w, h };
    let mut desk = Desk::floating(
        vec![screen("DP-1", WIDE), screen("DP-2", BESIDE)],
        &[
            (1, "DP-1", at(1000.0, 1000.0, 400.0, 300.0)),
            (2, "DP-2", at(3000.0, 300.0, 600.0, 400.0)),
        ],
    );
    // k, j, l and h, not the plain arrows: floating is #222's desktop
    // mode, and the arrows snap there now (`floating.lua`). k, j, l and h
    // still focus by direction, and the shift combos below are still a
    // move, which the arrows keep meaning everywhere.
    desk.focus(1);
    assert!(
        focuses(&desk.press("super+k")).is_empty(),
        "up from the big screen reached the small one beside it"
    );
    assert!(
        places(&desk.press("super+shift+up")).is_empty(),
        "a move up from the big screen went onto the small one beside it"
    );
    desk.focus(2);
    assert!(
        focuses(&desk.press("super+j")).is_empty(),
        "down from the small screen reached the big one beside it"
    );
    assert!(
        places(&desk.press("super+shift+down")).is_empty(),
        "a move down from the small screen went onto the big one beside it"
    );
    desk.focus(1);
    assert_eq!(focuses(&desk.press("super+l")), [2]);
    assert_eq!(focuses(&desk.press("super+h")), [1]);

    let mut desk = Desk::floating(
        vec![screen("DP-1", WIDE), screen("DP-3", CORNER)],
        &[
            (1, "DP-1", at(1000.0, 500.0, 400.0, 300.0)),
            (3, "DP-3", at(3000.0, 1700.0, 400.0, 300.0)),
        ],
    );
    for (from, key, to) in [
        (1, "super+j", 3),
        (1, "super+l", 3),
        (3, "super+k", 1),
        (3, "super+h", 1),
    ] {
        desk.focus(from);
        assert_eq!(
            focuses(&desk.press(key)),
            [to],
            "{key} from window {from}, to the screen off on a corner"
        );
    }
}

/// **In tiling, a tile off on a diagonal is not that way.** #150's review.
///
/// On the portrait screen, window 1 above 2 and 3 side by side, and
/// window 9 on the smaller screen to its left. Left of window 1 was
/// window 2, below it, by the nearest centre; right of it, window 3.
/// `super+shift+left` then traded 1 with 2 and `super+shift+right` 1 with
/// 3, leaving a desk the opposite key did not put back. Now left of 1 is
/// the next monitor, window 9, and right of it nothing: the move right
/// changes nothing, and the move left takes 1 across. Between tiles that
/// are level a move trades them, and the opposite move puts both back.
/// And from window 2, lower than anything on the screen to its left, left
/// is still window 9: on the next monitor, the nearest centre counts.
#[test]
fn in_tiling_a_tile_off_on_a_diagonal_is_not_that_way() {
    let mut desk = Desk::new("", vec![screen("DP-1", LEFT_OF_TALL), screen("DP-2", TALL)]);
    desk.press("super+t");
    desk.open(9, "DP-1", (1600.0, 540.0));
    desk.open(1, "DP-2", (3280.0, 640.0));
    desk.open(2, "DP-2", (3000.0, 2000.0));
    desk.open(3, "DP-2", (3800.0, 2000.0));
    let [one, two, three] = [1, 2, 3].map(|id| desk.rect(id));
    assert!(
        one.y + one.h < two.y
            && about(two.y, three.y)
            && two.x + two.w < three.x
            && inside(one, TALL)
            && inside(two, TALL)
            && inside(three, TALL)
            && inside(desk.rect(9), LEFT_OF_TALL),
        "the premise: 1 above 2 | 3 on the portrait screen, 9 on the other: {:?}",
        desk.rects()
    );

    for (from, key, to) in [
        (1, "super+left", Some(9)),
        (1, "super+h", Some(9)),
        (1, "super+right", None),
        (1, "super+l", None),
        (2, "super+up", Some(1)),
        (3, "super+up", Some(1)),
        (2, "super+right", Some(3)),
        (3, "super+left", Some(2)),
        (2, "super+left", Some(9)),
    ] {
        desk.focus(from);
        assert_eq!(
            focuses(&desk.press(key)),
            to.into_iter().collect::<Vec<_>>(),
            "{key} from window {from}"
        );
    }

    let start = desk.rects();
    desk.focus(1);
    let commands = desk.press("super+shift+right");
    assert!(
        places(&commands).is_empty(),
        "window 1 moved right, with nothing level with it that way: {commands:?}"
    );
    desk.focus(2);
    desk.press("super+shift+right");
    assert!(
        same(desk.rect(2), three) && same(desk.rect(3), two),
        "2 and 3 did not trade tiles: {:?}",
        desk.rects()
    );
    desk.press("super+shift+left");
    let now = desk.rects();
    assert!(
        start
            .iter()
            .zip(&now)
            .all(|((_, before), (_, after))| same(*before, *after)),
        "the opposite move did not put both back: {start:?} and now {now:?}"
    );

    desk.focus(1);
    desk.press("super+shift+left");
    assert_eq!(
        desk.monitor(1),
        "DP-1",
        "window 1 did not cross to the screen on its left: {:?}",
        desk.rects()
    );
    assert!(
        desk.rect(2).h > TALL.h - 40.0 && desk.rect(3).h > TALL.h - 40.0,
        "2 and 3 did not take the portrait screen: {:?}",
        desk.rects()
    );
}

/// **Of two monitors that way, the one level with the window is next.**
/// #150's second review.
///
/// The next monitor was ranked from the rectangle of the monitor the
/// window is on, so of two screens stacked beside it the same one was
/// next from every window: right of the big screen's lower tile was the
/// upper of the two screens beside it, and left of a portrait screen's
/// upper tile the lower of the two on its left. It is ranked from the
/// window now, as wlroots ranks from a point in the focused window: in
/// tiling for focus and for a move, and in scrolling past the end of the
/// strip.
#[test]
fn of_two_monitors_that_way_the_one_level_with_the_window_is_next() {
    let mut desk = Desk::new(
        "",
        vec![
            screen("DP-1", WIDE),
            screen("DP-2", BESIDE),
            screen("DP-3", UNDER_BESIDE),
        ],
    );
    desk.press("super+t");
    desk.open(1, "DP-1", (1280.0, 720.0));
    desk.open(2, "DP-1", (2000.0, 720.0));
    desk.open(3, "DP-1", (1920.0, 1200.0));
    desk.open(5, "DP-2", (3520.0, 540.0));
    desk.open(6, "DP-3", (3520.0, 1620.0));
    // Window 2 taller, so window 3 under it is level with DP-3 alone.
    desk.focus(2);
    for _ in 0..6 {
        desk.press("super+ctrl+equal");
    }
    let (upper, lower) = (desk.rect(2), desk.rect(3));
    assert!(
        about(upper.x, lower.x)
            && upper.y + upper.h < lower.y
            && lower.y > UNDER_BESIDE.y
            && upper.y + upper.h / 2.0 < BESIDE.y + BESIDE.h
            && inside(desk.rect(5), BESIDE)
            && inside(desk.rect(6), UNDER_BESIDE),
        "the premise: 2 above 3 on DP-1's right, 3 level with DP-3 alone, \
         5 on DP-2 and 6 on DP-3: {:?}",
        desk.rects()
    );
    for (from, key, to) in [
        (2, "super+right", 5),
        (2, "super+l", 5),
        (3, "super+right", 6),
        (3, "super+l", 6),
    ] {
        desk.focus(from);
        assert_eq!(focuses(&desk.press(key)), [to], "{key} from window {from}");
    }
    desk.focus(3);
    desk.press("super+shift+right");
    assert_eq!(
        desk.monitor(3),
        "DP-3",
        "super+shift+right from window 3 took it to the other screen: {:?}",
        desk.rects()
    );

    let stacked_left = || {
        vec![
            screen("DP-1", PORTRAIT),
            screen("DP-2", ABOVE_LEFT),
            screen("DP-3", BELOW_LEFT),
        ]
    };
    let mut desk = Desk::new("", stacked_left());
    desk.press("super+t");
    desk.open(7, "DP-2", (960.0, 540.0));
    desk.open(8, "DP-3", (960.0, 1620.0));
    desk.open(1, "DP-1", (2640.0, 640.0));
    desk.open(2, "DP-1", (2640.0, 2000.0));
    let (one, two) = (desk.rect(1), desk.rect(2));
    assert!(
        one.y + one.h < two.y
            && inside(one, PORTRAIT)
            && inside(two, PORTRAIT)
            && one.y + one.h / 2.0 < ABOVE_LEFT.y + ABOVE_LEFT.h
            && two.y > BELOW_LEFT.y,
        "the premise: 1 above 2 on the portrait screen, 2 level with DP-3 alone: {:?}",
        desk.rects()
    );
    for (from, key, to) in [
        (1, "super+left", 7),
        (1, "super+h", 7),
        (2, "super+left", 8),
        (2, "super+h", 8),
    ] {
        desk.focus(from);
        assert_eq!(focuses(&desk.press(key)), [to], "{key} from window {from}");
    }
    desk.focus(1);
    desk.press("super+shift+left");
    assert_eq!(
        desk.monitor(1),
        "DP-2",
        "super+shift+left from window 1 took it to the lower screen: {:?}",
        desk.rects()
    );

    let mut desk = Desk::new("", stacked_left());
    for (id, monitor, area) in [
        (7, "DP-2", ABOVE_LEFT),
        (8, "DP-3", BELOW_LEFT),
        (1, "DP-1", PORTRAIT),
        (2, "DP-1", PORTRAIT),
    ] {
        desk.windows.push(window(
            id,
            monitor,
            Rect {
                w: 400.0,
                h: 300.0,
                ..area
            },
        ));
    }
    desk.press("super+s");
    // Window 2 stacked under window 1: `super+comma` from column 1, on
    // DP-1 because the strip's own keys act on the monitor in front.
    desk.focus(2);
    desk.press("super+h");
    desk.press("super+comma");
    let (one, two) = (desk.rect(1), desk.rect(2));
    assert!(
        about(one.x, two.x)
            && one.y + one.h < two.y
            && inside(one, PORTRAIT)
            && inside(two, PORTRAIT)
            && one.y + one.h / 2.0 < ABOVE_LEFT.y + ABOVE_LEFT.h
            && two.y > BELOW_LEFT.y,
        "the premise: scrolling, 1 above 2 in the portrait screen's one column: {:?}",
        desk.rects()
    );
    for (from, key, to) in [(1, "super+left", 7), (2, "super+left", 8)] {
        desk.focus(from);
        assert_eq!(
            focuses(&desk.press(key)),
            [to],
            "scrolling: {key} from window {from}"
        );
    }
}

/// **`direction.beside` with no window, or one whose centre is on no
/// screen, ranks from the monitor.** #150's third review.
///
/// Round two made the window its third argument, and a mode of your own
/// calling it with the two it had before raised an error. Now, left of
/// the portrait screen with two stacked on its left, the one level with
/// the portrait screen's centre is next with no window, and with a window
/// whose centre is past both screens that way, which has nothing further
/// that way to rank -- where before that was no monitor at all. With a
/// window near the top it is still the upper screen.
#[test]
fn direction_beside_with_no_window_or_one_on_no_screen_ranks_from_the_monitor() {
    let desk = Desk::new(
        "",
        vec![
            screen("DP-1", PORTRAIT),
            screen("DP-2", ABOVE_LEFT),
            screen("DP-3", BELOW_LEFT),
        ],
    );
    for (from, next) in [
        ("", "DP-3"),
        (", { x = -5000, y = 0, w = 100, h = 100 }", "DP-3"),
        (", { x = 2000, y = 100, w = 800, h = 600 }", "DP-2"),
    ] {
        let answer = desk.scripts.evaluate_in(
            desk.snapshot(),
            &format!(
                "local next = require(\"direction\").beside(\"DP-1\", \"left\"{from})\n\
                 return next and next.name or \"no monitor\""
            ),
        );
        assert_eq!(answer, next, "left of DP-1{from}");
    }
}

/// **A window that belongs to no workspace belongs to none after a move
/// across.** #150's review.
///
/// With `follow_new_windows` off a window is on no workspace in
/// particular, so on every one, and a move onto the next monitor pinned
/// it to that monitor's: in tiling, in scrolling and with no layout.
#[test]
fn a_window_on_no_workspace_is_on_none_after_crossing() {
    let off = "require(\"config\").workspaces.follow_new_windows = false";

    let mut desk = Desk::new(off, two_screens());
    desk.press("super+t");
    desk.open(1, "DP-1", (1280.0, 720.0));
    desk.open(2, "DP-1", (2000.0, 720.0));
    assert_eq!(desk.workspace_of(2), "nil", "the premise");
    desk.press("super+shift+right");
    assert_eq!(desk.monitor(2), "DP-2", "tiling: window 2 did not cross");
    assert_eq!(desk.workspace_of(2), "nil", "tiling");

    let mut desk = Desk::new(off, two_screens());
    for id in [1, 2] {
        desk.windows.push(window(
            id,
            "DP-1",
            Rect {
                w: 400.0,
                h: 300.0,
                ..WIDE
            },
        ));
    }
    desk.press("super+s");
    desk.focus(2);
    desk.press("super+shift+right");
    assert_eq!(desk.monitor(2), "DP-2", "scrolling: window 2 did not cross");
    assert_eq!(desk.workspace_of(2), "nil", "scrolling");

    let mut desk = Desk::new(off, two_screens());
    desk.windows.push(window(
        1,
        "DP-1",
        Rect {
            x: 1280.0,
            y: 360.0,
            w: 400.0,
            h: 300.0,
        },
    ));
    desk.focus(1);
    desk.press("super+shift+right");
    assert_eq!(desk.monitor(1), "DP-2", "floating: window 1 did not cross");
    assert_eq!(desk.workspace_of(1), "nil", "floating");
}

/// **A window moved by key onto the other monitor is declared on the
/// workspace that monitor shows**, by the press that moves it, in
/// tiling, in scrolling and with no layout: nothing runs `layout` after
/// a move, so without it a hosted shell would count the window on the
/// workspace it left.
#[test]
fn a_window_moved_onto_the_other_monitor_is_declared_on_the_workspace_it_shows() {
    let declared = |commands: &[Command]| {
        commands.iter().rev().find_map(|command| match command {
            Command::Workspaces(declared) => Some(declared.windows.get(&2).cloned()),
            _ => None,
        })
    };
    let mut moved = Vec::new();
    for layout in ["super+t", "super+s", ""] {
        let mut desk = Desk::new("", two_screens());
        assert_eq!(
            desk.scripts
                .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 2 return \"\""),
            ""
        );
        if !layout.is_empty() {
            desk.press(layout);
        }
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        desk.focus(2);
        let commands = desk.press("super+shift+right");
        moved.push((desk.monitor(2), declared(&commands)));
    }
    let there = ("DP-2".to_owned(), Some(Some(vec!["2".to_owned()])));
    assert_eq!(
        moved,
        vec![there.clone(), there.clone(), there],
        "(window 2's monitor, where the press declared it) in tiling, scrolling and floating"
    );
}

/// **A window on no workspace moved by key onto the other monitor is
/// declared on the workspace that monitor shows**, by the press that moves
/// it: with `follow_new_windows` off it belongs to whatever its monitor
/// shows, and that monitor is now the other one.
#[test]
fn a_window_on_no_workspace_moved_across_is_declared_on_what_that_monitor_shows() {
    let off = "require(\"config\").workspaces.follow_new_windows = false";
    let declared = |commands: &[Command]| {
        commands.iter().rev().find_map(|command| match command {
            Command::Workspaces(declared) => Some(declared.windows.get(&2).cloned()),
            _ => None,
        })
    };
    let mut moved = Vec::new();
    for layout in ["super+t", "super+s", ""] {
        let mut desk = Desk::new(off, two_screens());
        assert_eq!(
            desk.scripts
                .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 2 return \"\""),
            ""
        );
        if !layout.is_empty() {
            desk.press(layout);
        }
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        desk.focus(2);
        assert_eq!(desk.workspace_of(2), "nil", "the premise");
        let commands = desk.press("super+shift+right");
        moved.push((desk.monitor(2), desk.workspace_of(2), declared(&commands)));
    }
    let there = (
        "DP-2".to_owned(),
        "nil".to_owned(),
        Some(Some(vec!["2".to_owned()])),
    );
    assert_eq!(
        moved,
        vec![there.clone(), there.clone(), there],
        "(window 2's monitor, its `of`, where the press declared it) in tiling, scrolling and floating"
    );
}

/// **In scrolling, the directions are the strip's own keys.** #150.
///
/// Left and right are the columns and up and down the windows in one,
/// for focus and for a move; past the end of the strip, focus goes to the
/// window the next monitor's strip has focused, and a move takes the
/// window into a column of its own there.
#[test]
fn in_scrolling_the_directions_are_the_strips_own_keys() {
    let mut desk = Desk::new("", two_screens());
    for id in [1, 2, 3, 5] {
        let monitor = if id == 5 { "DP-2" } else { "DP-1" };
        desk.windows.push(window(
            id,
            monitor,
            Rect {
                w: 400.0,
                h: 300.0,
                ..if id == 5 { TALL } else { WIDE }
            },
        ));
    }
    desk.press("super+s");
    let x = |desk: &Desk, id| desk.rect(id).x;
    assert!(
        x(&desk, 1) < x(&desk, 2) && x(&desk, 2) < x(&desk, 3) && desk.monitor(3) == "DP-1",
        "the premise: columns 1, 2, 3 on DP-1: {:?}",
        desk.rects()
    );

    desk.focus(2);
    assert_eq!(focuses(&desk.press("super+left")), [1]);
    desk.focus(2);
    assert_eq!(focuses(&desk.press("super+l")), [3]);

    // Window 2 stacked under window 1: `super+comma` from column 1.
    desk.focus(2);
    desk.press("super+h");
    desk.press("super+comma");
    let (one, two) = (desk.rect(1), desk.rect(2));
    assert!(
        about(one.x, two.x) && one.y < two.y,
        "the premise: 1 above 2 in one column: {:?}",
        desk.rects()
    );
    desk.focus(2);
    assert_eq!(focuses(&desk.press("super+up")), [1]);
    assert_eq!(focuses(&desk.press("super+j")), [2]);

    desk.press("super+alt+k");
    assert!(
        desk.rect(2).y < desk.rect(1).y,
        "super+alt+k did not move window 2 above window 1: {:?}",
        desk.rects()
    );
    desk.press("super+shift+down");
    assert!(desk.rect(1).y < desk.rect(2).y, "and back down");

    desk.press("super+shift+right");
    assert!(
        x(&desk, 3) < x(&desk, 2) && about(x(&desk, 1), x(&desk, 2)),
        "the column did not move right past window 3's: {:?}",
        desk.rects()
    );
    desk.press("super+shift+h");
    assert!(x(&desk, 2) < x(&desk, 3), "and back left");

    desk.focus(3);
    assert_eq!(
        focuses(&desk.press("super+right")),
        [5],
        "past the end of the strip, the next monitor's"
    );
    desk.focus(3);
    desk.press("super+shift+right");
    assert_eq!(
        desk.monitor(3),
        "DP-2",
        "window 3 did not cross: {:?}",
        desk.rects()
    );
    assert!(
        inside(desk.rect(3), TALL) && x(&desk, 5) < x(&desk, 3),
        "window 3 is not in a column of its own beside window 5: {:?}",
        desk.rects()
    );
    assert_eq!(
        desk.workspace_of(3),
        "nil",
        "window 3 was on no workspace, and is on none there either"
    );
}

/// **`super+shift+space` floats a tiled window, and tiles it again.**
/// #150.
///
/// Window 2 leaves its tile, window 1 takes the screen, and 2 is centred
/// on it at the size it had, not as a tile; pressed again, the two share
/// the screen. With no layout in charge the key changes nothing, now or
/// when tiling is turned on afterwards.
#[test]
fn super_shift_space_floats_a_tiled_window_and_tiles_it_again() {
    let mut desk = Desk::new("", one_screen());
    desk.press("super+t");
    desk.open(1, "DP-1", (1280.0, 720.0));
    desk.open(2, "DP-1", (2000.0, 720.0));
    let (one, two) = (desk.rect(1), desk.rect(2));

    let commands = desk.press("super+shift+space");
    let full = Rect {
        x: 12.0,
        y: 12.0,
        w: 2536.0,
        h: 1416.0,
    };
    let floated = desk.rect(2);
    assert!(
        same(desk.rect(1), full),
        "window 1 did not take the screen: {:?}",
        desk.rects()
    );
    assert!(
        about(floated.w, two.w)
            && about(floated.x + floated.w / 2.0, 1280.0)
            && about(floated.y + floated.h / 2.0, 720.0),
        "window 2 is not centred at the size it had: {floated:?}"
    );
    assert!(
        commands.iter().any(|command| matches!(
            command,
            Command::Place {
                id: 2,
                tile: false,
                ..
            }
        )),
        "window 2 was placed as a tile: {commands:?}"
    );

    desk.press("super+shift+space");
    assert!(
        same(desk.rect(1), one) && same(desk.rect(2), two),
        "pressed again, the two did not share the screen: {:?}",
        desk.rects()
    );

    let mut desk = Desk::new("", one_screen());
    desk.press("super+t");
    desk.open(1, "DP-1", (1280.0, 720.0));
    desk.open(2, "DP-1", (2000.0, 720.0));
    let (one, two) = (desk.rect(1), desk.rect(2));
    desk.press("super+t");
    let commands = desk.press("super+shift+space");
    assert!(
        places(&commands).is_empty(),
        "floating mode placed something: {commands:?}"
    );
    desk.press("super+t");
    assert!(
        same(desk.rect(1), one) && same(desk.rect(2), two),
        "super+shift+space in floating mode floated window 2 for later: {:?}",
        desk.rects()
    );
}

/// **A window the layout does not arrange gets the floating answer.**
/// #150.
///
/// Window 3 floated over tiling's `1 | 2`, and over scrolling's strip:
/// focus from it finds the nearest window that way among everything on
/// screen, and a move does nothing -- trading places with a tile would
/// hand the tile a place its layout takes straight back. Closed, the
/// window is forgotten as floated.
#[test]
fn a_window_the_layout_does_not_arrange_gets_the_floating_answer() {
    for (layout, key) in [("tiling", "super+t"), ("scrolling", "super+s")] {
        let mut desk = Desk::new("", one_screen());
        desk.press(key);
        desk.open(1, "DP-1", (1280.0, 720.0));
        desk.open(2, "DP-1", (2000.0, 720.0));
        desk.open(3, "DP-1", (2000.0, 1200.0));
        // Over to window 1 and back, so a strip is scrolled to its start
        // rather than to wherever opening window 3 left it.
        desk.press("super+left");
        desk.press("super+left");
        desk.focus(3);
        desk.press("super+shift+space");
        assert_eq!(
            focuses(&desk.press("super+left")),
            [1],
            "{layout}: focus left from the floated window"
        );
        if layout == "tiling" {
            desk.focus(3);
            assert_eq!(
                focuses(&desk.press("super+right")),
                [2],
                "{layout}: focus right from the floated window"
            );
        }
        desk.focus(3);
        let commands = desk.press("super+shift+left");
        assert!(
            places(&commands).is_empty(),
            "{layout}: the floated window moved, or moved something: {commands:?}"
        );
        let _ = desk.scripts.closed(3, desk.snapshot());
        assert_eq!(
            desk.scripts
                .evaluate("return tostring(require(\"dialogs\").lifted[3])"),
            "nil",
            "{layout}: a closed window is still marked floated"
        );
    }
}

/// **A window the user floated stays out through a reload.** #150.
#[test]
fn a_window_floated_by_the_user_stays_out_through_a_reload() {
    let mut desk = Desk::new("", one_screen());
    desk.press("super+t");
    desk.open(1, "DP-1", (1280.0, 720.0));
    desk.open(2, "DP-1", (2000.0, 720.0));
    desk.press("super+shift+space");
    let (one, two) = (desk.rect(1), desk.rect(2));
    desk.reload();
    assert!(
        same(desk.rect(1), one) && same(desk.rect(2), two),
        "the reload put the floated window back in a tile: {:?}",
        desk.rects()
    );
    desk.press("super+shift+space");
    assert!(
        desk.rect(1).w < 1400.0,
        "and it can still be tiled again: {:?}",
        desk.rects()
    );
}

/// A script of `text` alone, in a directory of its own.
fn plain(name: &str, text: &str) -> Scripts {
    let directory =
        std::env::temp_dir().join(format!("solium-directions-{name}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(&entry, text).expect("writing the test script");
    Scripts::load(&entry).expect("loading the test script")
}

/// **`sol.focus_direction` and `sol.move_direction` reach every
/// `direction` listener with the verb and the direction, and a direction
/// that is not one of the four is an error.** #150.
#[test]
fn sol_direction_reaches_the_listeners_and_a_bad_one_is_an_error() {
    let mut scripts = plain(
        "event",
        r#"
        sol.on("direction", function(verb, dir) sol.status(verb .. " " .. dir) end)
        sol.bind("super+a", function() sol.focus_direction("left") end)
        sol.bind("super+b", function() sol.move_direction("down") end)
        sol.bind("super+c", function()
            local ok, err = pcall(sol.focus_direction, "west")
            sol.status(tostring(ok) .. ": " .. tostring(err))
        end)
        "#,
    );
    let status = |scripts: &mut Scripts, combo: &str| {
        scripts
            .key(combo, Snapshot::default())
            .status
            .unwrap_or_default()
    };
    assert_eq!(status(&mut scripts, "super+a"), "focus left");
    assert_eq!(status(&mut scripts, "super+b"), "move down");
    let refused = status(&mut scripts, "super+c");
    assert!(
        refused.starts_with("false: ") && refused.contains("\"west\" is not a direction"),
        "{refused:?}"
    );
}

/// **`sol.toggle_fullscreen` and `sol.toggle_maximize` name the focused
/// window when given none**, the window given when there is one, and
/// nothing when there is neither. #150.
#[test]
fn sol_toggles_name_the_focused_window_when_given_none() {
    let mut scripts = plain(
        "toggles",
        r#"
        sol.bind("super+a", function() sol.toggle_fullscreen() end)
        sol.bind("super+b", function() sol.toggle_maximize() end)
        sol.bind("super+c", function() sol.toggle_fullscreen(9) end)
        "#,
    );
    let desk = |focused: bool| Snapshot {
        windows: vec![
            window(4, "DP-1", WIDE),
            WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                focused,
                ..window(3, "DP-1", WIDE)
            },
        ],
        ..Snapshot::default()
    };
    let asked = |commands: Vec<Command>| -> Vec<String> {
        commands
            .iter()
            .map(|command| format!("{command:?}"))
            .collect()
    };
    assert_eq!(
        asked(scripts.key("super+a", desk(true)).commands),
        [format!("{:?}", Command::ToggleFullscreen { id: 3 })]
    );
    assert_eq!(
        asked(scripts.key("super+b", desk(true)).commands),
        [format!("{:?}", Command::ToggleMaximize { id: 3 })]
    );
    assert_eq!(
        asked(scripts.key("super+c", desk(true)).commands),
        [format!("{:?}", Command::ToggleFullscreen { id: 9 })]
    );
    assert!(scripts.key("super+a", desk(false)).commands.is_empty());
    assert!(scripts.key("super+b", desk(false)).commands.is_empty());
}

/// **`sol.press` names a button or a key, and refuses what it cannot
/// reach**: a button by its name and a key as `sol.bind` spells it, on
/// the window named or the focused one, are pressed; a minimized, leaving
/// or unknown window, and a spec that names no button or key, or both,
/// or something else, are errors and ask nothing. With nothing focused
/// and no id, nothing. And it reaches past the lock, so a binding allowed
/// there cannot ask for it.
#[test]
fn sol_press_names_a_button_or_a_key_and_refuses_what_it_cannot_reach() {
    use smithay::input::keyboard::Keysym;
    let mut scripts = plain(
        "press",
        r#"
        said = {}
        local function try(...)
            local ok, err = pcall(sol.press, ...)
            said[#said + 1] = ok and "ok" or tostring(err):match("[^\n]*")
        end
        sol.bind("super+a", function()
            try(1, { button = "side" })
            try(nil, { key = "alt+Left" })
            try(1, { key = "Escape" })
            try(2, { button = "left" })
            try(3, { key = "Escape" })
            try(9, { button = "left" })
            try(1, { button = "back" })
            try(1, { key = "no_such_key" })
            try(1, { key = "alt+" })
            try(1, { button = "left", key = "Escape" })
            try(1, {})
            try(1, { button = "left", at = 3 })
            try(1, { button = 4 })
        end)
        sol.bind("super+b", function() try(nil, { button = "left" }) end)
        "#,
    );
    let desk = |focused: bool| Snapshot {
        windows: vec![
            WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                focused,
                ..window(1, "DP-1", WIDE)
            },
            WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                minimized: true,
                above: false,
                ..window(2, "DP-1", WIDE)
            },
            WindowInfo {
                icon: None,
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                leaving: true,
                ..window(3, "DP-1", WIDE)
            },
        ],
        ..Snapshot::default()
    };
    let press = |press: super::Press| Command::Press {
        id: 1,
        press,
        source: FocusSource::Binding,
    };
    let asked = |commands: Vec<Command>| -> Vec<String> {
        commands
            .iter()
            .map(|command| format!("{command:?}"))
            .collect()
    };
    assert_eq!(
        asked(scripts.key("super+a", desk(true)).commands),
        [
            press(super::Press::Button(0x113)),
            press(super::Press::Keys(vec![Keysym::Alt_L, Keysym::Left])),
            press(super::Press::Keys(vec![Keysym::Escape])),
        ]
        .map(|command| format!("{command:?}"))
    );
    let said = scripts.evaluate("return table.concat(said, \"\\n\")");
    let said: Vec<&str> = said.lines().collect();
    let wanted = [
        "ok",
        "ok",
        "ok",
        "2 is minimized",
        "3 is leaving",
        "9 is not a window",
        "no button is called `back`",
        "`no_such_key` names no key",
        "`alt+` names no key",
        "a button and a key at once",
        "it names nothing",
        "`at` is not in it",
        "a button or a key is a string",
    ];
    assert_eq!(said.len(), wanted.len(), "every call answered: {said:?}");
    for (said, wanted) in said.iter().zip(wanted) {
        assert!(said.contains(wanted), "{said:?} is not {wanted:?}");
    }
    assert!(
        scripts.key("super+b", desk(false)).commands.is_empty(),
        "no id and nothing focused presses nothing"
    );
    assert!(
        crate::wm::convert::reaches_past_the_lock(&solium_wm_protocol::Command::Press((
            1,
            solium_wm_protocol::Press::Button(0x110),
            solium_wm_protocol::FocusCause::Binding,
        ))),
        "a press reaches past the lock, so a binding at the lock cannot make one"
    );
}

/// **`sol.minimize` names the focused window when given none**, the
/// window given when there is one, and nothing when there is neither;
/// `sol.unminimize` and `sol.show_minimized` name the window given. And
/// all three, with `sol.act`'s two verbs, reach past the lock, so a
/// binding allowed there cannot ask for any of them (#30, #224).
#[test]
fn sol_minimize_names_the_focused_window_when_given_none_and_reaches_past_the_lock() {
    let mut scripts = plain(
        "minimize",
        r#"
        sol.bind("super+a", function() sol.minimize() end)
        sol.bind("super+b", function() sol.unminimize(9) end)
        sol.bind("super+c", function() sol.show_minimized(9) end)
        sol.bind("super+d", function() sol.show_minimized(9, false) end)
        "#,
    );
    let desk = |focused: bool| Snapshot {
        windows: vec![
            window(4, "DP-1", WIDE),
            WindowInfo {
                client_side: false,
                blur_region: false,
                insets: crate::decoration::Insets::default(),
                focused,
                ..window(3, "DP-1", WIDE)
            },
        ],
        ..Snapshot::default()
    };
    let asked = |commands: Vec<Command>| -> Vec<String> {
        commands
            .iter()
            .map(|command| format!("{command:?}"))
            .collect()
    };
    assert_eq!(
        asked(scripts.key("super+a", desk(true)).commands),
        [format!("{:?}", Command::Minimize { id: 3 })]
    );
    assert!(scripts.key("super+a", desk(false)).commands.is_empty());
    assert_eq!(
        asked(scripts.key("super+b", desk(true)).commands),
        [format!("{:?}", Command::Unminimize { id: 9 })]
    );
    assert_eq!(
        asked(scripts.key("super+c", desk(true)).commands),
        [format!(
            "{:?}",
            Command::ShowMinimized { id: 9, shown: true }
        )]
    );
    assert_eq!(
        asked(scripts.key("super+d", desk(true)).commands),
        [format!(
            "{:?}",
            Command::ShowMinimized {
                id: 9,
                shown: false
            }
        )]
    );
    use solium_wm_protocol::Command as W;
    let act = |action: &str| {
        W::Act((
            1,
            action.to_owned(),
            "null".to_owned(),
            solium_wm_protocol::FocusCause::Script,
        ))
    };
    for command in [
        W::Minimize(3),
        W::Unminimize(3),
        W::ShowMinimized((3, true)),
        act("windows.minimize"),
        act("windows.unminimize"),
    ] {
        assert!(
            crate::wm::convert::reaches_past_the_lock(&command),
            "{command:?} reaches past the lock, so a binding at the lock cannot ask for it"
        );
    }
}

/// **Desktop mode: the arrows snap the focused window to each half, and a
/// second press toward the edge it is already at crosses to the next
/// monitor that way.** #222.
#[test]
fn desktop_mode_the_arrows_snap_to_each_half_and_cross_at_the_edge() {
    let mut desk = Desk::new("", two_screens());
    desk.open(1, "DP-1", (1280.0, 720.0));

    desk.press("super+left");
    let left = desk.rect(1);
    assert!(
        about(left.x, WIDE.x)
            && about(left.w, WIDE.w / 2.0)
            && about(left.h, WIDE.h)
            && about(left.y, WIDE.y),
        "super+left did not snap to the left half of DP-1: {left:?}"
    );

    // Already as far left as it goes: DP-1 has no monitor to its left.
    let commands = desk.press("super+left");
    assert!(
        places(&commands).is_empty(),
        "a second super+left with no monitor further left placed something: {commands:?}"
    );
    assert!(
        same(desk.rect(1), left),
        "the window moved with nowhere to go: {:?}",
        desk.rect(1)
    );

    desk.press("super+right");
    let right = desk.rect(1);
    assert!(
        about(right.x + right.w, WIDE.x + WIDE.w) && about(right.w, WIDE.w / 2.0),
        "super+right did not snap to the right half of DP-1: {right:?}"
    );

    // Already at DP-1's right half: a second press crosses to DP-2.
    desk.press("super+right");
    assert_eq!(
        desk.monitor(1),
        "DP-2",
        "a second super+right did not cross to DP-2: {:?}",
        desk.rects()
    );
    let crossed = desk.rect(1);
    assert!(
        about(crossed.x + crossed.w, TALL.x + TALL.w) && about(crossed.w, TALL.w / 2.0),
        "window 1 is not the right half of DP-2: {crossed:?}"
    );
}

/// **Desktop mode: restoring puts a snapped window back exactly where it
/// was, and does nothing once there is nothing to restore.** #222.
#[test]
fn desktop_mode_restore_puts_a_snapped_window_back_where_it_was() {
    let mut desk = Desk::new("", one_screen());
    desk.open(1, "DP-1", (1280.0, 720.0));
    let original = desk.rect(1);

    desk.press("super+left");
    assert!(
        !same(desk.rect(1), original),
        "the premise: super+left actually moved the window"
    );
    desk.press("super+down");
    assert!(
        same(desk.rect(1), original),
        "super+down did not restore the pre-snap rectangle: {:?}",
        desk.rects()
    );

    let commands = desk.press("super+down");
    assert!(
        places(&commands).is_empty(),
        "super+down with nothing to restore placed something: {commands:?}"
    );
}

/// **Desktop mode: maximise asks the compositor rather than placing the
/// window itself, restore un-maximises a maximised window, and maximise
/// from a half goes to that half's top quarter instead.** #222.
///
/// The harness does not simulate what `sol.toggle_maximize` does to a
/// window's rectangle -- that is the compositor's, in production -- so
/// this asks only that the right command is sent, and stands in for the
/// compositor's own answer with `scripts.maximize`, exactly as
/// `sol_toggles_name_the_focused_window_when_given_none` does.
#[test]
fn desktop_mode_maximize_asks_the_compositor_and_a_half_goes_to_its_quarter() {
    let mut desk = Desk::new("", one_screen());
    desk.open(1, "DP-1", (1280.0, 720.0));
    let original = desk.rect(1);

    let commands = desk.press("super+up");
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::ToggleMaximize { id: 1 })),
        "super+up did not ask to maximise: {commands:?}"
    );
    let _ = desk.scripts.maximize(1, true, desk.snapshot());

    let commands = desk.press("super+down");
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::ToggleMaximize { id: 1 })),
        "super+down did not un-maximise a maximised window: {commands:?}"
    );
    let _ = desk.scripts.maximize(1, false, desk.snapshot());
    assert!(
        same(desk.rect(1), original),
        "the window did not keep its pre-maximise rectangle: {:?}",
        desk.rects()
    );

    desk.press("super+left");
    let left = desk.rect(1);
    let commands = desk.press("super+up");
    assert_eq!(
        places(&commands),
        vec![1],
        "super+up from a half did not place the window: {commands:?}"
    );
    let quarter = desk.rect(1);
    assert!(
        about(quarter.x, left.x)
            && about(quarter.w, left.w)
            && about(quarter.y, left.y)
            && about(quarter.h, left.h / 2.0),
        "super+up from the left half is not its top quarter: half {left:?} quarter {quarter:?}"
    );

    let commands = desk.press("super+up");
    assert!(
        places(&commands).is_empty(),
        "a second super+up from the quarter placed something: {commands:?}"
    );
}

/// **Desktop mode: a new window with no size of its own gets a share of
/// the work area, smaller than it on both axes.** #222.
#[test]
fn desktop_mode_a_window_with_no_size_of_its_own_gets_a_share_of_the_work_area() {
    let mut desk = Desk::new("", one_screen());
    let mut unshown = window(
        1,
        "DP-1",
        Rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        },
    );
    unshown.shown = false;
    desk.windows.push(unshown);
    let outcome = desk.scripts.opened(1, desk.snapshot());
    desk.apply(&outcome.commands);
    let rect = desk.rect(1);
    assert!(
        rect.w > 0.0 && rect.w < WIDE.w && rect.h > 0.0 && rect.h < WIDE.h,
        "a window that has shown no frame of its own did not get a share of the work \
         area smaller than it: {rect:?}"
    );
}

/// **Desktop mode: three windows opened in a row land at different
/// spots, not stacked on top of each other.** #222.
#[test]
fn desktop_mode_places_three_new_windows_without_stacking_them() {
    let mut desk = Desk::new("", one_screen());
    desk.open(1, "DP-1", (1280.0, 720.0));
    desk.open(2, "DP-1", (1280.0, 720.0));
    desk.open(3, "DP-1", (1280.0, 720.0));
    let rects = [desk.rect(1), desk.rect(2), desk.rect(3)];
    assert!(
        !same(rects[0], rects[1]) && !same(rects[1], rects[2]) && !same(rects[0], rects[2]),
        "three new windows landed on top of each other: {rects:?}"
    );
}

/// The work area #240 was measured on: the nested 1600x900 output.
const NESTED: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1600.0,
    h: 900.0,
};

/// Window `id` opens on `DP-1` before its application has shown a frame
/// -- every window `sol.spawn` opens, `super+return`'s terminal among
/// them -- and has the keyboard once it is placed.
fn open_unshown(desk: &mut Desk, id: u64) {
    let mut unshown = window(
        id,
        "DP-1",
        Rect {
            w: 0.0,
            h: 0.0,
            ..NESTED
        },
    );
    unshown.shown = false;
    desk.windows.insert(0, unshown);
    let outcome = desk.scripts.opened(id, desk.snapshot());
    desk.apply(&outcome.commands);
    desk.focus(id);
}

/// The atrium off: these check where floating placement puts windows on
/// the whole work area, which the atrium's strip would narrow.
const NO_ATRIUM: &str = "require(\"config\").floating.atrium.enabled = false\n";

/// Window `id` opens on `DP-1` already `w` by `h`: an application that
/// has shown a frame of its own size, which placement keeps.
fn open_shown(desk: &mut Desk, id: u64, w: f64, h: f64) {
    desk.windows
        .insert(0, window(id, "DP-1", Rect { w, h, ..NESTED }));
    let outcome = desk.scripts.opened(id, desk.snapshot());
    desk.apply(&outcome.commands);
    desk.focus(id);
}

/// Fails unless every window on `desk` is inside `area` and no two share
/// a spot: top-left corners nearer than half the shipped `cascade` step
/// (16) on both axes, `floating.lua`'s `taken`.
fn spread_inside(desk: &Desk, area: Rect, why: &str) {
    let rects = desk.rects();
    for (at, (id, rect)) in rects.iter().enumerate() {
        assert!(
            inside(*rect, area),
            "{why}: window {id} is not inside the work area: {rects:?}"
        );
        for (other, earlier) in &rects[..at] {
            assert!(
                (rect.x - earlier.x).abs() >= 16.0 || (rect.y - earlier.y).abs() >= 16.0,
                "{why}: windows {other} and {id} share a spot: {rects:?}"
            );
        }
    }
}

/// **Desktop mode: ten windows opened before their first frame, as
/// `super+return` opens them, land centred and then one `cascade` step
/// apart, each `placement.size` of the work area; the seventh, which one
/// more step would take past the bottom edge, starts a new diagonal at
/// the work area's top-left plus one step (`wrap = "corner"`).** #240,
/// measured nested on 1600x900: the second window landed on the work
/// area's bottom-right corner, and the third and fourth exactly on top of
/// it.
#[test]
fn desktop_mode_ten_spawned_windows_step_by_cascade_and_wrap_at_the_edge() {
    let mut desk = Desk::new(NO_ATRIUM, vec![screen("DP-1", NESTED)]);
    for id in 1..=10 {
        open_unshown(&mut desk, id);
    }
    let wanted = [
        (320.0, 180.0),
        (352.0, 212.0),
        (384.0, 244.0),
        (416.0, 276.0),
        (448.0, 308.0),
        (480.0, 340.0),
        (32.0, 32.0),
        (64.0, 64.0),
        (96.0, 96.0),
        (128.0, 128.0),
    ];
    for (id, (x, y)) in (1..=10).zip(wanted) {
        let rect = desk.rect(id);
        assert!(
            about(rect.x, x) && about(rect.y, y) && about(rect.w, 960.0) && about(rect.h, 540.0),
            "window {id} is not 960x540 at {x},{y}: {:?}",
            desk.rects()
        );
    }
}

/// **Desktop mode: a spot a window was dragged a few pixels off is still
/// taken**: nearer than half a `cascade` step on both axes, the next
/// window steps on rather than landing all but on top of it. #240.
#[test]
fn desktop_mode_a_spot_a_window_was_nudged_off_is_still_taken() {
    let mut desk = Desk::new(NO_ATRIUM, vec![screen("DP-1", NESTED)]);
    open_unshown(&mut desk, 1);
    for each in &mut desk.windows {
        each.rect.x += 5.0;
        each.rect.y += 5.0;
    }
    open_unshown(&mut desk, 2);
    let second = desk.rect(2);
    assert!(
        about(second.x, 352.0) && about(second.y, 212.0),
        "window 2 did not step past the centre window 1 was nudged 5 pixels off: {:?}",
        desk.rects()
    );
}

/// **Desktop mode: a `cascade` that is not a step of a pixel or more on
/// each axis is read as the shipped (32, 32)**, rather than a step of
/// nothing that would stack every window on the first. #240.
#[test]
fn desktop_mode_a_cascade_of_no_step_is_read_as_the_shipped_one() {
    let mut desk = Desk::new(
        "require(\"config\").floating.atrium.enabled = false\n\
         local config = require(\"config\")\n\
         config.floating.placement.cascade = { x = 0, y = \"far\" }\n",
        vec![screen("DP-1", NESTED)],
    );
    open_unshown(&mut desk, 1);
    open_unshown(&mut desk, 2);
    let second = desk.rect(2);
    assert!(
        about(second.x, 352.0) && about(second.y, 212.0),
        "a cascade of (0, \"far\") did not step by (32, 32): {:?}",
        desk.rects()
    );
}

/// **Desktop mode: with no spot free, a window lands where its policy
/// starts, inside the work area**: a window the size of the work area has
/// only the one spot, so the second lands on the first rather than the
/// walk looking for a spot that is not there. #240.
#[test]
fn desktop_mode_with_no_free_spot_a_window_lands_where_its_policy_starts() {
    let mut desk = Desk::new(
        "require(\"config\").floating.atrium.enabled = false\n\
         local config = require(\"config\")\n\
         config.floating.placement.size = 1\n",
        vec![screen("DP-1", NESTED)],
    );
    open_unshown(&mut desk, 1);
    open_unshown(&mut desk, 2);
    for id in [1, 2] {
        assert!(
            same(desk.rect(id), NESTED),
            "window {id} is not the whole work area: {:?}",
            desk.rects()
        );
    }
}

/// **Desktop mode: windows nearly as tall as the work area still get a
/// spot each**: with less room below the first than one `cascade` step,
/// the corner diagonals start as far down as the window leaves room for,
/// rather than not at all. #240's review: two 1200x880 windows on
/// 1600x900 both landed on the centre, since the corner's first spot,
/// (32, 32), did not fit and the walk gave up.
#[test]
fn desktop_mode_windows_nearly_the_work_area_tall_get_spots_of_their_own() {
    let mut desk = Desk::new(NO_ATRIUM, vec![screen("DP-1", NESTED)]);
    for id in 1..=4 {
        open_shown(&mut desk, id, 1200.0, 880.0);
    }
    spread_inside(&desk, NESTED, "1200x880, wrap = \"corner\"");
    let wanted = [(200.0, 10.0), (32.0, 20.0), (64.0, 20.0), (96.0, 20.0)];
    for (id, (x, y)) in (1..=4).zip(wanted) {
        let rect = desk.rect(id);
        assert!(
            about(rect.x, x) && about(rect.y, y),
            "window {id} is not at {x},{y}: {:?}",
            desk.rects()
        );
    }
}

/// **Desktop mode: once every diagonal its `wrap` starts is spent, a new
/// window takes the first free spot of the `cascade` grid over the work
/// area rather than landing on another.** `wrap = "center"` with
/// 1200x880 windows on 1600x900 has room for seven diagonals of one spot
/// each, (200, 10) to (392, 10); the eighth and ninth windows used to
/// land on the first. #240's review.
#[test]
fn desktop_mode_once_every_diagonal_is_spent_a_window_takes_a_free_spot_of_the_grid() {
    let mut desk = Desk::new(
        "require(\"config\").floating.atrium.enabled = false\n\
         local config = require(\"config\")\n\
         config.floating.placement.wrap = \"center\"\n",
        vec![screen("DP-1", NESTED)],
    );
    for id in 1..=9 {
        open_shown(&mut desk, id, 1200.0, 880.0);
    }
    spread_inside(&desk, NESTED, "1200x880, wrap = \"center\"");
    let diagonals = [200.0, 232.0, 264.0, 296.0, 328.0, 360.0, 392.0];
    for (id, x) in (1..=7).zip(diagonals) {
        let rect = desk.rect(id);
        assert!(
            about(rect.x, x) && about(rect.y, 10.0),
            "window {id} is not at {x},10: {:?}",
            desk.rects()
        );
    }
    for (id, (x, y)) in [(8, (0.0, 0.0)), (9, (32.0, 0.0))] {
        let rect = desk.rect(id);
        assert!(
            about(rect.x, x) && about(rect.y, y),
            "window {id} is not at {x},{y}: {:?}",
            desk.rects()
        );
    }
}

/// **Desktop mode: windows nearly as wide as the work area get a spot
/// each, whichever `wrap`**: no diagonal has room for even its first
/// spot, so each window after the first takes the next free spot of the
/// `cascade` grid, down the work area's left edge. #240's review: 1590x400
/// windows on 1600x900 all landed on the centre under both wraps.
#[test]
fn desktop_mode_windows_nearly_the_work_area_wide_get_spots_of_their_own() {
    for wrap in ["corner", "center"] {
        let mut desk = Desk::new(
            &format!(
                "require(\"config\").floating.atrium.enabled = false\n\
                 local config = require(\"config\")\n\
                 config.floating.placement.wrap = {wrap:?}\n"
            ),
            vec![screen("DP-1", NESTED)],
        );
        for id in 1..=4 {
            open_shown(&mut desk, id, 1590.0, 400.0);
        }
        spread_inside(&desk, NESTED, &format!("1590x400, wrap = {wrap:?}"));
        let wanted = [(5.0, 250.0), (0.0, 0.0), (0.0, 32.0), (0.0, 64.0)];
        for (id, (x, y)) in (1..=4).zip(wanted) {
            let rect = desk.rect(id);
            assert!(
                about(rect.x, x) && about(rect.y, y),
                "wrap = {wrap:?}: window {id} is not at {x},{y}: {:?}",
                desk.rects()
            );
        }
    }
}

/// **Tiling keeps the plain arrows for focus by direction, and desktop
/// mode gets them back the moment nothing else is in charge.** #222's
/// whole reason for going through `modes.watch` rather than binding the
/// four keys for good.
#[test]
fn tiling_keeps_the_arrows_for_focus_and_desktop_mode_gets_them_back() {
    let mut desk = grid("", one_screen());
    desk.focus(2);
    assert_eq!(
        focuses(&desk.press("super+left")),
        [1],
        "tiling: super+left still focuses the neighbouring tile"
    );

    // Self-toggle: tiling off is floating, #222's desktop mode.
    desk.press("super+t");
    desk.focus(1);
    let tiled = desk.rect(1);
    desk.press("super+left");
    let snapped = desk.rect(1);
    assert!(
        about(snapped.h, WIDE.h) && snapped.h > tiled.h + 100.0,
        "super+left did not snap once floating took over: tiled {tiled:?} snapped {snapped:?}"
    );
}

/// **A `config.bindings` override on a plain arrow survives leaving and
/// returning to desktop mode**: it must win once, at load (the premise
/// below), and go on winning every time `super+t`/`super+s` hands the
/// arrows back, not just until the first one (#222's review).
///
/// `require("bindings")` runs as `new_with`'s `after`, exactly where the
/// real `init.lua` has it -- last -- so `super+left`'s override wins the
/// same way it would in production, with no help from this file's own
/// fix: that part was never broken. What was broken is the *second*
/// assertion, once a mode switch has run `unbind_keys` at least once.
#[test]
fn a_config_bindings_override_on_an_arrow_survives_leaving_and_returning_to_floating() {
    let mut desk = Desk::new_with(
        "local config = require(\"config\")\n\
         config.bindings[\"super+left\"] = function() sol.status(\"custom-left\") end\n",
        "require(\"bindings\")\n",
        one_screen(),
    );

    let status = |desk: &mut Desk, combo: &str| {
        desk.scripts
            .key(combo, desk.snapshot())
            .status
            .unwrap_or_default()
    };
    assert_eq!(
        status(&mut desk, "super+left"),
        "custom-left",
        "the premise: the override wins while floating is in charge"
    );

    // Leave floating (tiling) and come back (self-toggle): both are
    // `unbind_keys`, the first into tiling and the second out of it.
    desk.press("super+t");
    assert_eq!(
        status(&mut desk, "super+left"),
        "custom-left",
        "leaving floating silently reverted super+left to the shipped default"
    );
    desk.press("super+t");
    assert_eq!(
        status(&mut desk, "super+left"),
        "custom-left",
        "returning to floating silently reverted super+left to the shipped default"
    );
}

/// **A snap key `config.floating.snap` has moved off its plain arrow is
/// not left bound once floating lets go.** `config.floating.snap.left =
/// "super+z"` moves what `super+left` used to mean off the arrow
/// entirely; leaving floating must give `super+z` back to nothing, not
/// leave it calling `snap("left")` in tiling for the rest of the session
/// (#222's review).
#[test]
fn a_snap_key_moved_off_the_plain_arrows_is_not_left_bound_once_floating_lets_go() {
    let mut desk = Desk::new(
        "local config = require(\"config\")\n\
         config.floating.snap.left = \"super+z\"\n",
        one_screen(),
    );
    desk.open(1, "DP-1", (1280.0, 720.0));

    assert!(
        desk.scripts.has_binding("super+z"),
        "the premise: super+z is bound while floating is in charge"
    );
    let commands = desk.press("super+z");
    assert_eq!(
        places(&commands),
        vec![1],
        "the premise: super+z snaps the focused window"
    );

    desk.press("super+t");
    assert!(
        !desk.scripts.has_binding("super+z"),
        "super+z is still bound after floating let go of it, to whatever it last did \
         with it -- it is not one of the four plain arrows, so nothing ever gives it \
         back to anything"
    );
}

/// **Desktop mode: crossing to the next monitor on a snap carries the
/// window's explicit workspace with it**, exactly as `direction.lua`'s own
/// cross-monitor `floating.move` does (`super+shift+left`), so a window
/// sent to the workspace its monitor happens to show does not end up
/// grouped onto a desk the monitor it lands on is not showing (#222's
/// review).
#[test]
fn desktop_mode_a_cross_monitor_snap_carries_the_window_to_the_workspace_its_new_monitor_shows() {
    let mut desk = Desk::new("", two_screens());
    desk.open(1, "DP-1", (1280.0, 720.0));

    // Explicitly assigned to workspace 1 -- which is also what DP-1,
    // unconfigured, already shows -- so the assignment changes nothing
    // visible yet, only `workspaces.of[1]`.
    desk.press("super+shift+1");
    assert_eq!(
        desk.workspace_of(1),
        "1",
        "the premise: window 1 has an explicit workspace"
    );

    // DP-2 shows workspace 2, the same fakery
    // `at_a_monitors_edge_tiling_crosses_to_the_desk_the_next_monitor_shows`
    // uses, since nothing in this harness switches a real workspace.
    assert_eq!(
        desk.scripts
            .evaluate("require(\"workspaces\").showing[\"DP-2\"] = 2 return \"\""),
        ""
    );

    desk.press("super+right");
    assert!(
        desk.monitor(1) == "DP-1",
        "the premise: one super+right only reaches DP-1's right half: {:?}",
        desk.rects()
    );
    desk.press("super+right");
    assert_eq!(
        desk.monitor(1),
        "DP-2",
        "a second super+right did not cross to DP-2: {:?}",
        desk.rects()
    );
    assert_eq!(
        desk.workspace_of(1),
        "2",
        "window 1 crossed to DP-2 but was not carried to the workspace it shows -- the \
         next `layout` would group it onto a desk DP-2 is not showing"
    );
}

/// **Desktop mode: snapping a window maximised through another door --
/// `super+shift+m`, or the frame's button -- does not capture a bogus
/// `before_snap` rectangle.** Lua reads `sol.windows()` before the
/// un-maximise this same key asks for has landed, so a window maximised
/// outside `floating.lua` has no valid pre-maximise rectangle this file
/// can capture at that point -- only the compositor remembers it, and a
/// snap straight after throws that memory away by placing the window
/// somewhere else. Restoring afterwards must not place the stale,
/// still-maximised-looking rectangle it would otherwise have recorded
/// (#222's review).
#[test]
fn desktop_mode_snap_after_an_external_maximize_does_not_capture_a_bogus_before_snap_rect() {
    let mut desk = Desk::new("", one_screen());
    desk.open(1, "DP-1", (1280.0, 720.0));

    let commands = desk.press("super+shift+m");
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, Command::ToggleMaximize { id: 1 })),
        "the premise: super+shift+m asks to maximise: {commands:?}"
    );
    let _ = desk.scripts.maximize(1, true, desk.snapshot());
    // What the compositor would have done to the window's own rectangle,
    // in production -- the harness does not simulate it, the same reason
    // `desktop_mode_maximize_asks_the_compositor_and_a_half_goes_to_its_quarter`
    // gives.
    for each in &mut desk.windows {
        if each.id == 1 {
            each.rect = WIDE;
        }
    }

    let commands = desk.press("super+left");
    assert_eq!(
        places(&commands),
        vec![1],
        "super+left did not un-maximise and snap: {commands:?}"
    );
    let left = desk.rect(1);

    let commands = desk.press("super+down");
    assert!(
        places(&commands).is_empty(),
        "restore after an externally-maximised window's first snap placed a bogus \
         rectangle rather than doing nothing: {commands:?}"
    );
    assert!(
        same(desk.rect(1), left),
        "restore moved the window with nothing valid to put it back to: {:?}",
        desk.rects()
    );
}

// ## The pointer follows the keyboard (#220)
//
// `lua/follow.lua` against the shipped layouts: which focus changes ask
// for a warp, to where, and how, by what moved the keyboard, and the
// two that tell no `focus` -- a move, and a switch to an empty
// workspace. The warp itself is the compositor's, and tested there:
// `state::tests::real_client::follow`.

/// The warps `commands` asked for, in order.
fn warps(commands: &[Command]) -> Vec<Warp> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Warp(warp) => Some(*warp),
            _ => None,
        })
        .collect()
}

/// A warp to window `id`'s `at`, along `motion`, that the mouse carries
/// on from where the pointer is.
fn to_window(id: u64, at: (f64, f64), motion: Option<AnimationSpec>) -> Warp {
    Warp {
        to: WarpTo::Window { id, at },
        motion,
        interrupt: Interrupt::Pointer,
    }
}

/// The shipped `focus.cursor.motion`.
fn shipped_motion() -> Option<AnimationSpec> {
    Some(AnimationSpec {
        duration: std::time::Duration::from_millis(260),
        easing: Curve::OutCubic,
    })
}

const CENTRE: (f64, f64) = (0.5, 0.5);

impl Desk {
    /// Press a bound key, and tell `focus` for every window it focused,
    /// with what it was answering, as the compositor does once the window
    /// has the keyboard. Everything asked for, in order.
    ///
    /// Each `focus` says whether the keyboard came from another window,
    /// as `Solium::focus_window` does: a layout focusing again the window
    /// that has it is told `changed = false`.
    fn press_told(&mut self, combo: &str) -> Vec<Command> {
        let mut had = self
            .windows
            .iter()
            .find(|window| window.focused)
            .map(|window| window.id);
        let mut commands = self.press(combo);
        let focused: Vec<(u64, FocusSource, bool)> = commands
            .iter()
            .filter_map(|command| match command {
                Command::Focus { id, source, .. } => {
                    let changed = had != Some(*id);
                    had = Some(*id);
                    Some((*id, *source, changed))
                }
                _ => None,
            })
            .collect();
        for (id, source, changed) in focused {
            commands.extend(self.told(id, source, changed));
        }
        commands
    }

    /// Window `id` takes the keyboard, moved by `source`, from another
    /// window if `changed`, and `focus` is told. What its listeners asked
    /// for.
    fn told(&mut self, id: u64, source: FocusSource, changed: bool) -> Vec<Command> {
        self.focus(id);
        let commands = self
            .scripts
            .focused(id, source, changed, self.snapshot())
            .commands;
        self.apply(&commands);
        commands
    }
}

/// **The pointer follows a key and not a click**: `super+left` in tiling
/// focuses the tile to the left, and its `focus` asks for a warp to that
/// window's centre along the shipped motion; any other key binding's
/// focus does too; a click, a bar's chip, a script, focus following the
/// mouse, a touch, the wheel, a drop, a window opening, an application
/// asking and the keyboard settling after a close ask for none.
#[test]
fn the_pointer_follows_a_key_and_not_a_click() {
    let mut desk = grid("", one_screen());
    desk.focus(4);
    let commands = desk.press_told("super+left");
    assert_eq!(
        (focuses(&commands), warps(&commands)),
        (vec![3], vec![to_window(3, CENTRE, shipped_motion())]),
        "(focused, warped): {commands:?}"
    );
    assert_eq!(
        warps(&desk.told(1, FocusSource::Binding, true)),
        vec![to_window(1, CENTRE, shipped_motion())],
        "a key binding's focus"
    );
    for source in [
        FocusSource::Click,
        FocusSource::Surface,
        FocusSource::Script,
        FocusSource::Hover,
        FocusSource::Touch,
        FocusSource::Scroll,
        FocusSource::Drop,
        FocusSource::Open,
        FocusSource::Activate,
        FocusSource::Settle,
    ] {
        assert_eq!(warps(&desk.told(2, source, true)), Vec::new(), "{source:?}");
    }
}

/// **Which focus changes take the pointer along, where to and how is
/// configured**: with `focus.cursor.follows.direction` off and `click`
/// on, `super+left` asks for no warp and a click does; `to` is where on
/// the window, `motion` how the picture flies, `interrupt` where the
/// mouse carries on from mid-flight, and `motion = false` puts it there
/// at once.
#[test]
fn which_focus_changes_take_the_pointer_along_where_to_and_how_is_configured() {
    let configured = "local cursor = require(\"config\").focus.cursor\n\
                      cursor.follows.direction = false\n\
                      cursor.follows.click = true\n\
                      cursor.to = { x = 0.5, y = 0.25 }\n\
                      cursor.motion = { duration = 100, easing = \"linear\" }\n\
                      cursor.interrupt = \"picture\"";
    let mut desk = grid(configured, one_screen());
    desk.focus(4);
    let commands = desk.press_told("super+left");
    assert_eq!(
        (focuses(&commands), warps(&commands)),
        (vec![3], Vec::new()),
        "direction off: (focused, warped)"
    );
    assert_eq!(
        warps(&desk.told(2, FocusSource::Click, true)),
        vec![Warp {
            interrupt: Interrupt::Picture,
            ..to_window(
                2,
                (0.5, 0.25),
                Some(AnimationSpec {
                    duration: std::time::Duration::from_millis(100),
                    easing: Curve::Linear,
                }),
            )
        }],
        "click on, to the top quarter, in 100 ms linear, the mouse going on from the picture"
    );

    let mut desk = grid(
        "require(\"config\").focus.cursor.motion = false",
        one_screen(),
    );
    desk.focus(4);
    assert_eq!(
        warps(&desk.press_told("super+left")),
        vec![to_window(3, CENTRE, None)],
        "motion = false: there at once"
    );
}

/// **A move takes the pointer to where the window went**: no `focus` is
/// told for a move, which keeps the keyboard on the window that moved,
/// so the `direction` listener asks for the warp to it -- after the
/// layout has placed it, so the compositor finds it where it went --
/// unless `focus.cursor.follows.move` is off.
#[test]
fn a_move_takes_the_pointer_to_where_the_window_went() {
    let mut desk = grid("", one_screen());
    desk.focus(4);
    let commands = desk.press_told("super+shift+left");
    let placed = commands
        .iter()
        .rposition(|command| matches!(command, Command::Place { id: 4, .. }));
    let warped = commands
        .iter()
        .position(|command| matches!(command, Command::Warp(_)));
    assert_eq!(
        (focuses(&commands), warps(&commands)),
        (Vec::new(), vec![to_window(4, CENTRE, shipped_motion())]),
        "(focused, warped): {commands:?}"
    );
    assert!(
        placed.is_some() && warped > placed,
        "the warp is asked for once the window is placed: {commands:?}"
    );

    let mut desk = grid(
        "require(\"config\").focus.cursor.follows.move = false",
        one_screen(),
    );
    desk.focus(4);
    assert_eq!(
        warps(&desk.press_told("super+shift+left")),
        Vec::new(),
        "follows.move off"
    );
}

/// **In scrolling, a key takes the pointer along only if it moves the
/// keyboard or the window that has it**: every key the strip binds ends
/// with the strip focusing its window again, and `focus` says
/// `changed = false` for the window that already had the keyboard. From
/// the first of three columns, `super+r`, which only widens the column,
/// asks for no warp; `super+comma` stacks the next column's window under
/// the first and focuses it, which takes the pointer to it;
/// `super+period` pushes it back out into a column of its own and
/// `super+ctrl+bracketleft` moves a column, and each takes the pointer
/// after the window it moved, as a move by direction does, unless
/// `focus.cursor.follows.move` is off; `focus.cursor.refocus = true`
/// takes the pointer along for `super+r` too.
#[test]
fn in_scrolling_a_key_takes_the_pointer_along_only_if_it_moves_the_keyboard_or_its_window() {
    let strip = |before: &str| {
        let mut desk = Desk::new(before, one_screen());
        for id in [1, 2, 3] {
            desk.windows.push(window(
                id,
                "DP-1",
                Rect {
                    w: 400.0,
                    h: 300.0,
                    ..WIDE
                },
            ));
        }
        desk.press("super+s");
        desk.press("super+bracketleft");
        desk.press("super+bracketleft");
        let x = |desk: &Desk, id| desk.rect(id).x;
        assert!(
            x(&desk, 1) < x(&desk, 2)
                && x(&desk, 2) < x(&desk, 3)
                && desk
                    .windows
                    .iter()
                    .any(|window| window.id == 1 && window.focused),
            "the premise: columns 1, 2, 3, and the first has the keyboard: {:?}",
            desk.rects()
        );
        desk
    };
    let to = |id| vec![to_window(id, CENTRE, shipped_motion())];

    let mut desk = strip("");
    let widened = desk.press_told("super+r");
    assert_eq!(
        (focuses(&widened), warps(&widened)),
        (vec![1], Vec::new()),
        "super+r: (focused again, warped)"
    );
    assert_eq!(warps(&desk.press_told("super+comma")), to(2), "super+comma");
    assert_eq!(
        warps(&desk.press_told("super+period")),
        to(2),
        "super+period"
    );
    assert_eq!(
        warps(&desk.press_told("super+bracketright")),
        to(3),
        "super+bracketright"
    );
    assert_eq!(
        warps(&desk.press_told("super+ctrl+bracketleft")),
        to(3),
        "super+ctrl+bracketleft"
    );

    let mut desk = strip("require(\"config\").focus.cursor.follows.move = false");
    assert_eq!(
        warps(&desk.press_told("super+comma")),
        to(2),
        "move off: super+comma"
    );
    assert_eq!(
        [
            warps(&desk.press_told("super+period")),
            warps(&desk.press_told("super+ctrl+bracketleft")),
        ],
        [Vec::new(), Vec::new()],
        "move off: [super+period, super+ctrl+bracketleft]"
    );

    let mut desk = strip("require(\"config\").focus.cursor.refocus = true");
    assert_eq!(
        warps(&desk.press_told("super+r")),
        to(1),
        "refocus on: super+r"
    );
}

/// **A switch to an empty workspace takes the pointer to the middle, or
/// leaves it**: `super+2` onto a workspace with no window asks for a warp
/// to the middle of the monitor that switched -- the one the pointer is
/// on, with two -- and `super+1` back focuses the window the keyboard
/// stayed on, and takes the pointer to it although its `focus` says the
/// keyboard did not move. `focus.cursor.empty_workspace =
/// "stay"` leaves it, and so does a key binding no longer taking it along.
#[test]
fn a_switch_to_an_empty_workspace_takes_the_pointer_to_the_middle_or_leaves_it() {
    let middle = |area: Rect| WarpTo::Point {
        x: area.x + area.w / 2.0,
        y: area.y + area.h / 2.0,
    };
    let mut desk = grid("", one_screen());
    let commands = desk.press_told("super+2");
    assert_eq!(
        (focuses(&commands), warps(&commands)),
        (
            Vec::new(),
            vec![Warp {
                to: middle(WIDE),
                motion: shipped_motion(),
                interrupt: Interrupt::Pointer,
            }]
        ),
        "onto an empty workspace: (focused, warped)"
    );
    let commands = desk.press_told("super+1");
    let back = focuses(&commands);
    assert_eq!(
        warps(&commands),
        back.iter()
            .map(|id| to_window(*id, CENTRE, shipped_motion()))
            .collect::<Vec<_>>(),
        "back: the window focused takes the pointer, {back:?}"
    );
    assert_eq!(back.len(), 1, "back: one window focused");

    for configured in [
        "require(\"config\").focus.cursor.empty_workspace = \"stay\"",
        "require(\"config\").focus.cursor.follows.binding = false",
    ] {
        let mut desk = grid(configured, one_screen());
        assert_eq!(
            warps(&desk.press_told("super+2")),
            Vec::new(),
            "{configured}"
        );
    }

    let mut desk = Desk::new("", two_screens());
    for monitor in &mut desk.monitors {
        monitor.focused = monitor.name == "DP-2";
    }
    assert_eq!(
        warps(&desk.press_told("super+2")),
        vec![Warp {
            to: middle(TALL),
            motion: shipped_motion(),
            interrupt: Interrupt::Pointer,
        }],
        "two monitors: the middle of the one the pointer is on"
    );
}

/// **The `focus` event says what moved the keyboard**, and
/// `sol.focus_source()` says the same in the handler: a key binding's
/// `sol.focus` is `"binding"`, one in `sol.focus_direction`'s or
/// `sol.move_direction`'s listeners `"direction"` or `"move"` -- and the
/// binding's again once that returns --, a scene's action `"surface"`, a
/// click while a script holds input `"click"`, and the `open`,
/// `activate`, `scroll` and `drop` listeners theirs; anything else is
/// `"script"`. The compositor's own are told as it says them.
#[test]
fn the_focus_event_says_what_moved_the_keyboard() {
    let mut scripts = plain(
        "focus-source",
        r#"
        heard = {}
        local function note(id) sol.focus(id) heard[#heard + 1] = sol.focus_source() end
        sol.on("focus", function(id, source) heard[#heard + 1] = id .. " " .. source end)
        sol.on("direction", function(verb) note(verb == "focus" and 2 or 3) end)
        sol.bind("super+a", function() note(1) end)
        sol.bind("super+d", function() sol.focus_direction("left") note(4) end)
        sol.bind("super+m", function() sol.move_direction("left") end)
        sol.on("surface", function() note(5) end)
        sol.on("click", function() note(6) end)
        sol.on("open", function(id) note(id) end)
        sol.on("activate", function(id) note(id) end)
        sol.on("scroll", function() note(8) end)
        sol.on("drop", function(id) note(id) end)
        sol.on("layout", function() note(9) end)
        "#,
    );
    let sources = |outcome: super::Outcome| -> Vec<(u64, &'static str)> {
        outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Focus { id, source, .. } => {
                    Some((*id, crate::wm::convert::focus_cause(*source).name()))
                }
                _ => None,
            })
            .collect()
    };
    let none = Snapshot::default;
    assert_eq!(
        [
            sources(scripts.key("super+a", none())),
            sources(scripts.key("super+d", none())),
            sources(scripts.key("super+m", none())),
            sources(scripts.surface_action("bar", "go", &crate::json::Json::Null, none())),
            sources(scripts.click(1.0, 1.0, none())),
            sources(scripts.opened(7, none())),
            sources(scripts.activated(7, "request", none())),
            sources(scripts.scrolled(0.0, 1.0, none())),
            sources(scripts.dropped(7, 0.0, 0.0, none())),
            sources(scripts.relayout(none())),
        ],
        [
            vec![(1, "binding")],
            vec![(2, "direction"), (4, "binding")],
            vec![(3, "move")],
            vec![(5, "surface")],
            vec![(6, "click")],
            vec![(7, "open")],
            vec![(7, "activate")],
            vec![(8, "scroll")],
            vec![(7, "drop")],
            vec![(9, "script")],
        ],
        "what each `sol.focus` was asked with"
    );
    let _ = scripts.focused(7, FocusSource::Hover, true, none());
    let _ = scripts.focused(7, FocusSource::Settle, false, none());
    assert_eq!(
        scripts.evaluate("return table.concat(heard, \",\")"),
        "binding,direction,binding,move,surface,click,open,activate,scroll,drop,script,\
         7 hover,7 settle",
        "what `sol.focus_source()` said in each handler, then two `focus` events"
    );
}

/// **`sol.focus` carries its source and its warp**: a warp table is
/// `sol.warp`'s options aimed at the window focused, `true` is its centre
/// at once, none is none, a motion with nothing in it is the default
/// one, and a warp of any other kind is an error.
#[test]
fn sol_focus_carries_its_source_and_its_warp() {
    let mut scripts = plain(
        "focus-warp",
        r#"
        sol.bind("super+a", function()
            sol.focus(3, { warp = { to = { x = 0.25, y = 0.75 }, motion = { duration = 300, easing = "linear" } } })
        end)
        sol.bind("super+b", function() sol.focus(4, { warp = true }) end)
        sol.bind("super+c", function() sol.focus(5) end)
        sol.bind("super+d", function() sol.focus(6, { warp = { motion = {} } }) end)
        sol.bind("super+e", function()
            local ok, err = pcall(sol.focus, 7, { warp = "yes" })
            sol.status(tostring(ok) .. ": " .. tostring(err))
        end)
        "#,
    );
    let focus = |scripts: &mut Scripts, combo: &str| {
        scripts
            .key(combo, Snapshot::default())
            .commands
            .into_iter()
            .filter_map(|command| match command {
                Command::Focus { id, source, warp } => Some((id, source, warp)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        [
            focus(&mut scripts, "super+a"),
            focus(&mut scripts, "super+b"),
            focus(&mut scripts, "super+c"),
            focus(&mut scripts, "super+d"),
        ],
        [
            vec![(
                3,
                FocusSource::Binding,
                Some(to_window(
                    3,
                    (0.25, 0.75),
                    Some(AnimationSpec {
                        duration: std::time::Duration::from_millis(300),
                        easing: Curve::Linear,
                    }),
                )),
            )],
            vec![(4, FocusSource::Binding, Some(to_window(4, CENTRE, None)))],
            vec![(5, FocusSource::Binding, None)],
            vec![(
                6,
                FocusSource::Binding,
                Some(to_window(6, CENTRE, Some(AnimationSpec::default()))),
            )],
        ]
    );
    let refused = scripts
        .key("super+e", Snapshot::default())
        .status
        .unwrap_or_default();
    assert!(
        refused.starts_with("false: ") && refused.contains("warp is true, false or a table"),
        "{refused:?}"
    );
}

/// **`sol.warp` names a window or a point, and a bad one is an error**:
/// a window's id is its centre unless `to` says, a point is a point with
/// a motion's duration and the default easing, a spring is a spring, and
/// the mouse carries on from the pointer mid-flight unless `interrupt`
/// says `"picture"`; anything but an id or a point, half a point, `to`
/// with a point, a `to` that is neither `"center"` nor a share, half a
/// share, a motion that is not a table, a negative id and an `interrupt`
/// that is neither `"pointer"` nor `"picture"` are errors, and ask for
/// nothing.
#[test]
fn sol_warp_names_a_window_or_a_point_and_a_bad_one_is_an_error() {
    let mut scripts = plain(
        "warp",
        r#"
        sol.bind("super+a", function() sol.warp(5) end)
        sol.bind("super+b", function() sol.warp({ x = 10, y = 20 }, { motion = { duration = 100 } }) end)
        sol.bind("super+c", function() sol.warp(5, { to = "center", motion = { easing = "spring" } }) end)
        sol.bind("super+d", function()
            sol.warp(5, { interrupt = "picture" })
            sol.warp(6, { interrupt = "pointer" })
        end)
        sol.bind("super+e", function()
            local said = {}
            for _, call in ipairs({
                function() sol.warp("five") end,
                function() sol.warp({ x = 1 }) end,
                function() sol.warp({ x = 1, y = 2 }, { to = "center" }) end,
                function() sol.warp(5, { to = "middle" }) end,
                function() sol.warp(5, { to = { x = 0.5 } }) end,
                function() sol.warp(5, { motion = 3 }) end,
                function() sol.warp(-1) end,
                function() sol.warp(5, { interrupt = "wherever" }) end,
            }) do
                said[#said + 1] = tostring((pcall(call)))
            end
            sol.status(table.concat(said, ","))
        end)
        "#,
    );
    let warped = |scripts: &mut Scripts, combo: &str| {
        warps(&scripts.key(combo, Snapshot::default()).commands)
    };
    assert_eq!(
        [
            warped(&mut scripts, "super+a"),
            warped(&mut scripts, "super+b"),
            warped(&mut scripts, "super+c"),
            warped(&mut scripts, "super+d"),
        ],
        [
            vec![to_window(5, CENTRE, None)],
            vec![Warp {
                to: WarpTo::Point { x: 10.0, y: 20.0 },
                motion: Some(AnimationSpec {
                    duration: std::time::Duration::from_millis(100),
                    easing: Curve::OutCubic,
                }),
                interrupt: Interrupt::Pointer,
            }],
            vec![to_window(
                5,
                CENTRE,
                Curve::from_name("spring").map(|easing| AnimationSpec {
                    easing,
                    ..AnimationSpec::default()
                }),
            )],
            vec![
                Warp {
                    interrupt: Interrupt::Picture,
                    ..to_window(5, CENTRE, None)
                },
                to_window(6, CENTRE, None),
            ],
        ]
    );
    let outcome = scripts.key("super+e", Snapshot::default());
    assert_eq!(
        (outcome.status.unwrap_or_default(), warps(&outcome.commands)),
        (
            "false,false,false,false,false,false,false,false".to_owned(),
            Vec::new()
        ),
        "every bad call is an error, and none asks for a warp"
    );
}

/// Atrium's stage and strip (#449), on `WIDE`, whose strip is the left
/// 220 pixels with its one card's slot at (22, 657), 176 by 126.
mod atrium {
    use super::{Command, Desk, FocusSource, Rect, Snapshot, one_screen};
    use std::time::Duration;

    /// [`desk`], with the strip hiding as `hide` says.
    fn desk_hiding(hide: &str) -> Desk {
        let mut desk = desk();
        desk.scripts.evaluate(&format!(
            "require(\"config\").floating.atrium.hide = \"{hide}\"\nreturn \"\""
        ));
        let snapshot = desk.snapshot();
        act(&mut desk, "leave", "{}", snapshot);
        desk
    }

    /// Window 2 on the stage clear of the strip, window 1 the card.
    fn desk() -> Desk {
        let at = |x: f64| Rect {
            x,
            y: 100.0,
            w: 800.0,
            h: 600.0,
        };
        let mut desk = Desk::floating(
            one_screen(),
            &[(2, "DP-1", at(600.0)), (1, "DP-1", at(700.0))],
        );
        let snapshot = desk.snapshot();
        act(&mut desk, "leave", "{}", snapshot);
        desk
    }

    fn act(desk: &mut Desk, action: &str, data: &str, snapshot: Snapshot) -> super::super::Outcome {
        let data = crate::json::Json::parse(data).expect("valid JSON");
        desk.scripts
            .surface_action("atrium@DP-1", action, &data, snapshot)
    }

    fn set_rect(desk: &mut Desk, id: u64, rect: Rect) {
        if let Some(window) = desk.windows.iter_mut().find(|window| window.id == id) {
            window.rect = rect;
        }
    }

    /// Window `id`'s last present in `commands`: its rect, opacity, how
    /// long it moves and whether it named a timing per property.
    fn presented(
        commands: &[Command],
        id: u64,
    ) -> Option<(Option<Rect>, Option<f32>, Duration, bool)> {
        commands.iter().rev().find_map(|command| match command {
            Command::Present {
                id: each,
                rect,
                opacity,
                animation,
                channels,
                ..
            } if *each == id => Some((*rect, *opacity, animation.duration, channels.is_some())),
            _ => None,
        })
    }

    /// **A stage window maximised over the strip sends the strip away,
    /// and its restore brings it back** -- the cards with their icons:
    /// the cards are drawn under the stage but the icons over every
    /// window, so a strip left out under a maximised window showed its
    /// icons on top of it. Answered without `sol.animate`, which in a
    /// `maximize` listener would time the maximise itself.
    #[test]
    fn a_stage_window_maximised_over_the_strip_sends_the_strip_away() {
        let mut desk = desk_hiding("overlap");
        let restored = desk.rect(2);
        set_rect(&mut desk, 2, super::WIDE);
        let outcome = desk.scripts.maximize(2, true, desk.snapshot());
        let card = presented(&outcome.commands, 1).expect("the card is drawn again");
        assert_eq!(card.1, Some(0.0), "the strip slid away: {card:?}");
        assert!(card.3, "the card names its own timing");
        assert!(
            outcome.motion.is_none(),
            "the maximise keeps its own timing: {:?}",
            outcome.motion
        );

        set_rect(&mut desk, 2, restored);
        let outcome = desk.scripts.maximize(2, false, desk.snapshot());
        let card = presented(&outcome.commands, 1).expect("the card is drawn again");
        assert_eq!(card.1, Some(1.0), "the strip is back: {card:?}");
        assert!(outcome.motion.is_none());
    }

    /// **An edge dragged over the strip sends it away**, and an edge
    /// dragged on while it is away draws nothing again.
    #[test]
    fn a_stage_window_resized_over_the_strip_sends_the_strip_away() {
        let mut desk = desk_hiding("overlap");
        let wider = Rect {
            x: 100.0,
            y: 100.0,
            w: 1300.0,
            h: 600.0,
        };
        set_rect(&mut desk, 2, wider);
        let outcome =
            desk.scripts
                .resized(2, (100.0, 400.0), (Some("left"), None), desk.snapshot());
        let card = presented(&outcome.commands, 1).expect("the card is drawn again");
        assert_eq!(card.1, Some(0.0), "the strip slid away: {card:?}");

        set_rect(
            &mut desk,
            2,
            Rect {
                x: 90.0,
                w: 1310.0,
                ..wider
            },
        );
        let outcome = desk
            .scripts
            .resized(2, (90.0, 400.0), (Some("left"), None), desk.snapshot());
        assert_eq!(
            presented(&outcome.commands, 1),
            None,
            "nothing changed for the strip"
        );
    }

    /// The held card 1 dragged to `x`, 720, as the strip's scene says,
    /// and ticks of its frame clock, each `dt` seconds; its front window
    /// as last drawn.
    fn ticks(desk: &mut Desk, x: f64, dt: f64, count: usize) -> (Rect, Duration, bool) {
        let snapshot = desk.snapshot();
        act(
            desk,
            "drag",
            &format!(r#"{{"card":1,"x":{x},"y":720}}"#),
            snapshot,
        );
        let mut last = None;
        for _ in 0..count {
            let snapshot = desk.snapshot();
            let outcome = act(desk, "tick", &format!(r#"{{"dt":{dt}}}"#), snapshot);
            last = presented(&outcome.commands, 1).or(last);
        }
        let (rect, _, duration, channels) = last.expect("the held card is drawn each frame");
        (rect.expect("a rect"), duration, channels)
    }

    /// **A card dragged out of the strip is drawn once a frame, on the
    /// pointer while it is flat and easing after it while it is turned**,
    /// as macOS has it: the strip's tick draws it where the scene last
    /// said the drag was -- not at each of the scene's pointer events,
    /// which arrive unevenly between frames -- and no
    /// spring is put on top -- a present's spring starts again from rest
    /// at each new target, and trailed the pointer.
    #[test]
    fn a_dragged_card_follows_the_pointer_flat_and_eases_after_it_turned() {
        let mut desk = desk();
        desk.cursor = (100.0, 720.0);
        let snapshot = desk.snapshot();
        act(
            &mut desk,
            "drag",
            r#"{"card":1,"x":100,"y":720,"px":100,"py":712}"#,
            snapshot,
        );
        let snapshot = desk.snapshot();
        let again = act(
            &mut desk,
            "drag",
            r#"{"card":1,"x":300,"y":720,"px":100,"py":712}"#,
            snapshot,
        );
        assert_eq!(
            presented(&again.commands, 1),
            None,
            "a pointer event draws nothing"
        );

        // Pulled well past the strip: a window at its own size, carried
        // by its centre, under the pointer.
        let (window, duration, channels) = ticks(&mut desk, 900.0, 0.05, 20);
        assert_eq!(
            (duration, channels),
            (Duration::ZERO, false),
            "drawn, not animated"
        );
        assert_eq!((window.w, window.h), (800.0, 600.0));
        assert!(
            (window.x + 400.0 - 900.0).abs() < 0.5 && (window.y + 300.0 - 720.0).abs() < 0.5,
            "carried by its centre: {window:?}"
        );
        // Flat, it eases after the pointer only by `steady`: part of the
        // way in one 4 ms frame, the rest within a few more.
        let (step, ..) = ticks(&mut desk, 960.0, 0.004, 1);
        assert!(
            step.x > window.x + 5.0 && step.x < window.x + 60.0,
            "{window:?} then {step:?}"
        );
        let (moved, ..) = ticks(&mut desk, 960.0, 0.05, 5);
        assert!(
            (moved.x - window.x - 60.0).abs() < 0.5,
            "{window:?} then {moved:?}"
        );

        // Back towards the strip, turning: one frame later it is still
        // nearly where it was, and it gets there over the frames after.
        let (eased, ..) = ticks(&mut desk, 400.0, 0.004, 1);
        assert!(
            eased.w > 790.0 && eased.x > moved.x - 200.0,
            "eased after the pointer: {eased:?}"
        );
        let (arrived, ..) = ticks(&mut desk, 400.0, 0.05, 40);
        assert!(
            arrived.w < 700.0,
            "turned the way the pointer's place says: {arrived:?}"
        );
    }

    /// **A hovered card picked up does not move at all**: lifted, out
    /// towards the stage and flat, held by the point the pointer is on
    /// as the drag begins -- not snapped back to its slot's size, nor
    /// hopping by the drag threshold to where the press began.
    #[test]
    fn a_hovered_card_picked_up_does_not_jump() {
        let mut desk = desk();
        // The strip waits off the edge while the stage holds a window, so
        // a card can only be hovered once the pointer has brought it out.
        let snapshot = desk.snapshot();
        act(&mut desk, "edge", "{}", snapshot);
        let snapshot = desk.snapshot();
        let hovered = act(&mut desk, "hover", r#"{"card":1}"#, snapshot);
        let lifted = presented(&hovered.commands, 1)
            .expect("the hovered card is drawn")
            .0
            .expect("a rect");
        desk.cursor = (108.0, 720.0);
        let snapshot = desk.snapshot();
        let picked = act(
            &mut desk,
            "drag",
            r#"{"card":1,"x":108,"y":720,"px":100,"py":720}"#,
            snapshot,
        );
        let held = presented(&picked.commands, 1)
            .expect("the picked card is drawn")
            .0
            .expect("a rect");
        for (was, is) in [
            (lifted.x, held.x),
            (lifted.y, held.y),
            (lifted.w, held.w),
            (lifted.h, held.h),
        ] {
            assert!((was - is).abs() < 0.5, "lifted {lifted:?}, held {held:?}");
        }
    }

    /// **While the stage holds a window the strip waits off the edge**,
    /// comes out over the windows while the pointer is at the edge, and
    /// slides away again on its softer spring once the pointer has left
    /// it (the scene sends `leave` half a second after).
    #[test]
    fn the_strip_waits_at_the_edge_while_the_stage_holds_a_window() {
        let mut desk = desk();
        let snapshot = desk.snapshot();
        let away = act(&mut desk, "leave", "{}", snapshot);
        let card = presented(&away.commands, 1).expect("the card is drawn");
        assert_eq!(
            (card.1, card.3),
            (Some(0.0), true),
            "off the edge, on its own spring: {card:?}"
        );
        let snapshot = desk.snapshot();
        let out = act(&mut desk, "edge", "{}", snapshot);
        let card = presented(&out.commands, 1).expect("the card is drawn");
        assert_eq!(card.1, Some(1.0), "out at the edge: {card:?}");
        let z = out.commands.iter().rev().find_map(|command| match command {
            Command::Present { id: 1, z, .. } => Some(*z),
            _ => None,
        });
        assert!(
            z.is_some_and(|z| z > 0.0),
            "over the stage's windows: {z:?}"
        );
    }

    /// **A card dropped onto another card joins its group**, in front,
    /// settling over its windows, and the stage is left as it is. Two cards, so their places
    /// are 562 and 752 down, 176 by 126 at 22 across.
    #[test]
    fn a_card_dropped_onto_another_card_joins_its_group() {
        let at = |x: f64| Rect {
            x,
            y: 100.0,
            w: 800.0,
            h: 600.0,
        };
        let mut desk = Desk::floating(
            one_screen(),
            &[
                (3, "DP-1", at(600.0)),
                (2, "DP-1", at(700.0)),
                (1, "DP-1", at(800.0)),
            ],
        );
        let snapshot = desk.snapshot();
        act(&mut desk, "leave", "{}", snapshot);
        assert_eq!(groups(&desk), "3|2|1");

        desk.cursor = (100.0, 620.0);
        let snapshot = desk.snapshot();
        act(&mut desk, "drag", r#"{"card":1,"x":100,"y":620}"#, snapshot);
        let snapshot = desk.snapshot();
        act(&mut desk, "drag", r#"{"card":1,"x":100,"y":800}"#, snapshot);
        let snapshot = desk.snapshot();
        let over = act(&mut desk, "tick", r#"{"dt":0.004}"#, snapshot);
        assert!(
            presented(&over.commands, 1).is_some(),
            "the card it would join comes forward: {:?}",
            over.commands
        );
        let snapshot = desk.snapshot();
        act(&mut desk, "drop", r#"{"card":1,"x":100,"y":800}"#, snapshot);
        assert_eq!(
            groups(&desk),
            "3|2,1",
            "one group, the dropped card in front, the stage as it was"
        );
    }

    /// **A stage window dropped onto a card joins that card's group**, in
    /// front, the stage left bare behind it -- not a card of its own.
    /// One card, so its place is 657 down, 176 by 126 at 22 across.
    #[test]
    fn a_stage_window_dropped_onto_a_card_joins_its_group() {
        let mut desk = desk();
        desk.drop(2, "DP-1", (100.0, 700.0));
        assert_eq!(groups(&desk), "|2,1", "a bare stage and one card of both");
    }

    /// **A stage window dropped on the strip away from every card is a
    /// card of its own**, just under the stage.
    #[test]
    fn a_stage_window_dropped_between_cards_is_a_card_of_its_own() {
        let mut desk = desk();
        desk.drop(2, "DP-1", (100.0, 300.0));
        assert_eq!(groups(&desk), "|2|1");
    }

    /// **A window let go into a card stays where it was let go until
    /// it comes back.** Moved out of the strip's way there and then,
    /// while it was drawn as a card, it glided to that place at its own
    /// size and its card was undone; it is moved clear of the strip as
    /// it comes back to the stage instead, on the glide that brings it
    /// from its card, just past the strip's inner edge.
    #[test]
    fn a_window_let_go_into_a_card_is_moved_clear_of_the_strip_as_it_comes_back() {
        let mut desk = desk();
        desk.drop(2, "DP-1", (100.0, 700.0));
        assert_eq!(groups(&desk), "|2,1");
        let dropped = desk.rect(2);
        assert_eq!(
            (dropped.x, dropped.y),
            (-300.0, 400.0),
            "where it was let go"
        );

        let snapshot = desk.snapshot();
        let outcome = act(&mut desk, "pick", r#"{"card":1}"#, snapshot);
        desk.apply(&outcome.commands);
        assert!(groups(&desk).starts_with("2,1"), "{}", groups(&desk));
        let back = desk.rect(2);
        assert_eq!(
            (back.x, back.y, back.w, back.h),
            (244.0, 400.0, 800.0, 600.0),
            "clear of the strip, at its own size"
        );
        assert_eq!(desk.rect(1).x, 700.0, "the other was clear of it");
    }

    /// **A card turns no faster than `drag.turn` lets it**: pulled far
    /// out of the strip in one 50 ms frame it has turned a third of the
    /// way into its windows, where it would be all the way, the eased
    /// point already past `drag.reach`; and with `turn = 0`, it is.
    #[test]
    fn a_card_pulled_out_in_one_frame_turns_over_several() {
        let pulled = |turn: Option<u32>| {
            let mut desk = desk();
            if let Some(turn) = turn {
                desk.scripts.evaluate(&format!(
                    "require(\"config\").floating.atrium.drag.turn = {turn}\nreturn \"\""
                ));
            }
            desk.cursor = (100.0, 720.0);
            let snapshot = desk.snapshot();
            act(&mut desk, "drag", r#"{"card":1,"x":100,"y":720}"#, snapshot);
            ticks(&mut desk, 1200.0, 0.05, 1).0
        };
        let shipped = pulled(None);
        assert!(
            shipped.w > 300.0 && shipped.w < 500.0,
            "a third of the way: {shipped:?}"
        );
        assert_eq!(pulled(Some(0)).w, 800.0, "all of the way at once");
    }

    /// [`desk`], with the strip a row along the bottom, which on `WIDE`
    /// is six cards' places, 230 apart, from 590 to 1970 across, its
    /// inner edge 1220 down.
    fn desk_along_the_bottom(hide: &str) -> Desk {
        let mut desk = desk();
        desk.scripts.evaluate(&format!(
            "local atrium = require(\"config\").floating.atrium\n\
             atrium.side = \"bottom\"\n\
             atrium.hide = \"{hide}\"\n\
             return \"\""
        ));
        let snapshot = desk.snapshot();
        act(&mut desk, "leave", "{}", snapshot);
        desk
    }

    /// **Along the bottom a card stands in the order its group was last
    /// used, the most recent on the left**, the row centred: a card
    /// coming or going only slides the others along. From the middle
    /// out, one side and then the other, a card leaving sent every card
    /// after it across to the other side, through the cards there.
    #[test]
    fn along_the_bottom_the_most_recent_card_is_on_the_left() {
        let at = |x: f64| Rect {
            x,
            y: 100.0,
            w: 800.0,
            h: 600.0,
        };
        let mut desk = Desk::floating(
            one_screen(),
            &[
                (4, "DP-1", at(600.0)),
                (3, "DP-1", at(700.0)),
                (2, "DP-1", at(800.0)),
                (1, "DP-1", at(900.0)),
            ],
        );
        desk.scripts.evaluate(
            "local atrium = require(\"config\").floating.atrium\n\
             atrium.side = \"bottom\"\n\
             atrium.hide = \"never\"\n\
             return \"\"",
        );
        let snapshot = desk.snapshot();
        let outcome = act(&mut desk, "leave", "{}", snapshot);
        assert_eq!(groups(&desk), "4|3|2|1");
        let card = |id: u64| {
            presented(&outcome.commands, id)
                .and_then(|drawn| drawn.0)
                .expect("the card is drawn")
        };
        let (three, two, one) = (card(3), card(2), card(1));
        assert!(
            three.x < two.x && two.x < one.x,
            "{three:?} {two:?} {one:?}"
        );
        let middle = (three.x + one.x + one.w) / 2.0;
        assert!((middle - 1280.0).abs() < 1.0, "centred: {middle}");
    }

    /// **A window dragged beside the row along the bottom is not turned
    /// into a card**: past the row's ends the way along counts as well
    /// as the way up, so a window dragged to a side or a bottom corner
    /// to snap there stays a window, and one dragged down onto the row
    /// turns. Measured up from the strip alone, a window dragged to the
    /// right edge to snap half the screen leaned back as it got there.
    #[test]
    fn a_window_dragged_beside_the_row_along_the_bottom_is_not_turned() {
        let desk = desk_along_the_bottom("never");
        let closeness = |x: f64, y: f64| -> f64 {
            desk.scripts
                .evaluate(&format!(
                    "local area = require(\"monitors\").named(\"DP-1\")\n\
                     return tostring(require(\"atrium\").closeness(area, {x}, {y}))"
                ))
                .parse()
                .expect("a number")
        };
        let over = closeness(1280.0, 1120.0);
        assert!(over > 0.6 && over < 0.8, "over the row: {over}");
        assert_eq!(closeness(2555.0, 1120.0), 0.0, "at the right edge");
        assert_eq!(closeness(2500.0, 1435.0), 0.0, "in the bottom corner");
        assert_eq!(closeness(5.0, 1300.0), 0.0, "at the left edge");
    }

    /// **Along the bottom too, a hovered card picked up does not move**:
    /// it is lifted up out of the row about its bottom edge, and held as
    /// it is drawn. It was held as the strip on the left lifts one,
    /// sideways, and hopped there as the drag began.
    #[test]
    fn a_hovered_card_picked_up_along_the_bottom_does_not_jump() {
        let mut desk = desk_along_the_bottom("stage");
        let snapshot = desk.snapshot();
        act(&mut desk, "edge", "{}", snapshot);
        let snapshot = desk.snapshot();
        let hovered = act(&mut desk, "hover", r#"{"card":1}"#, snapshot);
        let lifted = presented(&hovered.commands, 1)
            .expect("the hovered card is drawn")
            .0
            .expect("a rect");
        // The strip's scene begins `strip` and `drag.reach` above the
        // bottom: 1440 - 220 - 320.
        let (x, y) = (lifted.x + lifted.w / 2.0, lifted.y + lifted.h / 2.0);
        desk.cursor = (x, y);
        let snapshot = desk.snapshot();
        let picked = act(
            &mut desk,
            "drag",
            &format!(r#"{{"card":1,"x":{x},"y":{}}}"#, y - 900.0),
            snapshot,
        );
        let held = presented(&picked.commands, 1)
            .expect("the picked card is drawn")
            .0
            .expect("a rect");
        for (was, is) in [
            (lifted.x, held.x),
            (lifted.y, held.y),
            (lifted.w, held.w),
            (lifted.h, held.h),
        ] {
            assert!((was - is).abs() < 0.5, "lifted {lifted:?}, held {held:?}");
        }
    }

    /// The stage's group and the cards behind it, as `"3,2|1"`: each
    /// group's windows front first, the stage first.
    fn groups(desk: &Desk) -> String {
        desk.scripts.evaluate(
            "local out = {}\n\
             for _, groups in pairs(sol.keep(\"atrium\", {}).desks) do\n\
                 for _, group in ipairs(groups) do out[#out + 1] = table.concat(group, \",\") end\n\
             end\n\
             return table.concat(out, \"|\")",
        )
    }

    fn closing(desk: &mut Desk, id: u64) {
        if let Some(window) = desk.windows.iter_mut().find(|window| window.id == id) {
            window.leaving = true;
        }
        let snapshot = desk.snapshot();
        let outcome = desk.scripts.closing(id, snapshot);
        desk.apply(&outcome.commands);
    }

    /// **A window closed on the stage leaves the stage as it was**: the
    /// keyboard the close hands to a card's window (floating's
    /// `after_close` is the window used before, most often a card's)
    /// goes to the stage's next window instead, and no card comes
    /// forward.
    #[test]
    fn a_window_closed_on_the_stage_keeps_the_stage() {
        let mut desk = desk();
        desk.windows.insert(
            0,
            super::window(
                3,
                "DP-1",
                Rect {
                    x: 640.0,
                    y: 140.0,
                    w: 800.0,
                    h: 600.0,
                },
            ),
        );
        for window in &mut desk.windows {
            if window.id != 1 {
                window.app_id = "editor".to_owned();
            }
        }
        let snapshot = desk.snapshot();
        act(&mut desk, "leave", "{}", snapshot);
        assert_eq!(
            groups(&desk),
            "2,3|1",
            "the editor's two windows on the stage"
        );

        closing(&mut desk, 2);
        let snapshot = desk.snapshot();
        let outcome = desk.scripts.focused(1, FocusSource::Settle, true, snapshot);
        assert!(
            outcome
                .commands
                .iter()
                .any(|command| matches!(command, Command::Focus { id: 3, .. })),
            "the keyboard goes to the stage's next window: {:?}",
            outcome.commands
        );
        assert_eq!(groups(&desk), "3|1", "the card stays a card");
    }

    /// **The stage's last window closed leaves the stage bare**, the
    /// desktop showing, until a card is picked or a window opens -- not
    /// the next card brought forward in its place.
    #[test]
    fn the_stages_last_window_closed_leaves_it_bare() {
        let mut desk = desk();
        closing(&mut desk, 2);
        assert_eq!(groups(&desk), "|1", "a bare stage and the card behind it");
        let snapshot = desk.snapshot();
        desk.scripts.focused(1, FocusSource::Settle, true, snapshot);
        assert_eq!(
            groups(&desk),
            "|1",
            "the keyboard handed over brings nothing forward"
        );
    }
}
