//! What the compositor tells a window manager, as the protocol's rows:
//! plain data, with no Smithay or renderer type in them, made in this one
//! place so the wire never sees an internal type.

use solium_wm_protocol as wire;

use crate::script::{MonitorInfo, Parentage, ProblemRow, Rect, WindowInfo};

pub(crate) fn rect(rect: Rect) -> wire::Rect {
    wire::Rect {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
    }
}

pub(crate) fn window(window: &WindowInfo) -> wire::Window {
    let size = |size: smithay::utils::Size<i32, smithay::utils::Logical>| wire::Size {
        w: size.w,
        h: size.h,
    };
    wire::Window {
        id: window.id,
        rect: rect(window.rect),
        insets: wire::Insets {
            top: window.insets.top,
            right: window.insets.right,
            bottom: window.insets.bottom,
            left: window.insets.left,
        },
        title: window.title.clone(),
        focused: window.focused,
        monitor: window.monitor.clone(),
        modal: window.modal,
        parent: match window.parent {
            Parentage::None => wire::Parentage::None,
            Parentage::Unknown => wire::Parentage::Unknown,
            Parentage::Window(id) => wire::Parentage::Window(id),
        },
        leaving: window.leaving,
        minimized: window.minimized,
        above: window.above,
        icon: window.icon.map(rect),
        app_id: window.app_id.clone(),
        min: window.min.map(size),
        max: window.max.map(size),
        cramped: window.cramped,
        shown: window.shown,
        fullscreen: window.fullscreen,
        maximized: window.maximized,
        style: window.style.clone(),
        client_side: window.client_side,
        blur_region: window.blur_region,
        x11_type: window.x11_type.clone(),
        accepts_input: window.accepts_input,
        class: window.class.clone(),
        instance: window.instance.clone(),
        content_type: window.content_type.to_owned(),
        pid: window.pid,
    }
}

pub(crate) fn monitor(monitor: &MonitorInfo) -> wire::Monitor {
    wire::Monitor {
        name: monitor.name.clone(),
        area: rect(monitor.area),
        whole: rect(monitor.whole),
        scale: monitor.scale,
        focused: monitor.focused,
        primary: monitor.primary,
        transform: monitor.transform.clone(),
        off: monitor.off,
    }
}

pub(crate) fn keyboard(keyboard: &crate::keymap::State) -> wire::Keyboard {
    wire::Keyboard {
        layouts: keyboard.layouts.clone(),
        short_names: keyboard.short.clone(),
        active: u32::try_from(keyboard.active).unwrap_or(u32::MAX),
        caps: keyboard.caps,
        num: keyboard.num,
        repeat_rate: keyboard.repeat_rate,
        repeat_delay: keyboard.repeat_delay,
    }
}

pub(crate) fn text_field(field: &crate::text_input::Field) -> wire::TextField {
    wire::TextField {
        window: field.window,
        caret: field.caret.map(|caret| wire::Rect {
            x: caret.loc.x,
            y: caret.loc.y,
            w: caret.size.w,
            h: caret.size.h,
        }),
        framed: field.framed,
    }
}

pub(crate) fn problem(problem: &ProblemRow) -> wire::Problem {
    wire::Problem {
        effect: problem.effect.clone(),
        file: problem.file.clone(),
        line: problem.line,
        column: problem.column,
        message: problem.message.clone(),
        severity: problem.severity.to_owned(),
    }
}

/// A document under construction: its nodes, each table's children by
/// index, written in the shape the `sol` getters return.
#[derive(Debug, Default)]
pub(crate) struct DocBuilder {
    nodes: Vec<wire::DocNode>,
}

impl DocBuilder {
    fn at(&self) -> u32 {
        u32::try_from(self.nodes.len()).unwrap_or(u32::MAX)
    }

    fn push(&mut self, node: wire::DocNode) -> u32 {
        let at = self.at();
        self.nodes.push(node);
        at
    }

    /// A JSON value: a whole number below 2^53 as an integer, as Lua reads
    /// one (`Json::to_lua`), null as nil.
    pub(crate) fn json(&mut self, value: &crate::json::Json) -> u32 {
        use crate::json::Json;
        const EXACT: f64 = 9_007_199_254_740_992.0;
        match value {
            Json::Null => self.push(wire::DocNode::Nil),
            Json::Bool(on) => self.push(wire::DocNode::Boolean(*on)),
            #[expect(
                clippy::cast_possible_truncation,
                reason = "whole, and inside f64's exact range"
            )]
            Json::Number(number) if number.fract() == 0.0 && number.abs() < EXACT => {
                self.push(wire::DocNode::Integer(*number as i64))
            }
            Json::Number(number) => self.push(wire::DocNode::Number(*number)),
            Json::Text(text) => self.push(wire::DocNode::Text(text.clone())),
            Json::List(items) => {
                let at = self.push(wire::DocNode::Nil);
                let list = items.iter().map(|item| self.json(item)).collect();
                self.set(at, list, Vec::new(), Vec::new());
                at
            }
            Json::Object(fields) => {
                let at = self.push(wire::DocNode::Nil);
                let fields = fields
                    .iter()
                    .map(|(name, value)| (name.clone(), self.json(value)))
                    .collect();
                self.set(at, Vec::new(), fields, Vec::new());
                at
            }
        }
    }

    /// A table whose entries are written by `fill`, which gets the builder
    /// and adds its children through the table it is handed.
    pub(crate) fn table(&mut self, fill: impl FnOnce(&mut Self, &mut TableParts)) -> u32 {
        let at = self.push(wire::DocNode::Nil);
        let mut parts = TableParts::default();
        fill(self, &mut parts);
        self.set(at, parts.list, parts.fields, parts.numbered);
        at
    }

    pub(crate) fn boolean(&mut self, on: bool) -> u32 {
        self.push(wire::DocNode::Boolean(on))
    }

    pub(crate) fn text(&mut self, text: &str) -> u32 {
        self.push(wire::DocNode::Text(text.to_owned()))
    }

    pub(crate) fn integer(&mut self, whole: i64) -> u32 {
        self.push(wire::DocNode::Integer(whole))
    }

    fn set(
        &mut self,
        at: u32,
        list: Vec<u32>,
        fields: Vec<(String, u32)>,
        numbered: Vec<(i64, u32)>,
    ) {
        if let Some(node) = self.nodes.get_mut(at as usize) {
            *node = wire::DocNode::Table(wire::DocTable {
                list,
                fields,
                numbered,
            });
        }
    }

    /// The document, its first node the root.
    pub(crate) fn finish(self) -> wire::Doc {
        self.nodes
    }
}

