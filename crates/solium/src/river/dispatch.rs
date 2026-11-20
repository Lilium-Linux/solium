//! River's requests, as the compositor takes them: each one handed to the
//! bridge of the window manager running now ([`super::Bridge`]), which
//! turns what it asks into commands.
//!
//! A request river's sequences do not allow (a window's dimensions proposed
//! outside a manage sequence) is left out and logged rather than made a
//! protocol error, except where the protocol makes it one and the window
//! manager cannot go on without knowing: a `manage_finish` or a
//! `render_finish` out of order.

use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, backend::ClientId,
};
use solium_wm_protocol as wire;

use super::bridge::{OutputName, WindowId};
use super::protocol::window_management::{
    river_decoration_v1::{self, RiverDecorationV1},
    river_node_v1::{self, RiverNodeV1},
    river_output_v1::{self, RiverOutputV1},
    river_pointer_binding_v1::{self, RiverPointerBindingV1},
    river_seat_v1::{self, RiverSeatV1},
    river_shell_surface_v1::{self, RiverShellSurfaceV1},
    river_window_manager_v1::{self, RiverWindowManagerV1},
    river_window_v1::{self, RiverWindowV1},
};
use super::protocol::xkb_bindings::{
    river_xkb_binding_v1::{self, RiverXkbBindingV1},
    river_xkb_bindings_seat_v1::{self, RiverXkbBindingsSeatV1},
    river_xkb_bindings_v1::{self, RiverXkbBindingsV1},
};
use crate::state::Solium;

/// What a node stands for: a window's, by its id, or a shell surface's,
/// which the bridge does not draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeOf {
    Window(u64),
    ShellSurface,
}

/// The versions advertised: what a window manager for river 0.4 binds.
pub(crate) const MANAGER_VERSION: u32 = 4;
pub(crate) const XKB_VERSION: u32 = 3;

/// Create the two globals. Any client sees them; only the window manager
/// `wm.toml` started is answered by the bridge, and any other is told
/// `unavailable`, as river's protocol has it.
pub(crate) fn create_globals(display: &DisplayHandle) {
    display.create_global::<Solium, RiverWindowManagerV1, _>(MANAGER_VERSION, ());
    display.create_global::<Solium, RiverXkbBindingsV1, _>(XKB_VERSION, ());
}

impl GlobalDispatch<RiverWindowManagerV1, ()> for Solium {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<RiverWindowManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        let pid = client
            .get_credentials(handle)
            .ok()
            .and_then(|credentials| u32::try_from(credentials.pid).ok());
        match state.river_bridge() {
            Some(bridge) => {
                let mut bridge = bridge.borrow_mut();
                if !bridge.is_bound() && pid.is_some_and(|pid| bridge.may_bind(pid)) {
                    tracing::info!(?pid, "a river window manager bound river_window_manager_v1");
                    bridge.bind(handle.clone(), client.clone(), manager);
                } else {
                    tracing::warn!(
                        ?pid,
                        "river_window_manager_v1 bound by a program that is not the window \
                         manager wm.toml started; told it is unavailable"
                    );
                    manager.unavailable();
                }
            }
            None => manager.unavailable(),
        }
    }
}

impl Dispatch<RiverWindowManagerV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &RiverWindowManagerV1,
        request: river_window_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use river_window_manager_v1::Request;
        let bridge = state.river_bridge();
        match request {
            Request::ManageFinish => {
                if let Some(bridge) = &bridge
                    && let Err(why) = bridge.borrow_mut().manage_finish()
                {
                    manager.post_error(river_window_manager_v1::Error::SequenceOrder, why);
                }
            }
            Request::RenderFinish => {
                if let Some(bridge) = &bridge
                    && let Err(why) = bridge.borrow_mut().render_finish()
                {
                    manager.post_error(river_window_manager_v1::Error::SequenceOrder, why);
                }
            }
            Request::ManageDirty => {
                if let Some(bridge) = &bridge {
                    bridge.borrow_mut().manage_dirty();
                }
            }
            Request::ExitSession => {
                if let Some(bridge) = &bridge {
                    bridge.borrow().now(vec![wire::Command::Quit]);
                }
            }
            Request::Stop => {
                manager.finished();
                if let Some(bridge) = &bridge {
                    bridge.borrow_mut().unbind();
                }
            }
            Request::GetShellSurface { id, surface: _ } => {
                data_init.init(id, ());
            }
            Request::Destroy => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, _manager: &RiverWindowManagerV1, (): &()) {
        if let Some(bridge) = state.river_bridge() {
            bridge.borrow_mut().unbind();
        }
    }
}

