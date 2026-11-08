//! What a configuration keeps across a reload. Plain data, in no language.

/// What a script handed the host to hold while the Lua state is replaced.
///
/// Plain data and the layouts `sol.layout` makes, because those are the only
/// things that can cross. A reload builds a whole new Lua state; a table, a
/// closure, an upvalue — every Lua value there is — dies with the old one. So
/// what crosses is this, and it is rebuilt as a fresh value in the new state.
///
/// A tree or a strip is userdata and dies with the state like the rest, but
/// what it holds is a Rust value, which the host can copy out of the old state
/// and into the new one whole -- a closure it cannot. Both shipped layouts keep
/// their arrangement in one, and before they could keep it a reload rebuilt
/// the layouts in view from the window list, topmost first, and windows traded
/// places (#118). See `a_reload_leaves_a_tiled_desk_exactly_as_it_was` and
/// `a_reload_leaves_every_scrolling_strip_as_it_was`.
///
/// Tables are a list of pairs rather than a map for two reasons. Lua has one
/// table type that is both a list and a dictionary, and both shipped keeps are
/// dictionaries with keys of different types: `workspaces.of` is keyed by
/// window id, which is an integer, and `workspaces.showing` by connector name,
/// which is a string. And a `Vec` of pairs needs no `Hash` or `Eq` on the key,
/// which a float key could not honestly have.
#[derive(Clone, Debug)]
pub(crate) enum Kept {
    Bool(bool),
    /// Kept apart from `Number` so a window id survives as the integer it is.
    /// Round-tripped through `f64` it would come back as `7.0`, which is a
    /// *different table key* in Lua from `7` — so every entry in
    /// `workspaces.of` would be unreachable by the id that wrote it.
    Int(i64),
    Number(f64),
    Text(String),
    Table(Vec<(Kept, Kept)>),
    /// A dwindle tree, split ratios and all: `sol.layout.tree()`.
    Tree(solium_layout::tree::Tiling),
    /// A scrolling strip, columns, widths, focus and view: `sol.layout.scroller()`.
    Strip(solium_layout::scroller::Scroller),
}

/// Everything kept, under the names the scripts gave it: `sol.keep`'s
/// tables, and `sol.store`'s as the plain data each saves (#473).
#[derive(Debug, Default)]
pub(crate) struct Keep {
    pub(super) keeps: std::collections::HashMap<String, Kept>,
    pub(super) stores: std::collections::HashMap<String, crate::json::Json>,
}

impl Keep {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The stores, by name, as they are saved.
    pub(crate) fn stores(&self) -> impl Iterator<Item = (&String, &crate::json::Json)> {
        self.stores.iter()
    }
}

/// How deep a kept table is followed.
///
/// A budget rather than a cycle check. A table holding itself — directly, or
/// around through three others — would otherwise be copied until the stack ran
/// out, and the compositor does not get to die because a configuration made a
/// ring. Nothing that belongs in a keep is eight tables deep; what is cut is
/// named in the log rather than dropped in silence, because half a table kept
/// quietly is worse than a table that was not kept at all.
pub(super) const KEEP_DEPTH: usize = 8;
