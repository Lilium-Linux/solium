//! What a configuration keeps across a reload, and across a window manager
//! started in its place. Plain data, in no language: within one window
//! manager as [`Kept`], and between two as a document the compositor holds
//! ([`Kept::to_doc`]).

use std::collections::{BTreeMap, HashMap, HashSet};

use solium_wm_protocol::{Doc, DocField, DocNode, DocNumbered, DocTable};

/// What a script handed the host to hold while the Lua state is replaced.
///
/// Plain data, because that is the only thing that can cross. A reload builds
/// a whole new Lua state; a table, a closure, an upvalue — every Lua value
/// there is — dies with the old one. So what crosses is this, and it is
/// rebuilt as a fresh value in the new state. Both shipped layouts keep their
/// arrangement as plain data (`lua/layouts/`), and before they could keep it
/// a reload rebuilt the layouts in view from the window list, topmost first,
/// and windows traded places (#118). See
/// `a_reload_leaves_a_tiled_desk_exactly_as_it_was` and
/// `a_reload_leaves_every_scrolling_strip_as_it_was`.
///
/// Tables are a list of pairs rather than a map for two reasons. Lua has one
/// table type that is both a list and a dictionary, and both shipped keeps are
/// dictionaries with keys of different types: `workspaces.of` is keyed by
/// window id, which is an integer, and `workspaces.showing` by connector name,
/// which is a string. And a `Vec` of pairs needs no `Hash` or `Eq` on the key,
/// which a float key could not honestly have.
#[derive(Clone, Debug)]
pub enum Kept {
    Bool(bool),
    /// Kept apart from `Number` so a window id survives as the integer it is.
    /// Round-tripped through `f64` it would come back as `7.0`, which is a
    /// *different table key* in Lua from `7` — so every entry in
    /// `workspaces.of` would be unreachable by the id that wrote it.
    Int(i64),
    Number(f64),
    Text(String),
    Table(Vec<(Kept, Kept)>),
}

/// Everything kept, under the names the scripts gave it: `sol.keep`'s
/// tables, and `sol.store`'s as the plain data each saves (#473).
#[derive(Debug, Default)]
pub struct Keep {
    pub(super) keeps: HashMap<String, Kept>,
    pub(super) stores: HashMap<String, solium_json::Json>,
}

impl Keep {
    pub fn new() -> Self {
        Self::default()
    }

    /// What the compositor held for the window manager this one is started
    /// in place of, as `hello` hands it back. One that does not read back
    /// is left out, and said.
    pub fn from_wire(keeps: &[solium_wm_protocol::Kept]) -> Self {
        Self {
            keeps: keeps
                .iter()
                .filter_map(|held| {
                    let name = &held.name;
                    let kept = Kept::from_doc(&held.document);
                    if kept.is_none() {
                        tracing::warn!(
                            keep = name,
                            "what the compositor held under this name is not a keep; left out"
                        );
                    }
                    Some((name.clone(), kept?))
                })
                .collect(),
            stores: HashMap::new(),
        }
    }
}

/// The keeps as the compositor was last sent them, so only one that
/// changed is sent again.
#[derive(Debug, Default)]
pub struct KeepsSent(pub HashMap<String, Doc>);

/// The paths in a keep already said to be unkeepable, so that asking for
/// the keeps every second says each once.
#[derive(Debug, Default)]
pub struct KeepWarnings(pub HashSet<String>);

/// How deep a document is followed when it is read back. No keep is this
/// deep, so one whose tables lead round in a ring is refused rather than
/// followed for ever.
const DOC_DEPTH: usize = 64;

impl Kept {
    /// As a document, for the compositor to hold: a table's whole-number
    /// keys and its named ones. A key that is neither a whole number nor
    /// text does not cross, and is said.
    pub fn to_doc(&self) -> Doc {
        let mut nodes = Vec::new();
        self.node(&mut nodes);
        nodes
    }