impl Dispatch<RiverWindowV1, WindowId> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        _window: &RiverWindowV1,
        request: river_window_v1::Request,
        &WindowId(id): &WindowId,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use river_window_v1::Request;
        // Objects a request makes are made whether or not there is a bridge
        // to tell, so the client's object ids stay in step.
        let Some(bridge) = state.river_bridge() else {
            match request {
                Request::GetNode { id: node } => {
                    data_init.init(node, NodeOf::Window(id));
                }
                Request::GetDecorationAbove { id: decoration, .. }
                | Request::GetDecorationBelow { id: decoration, .. } => {
                    data_init.init(decoration, ());
                }
                _ => {}
            }
            return;
        };
        let mut bridge = bridge.borrow_mut();
        match request {
            Request::Close => bridge.manage(wire::Command::Close(id)),
            Request::GetNode { id: node } => {
                let node = data_init.init(node, NodeOf::Window(id));
                bridge.node(id, node);
            }
            Request::ProposeDimensions { width, height } => bridge.propose(id, width, height),
            Request::Hide => bridge.show(id, false),
            Request::Show => bridge.show(id, true),
            Request::Fullscreen { output: _ } => {
                if !bridge.fullscreen(id) {
                    bridge.manage(wire::Command::ToggleFullscreen(id));
                }
            }
            Request::ExitFullscreen => {
                if bridge.fullscreen(id) {
                    bridge.manage(wire::Command::ToggleFullscreen(id));
                }
            }
            Request::GetDecorationAbove { id: decoration, .. }
            | Request::GetDecorationBelow { id: decoration, .. } => {
                // Taken and not drawn: the compositor's frames are QML.
                data_init.init(decoration, ());
            }
            // Frames are the compositor's, the client's own state comes from
            // the compositor's own handling, and a clip box is not drawn: these
            // are taken and change nothing.
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        _window: &RiverWindowV1,
        &WindowId(id): &WindowId,
    ) {
        if let Some(bridge) = state.river_bridge() {
            bridge.borrow_mut().forget(id);
        }
    }
}

impl Dispatch<RiverNodeV1, NodeOf> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        _node: &RiverNodeV1,
        request: river_node_v1::Request,
        of: &NodeOf,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let NodeOf::Window(id) = *of else {
            return;
        };
        if let river_node_v1::Request::SetPosition { x, y } = request
            && let Some(bridge) = state.river_bridge()
        {
            bridge.borrow_mut().position(id, x, y);
        }
        // Stacking is the compositor's: place_top and the rest change nothing.
    }
}

impl Dispatch<RiverDecorationV1, ()> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _decoration: &RiverDecorationV1,
        _request: river_decoration_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<RiverShellSurfaceV1, ()> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _surface: &RiverShellSurfaceV1,
        request: river_shell_surface_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let river_shell_surface_v1::Request::GetNode { id } = request {
            data_init.init(id, NodeOf::ShellSurface);
        }
    }
}

impl Dispatch<RiverOutputV1, OutputName> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _output: &RiverOutputV1,
        _request: river_output_v1::Request,
        _name: &OutputName,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(state: &mut Self, _client: ClientId, _output: &RiverOutputV1, name: &OutputName) {
        if let Some(bridge) = state.river_bridge() {
            bridge.borrow_mut().forget_output(&name.0);
        }
    }
}

impl Dispatch<RiverSeatV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        seat: &RiverSeatV1,
        request: river_seat_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use river_seat_v1::Request;
        let bridge = state.river_bridge();
        match request {
            Request::FocusWindow { window } => {
                if let (Some(bridge), Some(&WindowId(id))) = (&bridge, window.data::<WindowId>()) {
                    bridge
                        .borrow_mut()
                        .manage(wire::Command::Focus(wire::Focus {
                            id,
                            cause: wire::FocusCause::Script,
                            warp: None,
                        }));
                }
            }
            Request::GetPointerBinding { id, .. } => {
                // Taken: the protocol has no pointer button for a window
                // manager to bind, so it is never pressed.
                data_init.init(id, ());
            }
            Request::OpStartPointer => {
                // No stream of pointer motion to follow: the operation ends
                // as soon as it starts, and the window manager ends it.
                seat.op_release();
            }
            Request::PointerWarp { x, y } => {
                if let Some(bridge) = &bridge {
                    bridge.borrow().now(vec![wire::Command::Warp(wire::Warp {
                        to: wire::WarpTo::Point(wire::Point {
                            x: f64::from(x),
                            y: f64::from(y),
                        }),
                        ..wire::Warp::default()
                    })]);
                }
            }
            // Focus that leaves every window, a shell surface's focus, and
            // the cursor theme are the compositor's.
            _ => {}
        }
    }
}

impl Dispatch<RiverPointerBindingV1, ()> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _binding: &RiverPointerBindingV1,
        _request: river_pointer_binding_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<RiverXkbBindingsV1, ()> for Solium {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<RiverXkbBindingsV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<RiverXkbBindingsV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        _bindings: &RiverXkbBindingsV1,
        request: river_xkb_bindings_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        use river_xkb_bindings_v1::Request;
        match request {
            Request::GetXkbBinding {
                seat: _,
                id,
                keysym,
                modifiers,
            } => {
                let binding = data_init.init(id, ());
                let modifiers = match modifiers {
                    smithay::reexports::wayland_server::WEnum::Value(value) => value.bits(),
                    smithay::reexports::wayland_server::WEnum::Unknown(bits) => bits,
                };
                if let Some(bridge) = state.river_bridge() {
                    bridge.borrow_mut().add_binding(binding, keysym, modifiers);
                }
            }
            Request::GetSeat { id, seat: _ } => {
                data_init.init(id, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverXkbBindingV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        binding: &RiverXkbBindingV1,
        request: river_xkb_binding_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use river_xkb_binding_v1::Request;
        let enabled = match request {
            Request::Enable => Some(true),
            Request::Disable => Some(false),
            _ => return,
        };
        if let Some(bridge) = state.river_bridge() {
            bridge.borrow_mut().enable_binding(binding, enabled);
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, binding: &RiverXkbBindingV1, (): &()) {
        if let Some(bridge) = state.river_bridge() {
            bridge.borrow_mut().enable_binding(binding, None);
        }
    }
}

impl Dispatch<RiverXkbBindingsSeatV1, ()> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _seat: &RiverXkbBindingsSeatV1,
        _request: river_xkb_bindings_seat_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}
