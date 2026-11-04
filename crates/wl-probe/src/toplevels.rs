//! The windows the compositor lists on `ext-foreign-toplevel-list-v1`, one a
//! line: `identifier<TAB>app_id<TAB>title`.
//!
//! The identifier is what a capture of one window names it by (`grim -T`,
//! #47), and nothing installed here prints it: `wayland-info` lists the
//! global and not what it carries.

use wayland_client::{
    Connection, Dispatch, QueueHandle, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};

#[derive(Default)]
struct Listed {
    identifier: String,
    app_id: String,
    title: String,
}

#[derive(Default)]
struct List {
    windows: Vec<(ExtForeignToplevelHandleV1, Listed)>,
}

pub(super) fn list() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<List>(&connection)?;
    let handle = queue.handle();
    let _list: ExtForeignToplevelListV1 = globals.bind(&handle, 1..=1, ())?;
    let mut state = List::default();
    // The first brings every window, the second every window's events.
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    for (_, listed) in &state.windows {
        println!("{}\t{}\t{}", listed.identifier, listed.app_id, listed.title);
    }
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for List {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for List {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.windows.push((toplevel, Listed::default()));
        }
    }

    event_created_child!(List, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for List {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some((_, listed)) = state.windows.iter_mut().find(|(each, _)| each == handle) else {
            return;
        };
        match event {
            ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } => {
                listed.identifier = identifier;
            }
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => listed.app_id = app_id,
            ext_foreign_toplevel_handle_v1::Event::Title { title } => listed.title = title,
            _ => {}
        }
    }
}