    fn node(&self, nodes: &mut Vec<DocNode>) -> u32 {
        let at = place(nodes);
        match self {
            Self::Bool(flag) => nodes.push(DocNode::Boolean(*flag)),
            Self::Int(whole) => nodes.push(DocNode::Integer(*whole)),
            Self::Number(number) => nodes.push(DocNode::Number(*number)),
            Self::Text(text) => nodes.push(DocNode::Text(text.clone())),
            Self::Table(pairs) => {
                // Its place is taken first, so its children come after it.
                nodes.push(DocNode::Nil);
                let mut numbered = BTreeMap::new();
                let mut fields = BTreeMap::new();
                for (key, value) in pairs {
                    match key {
                        Self::Int(key) => {
                            numbered.insert(*key, value);
                        }
                        Self::Text(key) => {
                            fields.insert(key.clone(), value);
                        }
                        other => tracing::warn!(
                            key = ?other,
                            "a kept table's key is neither a whole number nor text; what is \
                             under it would not survive the window manager being started again"
                        ),
                    }
                }
                let mut table = DocTable {
                    list: Vec::new(),
                    fields: Vec::new(),
                    numbered: Vec::new(),
                };
                let mut next = 1;
                while let Some(value) = numbered.remove(&next) {
                    table.list.push(value.node(nodes));
                    next += 1;
                }
                for (key, value) in numbered {
                    table.numbered.push(DocNumbered {
                        key,
                        node: value.node(nodes),
                    });
                }
                for (key, value) in fields {
                    table.fields.push(DocField {
                        name: key,
                        node: value.node(nodes),
                    });
                }
                put(nodes, at, DocNode::Table(table));
            }
        }
        at
    }

    /// The keep a document was made from, or `None` when it is not one: a
    /// table that leads back to itself, or a child that is not there.
    pub fn from_doc(doc: &Doc) -> Option<Self> {
        Self::read_node(doc, 0, 0).ok().flatten()
    }

    /// The value at `at`: `Ok(None)` for one that is no keep by itself (a
    /// hole, a function's name), which its table is kept without, and
    /// `Err` when the document is not one.
    fn read_node(doc: &Doc, at: u32, depth: usize) -> Result<Option<Self>, NotADocument> {
        if depth > DOC_DEPTH {
            return Err(NotADocument);
        }
        let node = usize::try_from(at)
            .ok()
            .and_then(|at| doc.get(at))
            .ok_or(NotADocument)?;
        Ok(Some(match node {
            DocNode::Boolean(flag) => Self::Bool(*flag),
            DocNode::Integer(whole) => Self::Int(*whole),
            DocNode::Number(number) => Self::Number(*number),
            DocNode::Text(text) => Self::Text(text.clone()),
            DocNode::Nil | DocNode::Other(_) => return Ok(None),
            DocNode::Table(table) => {
                let mut pairs = Vec::new();
                let listed = (1..)
                    .zip(&table.list)
                    .map(|(key, at)| (Self::Int(key), *at));
                let numbered = table
                    .numbered
                    .iter()
                    .map(|numbered| (Self::Int(numbered.key), numbered.node));
                let named = table
                    .fields
                    .iter()
                    .map(|field| (Self::Text(field.name.clone()), field.node));
                for (key, at) in listed.chain(numbered).chain(named) {
                    if let Some(value) = Self::read_node(doc, at, depth + 1)? {
                        pairs.push((key, value));
                    }
                }
                Self::Table(pairs)
            }
        }))
    }
}

/// A document that is not one.
struct NotADocument;

/// Where the next node of a document goes.
fn place(nodes: &[DocNode]) -> u32 {
    u32::try_from(nodes.len()).unwrap_or(u32::MAX)
}

