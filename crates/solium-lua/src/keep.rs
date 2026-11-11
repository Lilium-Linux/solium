//! What a configuration keeps across a reload, and across a window manager
//! started in its place. Plain data, in no language: within one window
//! manager as [`Kept`], and between two as a document the compositor holds
//! ([`Kept::to_doc`]).

use std::collections::{BTreeMap, HashMap, HashSet};

use solium_layout::plain::Plain;
use solium_wm_protocol::{Doc, DocField, DocNode, DocNumbered, DocTable};

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
    /// A dwindle tree, split ratios and all: `sol.layout.tree()`.
    Tree(solium_layout::tree::Tiling),
    /// A scrolling strip, columns, widths, focus and view: `sol.layout.scroller()`.
    Strip(solium_layout::scroller::Scroller),
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

/// The field that tells a layout from a table, in the document a keep
/// crosses as, and the field its data is under.
const LAYOUT: &str = "solium.layout";
const PLAIN: &str = "plain";

/// How deep a document is followed when it is read back. No keep is this
/// deep, so one whose tables lead round in a ring is refused rather than
/// followed for ever.
const DOC_DEPTH: usize = 64;

impl Kept {
    /// As a document, for the compositor to hold: a table's whole-number
    /// keys and its named ones, a tree or a strip as a table of one marked
    /// field and its plain data. A key that is neither a whole number nor
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
            Self::Tree(tree) => layout_node("tree", &tree.to_plain(), nodes),
            Self::Strip(strip) => layout_node("strip", &strip.to_plain(), nodes),
        }
        at
    }

    /// The keep a document was made from, or `None` when it is not one: a
    /// table that leads back to itself, a child that is not there, a layout
    /// whose data is not a layout.
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
                let field = |name: &str| {
                    table
                        .fields
                        .iter()
                        .find(|field| field.name == name)
                        .map(|field| field.node)
                };
                if let Some(kind) = field(LAYOUT) {
                    let plain = field(PLAIN)
                        .and_then(|at| plain_of(doc, at, depth + 1))
                        .ok_or(NotADocument)?;
                    let kind = usize::try_from(kind).ok().and_then(|at| doc.get(at));
                    return match kind {
                        Some(DocNode::Text(kind)) if kind == "tree" => {
                            solium_layout::tree::Tiling::from_plain(&plain).map(Self::Tree)
                        }
                        Some(DocNode::Text(kind)) if kind == "strip" => {
                            solium_layout::scroller::Scroller::from_plain(&plain).map(Self::Strip)
                        }
                        _ => None,
                    }
                    .map(Some)
                    .ok_or(NotADocument);
                }
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

/// A layout as a table of its kind and its plain data.
fn layout_node(kind: &str, plain: &Plain, nodes: &mut Vec<DocNode>) {
    let at = place(nodes);
    nodes.push(DocNode::Nil);
    let data = plain_node(plain, nodes);
    let named = place(nodes);
    nodes.push(DocNode::Text(kind.to_owned()));
    put(
        nodes,
        at,
        DocNode::Table(DocTable {
            list: Vec::new(),
            fields: vec![
                DocField {
                    name: PLAIN.to_owned(),
                    node: data,
                },
                DocField {
                    name: LAYOUT.to_owned(),
                    node: named,
                },
            ],
            numbered: Vec::new(),
        }),
    );
}

fn plain_node(plain: &Plain, nodes: &mut Vec<DocNode>) -> u32 {
    let at = place(nodes);
    match plain {
        Plain::Int(whole) => nodes.push(DocNode::Integer(*whole)),
        Plain::Num(number) => nodes.push(DocNode::Number(*number)),
        Plain::List(items) => {
            nodes.push(DocNode::Nil);
            let list = items.iter().map(|item| plain_node(item, nodes)).collect();
            put(
                nodes,
                at,
                DocNode::Table(DocTable {
                    list,
                    fields: Vec::new(),
                    numbered: Vec::new(),
                }),
            );
        }
    }
    at
}

fn plain_of(doc: &Doc, at: u32, depth: usize) -> Option<Plain> {
    if depth > DOC_DEPTH {
        return None;
    }
    Some(match doc.get(usize::try_from(at).ok()?)? {
        DocNode::Integer(whole) => Plain::Int(*whole),
        DocNode::Number(number) => Plain::Num(*number),
        DocNode::Table(table) if table.fields.is_empty() && table.numbered.is_empty() => {
            Plain::List(
                table
                    .list
                    .iter()
                    .map(|item| plain_of(doc, *item, depth + 1))
                    .collect::<Option<Vec<Plain>>>()?,
            )
        }
        _ => return None,
    })
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
    use solium_layout::{Rect, Settings, scroller::Scroller, tree::Tiling};
    use solium_wm_protocol::{DocNode, DocTable};

    use super::{Keep, Kept};

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 1000.0, 600.0)
    }

    /// A keep as `workspaces.lua`, `tiling.lua` and `scrolling.lua` leave
    /// one: window ids for keys, names for keys, a list, a tree and a strip.
    fn kept() -> (Kept, Tiling, Scroller) {
        let mut tree = Tiling::new();
        tree.insert(7, None, None, area(), Settings::default());
        tree.insert_largest(9, area(), Settings::default());
        let mut strip = Scroller::with_widths(&[0.4, 0.8], 1);
        for id in [7, 9, 11] {
            strip.insert(id, area(), Settings::default());
        }
        let table = Kept::Table(vec![
            (
                Kept::Text("of".to_owned()),
                Kept::Table(vec![
                    (Kept::Int(7), Kept::Int(2)),
                    (Kept::Int(9), Kept::Int(1)),
                ]),
            ),
            (
                Kept::Text("order".to_owned()),
                Kept::Table(vec![
                    (Kept::Int(1), Kept::Text("left".to_owned())),
                    (Kept::Int(2), Kept::Text("right".to_owned())),
                ]),
            ),
            (Kept::Text("ratio".to_owned()), Kept::Number(0.5)),
            (Kept::Text("on".to_owned()), Kept::Bool(true)),
            (Kept::Text("tree".to_owned()), Kept::Tree(tree.clone())),
            (Kept::Text("strip".to_owned()), Kept::Strip(strip.clone())),
        ]);
        (table, tree, strip)
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
        let (kept, tree, strip) = kept();
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
        let Kept::Tree(same) = under(&back, "tree") else {
            panic!("the tree came back as something else");
        };
        assert_eq!(
            same.layout(area(), Settings::default()),
            tree.layout(area(), Settings::default())
        );
        let Kept::Strip(same) = under(&back, "strip") else {
            panic!("the strip came back as something else");
        };
        assert_eq!(
            same.layout(area(), Settings::default()),
            strip.layout(area(), Settings::default())
        );
    }

    #[test]
    fn a_document_that_is_not_a_keep_is_left_out_of_what_is_handed_back() {
        // A table that holds itself.
        let ring = vec![DocNode::Table(DocTable {
            list: vec![0],
            fields: Vec::new(),
            numbered: Vec::new(),
        })];
        // A layout whose data is not one.
        let mut broken = Kept::Tree(Tiling::new()).to_doc();
        for node in &mut broken {
            if let DocNode::Integer(root) = node {
                *root = 40;
            }
        }
        let held = |name: &str, document| solium_wm_protocol::Kept {
            name: name.to_owned(),
            document,
        };
        let keep = Keep::from_wire(&[
            held("ring", ring),
            held("broken", broken),
            held("whole", Kept::Int(3).to_doc()),
        ]);
        let names: Vec<&String> = keep.keeps.keys().collect();
        assert_eq!(names, ["whole"], "{keep:?}");
    }
}
