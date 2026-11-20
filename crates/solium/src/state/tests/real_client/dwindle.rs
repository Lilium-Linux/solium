//! The shipped dwindle tree, `layouts/dwindle.lua`, held from Rust for the
//! tests that lay a real tree out through `Solium::place` and drag its seams
//! as the tiling mode does: the arithmetic under test is the shipped
//! configuration's, run as it is.

use mlua::{Function, Lua, Table, Value};

/// A rectangle as the tree hands it back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rect {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) w: f64,
    pub(super) h: f64,
}

impl Rect {
    pub(super) fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }
}

/// What the tree is laid out with besides the area.
#[derive(Clone, Copy, Debug)]
pub(super) struct Settings {
    pub(super) gap: f64,
    pub(super) split: f64,
}

/// A window's side, as the `resize` event names it.
#[derive(Clone, Copy, Debug)]
pub(super) enum Edge {
    Right,
}

impl Edge {
    fn name(self) -> &'static str {
        match self {
            Self::Right => "right",
        }
    }
}

/// A tree, in a Lua of its own.
pub(super) struct Tiling {
    lua: Lua,
    tree: Table,
}

const DWINDLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../solium-lua/lua/layouts/dwindle.lua"
);

fn options(lua: &Lua, area: Rect, settings: Settings) -> Table {
    let table = lua.create_table().expect("a table");
    for (key, value) in [
        ("x", area.x),
        ("y", area.y),
        ("w", area.w),
        ("h", area.h),
        ("gap", settings.gap),
        ("split", settings.split),
    ] {
        table.set(key, value).expect("an option");
    }
    table
}

impl Tiling {
    pub(super) fn new() -> Self {
        let lua = Lua::new();
        let source = std::fs::read_to_string(DWINDLE).expect("the shipped dwindle.lua reads");
        let module: Table = lua
            .load(&source)
            .set_name(format!("@{DWINDLE}"))
            .eval()
            .expect("dwindle.lua loads");
        lua.globals().set("dwindle", module.clone()).expect("held");
        let made: Function = module.get("new").expect("dwindle.new");
        let tree = made.call(()).expect("a tree");
        Self { lua, tree }
    }

    fn method(&self, name: &str) -> Function {
        self.tree.get(name).expect("a method of the tree")
    }

    pub(super) fn insert(
        &mut self,
        id: u64,
        target: Option<u64>,
        at: Option<(f64, f64)>,
        area: Rect,
        settings: Settings,
    ) {
        let (x, y) = at.map_or((None, None), |(x, y)| (Some(x), Some(y)));
        self.method("insert")
            .call::<()>((
                self.tree.clone(),
                id,
                target,
                x,
                y,
                options(&self.lua, area, settings),
            ))
            .expect("insert");
    }

    pub(super) fn drag_seam(
        &mut self,
        id: u64,
        edge: Edge,
        at: (f64, f64),
        area: Rect,
        settings: Settings,
    ) {
        let known: bool = self
            .method("drag_seam")
            .call((
                self.tree.clone(),
                id,
                edge.name(),
                at.0,
                at.1,
                options(&self.lua, area, settings),
            ))
            .expect("drag_seam");
        assert!(known, "{edge:?} is a side the tree knows");
    }

    pub(super) fn layout(&self, area: Rect, settings: Settings) -> Vec<(u64, Rect)> {
        let boxes: Table = self
            .method("layout")
            .call((self.tree.clone(), options(&self.lua, area, settings)))
            .expect("layout");
        boxes
            .sequence_values::<Table>()
            .map(|placed| {
                let placed = placed.expect("a placed window");
                let side = |name: &str| placed.get::<f64>(name).expect("a side");
                (
                    placed.get("id").expect("its window"),
                    Rect::new(side("x"), side("y"), side("w"), side("h")),
                )
            })
            .collect()
    }
}

impl Clone for Tiling {
    /// The same tree, split for split, that changes apart from this one: a
    /// copy of its plain data, made a tree again.
    fn clone(&self) -> Self {
        let copy: Table = self
            .lua
            .load(
                "local function copy(value)\n\
                     if type(value) ~= 'table' then return value end\n\
                     local out = {}\n\
                     for key, inner in pairs(value) do out[key] = copy(inner) end\n\
                     return out\n\
                 end\n\
                 return dwindle.adopt(copy(...))",
            )
            .call(Value::Table(self.tree.clone()))
            .expect("a copy of the tree");
        Self {
            lua: self.lua.clone(),
            tree: copy,
        }
    }
}