/// Put `node` in a place taken earlier.
fn put(nodes: &mut [DocNode], at: u32, node: DocNode) {
    if let Some(place) = usize::try_from(at).ok().and_then(|at| nodes.get_mut(at)) {
        *place = node;
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

#[cfg(test)]
mod tests {
    use solium_wm_protocol::{DocNode, DocTable};

    use super::{Keep, Kept};

    fn int(value: i64) -> Kept {
        Kept::Int(value)
    }

    fn text(value: &str) -> Kept {
        Kept::Text(value.to_owned())
    }

    /// A keep as `workspaces.lua` and `tiling.lua` leave one: window ids for
    /// keys, names for keys, a list, and a tree as `layouts/dwindle.lua`
    /// holds it -- its nodes under numbers with holes between them, each a
    /// window or a split whose sides are a list.
    fn kept() -> Kept {
        Kept::Table(vec![
            (
                text("of"),
                Kept::Table(vec![(int(7), int(2)), (int(9), int(1))]),
            ),
            (
                text("order"),
                Kept::Table(vec![(int(1), text("left")), (int(2), text("right"))]),
            ),
            (text("ratio"), Kept::Number(0.5)),
            (text("on"), Kept::Bool(true)),
            (
                text("tree"),
                Kept::Table(vec![
                    (text("root"), int(5)),
                    (text("serial"), int(5)),
                    (
                        text("nodes"),
                        Kept::Table(vec![
                            (int(1), Kept::Table(vec![(text("id"), int(7))])),
                            (int(4), Kept::Table(vec![(text("id"), int(9))])),
                            (
                                int(5),
                                Kept::Table(vec![
                                    (int(1), int(1)),
                                    (int(2), int(4)),
                                    (text("axis"), text("x")),
                                    (text("ratio"), Kept::Number(0.375)),
                                ]),
                            ),
                        ]),
                    ),
                ]),
            ),
        ])
    }

    /// The value under `name` in a kept table.
    fn under<'a>(kept: &'a Kept, name: &str) -> &'a Kept {
        let Kept::Table(pairs) = kept else {
            panic!("not a table: {kept:?}");
        };
        pairs
            .iter()
            .find(|(key, _)| matches!(key, Kept::Text(key) if key == name))
            .map(|(_, value)| value)
            .unwrap_or_else(|| panic!("nothing under {name}"))
    }

    #[test]
    fn a_keep_comes_back_from_its_document_as_it_went() {
        let kept = kept();
        let back = Kept::from_doc(&kept.to_doc()).expect("a keep");
        assert_eq!(back.to_doc(), kept.to_doc(), "the same document again");
        assert!(matches!(under(&back, "ratio"), Kept::Number(ratio) if *ratio == 0.5));
        assert!(matches!(under(&back, "on"), Kept::Bool(true)));
        // A window id is still the whole number it was: the key Lua looks
        // it up by.
        assert!(matches!(
            under(&back, "of"),
            Kept::Table(pairs) if pairs.iter().any(|pair| matches!(pair, (Kept::Int(7), Kept::Int(2))))
        ));
        // And a node's number past a hole is still that number.
        let nodes = under(under(&back, "tree"), "nodes");
        assert!(matches!(
            nodes,
            Kept::Table(pairs) if pairs.iter().any(|pair| matches!(pair, (Kept::Int(4), Kept::Table(_))))
        ));
    }

    #[test]
    fn a_document_that_is_not_a_keep_is_left_out_of_what_is_handed_back() {
        // A table that holds itself.
        let ring = vec![DocNode::Table(DocTable {
            list: vec![0],
            fields: Vec::new(),
            numbered: Vec::new(),
        })];
        // A table whose child is not there.
        let missing = vec![DocNode::Table(DocTable {
            list: vec![40],
            fields: Vec::new(),
            numbered: Vec::new(),
        })];
        let held = |name: &str, document| solium_wm_protocol::Kept {
            name: name.to_owned(),
            document,
        };
        let keep = Keep::from_wire(&[
            held("ring", ring),
            held("missing", missing),
            held("whole", Kept::Int(3).to_doc()),
        ]);
        let names: Vec<&String> = keep.keeps.keys().collect();
        assert_eq!(names, ["whole"], "{keep:?}");
    }
}