/// A table's entries while it is being written.
#[derive(Debug, Default)]
pub(crate) struct TableParts {
    list: Vec<u32>,
    fields: Vec<(String, u32)>,
    numbered: Vec<(i64, u32)>,
}

impl TableParts {
    pub(crate) fn item(&mut self, node: u32) {
        self.list.push(node);
    }

    pub(crate) fn field(&mut self, name: &str, node: u32) {
        self.fields.push((name.to_owned(), node));
    }

    pub(crate) fn numbered(&mut self, key: i64, node: u32) {
        self.numbered.push((key, node));
    }
}

/// A row of a service, a JSON object.
fn row(
    doc: &mut DocBuilder,
    row: &std::collections::BTreeMap<&'static str, crate::json::Json>,
) -> u32 {
    doc.json(&crate::services::notifications::object(row))
}

/// Who serves a service, when it is not Solium: its bus name, process and
/// program.
fn owner(
    doc: &mut DocBuilder,
    parts: &mut TableParts,
    owner: Option<&crate::services::notifications::server::Owner>,
) {
    if let Some(owner) = owner {
        let at = doc.table(|doc, row| {
            row.field("name", doc.text(&owner.name));
            if let Some(pid) = owner.pid {
                row.field("pid", doc.integer(i64::from(pid)));
            }
            row.field("program", doc.text(&owner.program));
        });
        parts.field("owner", at);
    }
}

/// The notifications' view, as the document a window manager mirrors.
pub(crate) fn notifications(view: &crate::services::notifications::View) -> wire::Doc {
    let mut doc = DocBuilder::default();
    doc.table(|doc, table| {
        table.field("available", doc.boolean(view.available));
        table.field("serving", doc.boolean(view.serving));
        table.field("dnd", doc.boolean(view.dnd));
        for (key, rows) in [("banners", &view.banners), ("history", &view.history)] {
            let list = doc.table(|doc, list| {
                for each in rows {
                    list.item(row(doc, each));
                }
            });
            table.field(key, list);
        }
        owner(doc, table, view.owner.as_ref());
    });
    doc.finish()
}

/// The tray's view, as the document a window manager mirrors.
pub(crate) fn tray(view: &crate::services::tray::View) -> wire::Doc {
    let mut doc = DocBuilder::default();
    doc.table(|doc, table| {
        table.field("available", doc.boolean(view.available));
        table.field("serving", doc.boolean(view.serving));
        let items = doc.table(|doc, list| {
            for each in &view.items {
                list.item(row(doc, each));
            }
        });
        table.field("items", items);
        owner(doc, table, view.owner.as_ref());
    });
    doc.finish()
}

/// The media players, each where it is playing now, as the document a
/// window manager mirrors.
pub(crate) fn media(seen: &crate::services::media::Seen) -> wire::Doc {
    let mut doc = DocBuilder::default();
    doc.json(&seen.to_json());
    doc.finish()
}

/// Audio's view, as the document a window manager mirrors.
pub(crate) fn audio(view: &crate::services::audio::View) -> wire::Doc {
    let mut doc = DocBuilder::default();
    doc.table(|doc, table| {
        table.field("available", doc.boolean(view.available));
        table.field("ready", doc.boolean(view.ready));
        for (key, each) in [("output", &view.output), ("input", &view.input)] {
            if let Some(each) = each {
                table.field(key, row(doc, each));
            }
        }
        for (key, rows) in [
            ("outputs", &view.outputs),
            ("inputs", &view.inputs),
            ("streams", &view.streams),
        ] {
            let list = doc.table(|doc, list| {
                for each in rows {
                    list.item(row(doc, each));
                }
            });
            table.field(key, list);
        }
        let windows = doc.table(|doc, windows| {
            for (id, each) in &view.windows {
                windows.numbered(i64::try_from(*id).unwrap_or(i64::MAX), row(doc, each));
            }
        });
        table.field("windows", windows);
    });
    doc.finish()
}

/// The installed applications, as the document a window manager mirrors.
pub(crate) fn apps(apps: &[crate::apps::Entry]) -> wire::Doc {
    let mut doc = DocBuilder::default();
    doc.table(|doc, list| {
        for app in apps {
            let entry = doc.table(|doc, entry| {
                entry.field("id", doc.text(&app.id));
                entry.field("name", doc.text(&app.name));
                entry.field("generic_name", doc.text(&app.generic_name));
                entry.field("icon", doc.text(&app.icon));
                let categories = doc.table(|doc, categories| {
                    for category in &app.categories {
                        categories.item(doc.text(category));
                    }
                });
                entry.field("categories", categories);
            });
            list.item(entry);
        }
    });
    doc.finish()
}
