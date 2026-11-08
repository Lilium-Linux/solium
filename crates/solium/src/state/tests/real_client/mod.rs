//! The tests that need a client on the other end of a socket.
//!
//! A bare `WlSurface` is not enough for any of them: `Window` only wraps a
//! real `ToplevelSurface`, and Smithay gives no way to fabricate one except
//! a client asking for it over the wire. Anything that is *state a client
//! sets* -- the scale it was told, the parent it named, the modal flag it
//! raised -- can only be reached from this side. `wl-probe` (see its own
//! `Cargo.toml`) exists in this workspace for the identical reason, and its
//! dependencies are what make this affordable here: `wayland-client` and
//! `wayland-protocols`'s `client` feature were already in the lockfile.
//!
//! The module was one test's and was named after it. It is two now, and the
//! fixture was always the expensive part of it.

mod explicit_sync;
use super::*;
use smithay::output::{Mode, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_server::Display;
use std::os::unix::io::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer,
    wl_keyboard, wl_output, wl_pointer, wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_subcompositor, wl_subsurface, wl_surface, wl_touch,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1, ext_background_effect_surface_v1,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1, ext_image_capture_source_v1,
    ext_output_image_capture_source_manager_v1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1, ext_image_copy_capture_manager_v1,
    ext_image_copy_capture_session_v1,
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1, ext_workspace_handle_v1, ext_workspace_manager_v1,
};
use wayland_protocols::wp::content_type::v1::client::{
    wp_content_type_manager_v1, wp_content_type_v1,
};
use wayland_protocols::wp::idle_inhibit::zv1::client::{
    zwp_idle_inhibit_manager_v1, zwp_idle_inhibitor_v1,
};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
    zwp_keyboard_shortcuts_inhibit_manager_v1, zwp_keyboard_shortcuts_inhibitor_v1,
};
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_confined_pointer_v1, zwp_locked_pointer_v1, zwp_pointer_constraints_v1,
};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::dialog::v1::client::{xdg_dialog_v1, xdg_wm_dialog_v1};
use wayland_protocols::xdg::foreign::zv2::client::{
    zxdg_exported_v2, zxdg_exporter_v2, zxdg_imported_v2, zxdg_importer_v2,
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols_misc::server_decoration::client::{
    org_kde_kwin_server_decoration, org_kde_kwin_server_decoration_manager,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1, zwlr_foreign_toplevel_manager_v1,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1, zwlr_output_configuration_v1, zwlr_output_head_v1,
    zwlr_output_manager_v1, zwlr_output_mode_v1,
};
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1, zwlr_output_power_v1,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

/// The `edge_at` a `ResizeGrab` would record on the frame it produced
/// `wanted`, for a pane the layout has at that same rectangle.
///
/// These fixtures build a `ResizeRequest` by hand, because what is under
/// test is what `settle_resize` does with one rather than how a grab
/// assembles one. It still has to be a payload a grab could *have*
/// assembled. Spelled `(wanted.loc.x, wanted.loc.y)` it was not: that is
/// the top-left corner whatever the edges say, so every `Right`,
/// `Bottom` and `BottomRight` case carried a pair
/// `crate::input::resize::dragged_edge` cannot produce. Nothing read it
/// -- every one of these takes the floating path, where no layout is
/// offered the drag at all -- which is exactly why it would have sat
/// there until a tiled test was written against it and believed.
///
/// Built by the real function for that reason, rather than by a second
/// copy of its rules here.
fn payload_edge(wanted: Rectangle<i32, Logical>, edges: ResizeEdge) -> (f64, f64) {
    // The pointer, for an axis this drag has no hold of. Its centre is
    // as good as anywhere: the value is passed straight through and
    // these fixtures never look at it.
    let pointer: Point<f64, Logical> = (
        f64::from(wanted.loc.x) + f64::from(wanted.size.w) / 2.0,
        f64::from(wanted.loc.y) + f64::from(wanted.size.h) / 2.0,
    )
        .into();
    crate::input::resize::dragged_edge(
        crate::input::resize::LaidOut(wanted),
        edges,
        pointer,
        pointer,
    )
}

/// The client side of the fixture. Binds exactly the globals a window
/// needs and nothing else -- there is no renderer on this end to answer
/// anything more, and none of what follows needs one.
#[derive(Debug, Default)]
struct Client {
    syncobj: Option<explicit_sync::WpLinuxDrmSyncobjManagerV1>,
    dmabuf: Option<explicit_sync::ZwpLinuxDmabufV1>,
    compositor: Option<wl_compositor::WlCompositor>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    /// `ext_background_effect_manager_v1`, and the capabilities it said
    /// it has (#78).
    background_effect: Option<ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1>,
    blur_capabilities: Option<u32>,
    /// `ext_workspace_manager_v1`'s global name, as the registry announced
    /// it: bound only by a test that asks, with [`Self::bind_workspaces`]
    /// (#60).
    workspace_global: Option<u32>,
    registry: Option<wl_registry::WlRegistry>,
    /// What the workspace manager told this client.
    workspaces_told: WorkspacesTold,
    /// The window protocols' globals, by interface, as the registry
    /// announced them: bound only by a test that asks, with
    /// [`Self::bind_toplevels`], so no other test hears of every window
    /// it opens twice over (#50).
    toplevel_globals: Vec<(String, u32)>,
    xwayland_grab_global: bool,
    /// `ext_foreign_toplevel_image_capture_source_manager_v1`'s global,
    /// as the registry announced it: bound only by a test that captures
    /// a window (#47).
    window_source_global: Option<u32>,
    /// `zwlr-output-management-v1` (#51): its global as the registry
    /// announced it, bound only by a test that asks, and what it told.
    output_manager_global: Option<u32>,
    heads_told: Vec<(zwlr_output_head_v1::ZwlrOutputHeadV1, HeadTold)>,
    modes_told: Vec<(zwlr_output_mode_v1::ZwlrOutputModeV1, ModeTold)>,
    /// The serial of each `done`, in order.
    output_dones: Vec<u32>,
    output_manager_finished: bool,
    /// Each configuration's answer, in order.
    configuration_answers: Vec<&'static str>,
    toplevel_manager: Option<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1>,
    /// Whether the manager was told `finished`.
    toplevel_manager_finished: bool,
    /// Each wlr handle this client was given, in the order given, and
    /// what it was told.
    wlr_told: Vec<(
        zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        Told,
    )>,
    /// The same for the ext list's handles.
    ext_told: Vec<(
        ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        Told,
    )>,
    /// `zxdg_decoration_manager_v1`: each toplevel the fixtures open
    /// leaves its frame to the compositor, as a Qt client does
    /// ([`Self::leave_frame`], #462).
    decoration: Option<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
    /// Every mode a decoration object of this client's was told, in
    /// order (#462).
    decoration_modes: Vec<zxdg_toplevel_decoration_v1::Mode>,
    /// KDE's server-decoration manager, how GTK3 asks for a frame, and
    /// every mode a decoration object of this client's made through it
    /// was told (#462).
    kde_decoration:
        Option<org_kde_kwin_server_decoration_manager::OrgKdeKwinServerDecorationManager>,
    kde_modes: Vec<org_kde_kwin_server_decoration::Mode>,
    shm: Option<wl_shm::WlShm>,
    /// For #126: a window whose picture is more than its root surface.
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    /// Every `wl_buffer.release` this client has been sent, by buffer.
    /// For #126, whose fade draws from a buffer the client must not be
    /// told it may reuse until the fade is over.
    released: Vec<wayland_client::backend::ObjectId>,
    /// The global #72 added. Bound here because "the client said modal"
    /// is not a thing the server side can say on a client's behalf --
    /// which is the whole reason this test is in this module.
    dialogs: Option<xdg_wm_dialog_v1::XdgWmDialogV1>,
    /// The two halves of #67: an application exports a window, and a
    /// portal imports it to parent a dialog of its own to it.
    exporter: Option<zxdg_exporter_v2::ZxdgExporterV2>,
    importer: Option<zxdg_importer_v2::ZxdgImporterV2>,
    /// Every `zxdg_exported_v2.handle` this client has been sent: the
    /// string an application passes to a portal.
    exported_handles: Vec<String>,
    /// #71: how a surface says what it contains -- a video, a game, a
    /// photo -- which only its client can say.
    content_types: Option<wp_content_type_manager_v1::WpContentTypeManagerV1>,
    /// For a window that asks to be brought forward with a token, as
    /// an application launched with one does as it opens -- GTK, Qt
    /// and winit all do. A raw client that never sends it cannot see
    /// what the compositor does with the request (#134's second
    /// review, finding 2), so `keyboard_at_open` sends it.
    activation: Option<xdg_activation_v1::XdgActivationV1>,
    /// For [`bar`]: the one way to make a monitor's work area smaller
    /// than the monitor, which is what tells a maximised window from a
    /// fullscreen one.
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    layer_configures: Vec<(wayland_client::backend::ObjectId, u32)>,
    /// Every `xdg_toplevel.configure` this client has been sent, with
    /// the toplevel it was sent to.
    ///
    /// The *rate* a client is configured at is the subject of #123 and
    /// there is no way to see it from the server side: `size_window`
    /// hands a size to Smithay and Smithay decides whether that is a
    /// change worth a wire message. Counting them here counts what the
    /// client actually receives, which is the only number the issue is
    /// about — and it is also what keeps a test honest, because a
    /// compositor that sends the same size sixty times a second looks
    /// identical from its own side and costs nothing on the wire.
    configures: Vec<(wayland_client::backend::ObjectId, i32, i32)>,
    /// Whether each of those configures said the window is tiled, in the
    /// same order: all four `tiled_*` states, for #228. Read through
    /// [`tiled_sent`].
    tiled: Vec<(wayland_client::backend::ObjectId, bool)>,
    /// The states each of those configures carried, in the same order:
    /// what says `resizing` while a drag holds the window.
    configure_states: Vec<(wayland_client::backend::ObjectId, Vec<u32>)>,
    /// And whether each said it is maximised, in the same order: for
    /// #445 and #450. Read through [`maximized_sent`].
    maximized: Vec<(wayland_client::backend::ObjectId, bool)>,

    /// The monitor, the lock manager and the clipboard, for
    /// `lock_focus`: the only tests that need a surface to lock a
    /// screen with, or a data device. Bound by every client and used by
    /// none of the others, which is harmless -- binding a global asks
    /// for nothing until a request is made on it. The seat, which
    /// `lock_focus` needs as well, is with the keyboard below.
    output: Option<wl_output::WlOutput>,
    /// Every monitor, where `output` is the last one bound: a lock
    /// client covers them all.
    outputs: Vec<wl_output::WlOutput>,
    locks: Option<ext_session_lock_manager_v1::ExtSessionLockManagerV1>,
    data_devices: Option<wl_data_device_manager::WlDataDeviceManager>,
    screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    /// Every `wl_keyboard.key` this client has been sent, as evdev
    /// codes, presses and releases alike. What the lock tests are
    /// about in the end: a key that reaches an application's client
    /// is a key the application has.
    keys: Vec<u32>,
    /// The same keys with whether each was a press, for the one
    /// question `keys` cannot answer: whether a key the client was
    /// told went down was ever told it came up.
    key_events: Vec<(u32, bool)>,
    /// The keys the last `wl_keyboard.enter` said were already held.
    enter_keys: Vec<u32>,
    /// The lock objects this client was told `locked` on, and the
    /// ones it was told `finished` on.
    locked: Vec<wayland_client::backend::ObjectId>,
    finished: Vec<wayland_client::backend::ObjectId>,
    /// Screen captures this client was offered a buffer for, and ones
    /// it was told failed.
    captures_offered: usize,
    captures_failed: usize,
    /// `ext-image-copy-capture-v1`, the protocol a screen share through
    /// the portal asks for first (#387): its two managers, and what its
    /// sessions and frames have been told.
    image_sources:
        Option<ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1>,
    image_copy: Option<ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1>,
    /// Every `buffer_size`, `shm_format` and `done`, in order, so a test
    /// can tell an announcement from its repeat.
    copy_constraints: Vec<CopyConstraint>,
    /// How many sessions were told `stopped`.
    copy_stopped: usize,
    /// Every frame's answer: `Ok` for `ready`, the reason for `failed`.
    copy_answers: Vec<Result<(), u32>>,
    /// The surface this client's keyboard is on, as its own
    /// `enter`/`leave` events say -- the client's view, which is the
    /// one that matters, and not the server's.
    keyboard_on: Option<wayland_client::backend::ObjectId>,
    key_serial: Option<u32>,
    /// The serial of the last `wl_keyboard.enter`, for a menu to grab
    /// with the way a real one does.
    serial: u32,
    /// How many `wl_data_device.selection` events have arrived. One is
    /// sent each time this client is made the clipboard's client, so a
    /// count that moves while the session is locked is an application
    /// behind the lock being handed the clipboard.
    selections: usize,
    /// How many `xdg_popup.popup_done` events have arrived.
    popups_done: usize,
    /// Every `xdg_surface.configure` serial, with the `xdg_surface` it
    /// was sent to. Recorded and not acked, for the reason
    /// [`Self::configures`] gives -- and so a test that needs a popup
    /// to draw can ack one itself, which xdg-shell requires before a
    /// popup may commit anything.
    surface_configures: Vec<(wayland_client::backend::ObjectId, u32)>,
    /// Every `xdg_toplevel.close` this client has been sent, with the
    /// toplevel it was sent to.
    ///
    /// Counted here for the same reason `configures` is: `send_close`
    /// is a call the server makes into Smithay, and from the server's
    /// own side a compositor that asks twice looks exactly like one
    /// that asks once. Issue #127's second fault is precisely a second
    /// request going out, so the only honest place to count is the end
    /// that receives them.
    closes: Vec<wayland_client::backend::ObjectId>,
    /// The seat and its keyboard, bound so that a test can ask where
    /// typing *went* rather than where focus was set.
    ///
    /// The distinction is the whole of #127's second review, finding 1:
    /// a compositor that leaves focus on a window nobody can see is
    /// indistinguishable, from its own side, from one that moved it —
    /// the seat is perfectly happy to hold a surface that is drawn at
    /// opacity zero, and `give_keyboard`'s `true` says only that the
    /// lock's rule allowed it, not that anyone can see the surface. The
    /// end that receives the keystrokes is the only one that can say,
    /// which is why `typing_after_a_close_reaches_the_window_that_is_drawn`
    /// asserts here and not at the seat.
    ///
    /// The keyboard is the one the seat's handler asks for, and the
    /// only one: see that handler.
    seat: Option<wl_seat::WlSeat>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// The surface this client currently has keyboard focus on, as
    /// `wl_keyboard.enter` and `.leave` report it.
    ///
    /// Kept as the protocol id rather than the proxy, because that is
    /// the one number both ends of this fixture agree on: a
    /// `wl_surface` is created by the client and carries the same id in
    /// the server's object map, so a test holding only the server's
    /// `Window` can still say which surface this was.
    entered: Option<u32>,
    /// Every key this client was sent, paired with the surface it was
    /// focused on when it arrived.
    ///
    /// The pairing is the point. `wl_keyboard.key` carries no surface —
    /// it goes wherever the last `enter` put the focus — so "which
    /// window did this character reach" is a question only a client
    /// tracking both events can answer, and it is exactly the question
    /// the user asks when they close a window and keep typing.
    typed: Vec<(Option<u32>, u32)>,
    /// Every activation token this client asked for and was handed,
    /// by `xdg_activation_token_v1.done`, oldest first.
    tokens: Vec<String>,
    /// For `power`: the manager `wlopm` binds, every `mode` its
    /// controls were told -- the control, and whether it said on -- and
    /// the controls told `failed`.
    output_power: Option<zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1>,
    power_modes: Vec<(wayland_client::backend::ObjectId, bool)>,
    power_failed: Vec<wayland_client::backend::ObjectId>,
    /// For `power`: what a video player holds the machine awake with.
    inhibit: Option<zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1>,
    /// For #58: what a VM viewer asks for every key with, and every
    /// `active` (true) and `inactive` (false) it has been told, in order.
    shortcuts:
        Option<zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1>,
    shortcuts_held: Vec<bool>,
    shortcuts_request: Option<(
        zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
        wl_surface::WlSurface,
    )>,
    /// For `power`: what `swayidle` asks with, and every `idled` (true)
    /// and `resumed` (false) its notifications have been told.
    idle_notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    idle_events: Vec<(wayland_client::backend::ObjectId, bool)>,
    /// How many frame callbacks -- [`FrameDone`], not the `sync` every
    /// round trip makes -- have come back done.
    frames_done: usize,
    /// Every `ext_session_lock_surface_v1.configure` serial, with the
    /// lock surface it was sent to. A lock surface that commits before
    /// it has acked one is a protocol error, so
    /// `a_lock_surface_is_told_to_draw_while_locked` acks one before
    /// it asks for a frame callback.
    lock_configures: Vec<(wayland_client::backend::ObjectId, u32)>,
    /// Every global the registry has taken away, by name. A monitor
    /// that is only off must never be one.
    globals_removed: Vec<u32>,
    /// For a hosted grab: what a game locks the pointer with.
    pointer_constraints: Option<zwp_pointer_constraints_v1::ZwpPointerConstraintsV1>,
    /// Every `wl_pointer.button` this client was sent, as the evdev code
    /// and whether it was a press, once it has asked for a pointer with
    /// [`Self::seat_pointer`]: a button a window was never told went
    /// down must not be told it came up either.
    buttons: Vec<(u32, bool)>,
    /// The surface the pointer is on, as this client's own `enter` and
    /// `leave` say, and the one each of [`Self::buttons`] arrived on:
    /// `wl_pointer.button` names no surface, so which window was
    /// clicked is only this end's to say.
    pointer_on: Option<u32>,
    /// The serial of the last `wl_pointer.enter`, which a cursor surface
    /// is set with.
    pointer_entered: Option<u32>,
    clicked_on: Vec<Option<u32>>,
    /// Raw wl_touch events, preserving slot ids, coordinates and frames.
    touches: Vec<wl_touch::Event>,
}

/// What `ext_workspace_manager_v1` told a client (#60).
#[derive(Debug, Default)]
struct WorkspacesTold {
    manager: Option<ext_workspace_manager_v1::ExtWorkspaceManagerV1>,
    /// Each group, in the order announced: its handle, the monitors it
    /// entered less those it left, its workspaces, and whether it went.
    groups: Vec<(
        ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
        i32,
        Vec<ext_workspace_handle_v1::ExtWorkspaceHandleV1>,
        bool,
    )>,
    /// Each workspace, in the order announced, and what it was told.
    workspaces: Vec<(ext_workspace_handle_v1::ExtWorkspaceHandleV1, WorkspaceTold)>,
    done: u32,
    finished: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct WorkspaceTold {
    id: String,
    name: String,
    coordinates: Vec<u32>,
    state: u32,
    capabilities: u32,
    removed: bool,
}

/// What one output head was told (#51).
#[derive(Clone, Debug, Default, PartialEq)]
struct HeadTold {
    name: String,
    enabled: bool,
    position: (i32, i32),
    scale: f64,
    transform: Option<u32>,
    current: Option<zwlr_output_mode_v1::ZwlrOutputModeV1>,
    modes: Vec<zwlr_output_mode_v1::ZwlrOutputModeV1>,
    finished: bool,
}

/// What one output mode was told (#51).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ModeTold {
    size: (i32, i32),
    refresh: i32,
    preferred: bool,
}

impl Dispatch<zwlr_output_manager_v1::ZwlrOutputManagerV1, ()> for Client {
    fn event(
        state: &mut Self,
        _manager: &zwlr_output_manager_v1::ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.heads_told.push((head, HeadTold::default()));
            }
            zwlr_output_manager_v1::Event::Done { serial } => state.output_dones.push(serial),
            zwlr_output_manager_v1::Event::Finished => state.output_manager_finished = true,
            _ => {}
        }
    }

    wayland_client::event_created_child!(Client, zwlr_output_manager_v1::ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (zwlr_output_head_v1::ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<zwlr_output_head_v1::ZwlrOutputHeadV1, ()> for Client {
    fn event(
        state: &mut Self,
        head: &zwlr_output_head_v1::ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_output_head_v1::Event::Mode { mode } = &event {
            state.modes_told.push((mode.clone(), ModeTold::default()));
        }
        let Some((_, told)) = state.heads_told.iter_mut().find(|(each, _)| each == head) else {
            return;
        };
        match event {
            zwlr_output_head_v1::Event::Name { name } => told.name = name,
            zwlr_output_head_v1::Event::Mode { mode } => told.modes.push(mode),
            zwlr_output_head_v1::Event::Enabled { enabled } => told.enabled = enabled != 0,
            zwlr_output_head_v1::Event::CurrentMode { mode } => told.current = Some(mode),
            zwlr_output_head_v1::Event::Position { x, y } => told.position = (x, y),
            zwlr_output_head_v1::Event::Scale { scale } => told.scale = scale,
            zwlr_output_head_v1::Event::Transform { transform } => {
                told.transform = Some(match transform {
                    wayland_client::WEnum::Value(value) => u32::from(value),
                    wayland_client::WEnum::Unknown(value) => value,
                });
            }
            zwlr_output_head_v1::Event::Finished => told.finished = true,
            _ => {}
        }
    }

    wayland_client::event_created_child!(Client, zwlr_output_head_v1::ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (zwlr_output_mode_v1::ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<zwlr_output_mode_v1::ZwlrOutputModeV1, ()> for Client {
    fn event(
        state: &mut Self,
        mode: &zwlr_output_mode_v1::ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some((_, told)) = state.modes_told.iter_mut().find(|(each, _)| each == mode) else {
            return;
        };
        match event {
            zwlr_output_mode_v1::Event::Size { width, height } => told.size = (width, height),
            zwlr_output_mode_v1::Event::Refresh { refresh } => told.refresh = refresh,
            zwlr_output_mode_v1::Event::Preferred => told.preferred = true,
            _ => {}
        }
    }
}

impl Dispatch<zwlr_output_configuration_v1::ZwlrOutputConfigurationV1, ()> for Client {
    fn event(
        state: &mut Self,
        _configuration: &zwlr_output_configuration_v1::ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        state.configuration_answers.push(match event {
            zwlr_output_configuration_v1::Event::Succeeded => "succeeded",
            zwlr_output_configuration_v1::Event::Failed => "failed",
            zwlr_output_configuration_v1::Event::Cancelled => "cancelled",
            _ => "?",
        });
    }
}

wayland_client::delegate_noop!(
    Client: ignore zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1
);

/// What one foreign-toplevel handle was told (#50).
#[derive(Clone, Debug, Default, PartialEq)]
struct Told {
    title: String,
    app_id: String,
    /// The wlr states, as numbers.
    states: Vec<u32>,
    /// `output_enter` less `output_leave`.
    outputs: i32,
    done: u32,
    closed: bool,
}

impl Client {
    /// Bind the workspace manager, as a bar does.
    fn bind_workspaces(&mut self, qh: &QueueHandle<Self>) {
        let registry = self.registry.clone().expect("the registry");
        let name = self
            .workspace_global
            .expect("ext_workspace_manager_v1 offered");
        self.workspaces_told.manager = Some(registry.bind(name, 1, qh, ()));
    }

    /// Bind both window protocols, as a dock does.
    fn bind_toplevels(&mut self, qh: &QueueHandle<Self>) {
        let registry = self.registry.clone().expect("the registry");
        for (interface, name) in self.toplevel_globals.clone() {
            match interface.as_str() {
                "zwlr_foreign_toplevel_manager_v1" => {
                    self.toplevel_manager = Some(registry.bind(name, 3, qh, ()));
                }
                _ => {
                    let _list: ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 =
                        registry.bind(name, 1, qh, ());
                }
            }
        }
    }

    /// Make `toplevel` a decoration object and state no preference, so
    /// the compositor chooses its frame: what a Qt client does, and what
    /// every window the fixtures open did by never negotiating before
    /// `frames` (#462) left a client that never asks to draw its own.
    fn leave_frame(&self, toplevel: &xdg_toplevel::XdgToplevel, qh: &QueueHandle<Self>) {
        if let Some(manager) = self.decoration.as_ref() {
            let _decoration = manager.get_toplevel_decoration(toplevel, qh, ());
        }
    }

    fn seat_touch(&self, qh: &QueueHandle<Self>) -> wl_touch::WlTouch {
        self.seat.as_ref().expect("wl_seat bound").get_touch(qh, ())
    }

    /// The seat's pointer, asked for now: only the tests that read what
    /// reached a window's pointer have one.
    fn seat_pointer(&self, qh: &QueueHandle<Self>) -> wl_pointer::WlPointer {
        self.seat
            .as_ref()
            .expect("wl_seat bound")
            .get_pointer(qh, ())
    }
}

impl Dispatch<wl_touch::WlTouch, ()> for Client {
    fn event(
        state: &mut Self,
        _touch: &wl_touch::WlTouch,
        event: wl_touch::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        state.touches.push(event);
    }
}

/// See [`Client::buttons`].
impl Dispatch<wl_pointer::WlPointer, ()> for Client {
    fn event(
        state: &mut Self,
        _pointer: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                serial, surface, ..
            } => {
                state.pointer_on = Some(wayland_client::Proxy::id(&surface).protocol_id());
                state.pointer_entered = Some(serial);
            }
            wl_pointer::Event::Leave { .. } => state.pointer_on = None,
            wl_pointer::Event::Button {
                button,
                state: button_state,
                ..
            } => {
                state.buttons.push((
                    button,
                    button_state == wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed),
                ));
                state.clicked_on.push(state.pointer_on);
            }
            _ => {}
        }
    }
}

/// A `wl_surface.frame` callback, told apart from a round trip's
/// `wl_display.sync` by its data. See [`Client::frames_done`].
struct FrameDone;

impl Dispatch<wl_callback::WlCallback, FrameDone> for Client {
    fn event(
        state: &mut Self,
        _callback: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _data: &FrameDone,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.frames_done += 1;
        }
    }
}

/// See [`Client::power_modes`].
impl Dispatch<zwlr_output_power_v1::ZwlrOutputPowerV1, ()> for Client {
    fn event(
        state: &mut Self,
        control: &zwlr_output_power_v1::ZwlrOutputPowerV1,
        event: zwlr_output_power_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let id = wayland_client::Proxy::id(control);
        match event {
            zwlr_output_power_v1::Event::Mode { mode } => state.power_modes.push((
                id,
                matches!(
                    mode,
                    wayland_client::WEnum::Value(zwlr_output_power_v1::Mode::On)
                ),
            )),
            zwlr_output_power_v1::Event::Failed => state.power_failed.push(id),
            _ => {}
        }
    }
}
wayland_client::delegate_noop!(
    Client: ignore zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1
);
wayland_client::delegate_noop!(
    Client: ignore zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1
);
wayland_client::delegate_noop!(Client: ignore zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1);

/// See [`Client::idle_events`].
impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, ()> for Client {
    fn event(
        state: &mut Self,
        notification: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let id = wayland_client::Proxy::id(notification);
        match event {
            ext_idle_notification_v1::Event::Idled => state.idle_events.push((id, true)),
            ext_idle_notification_v1::Event::Resumed => state.idle_events.push((id, false)),
            _ => {}
        }
    }
}
wayland_client::delegate_noop!(Client: ignore ext_idle_notifier_v1::ExtIdleNotifierV1);

/// See [`Client::tokens`].
impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for Client {
    fn event(
        state: &mut Self,
        _token: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.tokens.push(token);
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let (name, interface, version) = match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => (name, interface, version),
            wl_registry::Event::GlobalRemove { name } => {
                state.globals_removed.push(name);
                return;
            }
            _ => return,
        };
        state.registry = Some(registry.clone());
        match interface.as_str() {
            "zwp_xwayland_keyboard_grab_manager_v1" => state.xwayland_grab_global = true,
            "ext_workspace_manager_v1" => state.workspace_global = Some(name),
            "zwlr_foreign_toplevel_manager_v1" | "ext_foreign_toplevel_list_v1" => {
                state.toplevel_globals.push((interface.clone(), name));
            }
            "ext_foreign_toplevel_image_capture_source_manager_v1" => {
                state.window_source_global = Some(name);
            }
            "zwlr_output_manager_v1" => state.output_manager_global = Some(name),
            "wp_linux_drm_syncobj_manager_v1" => {
                state.syncobj = Some(registry.bind(name, 1, qh, ()))
            }
            "zwp_linux_dmabuf_v1" => state.dmabuf = Some(registry.bind(name, 3, qh, ())),
            "zwlr_output_power_manager_v1" => {
                state.output_power = Some(registry.bind(name, 1, qh, ()));
            }
            "zwp_idle_inhibit_manager_v1" => {
                state.inhibit = Some(registry.bind(name, 1, qh, ()));
            }
            // Version 2, for `get_input_idle_notification`.
            "ext_idle_notifier_v1" => {
                state.idle_notifier = Some(registry.bind(name, 2, qh, ()));
            }
            "wl_compositor" => state.compositor = Some(registry.bind(name, 1, qh, ())),
            // Version 2 for the tiled states, #228; a client bound at 1
            // is never sent them.
            "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 2, qh, ())),
            "ext_background_effect_manager_v1" => {
                state.background_effect = Some(registry.bind(name, 1, qh, ()));
            }
            "zxdg_decoration_manager_v1" => {
                state.decoration = Some(registry.bind(name, 1, qh, ()));
            }
            "org_kde_kwin_server_decoration_manager" => {
                state.kde_decoration = Some(registry.bind(name, 1, qh, ()));
            }
            "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
            "wl_subcompositor" => {
                state.subcompositor = Some(registry.bind(name, 1, qh, ()));
            }
            "xdg_wm_dialog_v1" => state.dialogs = Some(registry.bind(name, 1, qh, ())),
            "zxdg_exporter_v2" => state.exporter = Some(registry.bind(name, 1, qh, ())),
            "zxdg_importer_v2" => state.importer = Some(registry.bind(name, 1, qh, ())),
            "wp_content_type_manager_v1" => {
                state.content_types = Some(registry.bind(name, 1, qh, ()));
            }
            "zwp_keyboard_shortcuts_inhibit_manager_v1" => {
                state.shortcuts = Some(registry.bind(name, 1, qh, ()));
            }
            "xdg_activation_v1" => {
                state.activation = Some(registry.bind(name, 1, qh, ()));
            }
            // Version 5 rather than the newest: everything below needs
            // `enter`, `leave` and `key`, which are version 1, and the
            // lower the bind the fewer ways this fixture can stop
            // matching the compositor's advertised version.
            "wl_seat" => state.seat = Some(registry.bind(name, 5, qh, ())),
            "wl_output" => {
                let output: wl_output::WlOutput = registry.bind(name, 1, qh, ());
                state.outputs.push(output.clone());
                state.output = Some(output);
            }
            "zwlr_screencopy_manager_v1" => {
                state.screencopy = Some(registry.bind(name, 3, qh, ()));
            }
            "ext_output_image_capture_source_manager_v1" => {
                state.image_sources = Some(registry.bind(name, 1, qh, ()));
            }
            "ext_image_copy_capture_manager_v1" => {
                state.image_copy = Some(registry.bind(name, 1, qh, ()));
            }
            "ext_session_lock_manager_v1" => {
                state.locks = Some(registry.bind(name, 1, qh, ()));
            }
            "wl_data_device_manager" => {
                state.data_devices = Some(registry.bind(name, 1, qh, ()));
            }
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
            }
            "zwp_pointer_constraints_v1" => {
                state.pointer_constraints = Some(registry.bind(name, 1, qh, ()));
            }
            _ => {}
        }
    }
}

/// The keyboard is taken as soon as the seat says it has one.
///
/// Here rather than at a call site, because capabilities arrive
/// asynchronously: a test that asked for the keyboard at the moment it
/// wanted to type would be asking before the registry round trip that
/// `connect` ends with had delivered this event.
///
/// **Here and nowhere else**, so that a client has one keyboard. A
/// second one -- `lock_focus`'s `Side::connect` asked for its own
/// before the two suites met -- is sent every event again, and a
/// client with two records every key twice.
/// `a_mode_active_at_the_lock_does_not_garble_the_password` asserts
/// the lock client's exact key sequence, and fails on the doubling.
impl Dispatch<wl_seat::WlSeat, ()> for Client {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        (): &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities { capabilities } = event else {
            return;
        };
        let has_keyboard = capabilities
            .into_result()
            .is_ok_and(|capabilities| capabilities.contains(wl_seat::Capability::Keyboard));
        if has_keyboard && state.keyboard.is_none() {
            state.keyboard = Some(seat.get_keyboard(qh, ()));
        }
    }
}

/// Where the typing went, as both suites ask it: see [`Client::keys`]
/// and [`Client::keyboard_on`] for `lock_focus`, and [`Client::typed`]
/// for #127's.
///
/// The keymap fd is dropped rather than read: nothing here interprets
/// keysyms, and the assertion is about which surface received a key
/// rather than which character it was.
impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
    fn event(
        state: &mut Self,
        _keyboard: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter {
                serial,
                surface,
                keys,
            } => {
                let id = wayland_client::Proxy::id(&surface);
                state.entered = Some(id.protocol_id());
                state.keyboard_on = Some(id);
                state.serial = serial;
                state.enter_keys = keys
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|key| u32::from_ne_bytes(*key))
                    .collect();
            }
            wl_keyboard::Event::Leave { .. } => {
                state.entered = None;
                state.keyboard_on = None;
            }
            wl_keyboard::Event::Key {
                serial,
                key,
                state: key_state,
                ..
            } => {
                state.key_serial = Some(serial);
                state.keys.push(key);
                state.key_events.push((
                    key,
                    key_state == wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed),
                ));
                state.typed.push((state.entered, key));
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(Client: ignore xdg_wm_dialog_v1::XdgWmDialogV1);
wayland_client::delegate_noop!(Client: ignore xdg_dialog_v1::XdgDialogV1);
wayland_client::delegate_noop!(Client: ignore zxdg_exporter_v2::ZxdgExporterV2);
wayland_client::delegate_noop!(Client: ignore zxdg_importer_v2::ZxdgImporterV2);
wayland_client::delegate_noop!(Client: ignore zxdg_imported_v2::ZxdgImportedV2);
wayland_client::delegate_noop!(Client: ignore wp_content_type_manager_v1::WpContentTypeManagerV1);
wayland_client::delegate_noop!(Client: ignore wp_content_type_v1::WpContentTypeV1);
wayland_client::delegate_noop!(
    Client: ignore zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1
);

/// See [`Client::shortcuts_held`].
impl Dispatch<zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1, ()> for Client {
    fn event(
        state: &mut Self,
        _inhibitor: &zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
        event: zwp_keyboard_shortcuts_inhibitor_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Active => {
                state.shortcuts_held.push(true);
            }
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Inactive => {
                state.shortcuts_held.push(false);
            }
            _ => {}
        }
    }
}

/// See [`Client::exported_handles`].
impl Dispatch<zxdg_exported_v2::ZxdgExportedV2, ()> for Client {
    fn event(
        state: &mut Self,
        _exported: &zxdg_exported_v2::ZxdgExportedV2,
        event: zxdg_exported_v2::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zxdg_exported_v2::Event::Handle { handle } = event {
            state.exported_handles.push(handle);
        }
    }
}
wayland_client::delegate_noop!(Client: ignore xdg_activation_v1::XdgActivationV1);
wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Client: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Client: ignore xdg_wm_base::XdgWmBase);
wayland_client::delegate_noop!(Client: ignore ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1);

/// See [`Client::blur_capabilities`].
impl Dispatch<ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1, ()> for Client {
    fn event(
        state: &mut Self,
        _manager: &ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities {
            flags: wayland_client::WEnum::Value(flags),
        } = event
        {
            state.blur_capabilities = Some(flags.bits());
        }
    }
}

impl Dispatch<ext_workspace_manager_v1::ExtWorkspaceManagerV1, ()> for Client {
    fn event(
        state: &mut Self,
        _manager: &ext_workspace_manager_v1::ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use ext_workspace_manager_v1::Event;
        let told = &mut state.workspaces_told;
        match event {
            Event::WorkspaceGroup { workspace_group } => {
                told.groups.push((workspace_group, 0, Vec::new(), false));
            }
            Event::Workspace { workspace } => {
                told.workspaces.push((workspace, WorkspaceTold::default()));
            }
            Event::Done => told.done += 1,
            Event::Finished => told.finished = true,
            _ => {}
        }
    }

    wayland_client::event_created_child!(Client, ext_workspace_manager_v1::ExtWorkspaceManagerV1, [
        ext_workspace_manager_v1::EVT_WORKSPACE_GROUP_OPCODE => (ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1, ()),
        ext_workspace_manager_v1::EVT_WORKSPACE_OPCODE => (ext_workspace_handle_v1::ExtWorkspaceHandleV1, ()),
    ]);
}

impl Dispatch<ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1, ()> for Client {
    fn event(
        state: &mut Self,
        group: &ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
        event: ext_workspace_group_handle_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use ext_workspace_group_handle_v1::Event;
        let Some((_, outputs, workspaces, removed)) = state
            .workspaces_told
            .groups
            .iter_mut()
            .find(|(each, ..)| each == group)
        else {
            return;
        };
        match event {
            Event::OutputEnter { .. } => *outputs += 1,
            Event::OutputLeave { .. } => *outputs -= 1,
            Event::WorkspaceEnter { workspace } => workspaces.push(workspace),
            Event::WorkspaceLeave { workspace } => workspaces.retain(|each| *each != workspace),
            Event::Removed => *removed = true,
            _ => {}
        }
    }
}

impl Dispatch<ext_workspace_handle_v1::ExtWorkspaceHandleV1, ()> for Client {
    fn event(
        state: &mut Self,
        workspace: &ext_workspace_handle_v1::ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use ext_workspace_handle_v1::Event;
        let Some((_, told)) = state
            .workspaces_told
            .workspaces
            .iter_mut()
            .find(|(each, _)| each == workspace)
        else {
            return;
        };
        match event {
            Event::Id { id } => told.id = id,
            Event::Name { name } => told.name = name,
            Event::Coordinates { coordinates } => {
                told.coordinates = coordinates
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|four| u32::from_ne_bytes(*four))
                    .collect();
            }
            Event::State {
                state: wayland_client::WEnum::Value(bits),
            } => told.state = bits.bits(),
            Event::Capabilities {
                capabilities: wayland_client::WEnum::Value(bits),
            } => {
                told.capabilities = bits.bits();
            }
            Event::Removed => told.removed = true,
            _ => {}
        }
    }
}

impl Dispatch<zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, ()> for Client {
    fn event(
        state: &mut Self,
        _manager: &zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                state.wlr_told.push((toplevel, Told::default()));
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                state.toplevel_manager_finished = true;
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(Client, zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1, ()> for Client {
    fn event(
        state: &mut Self,
        handle: &zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Event;
        let Some((_, told)) = state.wlr_told.iter_mut().find(|(each, _)| each == handle) else {
            return;
        };
        match event {
            Event::Title { title } => told.title = title,
            Event::AppId { app_id } => told.app_id = app_id,
            Event::OutputEnter { .. } => told.outputs += 1,
            Event::OutputLeave { .. } => told.outputs -= 1,
            Event::State { state: bytes } => {
                told.states = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|four| u32::from_ne_bytes(*four))
                    .collect();
            }
            Event::Done => told.done += 1,
            Event::Closed => told.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, ()> for Client {
    fn event(
        state: &mut Self,
        _list: &ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.ext_told.push((toplevel, Told::default()));
        }
    }

    wayland_client::event_created_child!(Client, ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()> for Client {
    fn event(
        state: &mut Self,
        handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use ext_foreign_toplevel_handle_v1::Event;
        let Some((_, told)) = state.ext_told.iter_mut().find(|(each, _)| each == handle) else {
            return;
        };
        match event {
            Event::Title { title } => told.title = title,
            Event::AppId { app_id } => told.app_id = app_id,
            Event::Done => told.done += 1,
            Event::Closed => told.closed = true,
            _ => {}
        }
    }
}
wayland_client::delegate_noop!(Client: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);

wayland_client::delegate_noop!(Client: ignore org_kde_kwin_server_decoration_manager::OrgKdeKwinServerDecorationManager);

/// See [`Client::kde_modes`].
impl Dispatch<org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration, ()> for Client {
    fn event(
        state: &mut Self,
        _decoration: &org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration,
        event: org_kde_kwin_server_decoration::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let org_kde_kwin_server_decoration::Event::Mode {
            mode: wayland_client::WEnum::Value(mode),
        } = event
        {
            state.kde_modes.push(mode);
        }
    }
}

/// See [`Client::decoration_modes`].
impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ()> for Client {
    fn event(
        state: &mut Self,
        _decoration: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure {
            mode: wayland_client::WEnum::Value(mode),
        } = event
        {
            state.decoration_modes.push(mode);
        }
    }
}
wayland_client::delegate_noop!(Client: ignore wl_subcompositor::WlSubcompositor);
wayland_client::delegate_noop!(Client: ignore wl_subsurface::WlSubsurface);

/// See [`Client::released`].
impl Dispatch<wl_buffer::WlBuffer, ()> for Client {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_buffer::Event::Release) {
            state.released.push(wayland_client::Proxy::id(buffer));
        }
    }
}

/// See [`Client::surface_configures`].
impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            state
                .surface_configures
                .push((wayland_client::Proxy::id(surface), serial));
        }
    }
}
wayland_client::delegate_noop!(Client: ignore wl_callback::WlCallback);
wayland_client::delegate_noop!(Client: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(Client: ignore wl_data_device_manager::WlDataDeviceManager);
wayland_client::delegate_noop!(Client: ignore wl_data_offer::WlDataOffer);
wayland_client::delegate_noop!(Client: ignore xdg_positioner::XdgPositioner);
wayland_client::delegate_noop!(
    Client: ignore ext_session_lock_manager_v1::ExtSessionLockManagerV1
);
wayland_client::delegate_noop!(
    Client: ignore zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1
);
wayland_client::delegate_noop!(
    Client: ignore ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1
);
wayland_client::delegate_noop!(
    Client: ignore ext_image_capture_source_v1::ExtImageCaptureSourceV1
);
wayland_client::delegate_noop!(
    Client: ignore ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1
);
wayland_client::delegate_noop!(
    Client: ignore ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1
);

/// One buffer constraint an `ext_image_copy_capture_session_v1` was
/// told. See [`Client::copy_constraints`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyConstraint {
    Size(u32, u32),
    Shm(u32),
    Done,
}

/// See [`Client::copy_constraints`] and [`Client::copy_stopped`].
impl Dispatch<ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1, ()> for Client {
    fn event(
        state: &mut Self,
        _session: &ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => state
                .copy_constraints
                .push(CopyConstraint::Size(width, height)),
            ext_image_copy_capture_session_v1::Event::ShmFormat { format } => {
                state
                    .copy_constraints
                    .push(CopyConstraint::Shm(u32::from(format)));
            }
            ext_image_copy_capture_session_v1::Event::Done => {
                state.copy_constraints.push(CopyConstraint::Done);
            }
            ext_image_copy_capture_session_v1::Event::Stopped => state.copy_stopped += 1,
            _ => {}
        }
    }
}

/// See [`Client::copy_answers`].
impl Dispatch<ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1, ()> for Client {
    fn event(
        state: &mut Self,
        _frame: &ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_frame_v1::Event::Ready => state.copy_answers.push(Ok(())),
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                state.copy_answers.push(Err(u32::from(reason)));
            }
            _ => {}
        }
    }
}

/// See [`Client::locked`] and [`Client::finished`].
impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for Client {
    fn event(
        state: &mut Self,
        lock: &ext_session_lock_v1::ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let id = wayland_client::Proxy::id(lock);
        match event {
            ext_session_lock_v1::Event::Locked => state.locked.push(id),
            ext_session_lock_v1::Event::Finished => state.finished.push(id),
            _ => {}
        }
    }
}

/// See [`Client::captures_offered`].
impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for Client {
    fn event(
        state: &mut Self,
        _frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer { .. } => state.captures_offered += 1,
            zwlr_screencopy_frame_v1::Event::Failed => state.captures_failed += 1,
            _ => {}
        }
    }
}

/// See [`Client::lock_configures`].
impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, ()> for Client {
    fn event(
        state: &mut Self,
        surface: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure { serial, .. } = event {
            state
                .lock_configures
                .push((wayland_client::Proxy::id(surface), serial));
        }
    }
}

/// See [`Client::selections`].
impl Dispatch<wl_data_device::WlDataDevice, ()> for Client {
    fn event(
        state: &mut Self,
        _device: &wl_data_device::WlDataDevice,
        event: wl_data_device::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_data_device::Event::Selection { .. } = event {
            state.selections += 1;
        }
    }

    // An offer is a new object the server creates, so the client has to
    // be told what to make of one even though no test sets a selection
    // and none should ever arrive.
    wayland_client::event_created_child!(Client, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

/// See [`Client::popups_done`].
impl Dispatch<xdg_popup::XdgPopup, ()> for Client {
    fn event(
        state: &mut Self,
        _popup: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = event {
            state.popups_done += 1;
        }
    }
}
wayland_client::delegate_noop!(Client: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Client {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            state
                .layer_configures
                .push((wayland_client::Proxy::id(layer), serial));
            layer.ack_configure(serial);
        }
    }
}
wayland_client::delegate_noop!(Client: ignore zwp_pointer_constraints_v1::ZwpPointerConstraintsV1);
wayland_client::delegate_noop!(Client: ignore zwp_locked_pointer_v1::ZwpLockedPointerV1);
// For a pointer confined to a region of a window: #220's
// `follow::a_pointer_confined_mid_flight_stays_where_the_client_holds_it`.
wayland_client::delegate_noop!(Client: ignore zwp_confined_pointer_v1::ZwpConfinedPointerV1);
wayland_client::delegate_noop!(Client: ignore wl_region::WlRegion);

/// The two events this fixture does not ignore. See
/// [`Client::configures`] and [`Client::closes`].
///
/// Configures are deliberately not acked:
/// `ToplevelSurface::send_pending_configure` compares against the last
/// configure it *sent*, not the last one a client acknowledged, so a
/// fixture that never acks still sees exactly the deduplication a real
/// client would.
///
/// A close is deliberately not acted on either, and that is the whole
/// of what makes this client a stand-in for a slow one. `close` is a
/// request with no reply in the protocol; a client honours it by
/// destroying its toplevel, and one that has not done so *yet* is
/// indistinguishable from one that has decided not to. This fixture
/// never destroys anything, so it is both — which is what issue #127's
/// third fault is about.
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
    fn event(
        state: &mut Self,
        toplevel: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                use xdg_toplevel::State;
                let states: Vec<u32> = states
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|word| u32::from_ne_bytes(*word))
                    .collect();
                let tiled = [
                    State::TiledLeft,
                    State::TiledRight,
                    State::TiledTop,
                    State::TiledBottom,
                ]
                .into_iter()
                .all(|edge| states.contains(&(edge as u32)));
                let id = wayland_client::Proxy::id(toplevel);
                state
                    .maximized
                    .push((id.clone(), states.contains(&(State::Maximized as u32))));
                state.tiled.push((id.clone(), tiled));
                state.configures.push((id.clone(), width, height));
                state.configure_states.push((id, states));
            }
            xdg_toplevel::Event::Close => {
                state.closes.push(wayland_client::Proxy::id(toplevel));
            }
            _ => {}
        }
    }
}

/// An anonymous, already-unlinked file of `size` bytes -- enough for a
/// client to back a `wl_shm_pool` with. Its contents are never read:
/// nothing in this fixture renders. Only its *size* matters, because
/// that is what gives the mapped `Window` a real, non-zero bounding
/// box instead of the `Rectangle::zero()` an uncommitted surface has,
/// which overlaps no output at all and so would never appear in
/// `elements_for_output` for either monitor below.
fn anon_file(size: i32) -> OwnedFd {
    let path = std::env::temp_dir().join(format!(
        "solium-scale-resend-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("creating a backing file for a test wl_shm_pool");
    std::fs::remove_file(&path).expect("unlinking the test shm file");
    file.set_len(u64::from(size.unsigned_abs()))
        .expect("sizing the test shm file");
    file.into()
}

/// A client on the other end of a socketpair, with every global this
/// compositor has already in its registry.
///
/// The queue comes back rather than a handle to it, because a
/// `QueueHandle` borrows its queue and the caller has to own one.
fn connect(
    display: &mut Display<Solium>,
    state: &mut Solium,
) -> (Connection, wayland_client::EventQueue<Client>, Client) {
    let (server_side, client_side) = UnixStream::pair().expect("a socketpair");
    display
        .handle()
        .insert_client(server_side, std::sync::Arc::new(ClientState::default()))
        .expect("inserting the test client");
    let conn = Connection::from_socket(client_side).expect("wrapping the client socket");
    let mut event_queue = conn.new_event_queue::<Client>();
    let qh = event_queue.handle();
    let mut client = Client::default();

    conn.display().get_registry(&qh, ());
    conn.flush().expect("flushing get_registry");
    display
        .dispatch_clients(state)
        .expect("dispatching get_registry");
    display
        .flush_clients()
        .expect("flushing the registry snapshot");
    // The one blocking read in this fixture: it is safe because the
    // server has, on the line above, already written the response --
    // every global this compositor has -- onto the socket. Blocking
    // here waits on bytes that are already in the kernel buffer, not
    // on the server, which nothing is driving but this same thread.
    event_queue
        .blocking_dispatch(&mut client)
        .expect("reading the registry snapshot");

    (conn, event_queue, client)
}

/// Opens one window through the real protocol: a surface, an
/// `xdg_toplevel`, and a tiny committed buffer, so `new_toplevel` maps
/// a `Window` with a real, non-zero bounding box at `(0, 0)` -- where
/// every window is first mapped; the caller repositions it from
/// there. Returns the newly-mapped `Window`.
///
/// The client's own `xdg_toplevel` comes back with it, because some of
/// what a window is only exists on that side: `set_parent` and the modal
/// flag are both requests, and there is no way to ask for them except as
/// the client.
fn open_window(
    display: &mut Display<Solium>,
    state: &mut Solium,
    conn: &Connection,
    client: &Client,
    qh: &QueueHandle<Client>,
) -> (Window, xdg_toplevel::XdgToplevel) {
    let (window, toplevel, _surface) = open_surface(display, state, conn, client, qh);
    (window, toplevel)
}

/// The same, handing back the `wl_surface` as well.
///
/// For the one thing no other fixture has needed: making the client
/// *answer* a configure with a size of its own. A window's size is its
/// surface's committed bounding box, so the only way to change it is to
/// attach another buffer from this side — which is exactly what a client
/// refusing a resize does, and refusal is the trap `crate::resizing` is
/// built around. Dropping a `wayland-client` proxy sends nothing, so the
/// callers that do not want it are unaffected.
fn open_surface(
    display: &mut Display<Solium>,
    state: &mut Solium,
    conn: &Connection,
    client: &Client,
    qh: &QueueHandle<Client>,
) -> (Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
    let (window, toplevel, surface, _xdg_surface) = open_xdg(display, state, conn, client, qh);
    (window, toplevel, surface)
}

/// The same again, with the `xdg_surface` too: a menu names its parent
/// by that, and there is no other way to open one.
fn open_xdg(
    display: &mut Display<Solium>,
    state: &mut Solium,
    conn: &Connection,
    client: &Client,
    qh: &QueueHandle<Client>,
) -> (
    Window,
    xdg_toplevel::XdgToplevel,
    wl_surface::WlSurface,
    xdg_surface::XdgSurface,
) {
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");

    let before: Vec<Window> = state.space.elements().cloned().collect();

    let surface = compositor.create_surface(qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, qh, ());
    let toplevel = xdg_surface.get_toplevel(qh, ());
    client.leave_frame(&toplevel, qh);

    const SIDE: i32 = 64;
    commit_buffer(client, qh, &surface, SIDE, SIDE);

    conn.flush().expect("flushing the window-open requests");
    display
        .dispatch_clients(state)
        .expect("dispatching the window-open requests");

    let window = state
        .space
        .elements()
        .find(|window| !before.contains(window))
        .cloned()
        .expect("new_toplevel mapped a window");
    (window, toplevel, surface, xdg_surface)
}

/// Attach a buffer of exactly this size and commit it.
///
/// A `Window`'s geometry is its surface's committed bounding box — this
/// fixture never calls `set_window_geometry` — so this *is* how a client
/// says what size it has become, and how it says it has become a
/// different one from the one it was asked for.
fn commit_buffer(
    client: &Client,
    qh: &QueueHandle<Client>,
    surface: &wl_surface::WlSurface,
    width: i32,
    height: i32,
) -> wl_buffer::WlBuffer {
    let shm = client.shm.clone().expect("wl_shm bound");
    let stride = width * 4;
    let bytes = stride * height;
    let fd = anon_file(bytes);
    let pool = shm.create_pool(fd.as_fd(), bytes, qh, ());
    let buffer = pool.create_buffer(0, width, height, stride, wl_shm::Format::Argb8888, qh, ());
    surface.attach(Some(&buffer), 0, 0);
    // Without this the server keeps the *old* buffer's damage and the
    // bounding box does not move: an attach is a promise and a commit is
    // the moment it counts.
    surface.damage(0, 0, width, height);
    surface.commit();
    buffer
}

/// One full round trip: the client's requests to the server, the
/// server's events back to the client.
///
/// **The `sync` is what makes the read safe.** `blocking_dispatch` waits
/// on the socket and nothing but this thread drives the server, so a
/// read issued when the server happened to have written nothing would
/// hang the test binary for ever — and "the server wrote nothing" is
/// precisely the assertion a throttle test is trying to make. A
/// `wl_display.sync` guarantees at least the `done` event, so the read
/// always returns and what it returns is *whatever else* was queued.
fn pump(
    display: &mut Display<Solium>,
    state: &mut Solium,
    conn: &Connection,
    qh: &QueueHandle<Client>,
    queue: &mut wayland_client::EventQueue<Client>,
    client: &mut Client,
) {
    conn.display().sync(qh, ());
    conn.flush().expect("flushing the round trip");
    display
        .dispatch_clients(state)
        .expect("dispatching the round trip");
    display
        .flush_clients()
        .expect("flushing the server's events");
    queue
        .blocking_dispatch(client)
        .expect("reading the server's events");
}

/// **Issue #99: a rescale left already-open windows blurry.**
///
/// Two protocols tell a client what scale to draw at. `commit`'s call to
/// `send_surface_state` resends `wl_surface.preferred_buffer_scale` on
/// every commit, so an existing client picks up a new output scale the
/// moment it next draws. `new_fractional_scale` answers the other one,
/// `wp_fractional_scale_v1`, but only when a client asks -- once, ever,
/// per surface, and nothing called it again when `scale_outputs` changed
/// a monitor's scale later. A window opened before a `super+shift+r`
/// rescale kept the scale it was told at startup, and the compositor
/// upscaled its buffer to fill the larger area the new scale gave it.
///
/// The scenario it describes: two monitors, a `super+shift+r` rescale of
/// one of them, and a window already open on each.
#[test]
fn changed_output_resends_fractional_scale_and_unchanged_output_does_not() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());

    // No decoration, and that is load-bearing rather than tidiness.
    //
    // This fixture is the only test in the tree that drives a *real*
    // wayland-client, and a window that never negotiates
    // `xdg_decoration` now has a frame built for it on first show
    // (#103) -- so its two windows would each construct a Qt scene.
    // Doing that in a process already holding a raw libwayland
    // connection of its own aborts the whole test binary: SIGABRT,
    // nothing on stderr even under `--nocapture`, taking every test
    // after it down as well.
    //
    // It surfaced only when #103 and this fixture first met in one
    // build, each having been green on its own branch. `bare()`
    // short-circuits `Decorations::insert` before it reaches Qt, so
    // this asks for the one thing the fixture does not need and
    // cannot survive.
    //
    // Nothing is weakened by it. The subject here is whether a scale
    // change reaches surfaces that are already open; a frame has no
    // part in that, and the two assertions are untouched.
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));

    // Two monitors side by side, both starting at 1x -- the state
    // they would be in right after `place_outputs` first ran.
    let output_a = Output::new(
        "scale-resend-test-a".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "solium".to_string(),
            model: "test-a".to_string(),
        },
    );
    output_a.change_current_state(
        Some(Mode {
            size: (1920, 1080).into(),
            refresh: 60_000,
        }),
        None,
        Some(Scale::Fractional(1.0)),
        None,
    );
    state.space.map_output(&output_a, (0, 0));

    let output_b = Output::new(
        "scale-resend-test-b".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "solium".to_string(),
            model: "test-b".to_string(),
        },
    );
    output_b.change_current_state(
        Some(Mode {
            size: (1920, 1080).into(),
            refresh: 60_000,
        }),
        None,
        Some(Scale::Fractional(1.0)),
        None,
    );
    state.space.map_output(&output_b, (1920, 0));

    // A real client -- see the module doc comment for why.
    let (conn, event_queue, client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();

    let (window_a, _toplevel_a) = open_window(&mut display, &mut state, &conn, &client, &qh);
    let (window_b, _toplevel_b) = open_window(&mut display, &mut state, &conn, &client, &qh);

    // Placed explicitly, one per monitor: `new_toplevel` maps every
    // window at `(0, 0)`, and where the pane system fits it from
    // there depends on a layout this test does not configure.
    state.space.map_element(window_a.clone(), (100, 100), false);
    state
        .space
        .map_element(window_b.clone(), (2100, 100), false);
    // `elements_for_output` reads overlap data that only
    // `Space::refresh` computes -- see its own doc comment. The real
    // backends call it once a frame, for the same reason.
    state.space.refresh();

    let surface_a = window_a
        .wl_surface()
        .expect("window a has a surface")
        .into_owned();
    let surface_b = window_b
        .wl_surface()
        .expect("window b has a surface")
        .into_owned();

    let preferred_scale = |surface: &WlSurface| {
        with_states(surface, |states| {
            with_fractional_scale(states, |fractional| fractional.preferred_scale())
        })
    };

    assert_eq!(
        preferred_scale(&surface_a),
        None,
        "neither window has asked for a fractional scale yet, so \
         neither should have one before the rescale"
    );
    assert_eq!(preferred_scale(&surface_b), None);

    // The rescale: `a`'s monitor is asked for 2x, as if someone had
    // just edited it in and pressed `super+shift+r`. `b`'s monitor is
    // asked for exactly the scale it already has.
    state.arrangement = crate::monitor::Arrangement::new(vec![
        crate::monitor::Placement {
            name: "scale-resend-test-a".to_string(),
            at: None,
            beside: None,
            mode: crate::monitor::Wanted::default(),
            vrr: None,
            transform: None,
            enabled: true,
            primary: false,
            scale: crate::monitor::Scaling::Fixed(2.0),
        },
        crate::monitor::Placement {
            name: "scale-resend-test-b".to_string(),
            at: None,
            beside: None,
            mode: crate::monitor::Wanted::default(),
            vrr: None,
            transform: None,
            enabled: true,
            primary: false,
            scale: crate::monitor::Scaling::Fixed(1.0),
        },
    ]);
    state.scale_outputs();

    assert_eq!(
        preferred_scale(&surface_a),
        Some(2.0),
        "a's monitor actually changed scale (1x to 2x), so a real \
         client's wp_fractional_scale_v1 object -- bound once, at \
         startup, and never asked again -- must be told the new \
         value, or it keeps drawing at the old one forever. This is \
         issue #99."
    );
    assert_eq!(
        preferred_scale(&surface_b),
        None,
        "b's monitor was asked for exactly the scale it already had, \
         so scale_outputs's own early `continue` means this function \
         never runs for it at all -- and separately, b's window was \
         never on the monitor that changed, so it must be untouched \
         even if it had been"
    );
}

/// One 1920x1080 monitor at the origin, at 1x.
///
/// Fullscreen fills the monitor a window is on and maximise fills its
/// work area, so both need one to exist; with no layer surfaces on it,
/// the work area is the whole monitor.
fn one_screen(state: &mut Solium) -> Output {
    a_screen(state, "restore-test", (0, 0))
}

/// Two 1920x1080 monitors side by side at 1x: `left` at the origin and
/// [`RIGHT_SCREEN`] beside it, top edges aligned.
///
/// The arrangement nothing tested until #134's third review, and the
/// one where "a screen away" stops meaning "nowhere": with the shipped
/// `workspaces.spread` the left monitor's workspace 2 is parked 1.06
/// screens to the right of it, which is squarely on the right monitor.
fn side_by_side(state: &mut Solium, left: &str) -> (Output, Output) {
    (
        a_screen(state, left, (0, 0)),
        a_screen(state, RIGHT_SCREEN, (1920, 0)),
    )
}

/// The name [`side_by_side`] gives the right-hand monitor.
const RIGHT_SCREEN: &str = "right-test";

/// **A surface redeclared onto another monitor drops its scene on the
/// first and has one on the second**: `declare_surface` syncs that
/// surface, so `sol.surface` following `sol.monitor()` from screen to
/// screen does not leave a whole Qt scene behind on each. No client, so
/// the scene may be a real one (the #99 rule).
#[test]
fn a_surface_redeclared_onto_another_monitor_drops_its_scene_on_the_first() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-redeclared-surface");
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join("Scene.qml");
        std::fs::write(&path, "import QtQuick\nItem {}\n").expect("writing the scene");

        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "redeclared-left");
        let mut declared = crate::scripted::Declaration::for_test(
            "bar",
            path,
            crate::scripted::Layer::Top,
            crate::scripted::On::Monitor("redeclared-left".to_owned()),
        );
        state.declare_surface(declared.clone());
        assert!(has_scene(&mut state, "bar", &left), "the scene builds");

        declared.on = crate::scripted::On::Monitor(RIGHT_SCREEN.to_owned());
        state.declare_surface(declared);
        assert!(
            !has_scene(&mut state, "bar", &left),
            "the monitor the surface left kept its scene"
        );
        assert!(
            has_scene(&mut state, "bar", &right),
            "the monitor the surface moved to has no scene"
        );
    });
}

/// A configuration whose `monitors` handler declares each of `names`
/// over the whole of [`RIGHT_SCREEN`], wherever that monitor now is, and
/// tells the scene where through `screenX`: what `shell.lua` does with
/// its canvas on the primary. The scene also holds a `kept` for a test
/// to write and read back.
fn declared_over_the_right_screen(directory: &str, names: &[&str]) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(directory);
    let _ = std::fs::create_dir_all(&directory);
    let scene = directory.join("Scene.qml");
    std::fs::write(
        &scene,
        "import QtQuick\nItem { property int kept: 0; property int screenX: -1 }\n",
    )
    .expect("writing the scene");
    let names = names
        .iter()
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            r#"
            sol.on("monitors", function()
                for _, m in ipairs(sol.monitors()) do
                    if m.name == "{RIGHT_SCREEN}" then
                        for _, name in ipairs({{ {names} }}) do
                            sol.surface(name, {{
                                scene = "{scene}",
                                layer = "top",
                                on = {{ x = m.whole.x, y = m.whole.y, w = m.whole.w, h = m.whole.h }},
                                properties = {{ screenX = m.whole.x }},
                            }})
                        end
                    end
                end
            end)
            "#,
            scene = scene.display()
        ),
    )
    .expect("writing the test script");
    entry
}

/// The scene surface `name` has on `output`.
fn scene_on<'a>(state: &'a mut Solium, name: &str, output: &Output) -> &'a mut crate::qml::Scene {
    let id = state.surfaces.named(name).expect("the surface is declared");
    state
        .surfaces
        .get_mut(id)
        .and_then(|surface| surface.instance_mut(output))
        .expect("the surface has a scene there")
        .scene_for_test()
}

/// Whether surface `name` has a scene on `output`.
fn has_scene(state: &mut Solium, name: &str, output: &Output) -> bool {
    let id = state.surfaces.named(name).expect("the surface is declared");
    state
        .surfaces
        .get_mut(id)
        .and_then(|surface| surface.instance_mut(output))
        .is_some()
}

/// **A monitor an unplug moves keeps the scene its `monitors` handler
/// declares there again.** With the left monitor gone the right one moves
/// to the origin, and the handler declares the surface over its new
/// rectangle, as `shell.lua` declares its canvas over the primary's
/// area. Judged by the rectangle it had before the handler ran, the
/// surface lost its scene and the handler had one built from nothing,
/// which restarted the bar on every hotplug. The new `screenX` is written
/// into the scene that was kept. No client, so the scene may be a real
/// one (the #99 rule).
#[test]
fn a_monitor_an_unplug_moves_keeps_the_scene_its_handler_declares_there() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let entry = declared_over_the_right_screen("solium-state-unplug-moves", &["bar"]);
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "unplug-moves-left");
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.settle_monitors();
        scene_on(&mut state, "bar", &right).set_int("kept", 7);

        state.space.unmap_output(&left);
        state.settle_monitors();
        let scene = scene_on(&mut state, "bar", &right);
        assert_eq!(
            (scene.get_int("kept"), scene.get_int("screenX")),
            (7, 0),
            "(kept, screenX): the unplug built the scene again, or did not move it"
        );
    });
}

/// **A click on a hosted button is acted on at its release**: the press
/// holds the pointer for the scene (Ruling 7), the release that clicks the
/// button reaches it through the hold, and the action the click set is
/// heard in that same dispatch, the release's, through the real input
/// path. Tested with the Cyrillic group active (#132). No client, so the
/// scene is a real one (the #99 rule).
#[test]
fn a_click_on_a_hosted_button_is_acted_on_at_its_release() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        use smithay::backend::input::ButtonState;
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-hosted-click");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let path = directory.join("Scene.qml");
        std::fs::write(
            &path,
            "import QtQuick\nItem {\n    property string action: \"\"\n    MouseArea { width: 100; height: 30; onClicked: parent.action = \"clicked\" }\n}\n",
        )
        .expect("writing the scene");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            r#"sol.on("surface", function(name, action) sol.status(name .. " " .. action) end)"#,
        )
        .expect("writing the test script");
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        one_screen(&mut state);
        russian(&mut state);
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.declare_surface(crate::scripted::Declaration::for_test(
            "bar",
            path,
            crate::scripted::Layer::Top,
            crate::scripted::On::Rect(Rectangle::new((0, 0).into(), (1920, 30).into())),
        ));
        move_pointer(&mut state, (50.0, 15.0), 1);
        let region = crate::monitor::union(&state.space).expect("a monitor");
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Pressed, 2);
        let pressed = (state.scene_press.is_some(), state.status.clone());
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Released, 3);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            (pressed, state.status.as_str()),
            ((true, String::new()), "bar clicked"),
            "((the scene holds the press, what was heard at the press), what was heard at \
             the release)"
        );
    });
}

/// **A reserve an action's handler changes re-flows the windows in the
/// dispatch of the click that sent it** (Ruling 11): a bar's button asks
/// Lua to hide the bar, the handler declares it again with `hidden`, the
/// scene's `Solium.surface.reserve.top` follows, and the work area and
/// the layout move at the release, not a frame later. Tested with the
/// Cyrillic group active (#132). No client, so the scene is a real one
/// (the #99 rule).
#[test]
fn a_reserve_an_action_handler_changes_reflows_the_windows_in_the_clicks_dispatch() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        use smithay::backend::input::ButtonState;
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-hosted-action-reserve");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let path = directory.join("Scene.qml");
        std::fs::write(
            &path,
            "import QtQuick\nimport Solium\nItem {\n    property string action: \"\"\n    property bool hidden: false\n    Solium.surface.reserve.top: hidden ? 0 : 30\n    MouseArea { width: 100; height: 30; onClicked: parent.action = \"hide\" }\n}\n",
        )
        .expect("writing the scene");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                r#"layouts = 0
sol.on("layout", function() layouts = layouts + 1 end)
sol.on("surface", function(name, action)
sol.surface("bar", {{ scene = "{}", layer = "top", on = {{ x = 0, y = 0, w = 1920, h = 30 }}, interactive = true, properties = {{ hidden = true }} }})
end)"#,
                path.display()
            ),
        )
        .expect("writing the test script");
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let output = one_screen(&mut state);
        russian(&mut state);
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.declare_surface(crate::scripted::Declaration::for_test(
            "bar",
            path,
            crate::scripted::Layer::Top,
            crate::scripted::On::Rect(Rectangle::new((0, 0).into(), (1920, 30).into())),
        ));
        let layouts = |state: &Solium| -> u32 {
            state
                .scripts
                .as_ref()
                .map(|scripts| scripts.evaluate("return tostring(layouts)"))
                .and_then(|said| said.parse().ok())
                .unwrap_or(0)
        };
        let shown = state.work_area_on(&output).map(|area| area.loc.y);
        let passes = layouts(&state);
        move_pointer(&mut state, (50.0, 15.0), 1);
        let region = crate::monitor::union(&state.space).expect("a monitor");
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Pressed, 2);
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Released, 3);
        let hidden = (
            state.work_area_on(&output).map(|area| area.loc.y),
            layouts(&state) - passes,
        );
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            (shown, hidden),
            (Some(30), (Some(0), 1)),
            "(the work area's top with the bar shown, (its top, the layout passes) once \
             the click's handler hid it)"
        );
    });
}

/// **Handlers that keep changing a reserve cannot hold a dispatch**
/// (Ruling 11): a scene that answers every change of `hidden` with
/// another action, and a handler that flips `hidden` at each one, are
/// read again in the click's dispatch for `SETTLE_ROUNDS` rounds and no
/// more, and the next settle carries on from there rather than losing
/// what was left. The handler stops by itself after 20 flips, so a
/// settle without the bound fails here instead of hanging. Tested with
/// the Cyrillic group active (#132). No client, so the scene is a real
/// one (the #99 rule).
#[test]
fn handlers_that_keep_changing_a_reserve_cannot_hold_the_clicks_dispatch() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        use smithay::backend::input::ButtonState;
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-hosted-reserve-ping-pong");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let path = directory.join("Scene.qml");
        std::fs::write(
            &path,
            "import QtQuick\nimport Solium\nItem {\n    property string action: \"\"\n    property bool hidden: false\n    onHiddenChanged: action = \"flip\"\n    Solium.surface.reserve.top: hidden ? 0 : 30\n    MouseArea { width: 100; height: 30; onClicked: parent.action = \"flip\" }\n}\n",
        )
        .expect("writing the scene");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                r#"layouts = 0
flips = 0
sol.on("layout", function() layouts = layouts + 1 end)
sol.on("surface", function(name, action)
if flips < 20 then
    flips = flips + 1
    sol.surface("bar", {{ scene = "{}", layer = "top", on = {{ x = 0, y = 0, w = 1920, h = 30 }}, interactive = true, properties = {{ hidden = flips % 2 == 1 }} }})
end
end)"#,
                path.display()
            ),
        )
        .expect("writing the test script");
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let _output = one_screen(&mut state);
        russian(&mut state);
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.declare_surface(crate::scripted::Declaration::for_test(
            "bar",
            path,
            crate::scripted::Layer::Top,
            crate::scripted::On::Rect(Rectangle::new((0, 0).into(), (1920, 30).into())),
        ));
        let said = |state: &Solium, what: &str| -> u32 {
            state
                .scripts
                .as_ref()
                .map(|scripts| scripts.evaluate(&format!("return tostring({what})")))
                .and_then(|said| said.parse().ok())
                .unwrap_or(0)
        };
        let passes = said(&state, "layouts");
        move_pointer(&mut state, (50.0, 15.0), 1);
        let region = crate::monitor::union(&state.space).expect("a monitor");
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Pressed, 2);
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Released, 3);
        let at_the_click = (said(&state, "flips"), said(&state, "layouts") - passes);
        state.settle_scenes();
        let at_the_next_settle = (said(&state, "flips"), said(&state, "layouts") - passes);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            (at_the_click, at_the_next_settle),
            ((4, 3), (8, 7)),
            "((the flips, the layout passes) once the click's dispatch was done, and once \
             the next settle was)"
        );
    });
}

/// **A scene an action's handler moves to the new primary reserves in
/// the click's dispatch** (Ruling 11): a bar on the primary monitor that
/// binds `Solium.surface.reserve.top`, and a button on it whose handler
/// makes the other monitor primary. The surfaces are placed once that
/// handler's dispatch is applied, inside the click's settle, and the
/// scene built there is read in that settle's next round, so the new
/// primary's work area loses the bar at the release, not a frame later.
/// Tested with the Cyrillic group active (#132). No client, so the scene
/// is a real one (the #99 rule).
#[test]
fn a_scene_an_action_moves_to_the_new_primary_reserves_in_the_clicks_dispatch() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        use smithay::backend::input::ButtonState;
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-hosted-action-primary");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let scene = directory.join("Scene.qml");
        std::fs::write(
            &scene,
            "import QtQuick\nimport Solium\nItem {\n    property string action: \"\"\n    Solium.surface.reserve.top: 30\n    MouseArea { width: 100; height: 30; onClicked: parent.action = \"move\" }\n}\n",
        )
        .expect("writing the scene");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                r#"
                sol.surface("bar", {{ scene = "{scene}", layer = "top", on = "primary", interactive = true }})
                sol.on("surface", function(name, action)
                    sol.monitors{{ {{ name = "{RIGHT_SCREEN}", primary = true }} }}
                end)
                "#,
                scene = scene.display()
            ),
        )
        .expect("writing the test script");
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "action-primary-left");
        russian(&mut state);
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.settle_monitors();
        let tops = |state: &Solium| {
            (
                state.work_area_on(&left).map(|area| area.loc.y),
                state.work_area_on(&right).map(|area| area.loc.y),
            )
        };
        let before = tops(&state);
        move_pointer(&mut state, (50.0, 15.0), 1);
        let region = crate::monitor::union(&state.space).expect("a monitor");
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Pressed, 2);
        crate::synth::send_button(&mut state, region, 0x110, ButtonState::Released, 3);
        let after = tops(&state);
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            (before, after),
            ((Some(30), Some(0)), (Some(0), Some(30))),
            "((the left and right work areas' tops) before the click, and once its \
             handler made the right monitor primary)"
        );
    });
}

/// **A scene built on a monitor that arrives has its reserve read in the
/// hotplug's own dispatch**: a bar that binds
/// `Solium.surface.reserve.bottom` and declares no reserve in Lua leaves
/// its strip of the new monitor as soon as the hotplug is done, so
/// windows sent there are not laid out under the bar first and moved a
/// frame later. No client, so the scene is a real one (the #99 rule).
#[test]
fn a_scene_built_on_a_monitor_that_arrives_reserves_in_the_hotplugs_dispatch() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-hosted-hotplug-reserve");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let path = directory.join("Scene.qml");
        std::fs::write(
            &path,
            "import QtQuick\nimport Solium\nItem { Solium.surface.reserve.bottom: 48 }\n",
        )
        .expect("writing the scene");
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let left = one_screen(&mut state);
        state.declare_surface(crate::scripted::Declaration::for_test(
            "bar",
            path,
            crate::scripted::Layer::Top,
            crate::scripted::On::EveryMonitor,
        ));
        let right = a_screen(&mut state, RIGHT_SCREEN, (1920, 0));
        state.settle_monitors();
        let heights = (
            state.work_area_on(&left).map(|area| area.size.h),
            state.work_area_on(&right).map(|area| area.size.h),
        );
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(
            heights,
            (Some(1032), Some(1032)),
            "(the first monitor's work area height, the one that arrived)"
        );
    });
}

/// **Declaring one surface does not judge another by its old placement.**
/// One `monitors` handler declares two surfaces over the monitor an
/// unplug moved; declaring the first must leave the second's scene alone
/// until its own declaration says where it now is. No client (the #99
/// rule).
#[test]
fn one_handler_declaring_two_surfaces_again_keeps_both_their_scenes() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let names = ["panel", "bar"];
        let entry = declared_over_the_right_screen("solium-state-unplug-two", &names);
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "unplug-two-left");
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.settle_monitors();
        for name in names {
            scene_on(&mut state, name, &right).set_int("kept", 7);
        }

        state.space.unmap_output(&left);
        state.settle_monitors();
        let kept: Vec<i32> = names
            .iter()
            .map(|name| scene_on(&mut state, name, &right).get_int("kept"))
            .collect();
        assert_eq!(
            kept,
            vec![7, 7],
            "[panel, bar]: the unplug built a scene again"
        );
    });
}

/// **A reload that makes another monitor primary drops the old primary's
/// scene and builds one on the new**, for a surface `on = "primary"` the
/// new configuration declares exactly as the old one did. Nothing about
/// the surface changed, so only the reload itself can see that its
/// monitor did. No client (the #99 rule).
#[test]
fn a_reload_that_moves_the_primary_drops_the_old_primarys_scene() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-reload-primary");
        let _ = std::fs::create_dir_all(&directory);
        let scene = directory.join("Scene.qml");
        std::fs::write(&scene, "import QtQuick\nItem {}\n").expect("writing the scene");
        let surface = format!(
            r#"sol.surface("bar", {{ scene = "{}", layer = "top", on = "primary" }})"#,
            scene.display()
        );
        let before = directory.join("before.lua");
        std::fs::write(&before, &surface).expect("writing the first configuration");
        let after = directory.join("after.lua");
        std::fs::write(
            &after,
            format!("sol.monitors{{ {{ name = \"{RIGHT_SCREEN}\", primary = true }} }}\n{surface}"),
        )
        .expect("writing the second configuration");

        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "reload-primary-left");
        state.start_scripts(Some(
            Scripts::load(&before).expect("loading the test script"),
        ));
        state.settle_monitors();
        scene_on(&mut state, "bar", &left);

        state.reload_from(&after);
        assert!(
            !has_scene(&mut state, "bar", &left),
            "the monitor that stopped being primary kept its scene"
        );
        assert!(
            has_scene(&mut state, "bar", &right),
            "the new primary has no scene"
        );
    });
}

/// **A `sol.monitors{}` at run time that makes another monitor primary
/// drops the old primary's scene and builds one on the new**, once the
/// dispatch that said it is done. A binding is neither a reload nor a
/// hotplug, and nothing about the surface changed, so only the dispatch
/// can see that its monitor did. No client (the #99 rule).
#[test]
fn a_runtime_primary_change_drops_the_old_primarys_scene() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-runtime-primary");
        let _ = std::fs::create_dir_all(&directory);
        let scene = directory.join("Scene.qml");
        std::fs::write(&scene, "import QtQuick\nItem {}\n").expect("writing the scene");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                r#"
                sol.surface("bar", {{ scene = "{scene}", layer = "top", on = "primary" }})
                sol.bind("super+p", function()
                    sol.monitors{{ {{ name = "{RIGHT_SCREEN}", primary = true }} }}
                end)
                "#,
                scene = scene.display()
            ),
        )
        .expect("writing the test script");

        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "runtime-primary-left");
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.settle_monitors();
        scene_on(&mut state, "bar", &left);

        assert!(state.trigger("super+p"), "super+p is bound");
        assert!(
            !has_scene(&mut state, "bar", &left),
            "the monitor that stopped being primary kept its scene"
        );
        assert!(
            has_scene(&mut state, "bar", &right),
            "the new primary has no scene"
        );
    });
}

/// **A binding that moves the primary and its surface in one go keeps the
/// surface's scene**: the surfaces are placed when the whole dispatch is
/// done, not when the layout pass `sol.monitors{}` runs inside it is,
/// which is before the binding's own `sol.surface` that follows it. No
/// client (the #99 rule).
#[test]
fn a_binding_that_moves_the_primary_and_its_surface_together_keeps_the_scene() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-primary-and-surface");
        let _ = std::fs::create_dir_all(&directory);
        let scene = directory.join("Scene.qml");
        std::fs::write(&scene, "import QtQuick\nItem { property int kept: 0 }\n")
            .expect("writing the scene");
        let left_name = "primary-and-surface-left";
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                r#"
                sol.surface("bar", {{ scene = "{scene}", layer = "top", on = "primary" }})
                sol.bind("super+p", function()
                    sol.monitors{{ {{ name = "{RIGHT_SCREEN}", primary = true }} }}
                    sol.surface("bar", {{ scene = "{scene}", layer = "top", on = "{left_name}" }})
                end)
                "#,
                scene = scene.display()
            ),
        )
        .expect("writing the test script");

        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, _) = side_by_side(&mut state, left_name);
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.settle_monitors();
        scene_on(&mut state, "bar", &left).set_int("kept", 7);

        assert!(state.trigger("super+p"), "super+p is bound");
        assert_eq!(
            scene_on(&mut state, "bar", &left).get_int("kept"),
            7,
            "the scene was dropped before the binding said where the surface now is"
        );
    });
}

/// **A reload that moves a monitor keeps the scene its `monitors`
/// handler declares there again**: a `sol.monitors{}` at the top of the
/// new configuration places the surfaces only once the reload's own
/// `monitors` and `layout` handlers have run, as a hotplug does
/// (`a_monitor_an_unplug_moves_keeps_the_scene_its_handler_declares_there`).
/// No client (the #99 rule).
#[test]
fn a_reload_that_moves_a_monitor_keeps_the_scene_its_handler_declares_there() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let left_name = "reload-moves-left";
        let before = declared_over_the_right_screen("solium-state-reload-moves", &["bar"]);
        let after = before.with_file_name("after.lua");
        let handler = std::fs::read_to_string(&before).expect("reading the test script");
        std::fs::write(
            &after,
            format!("sol.monitors{{ {{ name = \"{RIGHT_SCREEN}\", below = \"{left_name}\" }} }}\n{handler}"),
        )
        .expect("writing the second configuration");

        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (_, right) = side_by_side(&mut state, left_name);
        state.start_scripts(Some(
            Scripts::load(&before).expect("loading the test script"),
        ));
        state.settle_monitors();
        scene_on(&mut state, "bar", &right).set_int("kept", 7);

        state.reload_from(&after);
        assert_eq!(
            state
                .space
                .output_geometry(&right)
                .map(|geometry| geometry.loc),
            Some((0, 1080).into()),
            "the premise: the reload put the right monitor below the left one"
        );
        let scene = scene_on(&mut state, "bar", &right);
        assert_eq!(
            (scene.get_int("kept"), scene.get_int("screenX")),
            (7, 0),
            "(kept, screenX): the reload built the scene again, or did not move it"
        );
    });
}

/// **A reload tries a scene that would not load again**: it forgets Qt's
/// cache of the failure, and it is what anybody presses after mending the
/// file, so a typo in a hosted shell does not cost the session. No client
/// (the #99 rule).
#[test]
fn a_reload_tries_again_a_scene_that_would_not_load() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-reload-mended");
        let _ = std::fs::create_dir_all(&directory);
        let scene = directory.join("Scene.qml");
        std::fs::write(&scene, "import QtQuick\nItem { nonsense: }\n").expect("writing the scene");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                r#"sol.surface("bar", {{ scene = "{}", layer = "top", on = "every-monitor" }})"#,
                scene.display()
            ),
        )
        .expect("writing the test script");

        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "reload-mended-left");
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.settle_monitors();
        assert!(
            !has_scene(&mut state, "bar", &left),
            "the premise: the scene would not load"
        );

        std::fs::write(&scene, "import QtQuick\nItem {}\n").expect("mending the scene");
        state.reload_from(&entry);
        assert!(
            has_scene(&mut state, "bar", &left) && has_scene(&mut state, "bar", &right),
            "the reload did not try the mended scene again"
        );
    });
}

/// A strip a `layout` handler declares over the monitor an unplug moved
/// keeps its scene: docs/modes.md's cramped strip, and any bar declared
/// from `layout`. The prune runs after the `layout` handlers too.
#[test]
fn a_layout_declared_strip_keeps_its_scene_through_an_unplug() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-layout-strip-unplug");
        let _ = std::fs::create_dir_all(&directory);
        let scene = directory.join("Scene.qml");
        std::fs::write(
            &scene,
            "import QtQuick\nItem { property int kept: 0; property int screenX: -1 }\n",
        )
        .expect("writing the scene");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                r#"
                sol.on("layout", function()
                    for _, m in ipairs(sol.monitors()) do
                        if m.name == "{RIGHT_SCREEN}" then
                            sol.surface("strip", {{
                                scene = "{scene}",
                                layer = "top",
                                on = {{ x = m.whole.x, y = m.whole.y, w = m.whole.w, h = 24 }},
                                properties = {{ screenX = m.whole.x }},
                            }})
                        end
                    end
                end)
                "#,
                scene = scene.display()
            ),
        )
        .expect("writing the test script");
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "review-r3-layout-left");
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.settle_monitors();
        scene_on(&mut state, "strip", &right).set_int("kept", 7);

        state.space.unmap_output(&left);
        state.settle_monitors();
        let scene = scene_on(&mut state, "strip", &right);
        assert_eq!(
            (scene.get_int("kept"), scene.get_int("screenX")),
            (7, 0),
            "(kept, screenX): the unplug rebuilt the layout-declared strip"
        );
    });
}

/// A monitor that goes still drops every scene on it at the hotplug, now
/// that the prune runs after the handlers: keeping a moved bar's scene
/// must not keep a gone monitor's.
#[test]
fn an_unplugged_monitor_still_loses_its_scene() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-unplugged-monitor-scene");
        let _ = std::fs::create_dir_all(&directory);
        let scene = directory.join("Scene.qml");
        std::fs::write(&scene, "import QtQuick\nItem {}\n").expect("writing the scene");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                r#"sol.surface("wall", {{ scene = "{}", layer = "background", on = "every-monitor" }})"#,
                scene.display()
            ),
        )
        .expect("writing the test script");
        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "review-r3-gone-left");
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.settle_monitors();
        scene_on(&mut state, "wall", &left);
        scene_on(&mut state, "wall", &right);
        state.space.unmap_output(&left);
        state.settle_monitors();
        let id = state.surfaces.named("wall").expect("declared");
        assert_eq!(
            state
                .surfaces
                .get(id)
                .map(crate::scripted::Surface::instance_count),
            Some(1),
            "the unplugged monitor kept its scene"
        );
    });
}

/// **A hotplug places the surfaces once, after every handler it runs,
/// even when a `monitors` handler rearranges the monitors.** A
/// `sol.monitors{}` places the surfaces when the dispatch that ran it is
/// done, and a hotplug holds its `monitors` and `layout` handlers as one
/// dispatch, as a reload does
/// (`a_reload_that_moves_a_monitor_keeps_the_scene_its_handler_declares_there`).
/// Unheld, the `monitors` dispatch was done first, and the surfaces were
/// placed before the `layout` handlers had run as well as after. No
/// scene is needed to count them.
#[test]
fn a_hotplug_whose_handler_rearranges_the_monitors_places_the_surfaces_once() {
    let directory = std::env::temp_dir().join("solium-state-hotplug-held");
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            r#"
            sol.on("monitors", function()
                sol.monitors{{ {{ name = "{RIGHT_SCREEN}", primary = true }} }}
            end)
            "#
        ),
    )
    .expect("writing the test script");
    let display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let (left, _) = side_by_side(&mut state, "hotplug-held-left");
    state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the test script"),
    ));
    state.settle_monitors();

    let before = state.instances_synced;
    state.space.unmap_output(&left);
    state.settle_monitors();
    assert_eq!(
        state.instances_synced - before,
        1,
        "the surfaces were placed between the hotplug's handlers too"
    );
}

/// **A monitor resized without one coming or going tells the scripts
/// its new size, once**, as the nested window's do when its host resizes
/// it (#376): a `monitors` handler that places a bar on a rectangle it
/// reads from `sol.monitors()` has to hear of it, or the bar keeps the
/// old rectangle, off the screen.
#[test]
fn a_resized_monitor_tells_the_scripts_its_new_size() {
    let directory = std::env::temp_dir().join("solium-state-resized-tells");
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            r#"
            told = 0
            sol.on("monitors", function()
                told = told + 1
                for _, m in ipairs(sol.monitors()) do
                    if m.name == "{RIGHT_SCREEN}" then
                        sol.status(string.format("%d %dx%d", told, m.whole.w, m.whole.h))
                    end
                end
            end)
            "#
        ),
    )
    .expect("writing the test script");
    let display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let (left, right) = side_by_side(&mut state, "resized-tells-left");
    state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the test script"),
    ));
    state.settle_monitors();
    let before = state.status.clone();
    let count = |status: &str| -> u32 {
        status
            .split(' ')
            .next()
            .and_then(|told| told.parse().ok())
            .unwrap_or(0)
    };

    for output in [&left, &right] {
        output.change_current_state(
            Some(Mode {
                size: (2560, 1080).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
    }
    state.outputs_resized();
    let _ = std::fs::remove_dir_all(&directory);
    assert!(
        before.ends_with(" 1920x1080"),
        "the premise: the handler ran at the settle and read the old size, {before:?}"
    );
    assert!(
        state.status.ends_with(" 2560x1080"),
        "the handler did not hear the new size: {:?}",
        state.status
    );
    assert_eq!(
        count(&state.status),
        count(&before) + 1,
        "one resize, one `monitors` event: {before:?} then {:?}",
        state.status
    );
}

/// **A resize that brings a monitor under a surface gives the surface an
/// instance there**, as the nested window's resize does: its monitors
/// share its width, so every one right of the first moves, and a surface
/// declared over a fixed rectangle now covers one more of them. No
/// client (the #99 rule).
#[test]
fn a_resize_that_brings_a_monitor_under_a_surface_gives_it_an_instance_there() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-resized-monitors");
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join("Scene.qml");
        std::fs::write(&path, "import QtQuick\nItem {}\n").expect("writing the scene");

        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let (left, right) = side_by_side(&mut state, "resized-left");
        state.declare_surface(crate::scripted::Declaration::for_test(
            "strip",
            path,
            crate::scripted::Layer::Top,
            crate::scripted::On::Rect(Rectangle::new((1920, 0).into(), (1920, 30).into())),
        ));
        assert!(
            !has_scene(&mut state, "strip", &left) && has_scene(&mut state, "strip", &right),
            "the premise: the strip is on the right monitor only"
        );

        for output in [&left, &right] {
            output.change_current_state(
                Some(Mode {
                    size: (2560, 1080).into(),
                    refresh: 60_000,
                }),
                None,
                None,
                None,
            );
        }
        state.outputs_resized();
        assert_eq!(
            state
                .space
                .output_geometry(&right)
                .map(|geometry| geometry.loc),
            Some((2560, 0).into()),
            "the premise: the resize moved the right monitor"
        );
        assert!(
            has_scene(&mut state, "strip", &left),
            "the monitor the resize brought under the strip has no scene"
        );
        assert!(
            has_scene(&mut state, "strip", &right),
            "the right monitor lost its scene"
        );
    });
}

/// **From the monitors settling to each scene reading its own monitor,
/// end to end**: `settle_monitors` builds an instance of an
/// `every-monitor` surface on each of two monitors, and once
/// `publish_models` has run, each reads its own row through
/// `Solium.monitor`: present, at its own place, with its own size and
/// work area. No client (the #99 rule), and monitor names of its own,
/// because the rows are process-wide.
#[test]
fn each_monitors_instance_reads_its_own_monitor_once_the_models_are_published() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        let directory = std::env::temp_dir().join("solium-state-instance-rows");
        let _ = std::fs::create_dir_all(&directory);
        let scene = directory.join("Scene.qml");
        std::fs::write(
            &scene,
            r"
            import QtQuick
            import Solium
            Item {
                readonly property int present: Solium.monitor.present ? 1 : 0
                readonly property int wholeX: Solium.monitor.whole.x
                readonly property int wholeWidth: Solium.monitor.whole.width
                readonly property int areaWidth: Solium.monitor.area.width
                readonly property int areaHeight: Solium.monitor.area.height
            }
            ",
        )
        .expect("writing the scene");
        let entry = directory.join("init.lua");
        std::fs::write(
            &entry,
            format!(
                r#"sol.surface("shell", {{ scene = "{}", layer = "top", on = "every-monitor" }})"#,
                scene.display()
            ),
        )
        .expect("writing the test script");

        let display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        let left = a_screen(&mut state, "instance-rows-left", (0, 0));
        let right = a_screen(&mut state, "instance-rows-right", (1920, 0));
        right.change_current_state(
            Some(Mode {
                size: (1280, 720).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        state.start_scripts(Some(
            Scripts::load(&entry).expect("loading the test script"),
        ));
        state.settle_monitors();
        state.publish_models();

        let mut read = |output: &Output| {
            let scene = scene_on(&mut state, "shell", output);
            ["present", "wholeX", "wholeWidth", "areaWidth", "areaHeight"]
                .map(|name| scene.get_int(name))
        };
        assert_eq!(
            read(&left),
            [1, 0, 1920, 1920, 1080],
            "the left monitor's scene: [present, whole.x, whole.width, area.width, area.height]"
        );
        assert_eq!(
            read(&right),
            [1, 1920, 1280, 1280, 720],
            "the right monitor's scene: [present, whole.x, whole.width, area.width, area.height]"
        );
    });
}

/// The last size this toplevel was configured with, as the client saw it.
fn last_configured(client: &Client, toplevel: &xdg_toplevel::XdgToplevel) -> Option<(i32, i32)> {
    let id = wayland_client::Proxy::id(toplevel);
    client
        .configures
        .iter()
        .rev()
        .find(|(to, _, _)| *to == id)
        .map(|&(_, width, height)| (width, height))
}

/// **Issue #92: a window leaving fullscreen went nowhere.**
///
/// `fullscreen_request` kept the rect to come back to on the window's
/// `Decoration`, and then dropped that decoration so the fullscreen
/// window would have no titlebar; the one `unfullscreen_request` built
/// in its place had no rect in it. With nothing to re-place it, which is
/// how this test runs -- no layout script at all -- the window stayed
/// where fullscreen put it, covering the monitor, and was configured
/// with no size. A layout that re-places windows on the relayout that
/// follows covers for it, which is why the issue saw it on floating
/// layouts and not on tiled ones.
///
/// Run with `pane = "none"` for the reason every test in this module is
/// (see the #99 test), which means this window never has a `Decoration`
/// to lose. That fails on the old code all the same, and for the second
/// half of the same defect: a window with no server-side frame had
/// nowhere to keep the rect in the first place. The half with a real
/// frame is `decoration.rs`'s
/// `a_rebuilt_frame_does_not_take_the_way_back_with_it`.
///
/// The request is sent twice, because a client may, and the second one
/// must not overwrite the way back with the monitor it is already
/// covering.
#[test]
fn a_window_leaving_fullscreen_is_back_where_it_was() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.space.map_element(window.clone(), (400, 300), false);
    state.space.refresh();
    let before = state.real_geometry(&window).expect("the window is mapped");
    assert_eq!(
        before,
        Rectangle::new((400, 300).into(), (64, 64).into()),
        "the fixture's window, where this test put it"
    );
    let id = state.panes.id_of(&window).expect("the window has a pane");

    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert_eq!(
        state.panes.get(id).map(Pane::slot),
        Some(screen),
        "fullscreen really did move the window, or coming back would \
         prove nothing"
    );
    assert_eq!(state.space.element_location(&window), Some(screen.loc));

    toplevel.unset_fullscreen();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );

    assert_eq!(
        state.panes.get(id).map(Pane::slot),
        Some(before),
        "a window leaving fullscreen goes back to the rect it had before \
         -- not the monitor it was covering, which is where it stayed \
         when there was no rect kept to go back to"
    );
    assert_eq!(
        state.space.element_location(&window),
        Some(before.loc),
        "and it is drawn there, not only recorded there"
    );
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((before.size.w, before.size.h)),
        "and the client is told the size it had, rather than being \
         configured with no size and left to guess"
    );
}

/// **A key sends the focused window fullscreen, and maximises it, as the
/// client asking and the frame's button would.** #150.
///
/// `sol.toggle_fullscreen` and `sol.toggle_maximize` with no window named,
/// through the commands they queue: the window covers the monitor and is
/// told it is fullscreen, and the same key puts it back where it was; the
/// same again for maximised.
#[test]
fn a_script_toggles_fullscreen_and_maximised_as_the_client_would() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let directory = std::env::temp_dir().join(format!("solium-toggles-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        r#"
        sol.bind("super+f", function() sol.toggle_fullscreen() end)
        sol.bind("super+shift+m", function() sol.toggle_maximize() end)
        "#,
    )
    .expect("writing the test script");
    state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the test script"),
    ));

    let (conn, event_queue, client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.space.map_element(window.clone(), (400, 300), false);
    state.space.refresh();
    let before = state.real_geometry(&window).expect("the window is mapped");
    let id = state.panes.id_of(&window).expect("the window has a pane");
    state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );

    assert!(state.trigger("super+f"), "super+f is bound");
    assert!(
        in_state(&window, State::Fullscreen),
        "the key did not send it fullscreen"
    );
    assert_eq!(state.panes.get(id).map(Pane::slot), Some(screen));
    assert!(state.trigger("super+f"));
    assert!(
        !in_state(&window, State::Fullscreen),
        "and the key again did not bring it back"
    );
    assert_eq!(state.panes.get(id).map(Pane::slot), Some(before));

    assert!(state.trigger("super+shift+m"), "super+shift+m is bound");
    assert!(
        in_state(&window, State::Maximized),
        "the key did not maximise it"
    );
    assert!(state.trigger("super+shift+m"));
    assert!(
        !in_state(&window, State::Maximized),
        "and the key again did not restore it"
    );
    assert_eq!(state.panes.get(id).map(Pane::slot), Some(before));
}

/// **Maximising a fullscreen window leaves fullscreen for it**, and the
/// way back from both is kept. #150's review.
///
/// `super+shift+m` on a fullscreen window toggled the maximise under it
/// and spent the one rect kept for coming back: the window stayed
/// fullscreen, and `super+f` then left it the size of the monitor with
/// nowhere to go. Now `super+f`, `super+shift+m` is a maximised window
/// and no longer fullscreen, and `super+shift+m` again is the window
/// where it started. From maximised, `super+f` and `super+shift+m` is
/// the window where it started as well, neither fullscreen nor
/// maximised: the key flips the maximise under the fullscreen, whichever
/// way that is.
#[test]
fn maximising_a_fullscreen_window_leaves_fullscreen_for_it() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let directory = std::env::temp_dir().join(format!("solium-toggles-fs-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        r#"
        sol.bind("super+f", function() sol.toggle_fullscreen() end)
        sol.bind("super+shift+m", function() sol.toggle_maximize() end)
        "#,
    )
    .expect("writing the test script");
    state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the test script"),
    ));

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.space.map_element(window.clone(), (400, 300), false);
    state.space.refresh();
    let before = state.real_geometry(&window).expect("the window is mapped");
    state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    macro_rules! press {
        ($combo:expr) => {
            assert!(state.trigger($combo), "{} is bound", $combo);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut event_queue,
                &mut client,
            );
        };
    }

    press!("super+f");
    assert!(in_state(&window, State::Fullscreen), "the premise");
    press!("super+shift+m");
    assert!(
        !in_state(&window, State::Fullscreen),
        "super+shift+m left the window fullscreen"
    );
    assert!(
        in_state(&window, State::Maximized),
        "super+shift+m on a fullscreen window did not maximise it"
    );
    assert_eq!(state.space.element_location(&window), Some(screen.loc));
    press!("super+shift+m");
    assert!(
        !in_state(&window, State::Maximized) && !in_state(&window, State::Fullscreen),
        "super+shift+m again did not restore the window"
    );
    assert_eq!(
        state.space.element_location(&window),
        Some(before.loc),
        "the way back from fullscreen was spent"
    );
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((before.size.w, before.size.h))
    );

    press!("super+shift+m");
    press!("super+f");
    assert!(
        in_state(&window, State::Fullscreen) && in_state(&window, State::Maximized),
        "the premise: fullscreen over maximised"
    );
    press!("super+shift+m");
    assert!(
        !in_state(&window, State::Maximized) && !in_state(&window, State::Fullscreen),
        "super+shift+m on a fullscreen, maximised window left a state behind"
    );
    assert_eq!(state.space.element_location(&window), Some(before.loc));
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((before.size.w, before.size.h))
    );
}

/// **A window maximised with no way back kept is restored with no
/// size.** #150's second review.
///
/// A window that went fullscreen before it had drawn keeps no rect to
/// come back to, and `super+shift+m` takes it from fullscreen to
/// maximised. The next `super+shift+m` found nothing to restore and
/// maximised it again, so the press did nothing. Now it leaves
/// maximised as leaving fullscreen does with nothing kept: the client
/// is configured with no size, and picks its own.
#[test]
fn a_window_maximised_with_no_way_back_is_restored_with_no_size() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let directory =
        std::env::temp_dir().join(format!("solium-toggles-bare-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        r#"
        sol.bind("super+shift+m", function() sol.toggle_maximize() end)
        "#,
    )
    .expect("writing the test script");
    state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the test script"),
    ));

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    client.leave_frame(&toplevel, &qh);
    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    let window = state
        .space
        .elements()
        .next()
        .cloned()
        .expect("new_toplevel maps a window before it has drawn anything");
    let id = state.panes.id_of(&window).expect("the window has a pane");
    commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );

    assert!(state.trigger("super+shift+m"), "super+shift+m is bound");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert!(
        in_state(&window, State::Maximized) && !in_state(&window, State::Fullscreen),
        "the premise: out of fullscreen into maximised"
    );
    assert_eq!(
        state.panes.get(id).and_then(Pane::restore),
        None,
        "the premise: no way back kept"
    );

    client.configures.clear();
    assert!(state.trigger("super+shift+m"));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert!(
        !in_state(&window, State::Maximized),
        "the second press maximised the window again"
    );
    assert_eq!(
        sizes_sent(&client, &toplevel),
        vec![(0, 0)],
        "with nowhere kept to go back to, the client picks its own size"
    );
}

/// **A window with no frame toggles back from maximised.**
///
/// The rect a maximise goes back to lived on the window's `Decoration`,
/// and a window with no server-side frame -- one that draws its own, or
/// any window under `pane = "none"` -- has none, so on stage this
/// toggle kept nothing for such a window and the second call maximised
/// it again. Found with #92, and fixed by the same move.
///
/// **Latent until #150.** `toggle_maximize` had one caller,
/// `frame_action`, reached only from a button on a `Styled` frame, and a
/// window with no frame had no button to press. `sol.toggle_maximize`
/// and `super+shift+m` are a second way in, frame or none:
/// `a_script_toggles_fullscreen_and_maximised_as_the_client_would`. The
/// maximise-then-fullscreen tests below build on it working. The
/// frameless half of #92 that was seen is fullscreen's, in
/// `a_window_leaving_fullscreen_is_back_where_it_was`.
#[test]
fn a_window_with_no_frame_toggles_back_from_maximised() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.space.map_element(window.clone(), (400, 300), false);
    state.space.refresh();
    let before = state.real_geometry(&window).expect("the window is mapped");
    let surface = window.toplevel().cloned().expect("an xdg toplevel");
    // The server's `xdg_toplevel`, which this module's `use` of the
    // client's shadows.
    let maximized = || {
        surface.with_pending_state(|pending| {
            pending.states.contains(
                smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
            )
        })
    };

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert!(maximized(), "the first press maximises");
    assert_eq!(
        state.space.element_location(&window),
        Some(screen.loc),
        "and moves the window to the work area, which is the whole \
         monitor here, or coming back would prove nothing"
    );
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((screen.size.w, screen.size.h))
    );

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert!(
        !maximized(),
        "the second press restores, rather than maximising again"
    );
    assert_eq!(
        state.space.element_location(&window),
        Some(before.loc),
        "back to exactly where it was"
    );
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((before.size.w, before.size.h)),
        "at exactly the size it was"
    );
}

/// A 1920x1080 monitor at 1x, mapped at `at`.
///
/// Named, because two of them in one test must not share a name:
/// `place_outputs` goes by names, and the unplug tests below run it.
fn a_screen(state: &mut Solium, name: &str, at: (i32, i32)) -> Output {
    let output = Output::new(
        name.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "solium".to_string(),
            model: name.to_string(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: (1920, 1080).into(),
            refresh: 60_000,
        }),
        None,
        Some(Scale::Fractional(1.0)),
        None,
    );
    state.space.map_output(&output, at);
    output
}

/// Make `state`'s keyboard `us,ru` with Russian the active group: every
/// input change is tested with the Cyrillic group active (#132).
fn russian(state: &mut Solium) {
    use smithay::input::keyboard::{Layout, XkbConfig};
    let keyboard = state.seat.get_keyboard().expect("the seat has a keyboard");
    keyboard
        .set_xkb_config(
            state,
            XkbConfig {
                layout: "us,ru",
                ..Default::default()
            },
        )
        .expect("compiling us,ru; xkb data is missing, so this proves nothing");
    let active = keyboard.with_xkb_state(state, |mut context| {
        context.set_layout(Layout(1));
        context
            .xkb()
            .lock()
            .map(|xkb| xkb.active_layout().0)
            .unwrap_or(0)
    });
    assert_eq!(
        active, 1,
        "Russian did not become the active group, so this would test `us`"
    );
}

/// A script's interactive surface over `rect`, its scene stood in for by
/// `stand`: a Qt scene cannot be built beside a real client (the #99
/// test).
fn stand_in(
    state: &mut Solium,
    name: &str,
    layer: crate::scripted::Layer,
    rect: Rectangle<i32, Logical>,
    stand: crate::scripted::Stand,
) -> crate::scripted::SurfaceId {
    state.declare_surface(crate::scripted::Declaration::for_test(
        name,
        std::path::PathBuf::from("/nonexistent/hosted-test.qml"),
        layer,
        crate::scripted::On::Rect(rect),
    ));
    let id = state.surfaces.named(name).expect("declared");
    state.surfaces.get_mut(id).expect("live").stand_in(stand);
    id
}

/// Every pointer event the stand-in for surface `id` was told.
fn scene_events(
    state: &Solium,
    id: crate::scripted::SurfaceId,
) -> Vec<crate::qml::hosted::ScenePointer> {
    state
        .surfaces
        .get(id)
        .and_then(crate::scripted::Surface::stand)
        .map(|stand| stand.seen.iter().map(|(_, event)| *event).collect())
        .unwrap_or_default()
}

/// Move the pointer to `at`, through the real input path.
fn move_pointer(state: &mut Solium, at: (f64, f64), time: u64) {
    let was = state
        .seat
        .get_pointer()
        .map(|pointer| pointer.current_location())
        .unwrap_or_default();
    let region = crate::monitor::union(&state.space).expect("a monitor");
    crate::synth::send_motion(state, region, Point::from(at) - was, time);
}

/// A bar across the top of the primary monitor, `height` pixels tall,
/// holding them as its exclusive zone.
///
/// Committed once and with no buffer, which is enough: the zone is
/// double-buffered state that the initial commit applies, and
/// `configure_layer` arranges the monitor on that commit. The caller
/// checks the work area it leaves rather than trusting this.
fn bar(
    display: &mut Display<Solium>,
    state: &mut Solium,
    conn: &Connection,
    client: &Client,
    qh: &QueueHandle<Client>,
    height: i32,
) -> (
    wl_surface::WlSurface,
    zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
) {
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let shell = client
        .layer_shell
        .clone()
        .expect("zwlr_layer_shell_v1 bound");
    let surface = compositor.create_surface(qh, ());
    let layer = shell.get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Top,
        "restore-test-bar".to_string(),
        qh,
        (),
    );
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer.set_size(0, height.unsigned_abs());
    layer.set_exclusive_zone(height);
    surface.commit();
    conn.flush().expect("flushing the bar");
    display
        .dispatch_clients(state)
        .expect("dispatching the bar");
    (surface, layer)
}

/// Whether the server has this window in `wanted`, as the next
/// configure it is sent will say.
fn in_state(
    window: &Window,
    wanted: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State,
) -> bool {
    window.toplevel().is_some_and(|surface| {
        surface.with_pending_state(|pending| pending.states.contains(wanted))
    })
}

/// Every size this toplevel has been configured with, oldest first.
fn sizes_sent(client: &Client, toplevel: &xdg_toplevel::XdgToplevel) -> Vec<(i32, i32)> {
    let id = wayland_client::Proxy::id(toplevel);
    client
        .configures
        .iter()
        .filter(|(to, _, _)| *to == id)
        .map(|&(_, width, height)| (width, height))
        .collect()
}

/// Whether each configure this toplevel has been sent said it is tiled,
/// oldest first.
/// Whether each configure this toplevel has been sent said it is
/// maximised, oldest first.
fn maximized_sent(client: &Client, toplevel: &xdg_toplevel::XdgToplevel) -> Vec<bool> {
    let id = wayland_client::Proxy::id(toplevel);
    client
        .maximized
        .iter()
        .filter(|(to, _)| *to == id)
        .map(|&(_, maximized)| maximized)
        .collect()
}

fn tiled_sent(client: &Client, toplevel: &xdg_toplevel::XdgToplevel) -> Vec<bool> {
    let id = wayland_client::Proxy::id(toplevel);
    client
        .tiled
        .iter()
        .filter(|(to, _)| *to == id)
        .map(|&(_, tiled)| tiled)
        .collect()
}

/// **#92 review, finding 1: a maximised window leaving fullscreen is
/// maximised again.**
///
/// Maximise and fullscreen keep their way back in the one slot on the
/// pane, and a window maximised and then sent fullscreen keeps the rect
/// from before the maximise in it. Leaving fullscreen took that rect:
/// the window was placed and sized un-maximised while its xdg state
/// still said `Maximized`, and the slot was empty, so the next toggle
/// maximised it again rather than restoring it. A maximised browser
/// sent fullscreen for a video came back small on Esc, still told it
/// was maximised.
///
/// The bar is what makes the placement assertion mean anything. Without
/// one the work area is the whole monitor, which is also where
/// fullscreen put the window, so "back at the work area" and "left where
/// fullscreen put it" would be the same rectangle.
///
/// `pane = "none"`, as everywhere in this module, so there is no frame's
/// share to take out of the work area. `toggle_maximize` and leaving
/// fullscreen both take it from `frame_insets`, and this does not test
/// that they agree about a frame that is really there.
#[test]
fn a_maximised_window_leaving_fullscreen_is_maximised_again() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let _bar = bar(&mut display, &mut state, &conn, &client, &qh, 30);
    let work = state.work_area_on(&output).expect("the monitor is mapped");
    assert_eq!(
        work,
        Rectangle::new((0, 30).into(), (1920, 1050).into()),
        "the bar holds the top of the screen, or the work area is the \
         monitor and nothing below can tell maximised from fullscreen"
    );

    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.space.map_element(window.clone(), (400, 300), false);
    state.space.refresh();
    let before = state.real_geometry(&window).expect("the window is mapped");
    let id = state.panes.id_of(&window).expect("the window has a pane");

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert!(in_state(&window, State::Maximized));
    assert_eq!(state.space.element_location(&window), Some(work.loc));

    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert_eq!(
        state.space.element_location(&window),
        Some(screen.loc),
        "fullscreen covers the bar, so leaving it has somewhere to come \
         back from"
    );

    client.configures.clear();
    toplevel.unset_fullscreen();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );

    assert!(!in_state(&window, State::Fullscreen));
    assert!(
        in_state(&window, State::Maximized),
        "it was maximised when it went fullscreen and nothing has \
         un-maximised it since"
    );
    assert_eq!(
        state.space.element_location(&window),
        Some(work.loc),
        "so it is placed maximised, below the bar -- not at the rect \
         from before the maximise, which is where leaving fullscreen \
         used to put it while its state still said maximised"
    );
    assert_eq!(state.panes.get(id).map(Pane::slot), Some(work));
    assert_eq!(
        sizes_sent(&client, &toplevel),
        vec![(work.size.w, work.size.h)],
        "and it is told the work area's size, once"
    );
    assert_eq!(
        state.panes.get(id).and_then(Pane::restore),
        Some(before),
        "the rect from before the maximise is still kept, for the \
         un-maximise"
    );

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert!(
        !in_state(&window, State::Maximized),
        "the next toggle restores, rather than maximising again"
    );
    assert_eq!(
        state.space.element_location(&window),
        Some(before.loc),
        "to where it was before the maximise"
    );
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((before.size.w, before.size.h))
    );
}

/// **#92 review, finding 1: leaving fullscreen needs a window that is
/// fullscreen.**
///
/// A client may send `unset_fullscreen` whenever it likes. One that
/// sent it while merely maximised had the maximise's way back spent on
/// it, because the two share one slot: the window jumped back to its
/// pre-maximise rect still marked maximised, and the next toggle
/// maximised it again.
#[test]
fn a_window_that_is_not_fullscreen_has_nothing_to_leave() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.space.map_element(window.clone(), (400, 300), false);
    state.space.refresh();
    let before = state.real_geometry(&window).expect("the window is mapped");
    let id = state.panes.id_of(&window).expect("the window has a pane");

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert!(in_state(&window, State::Maximized));

    client.configures.clear();
    toplevel.unset_fullscreen();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert_eq!(
        state.panes.get(id).and_then(Pane::restore),
        Some(before),
        "a window that was never fullscreen keeps its maximise's way back"
    );
    assert_eq!(
        state.space.element_location(&window),
        Some(screen.loc),
        "and stays maximised where it was"
    );
    assert_eq!(
        sizes_sent(&client, &toplevel),
        Vec::<(i32, i32)>::new(),
        "and is told nothing, because nothing changed"
    );

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert!(!in_state(&window, State::Maximized));
    assert_eq!(state.space.element_location(&window), Some(before.loc));
}

/// **#92 review, finding 3: leaving fullscreen is one configure.**
///
/// It was two: the size cleared -- 0x0, "pick your own" -- and then the
/// size to go back to. A client that acts on every configure it reads,
/// a terminal reflowing its grid, resized twice, once to a size of its
/// own choosing and once to the real one. On stage the second was never
/// sent, because there was never a rect to send.
#[test]
fn a_window_leaving_fullscreen_is_told_its_size_once() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let _output = one_screen(&mut state);

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.space.map_element(window.clone(), (400, 300), false);
    state.space.refresh();
    let before = state.real_geometry(&window).expect("the window is mapped");

    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    client.configures.clear();
    toplevel.unset_fullscreen();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert_eq!(
        sizes_sent(&client, &toplevel),
        vec![(before.size.w, before.size.h)],
        "one configure, with the size it had before"
    );
}

/// **#92 review, finding 4: a window leaving fullscreen on a monitor
/// that has gone comes back on one that is here.**
///
/// The rect a window goes back to is stored, and a stored rect can be
/// on a monitor that has since been unplugged or gone to sleep.
/// `rescue_offscreen` brings the fullscreen window itself onto a
/// remaining screen, but not the rect it goes back to, and it runs
/// only when the monitors change: leaving fullscreen put the window on
/// no screen at all, and it stayed there until the next hotplug.
#[test]
fn a_window_leaving_fullscreen_after_its_monitor_went_is_on_a_screen() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let left = a_screen(&mut state, "restore-left", (0, 0));
    let right = a_screen(&mut state, "restore-right", (1920, 0));
    let remaining = state
        .space
        .output_geometry(&left)
        .expect("the left monitor is mapped");

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.space.map_element(window.clone(), (2320, 300), false);
    state.space.refresh();
    let before = state.real_geometry(&window).expect("the window is mapped");

    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert_eq!(
        state.space.element_location(&window),
        Some((1920, 0).into()),
        "fullscreen on the monitor the window is on, the right one"
    );

    // Unplugged, the way both backends do it.
    state.space.unmap_output(&right);
    state.settle_monitors();
    let rescued = state.real_geometry(&window).expect("the window is mapped");
    assert!(
        remaining.overlaps(rescued),
        "rescue_offscreen brought the fullscreen window onto the screen \
         that is left, or leaving fullscreen would start from nowhere: \
         {rescued:?}"
    );

    toplevel.unset_fullscreen();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    let back = state.real_geometry(&window).expect("the window is mapped");
    assert!(
        state.on_any_output(back),
        "the window that left fullscreen is on a screen: {back:?}, not \
         at the {before:?} it had on the monitor that went"
    );
    assert_eq!(
        back,
        Rectangle::new((1920 - 64, 300).into(), (64, 64).into()),
        "moved the way rescue_offscreen moves a stranded window: onto \
         the nearest screen, its size kept, clamped at the edge it was \
         beyond"
    );
    assert_eq!(last_configured(&client, &toplevel), Some((64, 64)));
}

/// **#92 review, finding 4, for a maximise.**
///
/// The same stored rect and the same hazard. Leaving fullscreen now
/// puts a maximised window back at its work area and leaves the rect
/// from before the maximise for the un-maximise, so a window maximised
/// on a monitor that then goes needs the un-maximise to check too.
#[test]
fn a_window_maximised_on_a_monitor_that_went_restores_onto_a_screen() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let left = a_screen(&mut state, "restore-left", (0, 0));
    let right = a_screen(&mut state, "restore-right", (1920, 0));
    let remaining = state
        .space
        .output_geometry(&left)
        .expect("the left monitor is mapped");

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.space.map_element(window.clone(), (2320, 300), false);
    state.space.refresh();

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert_eq!(
        state.space.element_location(&window),
        Some((1920, 0).into()),
        "maximised on the monitor the window is on, the right one"
    );

    state.space.unmap_output(&right);
    state.settle_monitors();
    let rescued = state.real_geometry(&window).expect("the window is mapped");
    assert!(
        remaining.overlaps(rescued),
        "rescue_offscreen brought the maximised window onto the screen \
         that is left: {rescued:?}"
    );

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    let back = state.real_geometry(&window).expect("the window is mapped");
    assert_eq!(
        back,
        Rectangle::new((1920 - 64, 300).into(), (64, 64).into()),
        "restored onto the screen that is left, not to 2320,300 on the \
         one that went"
    );
    assert_eq!(last_configured(&client, &toplevel), Some((64, 64)));
}

/// **#92 review, finding 5: a window fullscreen before it has drawn
/// keeps no way back.**
///
/// A player started with `--fs` asks before its first commit.
/// `new_toplevel` has mapped it at 0,0 by then, with no buffer and so no
/// size, and that 0x0 rect was kept as the way back: leaving fullscreen
/// configured 0x0 -- which on the wire is "pick your own", the same as
/// no rect at all -- and set the pane's slot to a rect of no size.
///
/// And with no rect kept, a second request once the window has drawn
/// must still not keep one: the window is fullscreen by then, and the
/// rect it has is the monitor.
#[test]
fn a_window_fullscreen_before_it_has_drawn_keeps_no_way_back() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    client.leave_frame(&toplevel, &qh);
    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    let window = state
        .space
        .elements()
        .next()
        .cloned()
        .expect("new_toplevel maps a window before it has drawn anything");
    let id = state.panes.id_of(&window).expect("the window has a pane");
    assert_eq!(
        state.panes.get(id).and_then(Pane::restore),
        None,
        "a window with no size has no rect worth going back to"
    );

    commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert_eq!(
        state.panes.get(id).and_then(Pane::restore),
        None,
        "asking again while fullscreen does not keep the monitor as the \
         way back"
    );

    client.configures.clear();
    toplevel.unset_fullscreen();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    assert_eq!(
        sizes_sent(&client, &toplevel),
        vec![(0, 0)],
        "with nowhere kept to go back to, the client picks its own size"
    );
    assert!(
        state
            .panes
            .get(id)
            .is_some_and(|pane| !pane.slot().is_empty()),
        "and the pane is not given a rect of no size"
    );
}

/// **#361: a window fullscreen before it has drawn opens over the bar,
/// at the monitor's size.**
///
/// `show_if_new` fitted every window into the work area at its first
/// frame, fullscreen or not, so a player started with `--fs` under a bar
/// was told the work area's size while still marked fullscreen, and was
/// put below the bar. The X11 game that maps covering its monitor goes
/// fullscreen at the same moment, before it has drawn, and takes the same
/// path; no test can make an X11 window, so this one stands for both.
#[test]
fn a_window_fullscreen_before_it_has_drawn_opens_over_the_bar_at_the_monitors_size() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");

    let (conn, mut event_queue, mut client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let _bar = bar(&mut display, &mut state, &conn, &client, &qh, 30);
    let work = state.work_area_on(&output).expect("the monitor is mapped");
    assert_ne!(
        work, screen,
        "the bar holds the top of the screen, or the work area is the \
         monitor and nothing below can tell the two apart"
    );

    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    client.leave_frame(&toplevel, &qh);
    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );
    let window = state
        .space
        .elements()
        .next()
        .cloned()
        .expect("new_toplevel maps a window before it has drawn anything");

    commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut event_queue,
        &mut client,
    );

    assert!(in_state(&window, State::Fullscreen));
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((screen.size.w, screen.size.h)),
        "its first frame did not fit it into the work area: it is told \
         the monitor's size, which is what fullscreen told it"
    );
    assert_eq!(
        state.real_geometry(&window),
        Some(screen),
        "and it covers the monitor, bar and all"
    );
}

mod fullscreen_glides;

/// **A modal cannot be buried under the window it is waiting on.**
///
/// The regression floating them introduced. Tiled, a dialog took a slot
/// of its own and overlapped nothing, so there was nowhere for it to be
/// lost; floated, it sits *on* its parent, and nothing in the stack said
/// which of the two belongs on top. `Space::map_element` takes an
/// element out of the stack and pushes it back at the top whatever
/// `activate` says, so every raise buried the prompt: one click on the
/// strip of document left showing, or one pointer crossing with
/// `focus_follows_mouse`, and "Discard changes?" was behind the window
/// refusing to accept keystrokes until it is answered.
///
/// The third window is what stops this passing for the wrong reason. If
/// `focus_window` had quietly stopped restacking at all, the dialog
/// would still be above its parent and the test would be green over a
/// broken raise -- so the parent is also asserted to have come above the
/// window it was under.
#[test]
fn a_modal_stays_above_the_window_it_is_waiting_on() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    // See the #99 test: a Qt scene in a process holding a libwayland
    // connection of its own aborts the whole test binary.
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));

    let (conn, event_queue, client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();

    let (parent, parent_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    let (dialog, dialog_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    let (other, _other_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);

    // The two requests that make this a modal dialog, in the order a
    // toolkit sends them and from the only side that can send them.
    let dialogs = client.dialogs.clone().expect("xdg_wm_dialog_v1 bound");
    dialog_toplevel.set_parent(Some(&parent_toplevel));
    let object = dialogs.get_xdg_dialog(&dialog_toplevel, &qh, ());
    object.set_modal();
    conn.flush().expect("flushing set_parent and set_modal");
    display
        .dispatch_clients(&mut state)
        .expect("dispatching set_parent and set_modal");

    // The fixture is doing what it claims before anything is asserted
    // about stacking: the compositor agrees this is a modal, and agrees
    // whose.
    assert!(
        state.is_modal(&dialog),
        "the client called set_modal and the compositor did not read it \
         back, so nothing below is about a modal dialog at all"
    );
    let parent_pane = state.panes.id_of(&parent).expect("the parent has a pane");
    assert_eq!(
        state.parent_of(&dialog),
        Parentage::Window(parent_pane.get()),
        "the client called set_parent and the compositor did not read it \
         back"
    );

    // `elements` is back to front, so a higher index is nearer the top.
    let depth = |state: &Solium, window: &Window| {
        state
            .space
            .elements()
            .position(|element| element == window)
            .expect("a mapped window is in the space")
    };
    assert!(
        depth(&state, &dialog) > depth(&state, &parent),
        "the dialog opened after its parent, so it starts above it -- and \
         a test that starts in the state it is checking for proves nothing"
    );

    // One click on the parent. This is the whole of the bug.
    state.focus_window(
        &parent,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );

    assert!(
        depth(&state, &parent) > depth(&state, &other),
        "focusing the parent did not raise it above the window that was \
         over it, so this run says nothing about what a raise does to its \
         dialog"
    );
    assert!(
        depth(&state, &dialog) > depth(&state, &parent),
        "the parent was raised over the dialog that is waiting on it: the \
         prompt is now behind the window it is blocking, where it cannot \
         be read or dismissed"
    );
}

/// **#67: a dialog drawn by another process belongs to the window it is
/// for.**
///
/// A file chooser opened through the portal is drawn by the portal, not by
/// the application that asked for it, so `xdg_toplevel.set_parent` cannot
/// name the window it is about: that window is another client's object.
/// `xdg-foreign` is the way across. The application exports its toplevel
/// and is handed a string, passes the string to the portal, and the portal
/// imports it and parents its own window to it.
///
/// Two clients, because with one the test could have been passed by
/// `set_parent` and would prove nothing about the crossing. The second
/// half is the application withdrawing the export: the relationship goes
/// with it, and `parent_of` has to come back `None` rather than keep a
/// parent the only client entitled to name it has taken back.
#[test]
fn a_dialog_from_another_client_is_parented_through_an_exported_handle() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    // See the #99 test: a Qt scene in a process holding a libwayland
    // connection of its own aborts the whole test binary.
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));

    let (app_conn, mut app_queue, mut app) = connect(&mut display, &mut state);
    let app_qh = app_queue.handle();
    let (portal_conn, portal_queue, portal) = connect(&mut display, &mut state);
    let portal_qh = portal_queue.handle();

    let (document, _document_toplevel, document_surface) =
        open_surface(&mut display, &mut state, &app_conn, &app, &app_qh);
    let (chooser, _chooser_toplevel, chooser_surface) =
        open_surface(&mut display, &mut state, &portal_conn, &portal, &portal_qh);

    let exporter = app
        .exporter
        .clone()
        .expect("zxdg_exporter_v2 is advertised, so an application can export a window");
    let exported = exporter.export_toplevel(&document_surface, &app_qh, ());
    pump(
        &mut display,
        &mut state,
        &app_conn,
        &app_qh,
        &mut app_queue,
        &mut app,
    );
    let handle = app
        .exported_handles
        .last()
        .cloned()
        .expect("the export was answered with a handle");

    let importer = portal
        .importer
        .clone()
        .expect("zxdg_importer_v2 is advertised, so a portal can import one");
    let imported = importer.import_toplevel(handle, &portal_qh, ());
    imported.set_parent_of(&chooser_surface);
    portal_conn.flush().expect("flushing the import");
    display
        .dispatch_clients(&mut state)
        .expect("dispatching the import");

    let document_pane = state
        .panes
        .id_of(&document)
        .expect("the document has a pane");
    assert_eq!(
        state.parent_of(&chooser),
        Parentage::Window(document_pane.get()),
        "the portal parented its window to the document through the \
         exported handle, and the compositor does not know whose dialog it is"
    );

    exported.destroy();
    app_conn.flush().expect("flushing the withdrawn export");
    display
        .dispatch_clients(&mut state)
        .expect("dispatching the withdrawn export");
    assert_eq!(
        state.parent_of(&chooser),
        Parentage::None,
        "the application withdrew the export, and the dialog still names \
         the window as its parent"
    );
}

/// **#71: a window says what it contains, and a script can read it.**
///
/// `wp_content_type_v1` is how a video player says "video" and a game says
/// "game". Nothing in the compositor decides anything with it yet; what is
/// under test is that the answer reaches the one place policy is written,
/// `sol.windows()`, so tearing (#37), a variable refresh rate or keeping a
/// screen awake can be decided per window rather than globally.
///
/// The type is double-buffered state, so the request alone must change
/// nothing and the commit after it must; and destroying the object goes
/// back to `"none"`, on the commit after that, as the protocol says.
/// Read through Lua rather than off `WindowInfo` so the whole path is
/// what is checked. It fails on stage at the missing
/// `wp_content_type_manager_v1`.
#[test]
fn a_window_says_what_it_contains_and_a_script_reads_what_it_committed() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    // See the #99 test: a Qt scene in a process holding a libwayland
    // connection of its own aborts the whole test binary.
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    a_screen(&mut state, "content-type-test", (0, 0));
    let (conn, mut queue, mut client) = connect(&mut display, &mut state);
    let qh = queue.handle();
    let (_window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let directory =
        std::env::temp_dir().join(format!("solium-content-type-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(&entry, "").expect("writing the test script");
    let scripts = Scripts::load(&entry).expect("loading the test script");
    let _ = std::fs::remove_dir_all(&directory);
    let said = |state: &Solium| {
        scripts.evaluate_in(
            state.snapshot(),
            "return tostring(sol.windows()[1].content_type)",
        )
    };
    assert_eq!(
        said(&state),
        "none",
        "a window that said nothing about its content does not read \"none\""
    );

    let manager = client
        .content_types
        .clone()
        .expect("wp_content_type_manager_v1 is advertised, so a window can say what it is");
    let content_type = manager.get_surface_content_type(&surface, &qh, ());
    content_type.set_content_type(wp_content_type_v1::Type::Video);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        said(&state),
        "none",
        "a content type asked for and not yet committed was read"
    );

    surface.commit();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(said(&state), "video");

    content_type.set_content_type(wp_content_type_v1::Type::Game);
    surface.commit();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(said(&state), "game");

    content_type.destroy();
    surface.commit();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        said(&state),
        "none",
        "the window took back what it said, and is still read as a game"
    );
}

/// **A window being dragged is told so: the xdg-shell `resizing` state,
/// on every configure of the drag, and gone at the release.**
///
/// GTK, Qt, Chromium and foot read it, and KWin and Hyprland both set it
/// for the length of a drag; Solium never did. The release takes it off so
/// the last configure a client hears is an ordinary one.
#[test]
fn a_dragged_window_is_told_it_is_resizing_until_the_release() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    // See the #99 test: a Qt scene in a process holding a libwayland
    // connection of its own aborts the whole test binary.
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let (conn, mut queue, mut client) = connect(&mut display, &mut state);
    let qh = queue.handle();
    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    let before = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configure_states.clear();
    let id = wayland_client::Proxy::id(&toplevel);
    let resizing = xdg_toplevel::State::Resizing as u32;
    let states = |client: &Client| -> Vec<Vec<u32>> {
        client
            .configure_states
            .iter()
            .filter(|(sent_to, _)| sent_to == &id)
            .map(|(_, states)| states.clone())
            .collect()
    };

    state.begin_resize(&window);
    for step in 1..=3 {
        let wanted = Rectangle::new(
            before.loc,
            (before.size.w + 10 * step, before.size.h).into(),
        );
        state.pending_resize = Some(ResizeRequest {
            window: window.clone(),
            wanted,
            edge_at: payload_edge(wanted, ResizeEdge::Right),
            edges: ResizeEdge::Right,
        });
        state.settle_resize();
    }
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let during = states(&client);
    assert!(
        !during.is_empty() && during.iter().all(|sent| sent.contains(&resizing)),
        "every configure of a live drag says the window is being resized; \
         got {during:?}"
    );

    client.configure_states.clear();
    state.release_resize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let after = states(&client);
    assert!(
        after.last().is_some_and(|sent| !sent.contains(&resizing)),
        "the release's configure is an ordinary one again; got {after:?}"
    );
}

/// **The window follows its application while an edge is dragged, unless
/// `resize.follow` says the pointer.**
///
/// Zoomed in on the corner of a fast drag on 2026-10-16, every client
/// showed a band of `resize.background` where the window had run ahead of
/// its picture, and its picture cut where the window had shrunk past it.
/// KWin shows neither: it draws the window at the size its application has
/// drawn. Here that is the default, pinned to the edges the drag is not
/// moving -- a top-left drag keeps the bottom-right corner where it is, so
/// the #113 shake cannot come back -- and the client is still told the
/// size the pointer asked for.
#[test]
fn a_dragged_window_follows_its_application_unless_told_otherwise() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    // See the #99 test: a Qt scene in a process holding a libwayland
    // connection of its own aborts the whole test binary.
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    let (conn, mut queue, mut client) = connect(&mut display, &mut state);
    let qh = queue.handle();
    let (window, toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    let before = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    let painted = window.geometry().size;
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    // The top-left corner, 20 right and 12 down.
    let wanted = Rectangle::new(
        (before.loc.x + 20, before.loc.y + 12).into(),
        (before.size.w - 20, before.size.h - 12).into(),
    );
    let drag = |state: &mut Solium| {
        state.begin_resize(&window);
        state.pending_resize = Some(ResizeRequest {
            window: window.clone(),
            wanted,
            edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
            edges: ResizeEdge::TopLeft,
        });
        state.settle_resize();
    };
    drag(&mut state);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        window.geometry().size,
        painted,
        "the fixture's client never draws the new size, which is the moment \
         this test is about"
    );
    let id = wayland_client::Proxy::id(&toplevel);
    assert!(
        client
            .configures
            .iter()
            .any(|(to, w, h)| to == &id && (*w, *h) == (wanted.size.w, wanted.size.h)),
        "the client is told the size the pointer asked for; got {:?}",
        client.configures
    );
    assert_eq!(
        state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle"),
        before,
        "drawn at the size its client has drawn, with the bottom-right corner \
         it was not dragged by where it was: nothing uncovered, nothing cut"
    );

    // Told the pointer, the same drag draws the pointer's rectangle.
    state.release_resize(&window);
    state.resizing.follow = crate::resizing::Follow::Pointer;
    drag(&mut state);
    assert_eq!(
        state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle"),
        wanted,
        "under `follow = \"pointer\"` the window is the rectangle being dragged"
    );
}

/// **This is issue #113, through the compositor rather than beside it.**
///
/// A top-left drag, on the first frame, with a real client that has
/// committed one buffer and will not commit another. That last part is
/// the whole test: the client answers *nothing*, so what is asserted is
/// what the compositor draws while it is waiting — which is exactly the
/// window the shake lives in.
///
/// **A test that checked the final rectangle would pass on `stage`.**
/// The client does commit eventually and the window does end up the
/// right size; the bug is entirely in the frames before that. So this
/// asserts the rectangle with no answer in hand, and it asserts the
/// *pinned* edge rather than the dragged one — `stage` gets the dragged
/// corner right, because it applies the origin immediately. What it
/// gets wrong is the opposite corner, which it moves by the whole of
/// the drag's delta and then snaps back.
///
/// Confirmed failing against `stage`'s rule, not merely assumed to: the
/// rectangle `stage` would produce is computed here from the same
/// inputs and asserted to be wrong. `stage`'s `pane_geometry` answers
/// `real_geometry`, which is the space's location paired with the
/// *client's* size — `on_stage` below, spelled out.
#[test]
fn the_edge_being_dragged_is_the_edge_that_moves_before_any_client_answers() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    // See the #99 test: a Qt scene in a process holding a libwayland
    // connection of its own aborts the whole test binary.
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    // The pointer's rectangle is what this test is about: see
    // `a_dragged_window_follows_its_application_unless_told_otherwise` for
    // the default, which draws the window at its client's size instead.
    state.resizing.follow = crate::resizing::Follow::Pointer;

    let (conn, event_queue, client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);

    // Away from the origin, so a drag that moved the window to where it
    // already was could not pass by accident.
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    let before = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    let painted = window.geometry().size;
    assert!(
        painted.w > 20 && painted.h > 12,
        "the drag below has to leave a window with a size"
    );

    // The drag: the top-left corner, 20 right and 12 down. Smaller by
    // that much, moved by that much, with the bottom-right corner
    // standing still — that is what dragging a top-left corner means.
    let wanted = Rectangle::new(
        (before.loc.x + 20, before.loc.y + 12).into(),
        (before.size.w - 20, before.size.h - 12).into(),
    );
    state.pending_resize = Some(ResizeRequest {
        window: window.clone(),
        wanted,
        edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
        edges: ResizeEdge::TopLeft,
    });
    state.settle_resize();

    // The client has been asked and has said nothing. Asserted rather
    // than assumed, because a client that *had* answered would make
    // every assertion below true for the wrong reason.
    assert_eq!(
        window.geometry().size,
        painted,
        "the fixture's client never commits a second buffer; if it had, \
         this test would be checking the easy case"
    );

    let after = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    assert_eq!(
        after, wanted,
        "the pane takes the dragged rectangle whole — origin and size \
         in the same frame — or the two halves land at different times \
         and that gap is the shake"
    );
    assert_eq!(
        (after.loc.x + after.size.w, after.loc.y + after.size.h),
        (before.loc.x + before.size.w, before.loc.y + before.size.h),
        "the bottom-right corner is not being dragged and must not move"
    );

    // And `stage`'s rule, computed from the same two inputs: the new
    // origin paired with the client's size, which is what
    // `real_geometry` answers and what `pane_geometry` used to return.
    let on_stage = Rectangle::new(wanted.loc, painted);
    assert_ne!(
        (
            on_stage.loc.x + on_stage.size.w,
            on_stage.loc.y + on_stage.size.h
        ),
        (before.loc.x + before.size.w, before.loc.y + before.size.h),
        "the control: pairing the new origin with the client's old size \
         walks the corner nobody is dragging across the desktop, and \
         snaps it back when the client finally commits. That is #113, \
         and it is what this test fails on against stage"
    );
    assert_eq!(
        on_stage.loc.x + on_stage.size.w,
        before.loc.x + before.size.w + 20,
        "by the drag's whole delta, every frame"
    );

    // The space agrees about *position* throughout. Only the size is
    // held back, because only the size needs the client's consent —
    // holding the position back too would put the space and the pane
    // into the standing disagreement #84 is about, for no gain.
    assert_eq!(
        state
            .space
            .element_location(&window)
            .expect("a mapped window has a location"),
        wanted.loc,
        "the slot is authoritative for the size, not for where the \
         window is; nothing here may add a fourth opinion about that"
    );
}

/// **The round trip this whole fix rests on, with a frame's insets in
/// it, on all eight edges.**
///
/// `hold_resize` writes the pane's slot as `inner(wanted)` and every
/// reader grows it back with `pane_outer`, so `pane_outer(inner(wanted))
/// == wanted` is what makes the dragged rectangle survive the trip. It
/// held for zero insets whichever spelling was used, which is why a
/// window with no frame could not catch this: `frame_insets` asks
/// `is_decorated` — `Styled` and nothing else — while every reader goes
/// through `insets_of`, which reserves a titlebar for `Frame::Pending`
/// so a window does not change shape when its frame arrives.
///
/// `Frame::Pending` is not a moment, it is where a pane whose
/// decoration *failed to build* stays for good — see
/// `decoration::Decorations::insert`, which logs and leaves it there. So
/// the two spellings disagreeing meant that for those windows the slot
/// grew by a titlebar on every frame of a drag: the top edge landing a
/// titlebar above the rectangle under the pointer, and the client asked
/// for a size a titlebar too tall.
///
/// Set by hand rather than by failing a build, because building a frame
/// needs Qt and a Qt scene in a process holding a libwayland connection
/// aborts the test binary (see the #99 test). The state is the same
/// state; how a pane got into it is `decoration.rs`'s business.
#[test]
fn a_framed_window_is_dragged_to_the_rectangle_the_pointer_asks_for() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    // The pointer's rectangle is what this test measures the insets
    // against; the default draws the window at its client's size.
    state.resizing.follow = crate::resizing::Follow::Pointer;

    let (conn, event_queue, client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");

    // A pane reserving room for a frame that is never coming.
    state
        .panes
        .get_mut(pane)
        .expect("the pane is here")
        .set_frame(crate::pane::Frame::Pending);
    assert!(
        !state.is_decorated(&window),
        "`Pending` is not decorated, which is exactly why the two \
         spellings of the insets disagree about it"
    );
    assert_eq!(
        state.insets_of(pane).top,
        TITLEBAR_HEIGHT,
        "and it reserves a titlebar all the same, or this test is about \
         two things that agree"
    );

    for edges in [
        ResizeEdge::Top,
        ResizeEdge::Bottom,
        ResizeEdge::Left,
        ResizeEdge::Right,
        ResizeEdge::TopLeft,
        ResizeEdge::TopRight,
        ResizeEdge::BottomLeft,
        ResizeEdge::BottomRight,
    ] {
        // A fresh gesture each time, which also reconciles the previous
        // one rather than leaving it hanging.
        let before = state
            .begin_resize(&window)
            .expect("a mapped pane has a rectangle");

        // Twenty pixels out of whichever edges this drag holds, with
        // the opposite ones standing still. `resized` is where that
        // arithmetic lives and it is tested beside itself; what is
        // being checked here is only that the rectangle survives the
        // compositor.
        let (left, top) = (
            crate::input::resize::pulls_left(edges),
            crate::input::resize::pulls_top(edges),
        );
        let right = matches!(
            edges,
            ResizeEdge::Right | ResizeEdge::TopRight | ResizeEdge::BottomRight
        );
        let bottom = matches!(
            edges,
            ResizeEdge::Bottom | ResizeEdge::BottomLeft | ResizeEdge::BottomRight
        );
        let wanted = Rectangle::new(
            (
                before.loc.x - i32::from(left) * 20,
                before.loc.y - i32::from(top) * 20,
            )
                .into(),
            (
                before.size.w + i32::from(left || right) * 20,
                before.size.h + i32::from(top || bottom) * 20,
            )
                .into(),
        );
        assert_ne!(wanted, before, "every edge has to actually move one");
        state.pending_resize = Some(ResizeRequest {
            window: window.clone(),
            wanted,
            edge_at: payload_edge(wanted, edges),
            edges,
        });
        state.settle_resize();

        assert_eq!(
            state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle"),
            wanted,
            "the drag asked for this rectangle and {edges:?} did not get \
             it: the slot is written with one spelling of the frame's \
             insets and read back with another"
        );
        // And the client is asked for what is left inside the frame,
        // not for the whole of it.
        assert_eq!(
            state
                .panes
                .get(pane)
                .expect("the pane is here")
                .slot()
                .size
                .h,
            wanted.size.h - TITLEBAR_HEIGHT,
            "a titlebar's worth of the dragged rectangle belongs to the \
             frame, so the client must not be sized the whole of it"
        );
    }
}

/// **A whole gesture inside one dispatch batch, which is an ordinary
/// quick nudge of a border.**
///
/// The press, the motion and the release all land in one calloop
/// dispatch — sixteen milliseconds is plenty — and `settle_resize` runs
/// at the frame *after* all three. So the hold is born after the
/// gesture it belongs to has already ended: `ResizeGrab::motion` only
/// records `pending_resize`, and `hold_resize` is what turns that into a
/// `Hold`.
///
/// A hold that cannot observe its own release never sets `released`,
/// and `Hold::settle` answers `Waiting` unconditionally without one. The
/// hold is then **permanent**: `holding_resize` stays true for ever,
/// `pane_geometry` keeps answering the slot, and every later size the
/// client chooses for itself is stretched into a rectangle from a drag
/// that finished minutes ago. A window soft until something drags it
/// again.
///
/// So what this waits for is the deadline, which is the *only* thing
/// that can end this gesture: the fixture's client answers nothing at
/// all. Slept rather than faked because `present::Clock` reads the
/// monotonic clock through — see its own documentation for why it has no
/// settable "now" to lie to.
#[test]
fn a_gesture_that_ends_before_its_first_frame_still_lets_go() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    // See the #99 test: a Qt scene in a process holding a libwayland
    // connection of its own aborts the whole test binary.
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));

    let (conn, event_queue, client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);

    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    let before = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");

    // The press.
    state.begin_resize(&window);
    // The motion. `ResizeGrab::motion` records and does not apply, so
    // this is the whole of what a motion does before a frame.
    let wanted = Rectangle::new(
        (before.loc.x + 20, before.loc.y + 12).into(),
        (before.size.w - 20, before.size.h - 12).into(),
    );
    state.pending_resize = Some(ResizeRequest {
        window: window.clone(),
        wanted,
        edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
        edges: ResizeEdge::TopLeft,
    });
    // And the release, still with no frame in between: this is
    // `ResizeGrab::unset`, which is where the button coming up lands.
    state.release_resize(&window);

    // *Now* the frame. This is where the hold is born, and it is born
    // into a gesture that is already over.
    state.settle_resize();
    assert!(
        state.holding_resize(pane),
        "the frame after the release is where this hold is born; if no \
         hold is created at all then this test is about nothing"
    );
    assert_eq!(
        window.geometry().size,
        before.size,
        "the fixture's client never commits a second buffer, which is \
         what leaves the deadline as the only thing that can end this"
    );

    // Past the deadline, and one more frame to notice it.
    std::thread::sleep(crate::resizing::PATIENCE + std::time::Duration::from_millis(100));
    state.settle_resize();
    assert!(
        !state.holding_resize(pane),
        "the hold outlived its own gesture's deadline, so it will outlive \
         everything: `pane_geometry` answers the slot for as long as this \
         is true and the client's own size is stretched into it for ever"
    );
    assert_eq!(
        state.pane_geometry(state.panes.get(pane).expect("the pane is still here")),
        state
            .real_geometry(&window)
            .expect("a mapped window has a rectangle"),
        "once the hold is gone the slot and the space agree again, which \
         is what makes the stretch exactly 1"
    );
}

/// **The same defect by the other route: a hold dropped mid-gesture and
/// re-created after the release.**
///
/// `settle_resize` drops the hold on any frame a layout claims the
/// drag — `trigger_resize` answering true — and creates a fresh one on
/// any frame it does not. A script whose answer changes between two
/// frames therefore destroys a hold and builds another, and if the
/// second one is built on the frame *after* the button came up it is
/// built into a gesture that is already over. Identical outcome to the
/// quick-nudge case and a completely different way in, which is why the
/// release is recorded on the compositor rather than guarded at each
/// place a hold is made.
///
/// The drop is driven directly here rather than through a Lua layout:
/// `drop_resize_hold_for` *is* the line `settle_resize` runs when a
/// layout claims the drag, and standing up a script that changes its
/// mind between frames would test mlua rather than this.
#[test]
fn a_hold_rebuilt_after_the_release_is_rebuilt_already_released() {
    let mut display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));

    let (conn, event_queue, client) = connect(&mut display, &mut state);
    let qh = event_queue.handle();
    let (window, _toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    let before = state
        .begin_resize(&window)
        .expect("a mapped pane has a rectangle");

    let dragged = |state: &mut Solium, by: i32| {
        let wanted = Rectangle::new(
            (before.loc.x + by, before.loc.y + by).into(),
            (before.size.w - by, before.size.h - by).into(),
        );
        state.pending_resize = Some(ResizeRequest {
            window: window.clone(),
            wanted,
            edge_at: payload_edge(wanted, ResizeEdge::TopLeft),
            edges: ResizeEdge::TopLeft,
        });
    };

    // A frame of ordinary drag, so a hold exists to be dropped.
    dragged(&mut state, 8);
    state.settle_resize();
    assert!(state.holding_resize(pane), "the drag is live");

    // The frame a layout claims it.
    state.drop_resize_hold_for(&window);
    assert!(!state.holding_resize(pane));

    // The button comes up with no hold to tell, and the frame after it
    // carries the last motion — which is where a hold is made again.
    state.release_resize(&window);
    dragged(&mut state, 12);
    state.settle_resize();
    assert!(
        state.holding_resize(pane),
        "the post-release frame is where the second hold is born; \
         without one there is nothing here to go wrong"
    );

    std::thread::sleep(crate::resizing::PATIENCE + std::time::Duration::from_millis(100));
    state.settle_resize();
    assert!(
        !state.holding_resize(pane),
        "the second hold never heard about the release that preceded it, \
         so nothing can ever end it"
    );
}

/// One frame of a drag that a layout claims, without standing up a
/// layout.
///
/// These three statements in this order *are* what `settle_resize` runs
/// on the claimed branch: it arms the gesture, asks the scripts, and the
/// scripts' own `apply` is what reaches `move_pane` — the whole layout
/// sweep happens inside `trigger_resize`, before the caller learns
/// whether the drag was claimed. Driving them directly is the same
/// choice `a_hold_rebuilt_after_the_release_is_rebuilt_already_released`
/// makes about `drop_resize_hold_for`, and for the same reason: a Lua
/// layout here would be testing mlua and `tiling.lua`'s arithmetic
/// rather than what the compositor does with the rectangle it is given.
///
/// `duration: ZERO` is not a simplification. `tiling.lua` passes
/// `{ duration = 0 }` for every frame of a seam drag, and the zero is
/// load-bearing: a transform that finishes instantly is retired by
/// `Solium::settle` after the very frame it was written on, which is why
/// the layout's rectangle used to survive exactly one frame.
fn tiled_frame(
    state: &mut Solium,
    request: &ResizeRequest,
    pane: crate::pane::PaneId,
    outer: Rectangle<i32, Logical>,
    now: Duration,
) {
    state.arm_resize_gesture(request);
    let was = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    state.move_pane(
        pane,
        outer,
        was,
        AnimationSpec {
            duration: Duration::ZERO,
            ..AnimationSpec::default()
        },
        now,
        Standing::Tile,
    );
    state.resize_gesture = None;
    // The claimed branch's own line: the bridge is watching every pane
    // the sweep moved, so a floating hold on this window would be a
    // second authority over one of them.
    state.drop_resize_hold_for(&request.window);
    // What `settle_resize` does after the layout has run. No-op while
    // the pointer is down, and included so the loop under test is the
    // loop that ships rather than a shorter one.
    paused_frame(state, now);
}

/// A frame of a live drag that carried **no** motion.
///
/// The whole of what `settle_resize` does when `pending_resize` is
/// empty, which is every frame the pointer is not moving on — including
/// the ones at the end of a gesture, because people stop moving before
/// they let go. The flush is the half `move_pane` cannot do: it is only
/// reached from a frame that carried a motion, so the offers made in
/// the last throttled interval of travel have nowhere else to come from.
fn paused_frame(state: &mut Solium, now: Duration) {
    state.flush_resize(now);
    state.settle_round(now);
    state.settle_resize_hold(now);
    state.settle_resize_bridge(now);
}

/// Everything a tiled-drag test needs: a compositor, a client, a mapped
/// window at a known place, and a clean configure log.
///
/// Returned as a tuple rather than a struct because the borrow checker
/// wants the queue and the client separately at every call site.
macro_rules! tiled_fixture {
    ($display:ident, $state:ident, $conn:ident, $queue:ident, $client:ident, $qh:ident) => {
        let mut $display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut $state = Solium::new($display.handle());
        // No decoration, for the reason `scale_resend` gives at length:
        // building a Qt scene inside a process that already holds a raw
        // libwayland connection aborts the whole test binary.
        $state
            .decorations
            .set_style(&mut $state.panes, Some("none".to_string()));
        // The bridge's own mechanics, a pane told on every frame and drawn
        // at its slot: `resize.sync = 0`, the default, said here so a
        // change of default cannot change what these tests are about.
        // Rounds have tests of their own,
        // `a_seam_moves_once_both_of_its_clients_have_drawn` and those
        // after it.
        $state.resizing.sync = Duration::ZERO;
        let ($conn, mut $queue, mut $client) = connect(&mut $display, &mut $state);
        let $qh = $queue.handle();
    };
}

/// **#124 review, finding 2: a client that rounds must not move the
/// seam on the first frame.**
///
/// **The cross-boundary test, and the only one there is.** Every other
/// test of this gesture lives on one side of the call or the other:
/// `input::resize::dragged_edge_tests` is arithmetic on rectangles the
/// test itself made up, and `solium_layout::tree::dragged_edge_tests`
/// feeds the tree a number the same tree produced. Neither can see the
/// link that actually broke — `Solium`'s rectangle for a pane against
/// the tree's — because neither crosses it. This one does: a real
/// `Tiling` is laid out, placed through `Solium::place` exactly as
/// `tiling.apply` places it, disagreed with by a real client over the
/// real protocol, and then the number the compositor would hand a
/// layout on the first frame of a drag is fed back into that same tree.
///
/// What #124 shipped read the edge off `Solium::pane_outer`, which goes
/// through `pane_geometry` and answers `real_geometry` for a mapped
/// client with no hold live: the space's location paired with the size
/// the *client* committed. So the first frame handed the layout the
/// client's edge and `drag_seam` obediently moved the seam there,
/// shifting the whole column by the client's rounding residue with the
/// pointer still on the pixel it pressed.
///
/// **Eight pixels, which is what makes it the nasty kind.** The client
/// below answers a cell short, which is what every terminal does to
/// every configure it is ever sent; `crate::resizing` puts the threshold
/// for calling such an answer a refusal rather than a rounding at
/// `max(asked / 20, CELL)`, so this is deliberately *under* it — the
/// compositor is meant to accept it and does. It is at or above the
/// half-gap that was the whole of #120, and it is silent.
///
/// **A right edge, because a left edge cannot show it.** `real.loc` is
/// compositor-set and `real.size` is the client's, so `pane_outer`'s
/// left and top edges are exact and its right and bottom carry the whole
/// of the disagreement. A version of this test on `Edge::Left` passes
/// against the defect.
///
/// **Off-centre first**, for the reason
/// `solium_layout::tree::dragged_edge_tests::a_window_handed_its_own_edge_does_not_move`
/// gives at length: a fixture at `split: 0.5` is where a skew is least
/// visible, and a test that cannot fail is not evidence. The seam is
/// dragged somewhere lopsided and re-placed before the client is ever
/// asked to disagree.
///
/// Confirmed failing against `ec1da24` rather than assumed: the value
/// that commit would have sent is computed here from the same live
/// compositor state and fed into a clone of the same tree, and the
/// second half of this test pins that it moves the window. Both halves
/// are needed — the first alone would pass on a tree that ignored
/// `drag_seam` entirely.
#[test]
fn a_client_that_rounds_its_size_does_not_move_the_seam() {
    use solium_layout::tree::{Edge, Tiling};

    tiled_fixture!(display, state, conn, queue, client, qh);
    let (left, _left_toplevel, left_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (right, _right_toplevel, _right_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&left)
        .expect("a client in the space has a pane");
    let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));

    let (area, settings) = (tiled_area(), tiled_settings());
    let mut tiling = Tiling::new();
    tiling.insert(left_id, None, None, area, settings);
    tiling.insert(right_id, Some(left_id), None, area, settings);
    sweep(&mut state, &tiling);

    // Lopsided, so a skew that is self-cancelling at the midpoint of
    // the seam's box cannot hide in this fixture.
    tiling.drag_seam(left_id, Edge::Right, (340.0, 300.0), area, settings);
    sweep(&mut state, &tiling);

    let slot = leaf_of(&tiling, left_id);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a rect from a layout is screen-sized, and `Solium::place` \
                  rounds these same numbers the same way"
    )]
    let asked = at(slot.x as i32, slot.y as i32, slot.w as i32, slot.h as i32);

    // First the obedient client, which is the round trip this whole
    // test rests on: what `tree:layout` returned, through `sol.place`,
    // is what the pane's rectangle becomes -- so `pane_outer` and the
    // layout's own answer agree whenever a client does as it is told.
    // The defect below is entirely about the case where one does not.
    commit_buffer(&client, &qh, &left_surface, asked.size.w, asked.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    state.sync_panes();
    assert_eq!(
        state.pane_outer_of(pane),
        Some(asked),
        "a client at the size it was asked for puts `pane_outer` on \
         the layout's own rectangle"
    );

    // And now the same client answering a cell short. This is the one
    // thing no rectangle arithmetic can fake, and the whole reason this
    // test needs a real client.
    let short = asked.size.w - 8;
    commit_buffer(&client, &qh, &left_surface, short, asked.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    state.sync_panes();
    assert_eq!(
        state.pane_outer_of(pane).map(|outer| outer.size.w),
        Some(short),
        "the client committed a size of its own and the compositor \
         took it -- if it had not, this test would be checking nothing"
    );

    // The drag begins. `begin_resize` is what both grab sites call
    // first, and its answer is `began` -- the client's rectangle.
    let began = state.begin_resize(&left).expect("a mapped pane");
    let laid_out = state
        .pane_laid_out(&left)
        .expect("a pane a layout has placed");

    // The first frame: the pointer has not moved.
    let grab: Point<f64, Logical> = (f64::from(began.loc.x + began.size.w) - 3.0, 300.0).into();
    let sent = crate::input::resize::dragged_edge(laid_out, ResizeEdge::Right, grab, grab);

    let mut unmoved = tiling.clone();
    unmoved.drag_seam(left_id, Edge::Right, sent, area, settings);
    let after = leaf_of(&unmoved, left_id);
    assert!(
        (after.x - slot.x).abs() < 0.5
            && (after.y - slot.y).abs() < 0.5
            && (after.w - slot.w).abs() < 0.5
            && (after.h - slot.h).abs() < 0.5,
        "a drag that has not moved must not move the seam. The layout \
         is handed {sent:?}; its own edge is at {:?}. {slot:?} became \
         {after:?}",
        (slot.x + slot.w, slot.y + slot.h)
    );

    // And what `ec1da24` sent, from the same state, into the same
    // tree. `began` is the rectangle that commit derived its edge from,
    // and it is a different rectangle from `laid_out` by exactly the
    // eight pixels the client kept -- asserted, because if they were
    // ever equal the half below would be testing the same thing twice.
    assert_ne!(
        began, laid_out.0,
        "the client's rectangle and the layout's have to differ here, \
         or the defect this test is about cannot arise"
    );
    let shipped = crate::input::resize::dragged_edge(
        crate::input::resize::LaidOut(began),
        ResizeEdge::Right,
        grab,
        grab,
    );
    let mut moved = tiling.clone();
    moved.drag_seam(left_id, Edge::Right, shipped, area, settings);
    let jumped = leaf_of(&moved, left_id);
    assert!(
        (jumped.w - slot.w).abs() > 4.0,
        "reading the edge off the client's rectangle moved the seam by \
         the client's rounding on frame one, which is the defect: \
         {slot:?} became {jumped:?}"
    );
}

/// Everything a #133 test needs after the fixture: one window in a
/// one-leaf tree, swept through `Solium::place` as `tiling.apply`
/// would, with every finished transform retired so that what is drawn
/// is what the pane's own geometry says -- the state a desktop is in
/// between gestures. Returns the pane and the tile it was given.
fn tiled_alone(
    state: &mut Solium,
    window: &Window,
) -> (crate::pane::PaneId, Rectangle<i32, Logical>) {
    use solium_layout::tree::Tiling;

    state.sync_panes();
    let pane = state
        .panes
        .id_of(window)
        .expect("a client in the space has a pane");
    let mut tiling = Tiling::new();
    tiling.insert(
        state.window_id(window),
        None,
        None,
        tiled_area(),
        tiled_settings(),
    );
    sweep(state, &tiling);
    let tile = state
        .panes
        .get(pane)
        .and_then(Pane::placed)
        .expect("a pane the layout has just placed is in a tile");
    state.settle(state.clock.now());
    (pane, tile)
}

/// A frame's worth of bookkeeping after a client has committed: the
/// space into the panes, and finished transforms retired.
fn a_frame(state: &mut Solium) {
    state.sync_panes();
    state.settle(state.clock.now());
}

/// **#133: a tiled client that commits more than its tile is held
/// inside it**, and the hit test stops where the tile does.
///
/// Two windows side by side, and the left one's client answers with a
/// buffer 120 pixels wider than its tile and 8 shorter -- a browser at
/// its minimum width, with a terminal's cell-grid residue on the other
/// axis. On stage `pane_geometry` answered the committed size, so the
/// pane, its frame canvas and its hit test all reached 120 pixels into
/// the right-hand window, and a press there went to the left one.
///
/// The left window is raised first, so that it is the one a hit test
/// meets first. Without that the right window would win the overlap by
/// being on top, and the last two assertions would pass on stage.
#[test]
fn a_tiled_client_that_commits_more_than_its_tile_is_held_inside_it() {
    use solium_layout::tree::Tiling;

    tiled_fixture!(display, state, conn, queue, client, qh);
    let (left, _left_toplevel, left_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (right, _right_toplevel, right_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&left)
        .expect("a client in the space has a pane");
    let beside_pane = state
        .panes
        .id_of(&right)
        .expect("a client in the space has a pane");
    let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));

    let (area, settings) = (tiled_area(), tiled_settings());
    let mut tiling = Tiling::new();
    tiling.insert(left_id, None, None, area, settings);
    tiling.insert(right_id, Some(left_id), None, area, settings);
    sweep(&mut state, &tiling);
    let placed = |state: &Solium, pane| {
        state
            .panes
            .get(pane)
            .and_then(Pane::placed)
            .expect("a pane the layout has placed is in a tile")
    };
    let (tile, beside) = (placed(&state, pane), placed(&state, beside_pane));
    assert!(
        beside.loc.x > tile.loc.x + tile.size.w,
        "the fixture is two tiles side by side, left then right: {tile:?}, {beside:?}"
    );

    // The right-hand client does as it is told, so its tile is all
    // its own; the left one does not.
    commit_buffer(&client, &qh, &right_surface, beside.size.w, beside.size.h);
    let wide = (tile.size.w + 120, tile.size.h - 8);
    commit_buffer(&client, &qh, &left_surface, wide.0, wide.1);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    state.space.raise_element(&left, false);
    a_frame(&mut state);
    assert_eq!(
        left.geometry().size,
        Size::from(wide),
        "the client really did commit more than its tile, or there is \
         nothing here to hold"
    );

    assert_eq!(
        state.pane_outer_of(pane),
        Some(Rectangle::new(tile.loc, (tile.size.w, wide.1).into())),
        "cut to the tile across, where the client is too wide, and left \
         at the client's own height, where it is short of the tile"
    );
    assert_eq!(
        state.pane_laid_out(&left).map(|laid_out| laid_out.0),
        Some(tile),
        "a tiled edge drag still starts from the layout's rectangle (#124)"
    );
    let drawn = state.drawn(pane, state.pane_outer_of(pane).expect("the pane is here"));
    assert!(
        (drawn.rect.size.w - f64::from(tile.size.w)).abs() < 0.5,
        "and drawn at the tile's width, which is what the frame canvas \
         and the titlebar are sized from: {drawn:?}"
    );

    // Inside the right-hand tile, and inside the left client's
    // committed width.
    let point: Point<f64, Logical> =
        (f64::from(beside.loc.x + 20), f64::from(beside.loc.y + 100)).into();
    assert!(
        point.x < f64::from(tile.loc.x + wide.0),
        "the point has to be one the oversized client would reach"
    );
    assert_eq!(
        state.window_under(point).map(|(window, _)| window),
        Some(right.clone()),
        "a press in the right-hand tile is the right-hand window's, \
         whatever the left one committed"
    );
    let right_surface = right
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone());
    assert_eq!(
        state.surface_under(point).map(|(surface, _)| surface),
        right_surface,
        "and so is the pointer's motion there"
    );
}

/// **Maximised is not tiled**: the work area a maximise configures is
/// not cut down to the tile the window left.
///
/// `Pane::placed` is the tile a client is held inside, and on stage
/// `toggle_maximize` never touched it -- harmless while nothing read
/// it but #124's edge drag. With the cap it is a maximised window
/// drawn in the corner its tile used to occupy.
#[test]
fn a_maximised_window_is_not_held_in_its_old_tile() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let (window, toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (pane, tile) = tiled_alone(&mut state, &window);
    assert!(
        tile.size.w < screen.size.w && tile.size.h < screen.size.h,
        "the tile has to be smaller than the monitor, or a cap to it \
         would cut nothing: {tile:?}"
    );

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((screen.size.w, screen.size.h)),
        "the client was asked for the work area"
    );
    commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);

    assert_eq!(
        state.panes.get(pane).and_then(Pane::placed),
        None,
        "a maximised window is in no tile"
    );
    assert_eq!(
        state.pane_outer_of(pane),
        Some(screen),
        "and it is the size it was maximised to, not the tile it left"
    );
}

/// **And the way back puts it in the tile again**, so a tiled edge drag
/// started from it begins at the layout's rectangle (#124).
///
/// The client answers a cell short on the way back, the way a terminal
/// does, so that the layout's rectangle and the pane's own differ: a
/// window that came back in no tile would have `pane_laid_out` fall
/// back to the client's rectangle, and a drag begun from it would move
/// the seam by the client's rounding on its first frame.
#[test]
fn a_restored_window_goes_back_into_its_tile() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (pane, tile) = tiled_alone(&mut state, &window);

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);

    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let short = tile.size.w - 8;
    commit_buffer(&client, &qh, &surface, short, tile.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);

    assert_eq!(
        state.panes.get(pane).and_then(Pane::placed),
        Some(tile),
        "the un-maximise put the window back in the tile it left"
    );
    assert_eq!(
        state.pane_outer_of(pane).map(|outer| outer.size.w),
        Some(short),
        "the client kept a cell short of its tile, so the two \
         rectangles below can be told apart"
    );
    assert_eq!(
        state.pane_laid_out(&window).map(|laid_out| laid_out.0),
        Some(tile),
        "and an edge drag starts from the layout's rectangle, not the \
         client's"
    );
}

/// **A tiled window is told it is tiled, and told when it no longer is**
/// (#228).
///
/// Without the four `tiled_*` states a client may answer a tile with a
/// size of its own, and foot did: it rounded down to whole character cells
/// and never filled its tile. So the client side is asked, not the server's
/// pending state: each step below reads the configure the client last
/// received. Maximised and fullscreen are out of the tile and say so;
/// leaving either goes back into it and says so; and floated by a layout
/// at the very size it had, the window is still told, because the state is
/// the whole of the change.
#[test]
fn a_tiled_window_is_told_it_is_tiled() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let _output = one_screen(&mut state);
    let (window, toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (_pane, tile) = tiled_alone(&mut state, &window);
    macro_rules! pumped {
        () => {
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            )
        };
    }
    pumped!();
    assert_eq!(
        tiled_sent(&client, &toplevel).last(),
        Some(&true),
        "the layout tiled it and the client was not told"
    );

    state.toggle_maximize(&window);
    pumped!();
    assert_eq!(
        tiled_sent(&client, &toplevel).last(),
        Some(&false),
        "a maximised window is out of its tile"
    );
    state.toggle_maximize(&window);
    pumped!();
    assert_eq!(
        tiled_sent(&client, &toplevel).last(),
        Some(&true),
        "and back in it once restored"
    );

    toplevel.set_fullscreen(None);
    pumped!();
    assert_eq!(
        tiled_sent(&client, &toplevel).last(),
        Some(&false),
        "a fullscreen window is out of its tile"
    );
    toplevel.unset_fullscreen();
    pumped!();
    assert_eq!(
        tiled_sent(&client, &toplevel).last(),
        Some(&true),
        "and back in it once it leaves fullscreen"
    );

    let sent = tiled_sent(&client, &toplevel).len();
    state.place(
        state.window_id(&window),
        Rect {
            x: f64::from(tile.loc.x),
            y: f64::from(tile.loc.y),
            w: f64::from(tile.size.w),
            h: f64::from(tile.size.h),
        },
        AnimationSpec {
            duration: Duration::ZERO,
            ..AnimationSpec::default()
        },
        Duration::ZERO,
        Standing::Free,
    );
    pumped!();
    let after = tiled_sent(&client, &toplevel);
    assert!(
        after.len() > sent,
        "floated at the size it had, the client heard nothing"
    );
    assert_eq!(after.last(), Some(&false), "a floated window is not tiled");
}

/// **Fullscreen is not tiled either**, and leaving it goes back into
/// the tile.
///
/// The same field and the same stage behaviour as the maximise above:
/// `fullscreen_request` kept a way back and moved the window, and left
/// `Pane::placed` saying it was still in its tile -- which with the cap
/// is a video cut down to the corner of the monitor it was tiled in.
#[test]
fn a_fullscreen_window_is_not_held_in_its_old_tile() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let (window, toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (pane, tile) = tiled_alone(&mut state, &window);

    toplevel.set_fullscreen(None);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((screen.size.w, screen.size.h)),
        "the client was asked for the whole monitor"
    );
    commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);

    assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
    assert_eq!(
        state.pane_outer_of(pane),
        Some(screen),
        "a fullscreen window covers its monitor, not the tile it left"
    );

    toplevel.unset_fullscreen();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        state.panes.get(pane).and_then(Pane::placed),
        Some(tile),
        "and leaving fullscreen puts it back in that tile"
    );
}

/// **A layout's sweep leaves a fullscreen or maximised window where it
/// is, and the way back is the tile it gave last.** #150's second
/// review.
///
/// The shipped tiling, two windows side by side. `super+f` on the left
/// one, then `super+shift+right`, which trades it with its neighbour and
/// lays the tree out again: that sweep put the fullscreen window into
/// its new tile, resized there and still fullscreen with no frame, and
/// raised the neighbour over it. Now it still covers the monitor, on
/// top, at the size it was told, and `super+f` puts it in the tile on
/// the right. The same for maximised, with `super+shift+m` and a move
/// back left.
#[test]
fn a_layout_leaves_a_fullscreen_or_maximised_window_where_it_is() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    tiled_fixture!(display, state, conn, queue, client, qh);
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let (first, first_toplevel, first_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (second, second_toplevel, second_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();

    let directory = std::env::temp_dir().join(format!("solium-kept-over-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            "package.path = {shipped:?} .. \"/?.lua\"\n\
             require(\"modes\")\n\
             require(\"workspaces\")\n\
             require(\"tiling\")\n\
             require(\"direction\")\n",
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    )
    .expect("writing the entry point");
    state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the shipped layouts"),
    ));
    macro_rules! round_trip {
        () => {
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
        };
    }
    // Every animation landed, the windows' own fade in among them: a
    // window drawn translucent is kept over a layout's placements
    // already, which would hide the stacking asserted below.
    macro_rules! rest {
        () => {
            state.clock.advance(Duration::from_secs(1));
            a_frame(&mut state);
        };
    }
    macro_rules! press {
        ($combo:expr) => {
            assert!(state.trigger($combo), "{} is bound", $combo);
            round_trip!();
        };
    }
    let tile_of = |state: &Solium, window: &Window| {
        state
            .panes
            .of(window)
            .and_then(Pane::placed)
            .expect("the layout has this window in a tile")
    };
    press!("super+t");
    for (surface, window) in [(&first_surface, &first), (&second_surface, &second)] {
        let tile = tile_of(&state, window);
        commit_buffer(&client, &qh, surface, tile.size.w, tile.size.h);
    }
    round_trip!();
    rest!();
    let ((window, toplevel, surface), other) =
        if tile_of(&state, &first).loc.x < tile_of(&state, &second).loc.x {
            ((&first, &first_toplevel, &first_surface), &second)
        } else {
            ((&second, &second_toplevel, &second_surface), &first)
        };
    let (left, right) = (tile_of(&state, window), tile_of(&state, other));
    assert!(
        left.loc.x + left.size.w < right.loc.x,
        "the premise: two tiles side by side: {left:?} and {right:?}"
    );
    let pane = state.panes.id_of(window).expect("the window has a pane");
    state.focus_window(
        window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );

    // Over the layout while it is fullscreen or maximised, and at the
    // size it was told for that; and the tile it goes back to is `tile`.
    let covers = |state: &Solium, client: &Client, wanted: State, tile| {
        assert!(in_state(window, wanted), "{wanted:?}: no longer in it");
        assert_eq!(
            state.space.element_location(window),
            Some(screen.loc),
            "{wanted:?}: the sweep moved the window"
        );
        assert_eq!(
            last_configured(client, toplevel),
            Some((screen.size.w, screen.size.h)),
            "{wanted:?}: the sweep resized the window to a tile"
        );
        assert_eq!(
            state
                .space
                .elements()
                .last()
                .map(|top| state.window_id(top)),
            Some(state.window_id(window)),
            "{wanted:?}: the sweep stacked a neighbour over the window"
        );
        assert_eq!(
            state
                .panes
                .get(pane)
                .map(|held| (held.placed(), held.left_tile())),
            Some((None, Some(tile))),
            "{wanted:?}: the window is not out of its tile with the newest one kept"
        );
    };

    press!("super+f");
    commit_buffer(&client, &qh, surface, screen.size.w, screen.size.h);
    round_trip!();
    rest!();
    press!("super+shift+right");
    assert_eq!(
        tile_of(&state, other),
        left,
        "the premise: the move traded the two, and the layout placed the neighbour"
    );
    covers(&state, &client, State::Fullscreen, right);

    press!("super+f");
    assert!(!in_state(window, State::Fullscreen));
    assert_eq!(
        state.panes.get(pane).and_then(Pane::placed),
        Some(right),
        "leaving fullscreen did not go into the newest tile"
    );
    assert_eq!(state.space.element_location(window), Some(right.loc));
    assert_eq!(
        last_configured(&client, toplevel),
        Some((right.size.w, right.size.h))
    );
    commit_buffer(&client, &qh, surface, right.size.w, right.size.h);
    round_trip!();
    rest!();

    press!("super+shift+m");
    commit_buffer(&client, &qh, surface, screen.size.w, screen.size.h);
    round_trip!();
    rest!();
    press!("super+shift+left");
    assert_eq!(
        tile_of(&state, other),
        right,
        "the premise: the move traded the two back"
    );
    covers(&state, &client, State::Maximized, left);

    press!("super+shift+m");
    assert!(!in_state(window, State::Maximized));
    assert_eq!(
        state.panes.get(pane).and_then(Pane::placed),
        Some(left),
        "leaving maximised did not go into the newest tile"
    );
    assert_eq!(
        state.space.element_location(window),
        Some(left.loc),
        "leaving maximised went back to the tile it left, not the one it has"
    );
    assert_eq!(
        last_configured(&client, toplevel),
        Some((left.size.w, left.size.h))
    );
}

/// The shipped modes and tiling, loaded into a fixture of two real
/// windows, for the two #443 tests below.
/// [`shipped_modes_and_tiling`], and `lua/floating.lua`.
fn shipped_modes_tiling_and_floating(state: &mut Solium, name: &str) {
    shipped_with(state, name, "require(\"floating\")\n");
}

fn shipped_modes_and_tiling(state: &mut Solium, name: &str) {
    shipped_with(state, name, "");
}

fn shipped_with(state: &mut Solium, name: &str, more: &str) {
    let directory = std::env::temp_dir().join(format!("solium-{name}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            "package.path = {shipped:?} .. \"/?.lua\"\n\
             require(\"modes\")\n\
             require(\"workspaces\")\n\
             require(\"tiling\")\n\
             require(\"direction\")\n{more}",
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    )
    .expect("writing the entry point");
    state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the shipped layouts"),
    ));
}

/// **#507: a window launched onto the floating desktop grows into the
/// place floating gives it**, from `open.scale` of that place about its
/// centre, and is never drawn larger than it.
///
/// A launch opens its window in a slot of the compositor's, the whole
/// work area less a margin, and in the same `open` the shipped
/// `open.lua` asked for its grow-in from 0.88 of the window's rectangle
/// as `sol.windows()` had it (that slot), before `floating.lua` placed
/// it smaller. The first frame was the window at 0.88 of the work area,
/// and it shrank into its place: a window "appearing and then changing
/// its size by 20-30%", as the maintainer saw it.
#[test]
fn a_launched_window_grows_into_its_place_not_out_of_its_slot() {
    let display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state
        .decorations
        .set_style(&mut state.panes, Some("none".to_string()));
    one_screen(&mut state);
    shipped_with(
        &mut state,
        "launched-window-grows-into-its-place",
        "require(\"open\")\nrequire(\"floating\")\n",
    );

    let pane = state.open_loading("app", None, crate::pane::loading_source(None), None);
    let now = state.clock.now();
    let held = state.panes.get(pane).expect("the launched window is here");
    // Where floating put it, as `sol.windows()` says it.
    let placed = state.asked_outer(held);
    let screen = state.work_area().expect("the monitor has a work area");
    assert!(
        placed.size.w < screen.size.w * 3 / 4,
        "the premise: floating placed it smaller than the slot: {placed:?} on {screen:?}"
    );

    let first = state.drawn_at(held, state.pane_outer(held), now);
    let (w, h) = (first.rect.size.w, first.rect.size.h);
    assert!(
        (w - f64::from(placed.size.w) * 0.88).abs() < 1.0
            && (h - f64::from(placed.size.h) * 0.88).abs() < 1.0,
        "its first frame is 0.88 of where it was placed ({placed:?}), not of the slot: {:?}",
        first.rect
    );
    let centre = |x: f64, w: f64| x + w / 2.0;
    assert!(
        (centre(first.rect.loc.x, w) - centre(f64::from(placed.loc.x), f64::from(placed.size.w)))
            .abs()
            < 1.0
            && (centre(first.rect.loc.y, h)
                - centre(f64::from(placed.loc.y), f64::from(placed.size.h)))
            .abs()
                < 1.0,
        "and about its centre: {:?} in {placed:?}",
        first.rect
    );
}

/// **#443: a window maximised while the desktop floated joins its tile
/// when a layout takes over.**
///
/// Illia maximised Claude Desktop on the floating desktop and later
/// switched to tiling. The layout gave it a tile and laid its neighbour
/// out beside that tile, while the compositor went on holding the window
/// maximised over the whole work area, on top of the neighbour: an empty
/// tile, a window drawn full screen, and no seam that could size it.
/// Now the switch lets it out of the maximise first, so it is in its
/// tile, at the tile's size, and told so.
#[test]
fn a_window_maximised_while_floating_joins_its_tile_when_a_layout_takes_over() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    tiled_fixture!(display, state, conn, queue, client, qh);
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let (window, toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (other, _other_toplevel, other_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    shipped_modes_and_tiling(&mut state, "maximised-joins-its-tile");
    macro_rules! round_trip {
        () => {
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
        };
    }
    macro_rules! rest {
        () => {
            state.clock.advance(Duration::from_secs(1));
            a_frame(&mut state);
        };
    }
    macro_rules! press {
        ($combo:expr) => {
            assert!(state.trigger($combo), "{} is bound", $combo);
            round_trip!();
        };
    }

    state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    press!("super+shift+m");
    commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
    round_trip!();
    rest!();
    assert!(
        in_state(&window, State::Maximized),
        "the premise: the floating window is maximised"
    );

    press!("super+t");
    let tile_of = |state: &Solium, window: &Window| {
        state
            .panes
            .of(window)
            .and_then(Pane::placed)
            .expect("the layout has this window in a tile")
    };
    let (tile, beside) = (tile_of(&state, &window), tile_of(&state, &other));
    assert!(
        tile.intersection(beside).is_none(),
        "the premise: two tiles side by side: {tile:?} and {beside:?}"
    );
    assert!(
        !in_state(&window, State::Maximized),
        "the window is still maximised under the layout"
    );
    assert_eq!(
        last_configured(&client, &toplevel),
        Some((tile.size.w, tile.size.h)),
        "the window was not told its tile"
    );
    commit_buffer(&client, &qh, &surface, tile.size.w, tile.size.h);
    commit_buffer(&client, &qh, &other_surface, beside.size.w, beside.size.h);
    round_trip!();
    rest!();
    assert_eq!(
        state.space.element_location(&window),
        Some(tile.loc),
        "the window is not where its tile is"
    );
    let pane = state.panes.id_of(&window).expect("the window has a pane");
    assert_eq!(
        state.pane_outer_of(pane),
        Some(tile),
        "the window is drawn at a size other than its tile's"
    );
}

/// **#443: a lock and an unlock leave every window as it was.**
///
/// Two windows in the shipped tiling, locked and unlocked by Solium's
/// own lock: each is in the tile it had, at the place and the size it
/// had, told nothing new, and neither maximised nor fullscreen. What
/// #443 reported after an unlock was a window maximised half an hour
/// before it, which the test above is about; this one holds the lock
/// itself to having changed nothing.
#[test]
fn a_lock_and_an_unlock_leave_every_tiled_window_as_it_was() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    tiled_fixture!(display, state, conn, queue, client, qh);
    let _output = one_screen(&mut state);
    let (first, first_toplevel, first_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (second, second_toplevel, second_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    shipped_modes_and_tiling(&mut state, "lock-leaves-tiles");
    macro_rules! round_trip {
        () => {
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
        };
    }
    macro_rules! rest {
        () => {
            state.clock.advance(Duration::from_secs(1));
            a_frame(&mut state);
        };
    }
    assert!(state.trigger("super+t"), "super+t is bound");
    round_trip!();
    let windows = [
        (&first, &first_toplevel, &first_surface),
        (&second, &second_toplevel, &second_surface),
    ];
    for (window, _, surface) in windows {
        let tile = state
            .panes
            .of(window)
            .and_then(Pane::placed)
            .expect("the layout has this window in a tile");
        commit_buffer(&client, &qh, surface, tile.size.w, tile.size.h);
    }
    round_trip!();
    rest!();
    state.focus_window(
        &first,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    round_trip!();
    let seen = |state: &Solium, client: &Client| {
        windows.map(|(window, toplevel, _)| {
            (
                state.panes.of(window).and_then(Pane::placed),
                state.space.element_location(window),
                sizes_sent(client, toplevel).len(),
                last_configured(client, toplevel),
                in_state(window, State::Maximized),
                in_state(window, State::Fullscreen),
            )
        })
    };
    let before = seen(&state, &client);

    state.lock_natively();
    round_trip!();
    for _ in 0..5 {
        rest!();
        round_trip!();
    }
    state.unlock_natively();
    round_trip!();
    for _ in 0..5 {
        rest!();
        round_trip!();
    }

    assert_eq!(
        seen(&state, &client),
        before,
        "a window's tile, place, configures or states changed across a lock"
    );
}

mod client_maximize;

/// **#49: a tiled window leaving fullscreen or maximised stays in front
/// of its neighbour while it shrinks into its tile**, whichever of the
/// two the layout's sweep places last, and one leaving fullscreen stays
/// over the bars too. The left tile's window is placed first, and the
/// neighbour placed after it was stacked over it: it shrank from behind
/// the neighbour and, no longer the front window, was not lifted. A
/// sweep part of the way through -- another window opening, a reload --
/// leaves it in front as well.
#[test]
fn a_tiled_window_leaving_fullscreen_stays_in_front_while_it_shrinks() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let output = one_screen(&mut state);
    let screen = state.space.output_geometry(&output).expect("mapped");
    let (first, first_toplevel, first_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (second, second_toplevel, second_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let directory =
        std::env::temp_dir().join(format!("solium-shrinks-in-front-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            "package.path = {shipped:?} .. \"/?.lua\"\n\
             require(\"modes\")\n\
             require(\"workspaces\")\n\
             require(\"tiling\")\n\
             require(\"fullscreen\")\n\
             require(\"direction\")\n",
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    )
    .expect("writing the entry point");
    state.start_scripts(Some(Scripts::load(&entry).expect("loading")));
    macro_rules! round_trip {
        () => {
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
        };
    }
    let tile_of = |state: &Solium, window: &Window| {
        state
            .panes
            .of(window)
            .and_then(Pane::placed)
            .expect("tiled")
    };
    assert!(state.trigger("super+t"));
    round_trip!();
    for (surface, window) in [(&first_surface, &first), (&second_surface, &second)] {
        let tile = tile_of(&state, window);
        commit_buffer(&client, &qh, surface, tile.size.w, tile.size.h);
    }
    round_trip!();
    state.clock.advance(Duration::from_secs(1));
    a_frame(&mut state);
    for (key, lifts) in [("super+f", true), ("super+shift+m", false)] {
        for (window, toplevel, surface, name) in [
            (&first, &first_toplevel, &first_surface, "first"),
            (&second, &second_toplevel, &second_surface, "second"),
        ] {
            let tile = tile_of(&state, window);
            let pane = state.panes.id_of(window).expect("pane");
            state.focus_window(
                window,
                SERIAL_COUNTER.next_serial(),
                crate::script::FocusSource::Script,
            );
            assert!(state.trigger(key));
            round_trip!();
            let (w, h) = last_configured(&client, toplevel).expect("told a size");
            commit_buffer(&client, &qh, surface, w, h);
            round_trip!();
            state.clock.advance(Duration::from_secs(1));
            a_frame(&mut state);
            assert!(state.trigger(key));
            round_trip!();
            let top = |state: &Solium| {
                state
                    .space
                    .elements()
                    .last()
                    .map(|top| state.window_id(top))
            };
            state.clock.advance(Duration::from_millis(50));
            state.sync_panes();
            assert_eq!(
                (top(&state), state.lifted_on(screen)),
                (Some(state.window_id(window)), lifts.then_some(pane)),
                "{key}, {name}: in front of its neighbour while it shrinks, \
                 and over the bars if it was fullscreen"
            );
            state.trigger_relayout();
            state.clock.advance(Duration::from_millis(50));
            state.sync_panes();
            assert_eq!(
                (top(&state), state.lifted_on(screen)),
                (Some(state.window_id(window)), lifts.then_some(pane)),
                "{key}, {name}: and still, after a sweep part of the way through"
            );
            commit_buffer(&client, &qh, surface, tile.size.w, tile.size.h);
            round_trip!();
            state.clock.advance(Duration::from_secs(1));
            a_frame(&mut state);
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// The shipped `modes`, `workspaces`, `tiling`, `scrolling` and
/// `direction`, with the shipped configuration, handed to the
/// compositor: every key the tests below press is the one a user
/// presses.
fn start_the_shipped_layouts(state: &mut Solium, tag: &str) {
    let directory = std::env::temp_dir().join(format!("solium-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!(
            "package.path = {shipped:?} .. \"/?.lua\"\n\
             require(\"modes\")\n\
             require(\"workspaces\")\n\
             require(\"tiling\")\n\
             require(\"scrolling\")\n\
             require(\"direction\")\n",
            shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua"),
        ),
    )
    .expect("writing the entry point");
    state.start_scripts(Some(
        Scripts::load(&entry).expect("loading the shipped layouts"),
    ));
}

/// The pointer, moved to `at` as a device moves it: which monitor a key
/// such as `super+2` is about.
fn pointer_to(state: &mut Solium, at: (f64, f64)) {
    let pointer = state
        .seat
        .get_pointer()
        .expect("the fixture's seat has a pointer");
    pointer.motion(
        state,
        None,
        &smithay::input::pointer::MotionEvent {
            location: at.into(),
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    pointer.frame(state);
}

/// **A fullscreen or maximised window moved to another monitor is
/// fullscreen or maximised on that one**, drawn there and with the
/// keyboard. #150's third review.
///
/// Two monitors side by side, workspaces per monitor as shipped, and the
/// right one switched to workspace 2 by its key, so the two show
/// different numbers. `super+f` on the one window, on the left, then
/// `super+shift+right`: the layout moved it onto workspace 2 and into
/// the right monitor's tree, strip or screen, and the window stayed
/// covering the left monitor -- so after the next relayout it was on the
/// left monitor's workspace 2, which nobody is looking at, with the
/// keyboard still on it. Now it covers the right monitor, and `super+f`
/// puts it on the right monitor too: into its tile there, or with no
/// layout where it was on the left one. The same the other way for
/// maximised, with `super+shift+m` and `super+shift+left`, onto the left
/// monitor's workspace 1. In tiling, in scrolling and with no layout.
#[test]
fn a_fullscreen_or_maximised_window_moved_to_another_monitor_is_so_on_that_one() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    for (mode, key) in [
        ("tiling", Some("super+t")),
        ("scrolling", Some("super+s")),
        ("no layout", None),
    ] {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (left_output, right_output) = side_by_side(&mut state, "carried-test");
        let left_screen = state
            .space
            .output_geometry(&left_output)
            .expect("the left monitor is mapped");
        let right_screen = state
            .space
            .output_geometry(&right_output)
            .expect("the right monitor is mapped");
        start_the_shipped_layouts(&mut state, "carried");
        macro_rules! round_trip {
            () => {
                pump(
                    &mut display,
                    &mut state,
                    &conn,
                    &qh,
                    &mut queue,
                    &mut client,
                );
            };
        }
        macro_rules! rest {
            () => {
                state.clock.advance(Duration::from_secs(1));
                a_frame(&mut state);
            };
        }
        macro_rules! press {
            ($combo:expr) => {
                assert!(state.trigger($combo), "{mode}: {} is bound", $combo);
                round_trip!();
            };
        }
        let says = |state: &Solium, chunk: &str| {
            state
                .scripts
                .as_ref()
                .map(|scripts| scripts.evaluate(chunk))
                .unwrap_or_default()
        };

        // Opened with the layouts running, so it belongs to the
        // workspace its monitor shows, as every window a user opens does.
        pointer_to(&mut state, (500.0, 500.0));
        if let Some(key) = key {
            press!(key);
        }
        let (window, toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        round_trip!();
        let pane = state.panes.id_of(&window).expect("the window has a pane");
        if let Some(tile) = state.panes.get(pane).and_then(Pane::placed) {
            commit_buffer(&client, &qh, &surface, tile.size.w, tile.size.h);
            round_trip!();
        }
        rest!();
        let workspace = format!(
            "return tostring(require(\"workspaces\").of[{}])",
            pane.get()
        );
        pointer_to(&mut state, (2500.0, 500.0));
        press!("super+2");
        pointer_to(&mut state, (500.0, 500.0));
        state.focus_window(
            &window,
            SERIAL_COUNTER.next_serial(),
            crate::script::FocusSource::Script,
        );
        rest!();
        let before = state.real_geometry(&window).expect("the window is mapped");
        assert_eq!(
            (
                left_screen.contains_rect(before),
                says(
                    &state,
                    "local workspaces = require(\"workspaces\")\n\
                     return workspaces.on(\"carried-test\") .. \",\" .. \
                     workspaces.on(\"right-test\")"
                ),
                says(&state, &workspace),
            ),
            (true, "1,2".to_owned(), "1".to_owned()),
            "{mode}: the premise: the window is on the left monitor's workspace 1, \
             and the right monitor shows workspace 2"
        );

        // Covering `screen` in `wanted`, at the size it was told for
        // that, drawn, on workspace `on`, and with the keyboard.
        let covers = |state: &Solium, client: &Client, wanted: State, screen, on: &str| {
            let screen: Rectangle<i32, Logical> = screen;
            assert!(
                in_state(&window, wanted),
                "{mode}, {wanted:?}: no longer in it"
            );
            assert_eq!(
                (
                    state.space.element_location(&window),
                    last_configured(client, &toplevel),
                ),
                (Some(screen.loc), Some((screen.size.w, screen.size.h))),
                "{mode}, {wanted:?}: the window is not on the monitor it was moved to"
            );
            assert_eq!(
                (
                    says(state, &workspace),
                    state.pane_on_stage(pane),
                    state.focused_window().as_ref() == Some(&window),
                ),
                (on.to_owned(), true, true),
                "{mode}, {wanted:?}: (its workspace, drawn, with the keyboard)"
            );
        };
        // Out of it and on `screen`: in the tile the layout has for it
        // there, or, with no layout, where it was on `from` before it
        // went in. At that rect, told its size, drawn, and with the
        // keyboard. Returns the rect.
        let left_onto = |state: &Solium, client: &Client, left: State, screen, from, was| {
            let (screen, from, was): (
                Rectangle<i32, Logical>,
                Rectangle<i32, Logical>,
                Rectangle<i32, Logical>,
            ) = (screen, from, was);
            assert!(!in_state(&window, left), "{mode}, {left:?}: still in it");
            let back = state
                .panes
                .get(pane)
                .and_then(Pane::placed)
                .unwrap_or_else(|| Rectangle::new(was.loc + (screen.loc - from.loc), was.size));
            assert!(
                screen.contains_rect(back),
                "{mode}, {left:?}: the window went back to {back:?}, off {screen:?}"
            );
            assert_eq!(
                (
                    state.space.element_location(&window),
                    last_configured(client, &toplevel),
                    state.pane_on_stage(pane),
                    state.focused_window().as_ref() == Some(&window),
                ),
                (Some(back.loc), Some((back.size.w, back.size.h)), true, true),
                "{mode}, {left:?}: (where the window is, its size, drawn, with the keyboard)"
            );
            back
        };

        press!("super+f");
        commit_buffer(
            &client,
            &qh,
            &surface,
            left_screen.size.w,
            left_screen.size.h,
        );
        round_trip!();
        rest!();
        press!("super+shift+right");
        state.trigger_relayout();
        round_trip!();
        rest!();
        covers(&state, &client, State::Fullscreen, right_screen, "2");

        press!("super+f");
        let back = left_onto(
            &state,
            &client,
            State::Fullscreen,
            right_screen,
            left_screen,
            before,
        );
        commit_buffer(&client, &qh, &surface, back.size.w, back.size.h);
        round_trip!();
        rest!();

        press!("super+shift+m");
        commit_buffer(
            &client,
            &qh,
            &surface,
            right_screen.size.w,
            right_screen.size.h,
        );
        round_trip!();
        rest!();
        press!("super+shift+left");
        state.trigger_relayout();
        round_trip!();
        rest!();
        covers(&state, &client, State::Maximized, left_screen, "1");

        press!("super+shift+m");
        let _ = left_onto(
            &state,
            &client,
            State::Maximized,
            left_screen,
            right_screen,
            back,
        );
    }
}

/// **Only a move by key carries a fullscreen window to another monitor.**
/// #150's third review.
///
/// Scrolling on the left of two monitors side by side, four columns at
/// the shipped third of a screen, and the last one sent fullscreen.
/// `super+left` three times goes to the first column, and the strip
/// scrolls so that the fullscreen window's column is placed over the
/// right monitor -- a strip's far columns are where the strip goes on,
/// whatever monitor that is. That is not a move to another monitor, and
/// the window still covers its own.
#[test]
fn a_fullscreen_window_whose_column_scrolls_over_the_next_monitor_stays_on_its_own() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    tiled_fixture!(display, state, conn, queue, client, qh);
    let (left_output, right_output) = side_by_side(&mut state, "strip-test");
    let left_screen = state
        .space
        .output_geometry(&left_output)
        .expect("the left monitor is mapped");
    start_the_shipped_layouts(&mut state, "strip");
    macro_rules! round_trip {
        () => {
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
        };
    }
    macro_rules! rest {
        () => {
            state.clock.advance(Duration::from_secs(1));
            a_frame(&mut state);
        };
    }
    macro_rules! press {
        ($combo:expr) => {
            assert!(state.trigger($combo), "{} is bound", $combo);
            round_trip!();
        };
    }
    pointer_to(&mut state, (500.0, 500.0));
    press!("super+s");
    let mut opened = Vec::new();
    for _ in 0..4 {
        let (window, toplevel, surface) =
            open_surface(&mut display, &mut state, &conn, &client, &qh);
        state.sync_panes();
        round_trip!();
        let tile = state
            .panes
            .of(&window)
            .and_then(Pane::placed)
            .expect("the strip has the window in a column");
        commit_buffer(&client, &qh, &surface, tile.size.w, tile.size.h);
        round_trip!();
        opened.push((window, toplevel, surface));
    }
    rest!();
    let Some((window, toplevel, surface)) = opened.pop() else {
        return;
    };
    let pane = state.panes.id_of(&window).expect("the window has a pane");
    assert_eq!(
        state.focused_window().as_ref(),
        Some(&window),
        "the premise: the last window opened has the keyboard"
    );

    press!("super+f");
    commit_buffer(
        &client,
        &qh,
        &surface,
        left_screen.size.w,
        left_screen.size.h,
    );
    round_trip!();
    rest!();
    for _ in 0..3 {
        press!("super+left");
        rest!();
    }
    let column = state
        .panes
        .get(pane)
        .and_then(Pane::left_tile)
        .expect("the strip gave the fullscreen window a column");
    assert_eq!(
        state.output_of(column),
        Some(right_output),
        "the premise: the strip placed the fullscreen window's column, {column:?}, \
         over the right monitor"
    );
    assert!(in_state(&window, State::Fullscreen), "no longer fullscreen");
    assert_eq!(
        (
            state.space.element_location(&window),
            last_configured(&client, &toplevel),
        ),
        (
            Some(left_screen.loc),
            Some((left_screen.size.w, left_screen.size.h))
        ),
        "the fullscreen window went to the monitor its column was scrolled over"
    );
}

/// **A floated window made fullscreen stays fullscreen through a
/// layout's sweep, and leaving fullscreen puts it back where it
/// floated.** #150's third review.
///
/// One monitor with a 30-pixel bar, tiling, two windows.
/// `super+shift+space` floats one and `super+f` sends it fullscreen.
/// `dialogs.lua` places every floated window on every sweep, centred on
/// its screen at the size it has and kept inside the work area, so the
/// next sweep put the fullscreen window under the bar and told it the
/// work area's size, still fullscreen. Now the sweep leaves it covering
/// the monitor, and `super+f` puts it back where it floated, at the size
/// it floated at, which a sweep asked for on leaving would place again
/// from the size the client has not changed yet.
///
/// And a tiled window floated while it is fullscreen floats when it
/// leaves fullscreen, where a floated window of its size goes, and in no
/// tile.
#[test]
fn a_floated_window_made_fullscreen_is_left_so_by_a_sweep_and_goes_back_where_it_floated() {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;

    tiled_fixture!(display, state, conn, queue, client, qh);
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let _bar = bar(&mut display, &mut state, &conn, &client, &qh, 30);
    assert_eq!(
        state.work_area_on(&output),
        Some(Rectangle::new((0, 30).into(), (1920, 1050).into())),
        "the premise: the bar holds the top of the screen"
    );
    let (window, toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (other, _other_toplevel, other_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let pane = state.panes.id_of(&window).expect("the window has a pane");
    start_the_shipped_layouts(&mut state, "floated-fullscreen");
    macro_rules! round_trip {
        () => {
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
        };
    }
    macro_rules! rest {
        () => {
            state.clock.advance(Duration::from_secs(1));
            a_frame(&mut state);
        };
    }
    macro_rules! press {
        ($combo:expr) => {
            assert!(state.trigger($combo), "{} is bound", $combo);
            round_trip!();
        };
    }
    let tile_of = |state: &Solium, window: &Window| {
        state
            .panes
            .of(window)
            .and_then(Pane::placed)
            .expect("the layout has this window in a tile")
    };
    // The window where it is, at the size it was last told.
    let at = |state: &Solium, client: &Client| {
        (
            state.space.element_location(&window),
            last_configured(client, &toplevel),
        )
    };

    press!("super+t");
    for (surface, window) in [(&surface, &window), (&other_surface, &other)] {
        let tile = tile_of(&state, window);
        commit_buffer(&client, &qh, surface, tile.size.w, tile.size.h);
    }
    round_trip!();
    rest!();
    state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    press!("super+shift+space");
    rest!();
    let floated = state.real_geometry(&window).expect("the window is mapped");
    assert_eq!(
        (
            state.panes.get(pane).and_then(Pane::placed),
            at(&state, &client)
        ),
        (
            None,
            (Some(floated.loc), Some((floated.size.w, floated.size.h)))
        ),
        "the premise: the window floats, out of its tile"
    );

    press!("super+f");
    commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
    round_trip!();
    rest!();
    state.trigger_relayout();
    round_trip!();
    rest!();
    assert!(in_state(&window, State::Fullscreen), "no longer fullscreen");
    assert_eq!(
        at(&state, &client),
        (Some(screen.loc), Some((screen.size.w, screen.size.h))),
        "the sweep placed the fullscreen window as a floating one"
    );

    press!("super+f");
    assert!(!in_state(&window, State::Fullscreen), "still fullscreen");
    assert_eq!(
        at(&state, &client),
        (Some(floated.loc), Some((floated.size.w, floated.size.h))),
        "leaving fullscreen did not go back to where the window floated"
    );
    commit_buffer(&client, &qh, &surface, floated.size.w, floated.size.h);
    round_trip!();
    state.trigger_relayout();
    round_trip!();
    rest!();
    assert_eq!(
        at(&state, &client),
        (Some(floated.loc), Some((floated.size.w, floated.size.h))),
        "the next sweep moved the window from where it floated"
    );

    // Tiled again, fullscreen, and floated while it is.
    press!("super+shift+space");
    let tile = tile_of(&state, &window);
    commit_buffer(&client, &qh, &surface, tile.size.w, tile.size.h);
    round_trip!();
    rest!();
    press!("super+f");
    commit_buffer(&client, &qh, &surface, screen.size.w, screen.size.h);
    round_trip!();
    rest!();
    press!("super+shift+space");
    rest!();
    assert!(in_state(&window, State::Fullscreen), "no longer fullscreen");
    assert_eq!(
        at(&state, &client),
        (Some(screen.loc), Some((screen.size.w, screen.size.h))),
        "floating the fullscreen window placed it as a floating one"
    );
    press!("super+f");
    assert_eq!(
        (tile.size, tile.loc == floated.loc),
        (floated.size, false),
        "the premise: the window's tile is the size it floated at, somewhere else"
    );
    assert_eq!(
        state
            .panes
            .get(pane)
            .map(|held| (held.placed(), held.left_tile())),
        Some((None, None)),
        "a window floated while fullscreen went back into the tile it had"
    );
    assert_eq!(
        at(&state, &client),
        (Some(floated.loc), Some((floated.size.w, floated.size.h))),
        "a window floated while fullscreen did not float where one of its size goes"
    );
}

/// **A changed `sol.status` with no commands still asks for a frame.**
///
/// The shipped `modes.lua`, `overview.lua` and `workspaces.lua` always
/// send a command alongside a status change, which already sets
/// `redraw`, so this went unnoticed. But `sol.status` is now documented
/// as how a configuration tells a hosted shell its mode, and a binding
/// that only calls it would otherwise leave `Solium.status` stale until
/// some unrelated frame.
#[test]
fn a_status_change_with_no_commands_still_asks_for_a_frame() {
    let display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    state.redraw = false;
    state.apply(Outcome {
        status: Some("mode".to_owned()),
        ..Outcome::default()
    });
    assert!(
        state.redraw,
        "a status change with no commands left redraw unset"
    );
}

/// **A window a layout lets go of is not held in the tile it had.**
///
/// `sol.unplace` is what `modes.use` sends for every window when the
/// layout in charge changes: the mode is the script's, so this is the
/// only way the compositor hears that a window is floating now. Without
/// it, a window left in its tile by a switch to floating is cut back to
/// that tile the moment its client commits more -- which a floating
/// window does whenever it is resized or grows itself.
#[test]
fn a_window_let_go_by_its_layout_is_not_held_in_its_old_tile() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (pane, tile) = tiled_alone(&mut state, &window);

    state.apply(Outcome {
        commands: vec![Command::Unplace {
            id: state.window_id(&window),
        }],
        ..Outcome::default()
    });
    let grown = (tile.size.w + 200, tile.size.h + 50);
    commit_buffer(&client, &qh, &surface, grown.0, grown.1);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);

    assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
    assert_eq!(
        state.pane_outer_of(pane).map(|outer| outer.size),
        Some(Size::from(grown)),
        "a floating window is the size its client committed"
    );
}

/// **A dialog a layout centres is placed and not tiled**, through
/// `sol.place` with `tile = false`.
///
/// A dialog is floating whichever layout is running -- `dialogs.lua`
/// centres it over its parent at the size it had -- and one that grows
/// after it was centred, as a file chooser settling on its size does,
/// must be drawn at the size it grew to. A layout's ordinary placement
/// beside it is still a tile, which the last assertion holds the
/// fixture to.
#[test]
fn a_dialog_a_layout_centres_is_not_held_in_its_rect() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    let id = state.window_id(&window);
    let rect = Rect {
        x: 300.0,
        y: 200.0,
        w: 400.0,
        h: 300.0,
    };
    let instant = AnimationSpec {
        duration: Duration::ZERO,
        ..AnimationSpec::default()
    };
    state.apply(Outcome {
        commands: vec![Command::Place {
            id,
            rect,
            animation: instant,
            tile: false,
            inside: None,
            cramped: false,
            moved: false,
        }],
        ..Outcome::default()
    });
    commit_buffer(&client, &qh, &surface, 520, 380);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);

    assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
    assert_eq!(
        state.pane_outer_of(pane),
        Some(at(300, 200, 520, 380)),
        "placed where the layout said, at the size the client grew to"
    );

    state.apply(Outcome {
        commands: vec![Command::Place {
            id,
            rect,
            animation: instant,
            tile: true,
            inside: None,
            cramped: false,
            moved: false,
        }],
        ..Outcome::default()
    });
    a_frame(&mut state);
    assert_eq!(
        state.pane_outer_of(pane),
        Some(at(300, 200, 400, 300)),
        "the same rect as a tile holds the same client inside it"
    );
}

/// **A pane under a resize hold is bridged, not cut to its tile.**
///
/// While an edge is being dragged the dragged rectangle is the truth
/// and the client's last buffer is stretched or held into it by
/// `resizing::factor` (#113, #123). The tile the drag is moving is
/// `Pane::placed` all the while, so a cut to it would cut the bridge:
/// `Solium::tile_of` answers `None` under a hold, and the committed
/// size is shown whole. Here the client is larger than the rectangle
/// the drag has reached, which is exactly the case a cut would change.
#[test]
fn a_held_pane_is_bridged_and_not_cut_to_its_tile() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    commit_buffer(&client, &qh, &surface, 400, 300);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    state.sync_panes();

    let outer = at(400, 300, 200, 150);
    let request = ResizeRequest {
        window: window.clone(),
        wanted: outer,
        edge_at: (600.0, 450.0),
        edges: ResizeEdge::BottomRight,
    };
    state.begin_resize(&window);
    tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
    assert!(state.holding_resize(pane), "the drag is holding the pane");
    assert_eq!(
        state.panes.get(pane).and_then(Pane::placed),
        Some(outer),
        "and the layout has it in the dragged tile, which a cut would use"
    );

    let held = state.panes.get(pane).expect("the pane is still here");
    assert_eq!(
        state.shown_size(held, window.geometry().size),
        Size::from((400, 300)),
        "the whole of the client's last buffer is shown, to be bridged \
         into the dragged rectangle rather than cut to it"
    );
    assert_eq!(
        state.pane_geometry(held),
        outer,
        "and the pane is the dragged rectangle, as #113 made it"
    );
}

/// **A floating window brought back onto a screen is still floating.**
///
/// `rescue_offscreen` reaches `move_pane`, which is where a layout's
/// tile is written -- so on the way to #133 a window lost off a
/// departed monitor came back *tiled* at the rectangle it was rescued
/// to, and was cut to it from then on. A rescue is not a layout's
/// opinion, and keeps a pane's standing as it found it.
#[test]
fn a_floating_window_brought_back_onto_a_screen_is_not_given_a_tile() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let _output = one_screen(&mut state);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (-5000, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");

    state.rescue_offscreen();
    assert!(
        state
            .pane_outer_of(pane)
            .is_some_and(|outer| state.on_any_output(outer)),
        "the rescue brought the window back, or it proves nothing"
    );
    commit_buffer(&client, &qh, &surface, 700, 500);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);

    assert_eq!(state.panes.get(pane).and_then(Pane::placed), None);
    assert_eq!(
        state.pane_outer_of(pane).map(|outer| outer.size),
        Some(Size::from((700, 500))),
        "and it is the size its client committed"
    );
}

/// `window` placed again, through `Solium::place` as `tiling.apply`
/// places it, at `rect` and gliding there for `duration`.
fn glide(
    state: &mut Solium,
    window: &Window,
    rect: Rectangle<i32, Logical>,
    duration: Duration,
    easing: present::Curve,
    start: Duration,
) {
    let id = state.window_id(window);
    state.place(
        id,
        to_rect(rect),
        AnimationSpec { duration, easing },
        start,
        Standing::Tile,
    );
}

/// A window alone in a tile whose client filled it, which is every
/// tiled window a moment before its neighbour opens.
fn filled_tile(
    display: &mut Display<Solium>,
    state: &mut Solium,
    conn: &Connection,
    qh: &QueueHandle<Client>,
    queue: &mut wayland_client::EventQueue<Client>,
    client: &mut Client,
) -> (
    Window,
    wl_surface::WlSurface,
    crate::pane::PaneId,
    Rectangle<i32, Logical>,
) {
    let (window, _toplevel, surface) = open_surface(display, state, conn, client, qh);
    let (pane, tile) = tiled_alone(state, &window);
    commit_buffer(client, qh, &surface, tile.size.w, tile.size.h);
    pump(display, state, conn, qh, queue, client);
    a_frame(state);
    assert_eq!(
        window.geometry().size,
        tile.size,
        "the client filled its tile, so the frame before the sweep is \
         the whole buffer at 1:1"
    );
    (window, surface, pane, tile)
}

/// **#133 review, finding 1: a glide that narrows a tiled window is
/// drawn 1:1, cut to the rectangle it has reached, from its first
/// frame.**
///
/// The case that made the finding: a window alone in its tile, a
/// second one opening, and the layout halving the first with a 240ms
/// glide while its client still has the whole width committed. The
/// cap takes the pane to its new tile at once, so the glide's first
/// frame is drawn at the old tile while `pane_outer` is the new one --
/// and what was shipped read the pair as the new tile zoomed 2x, so
/// the left half of the buffer was stretched over the whole of the old
/// tile on the frame after one that had drawn all of it 1:1.
///
/// Driven through the real placement and the real transform, and read
/// through `render::place_client`, which is what `elements` draws the
/// surfaces with. Three frames: the first, one a little way in, and one
/// most of the way.
#[test]
fn a_glide_that_narrows_a_tiled_window_is_drawn_1_to_1_on_its_first_frame() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _surface, pane, tile) = filled_tile(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let half = Rectangle::new(tile.loc, (tile.size.w / 2, tile.size.h).into());
    let start = state.clock.now();
    glide(
        &mut state,
        &window,
        half,
        Duration::from_millis(240),
        present::Curve::OutCubic,
        start,
    );
    let outer = state.pane_outer_of(pane).expect("the pane is here");
    assert_eq!(
        outer, half,
        "the pane is held in its new tile at once, which is what the \
         glide's frames are measured against"
    );
    let committed = window.geometry().size;
    assert_eq!(committed, tile.size, "and the client has not answered");

    let placed_at = |state: &Solium, at: Duration| {
        let held = state.panes.get(pane).expect("the pane is here");
        let frame = state.drawn_at(held, outer, start + at);
        (
            frame,
            crate::render::place_client(state, held, &frame, outer.size, committed),
        )
    };

    let (first, drawn) = placed_at(&state, Duration::ZERO);
    assert_eq!(
        first.rect,
        tile.to_f64(),
        "the glide starts at the old tile"
    );
    assert_eq!(
        drawn.fit.factor,
        smithay::utils::Scale::from((1.0, 1.0)),
        "its first frame is the whole buffer at its own size, as the frame \
         before the sweep was -- not the new tile zoomed"
    );
    assert_eq!(drawn.fit.crop, None, "and nothing of it is cut yet");

    for at in [Duration::from_millis(30), Duration::from_millis(200)] {
        let (frame, drawn) = placed_at(&state, at);
        assert!(
            frame.rect.size.w < f64::from(tile.size.w) - 1.0
                && frame.rect.size.w > f64::from(half.size.w) + 1.0,
            "at {at:?} the frame is part of the way, or it proves nothing: {frame:?}"
        );
        assert_eq!(
            drawn.fit.factor,
            smithay::utils::Scale::from((1.0, 1.0)),
            "at {at:?} the buffer is still drawn at its own size"
        );
        assert_eq!(
            drawn.fit.crop,
            Some(drawn.client),
            "at {at:?} it is cut to the rectangle the glide has reached"
        );
        #[expect(clippy::cast_possible_truncation, reason = "a screen-sized rect")]
        let reached = drawn.client.size.w.round() as i32;
        assert_eq!(
            drawn.fit.shown,
            Size::from((reached, committed.h)),
            "at {at:?} a masked client is captured at the same cut, not at \
             the new tile's share"
        );
    }
}

/// **#233: a glide that shrinks a window draws its client's answer at its
/// own size, against the edge the layout pins.**
///
/// A full-width window shrunk to the right half -- its left edge moves,
/// its right edge stays -- whose client answers with the half straight
/// away, which a terminal does within a frame or two of a 240 ms glide.
/// Before #233 the rest of the glide stretched that answer across the
/// rectangle the glide had reached, glyphs up to twice as wide: no hold
/// meant no `resize.fill`, and no fill meant a stretch. Now every pane is
/// fitted by the configured fill, which draws the answer 1:1, and under
/// `anchor` the glide's own two ends say which edge it stands against:
/// the right, so the gap is on the left, where the window is still on
/// its way in. (`crop`, the default, has it at the left throughout.)
#[test]
fn a_glide_that_shrinks_a_window_draws_its_answer_at_its_own_size() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    state.resizing.fill = crate::resizing::Fill::Anchor;
    let (window, surface, pane, tile) = filled_tile(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let right = Rectangle::new(
        (tile.loc.x + tile.size.w / 2, tile.loc.y).into(),
        (tile.size.w / 2, tile.size.h).into(),
    );
    let start = state.clock.now();
    glide(
        &mut state,
        &window,
        right,
        Duration::from_secs(10),
        present::Curve::Linear,
        start,
    );
    commit_buffer(&client, &qh, &surface, right.size.w, right.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let committed = window.geometry().size;
    assert_eq!(
        committed, right.size,
        "the premise: the client has answered"
    );
    assert_eq!(
        state.resize_pins(pane),
        (true, false),
        "the glide moves the left edge and not the right"
    );

    let outer = state.pane_outer_of(pane).expect("the pane is here");
    let held = state.panes.get(pane).expect("the pane is here");
    let frame = state.drawn_at(held, outer, start + Duration::from_secs(5));
    let drawn = crate::render::place_client(&state, held, &frame, outer.size, committed);
    assert!(
        drawn.client.size.w > f64::from(committed.w) + 1.0,
        "half way, the glide is wider than the answer, or this proves nothing: {frame:?}"
    );
    assert_eq!(
        drawn.fit.factor,
        smithay::utils::Scale::from((1.0, 1.0)),
        "the answer at its own size, not stretched across the glide"
    );
    assert_eq!(drawn.fit.crop, None, "and nothing of it to cut");
    assert!(
        (drawn.origin.x + f64::from(committed.w) - (drawn.client.loc.x + drawn.client.size.w))
            .abs()
            < 1e-9,
        "against the right edge, which is standing still: {drawn:?}"
    );
}

/// **#233: a glide that lands before its client answers keeps the
/// picture where it was, against the edge it pinned.**
///
/// The full-width window shrunk to the right half again, by a 240 ms
/// glide, and a client slower than that: the glide lands with the old,
/// full-width picture still the only one. Through the glide it stood,
/// anchored, against the right edge, cut on the left, where the window
/// was moving. (`crop`, the default, keeps it at the left throughout, so
/// it has nothing to jump back from.)
/// The transform is retired the pass it lands, and with nothing left to
/// say which edge stood still the picture went back to the top-left: the
/// whole of the window's contents jumped by half a screen at the moment
/// the glide finished. It stays against that edge for
/// `resizing::PATIENCE` past the landing, as a drag's picture does past
/// the release, and then the client's own picture is drawn as it is.
#[test]
fn a_glide_that_lands_before_its_client_answers_keeps_its_picture_where_it_was() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    state.resizing.fill = crate::resizing::Fill::Anchor;
    let (window, _surface, pane, tile) = filled_tile(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let right = Rectangle::new(
        (tile.loc.x + tile.size.w / 2, tile.loc.y).into(),
        (tile.size.w / 2, tile.size.h).into(),
    );
    let start = state.clock.now();
    glide(
        &mut state,
        &window,
        right,
        Duration::from_millis(240),
        present::Curve::Linear,
        start,
    );
    let committed = window.geometry().size;
    assert_eq!(
        committed, tile.size,
        "the premise: the client has not answered"
    );

    let anchored = |state: &Solium| {
        let held = state.panes.get(pane).expect("the pane is here");
        let outer = state.pane_outer(held);
        let frame = state.drawn_at(held, outer, state.clock.now());
        let drawn = crate::render::place_client(state, held, &frame, outer.size, committed);
        (
            drawn.origin.x + f64::from(committed.w) - (drawn.client.loc.x + drawn.client.size.w),
            drawn.origin.x - drawn.client.loc.x,
        )
    };
    state.clock.advance(Duration::from_millis(120));
    assert!(
        anchored(&state).0.abs() < 1e-9,
        "half way, the old picture stands against the right edge"
    );

    state.clock.advance(Duration::from_millis(140));
    state.settle(state.clock.now());
    assert!(
        state
            .panes
            .get(pane)
            .and_then(crate::present::course)
            .is_none(),
        "the premise: the glide has landed and its transform is gone"
    );
    assert_eq!(
        state.resize_pins(pane),
        (true, false),
        "landed, the edge the glide pinned is still the one standing still"
    );
    assert!(
        anchored(&state).0.abs() < 1e-9,
        "and the picture has not moved: {:?}",
        anchored(&state)
    );

    state.clock.advance(crate::resizing::PATIENCE);
    state.settle(state.clock.now());
    assert_eq!(state.resize_pins(pane), (false, false));
    assert!(
        anchored(&state).1.abs() < 1e-9,
        "once its patience is out, the client's picture is drawn from \
         the corner, as a window at rest is: {:?}",
        anchored(&state)
    );
}

/// **#233: a longer `resize.patience` keeps a landed glide's picture
/// where it was for longer.** The glide above, landed with its client
/// silent, under 600 ms of patience: 400 ms past the landing -- past the
/// shipped quarter second -- the picture still stands against the edge
/// the glide pinned, and at 600 it goes to the corner.
#[test]
fn a_longer_patience_keeps_a_landed_glides_picture_where_it_was_for_longer() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _surface, pane, tile) = filled_tile(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    state.resizing.patience = Duration::from_millis(600);
    let right = Rectangle::new(
        (tile.loc.x + tile.size.w / 2, tile.loc.y).into(),
        (tile.size.w / 2, tile.size.h).into(),
    );
    let start = state.clock.now();
    glide(
        &mut state,
        &window,
        right,
        Duration::from_millis(240),
        present::Curve::Linear,
        start,
    );
    state.clock.advance(Duration::from_millis(260));
    state.settle(state.clock.now());
    assert!(
        state
            .panes
            .get(pane)
            .and_then(crate::present::course)
            .is_none(),
        "the premise: the glide has landed and its transform is gone"
    );

    state.clock.advance(Duration::from_millis(400));
    assert_eq!(
        state.resize_pins(pane),
        (true, false),
        "400 ms past the landing, within the 600 configured, the edge the \
         glide pinned is still the one standing still"
    );
    state.clock.advance(Duration::from_millis(200));
    assert_eq!(
        state.resize_pins(pane),
        (false, false),
        "and at 600 the picture goes to the corner"
    );
}

/// **#233: a landed glide keeps the frames coming until its patience
/// runs out.** `Solium::settle` says whether anything needs the next
/// frame. A glide that has landed with its client silent holds its
/// picture against the edge it pinned until `resize.patience` is out, and
/// it is the frame after that which moves it to the corner -- so until
/// then something does: before, nothing did, and the picture moved on
/// whatever frame something else happened to ask for. Once the patience
/// is out nothing is waiting, and a glide whose client already has the
/// size it lands on keeps nothing coming at all.
#[test]
fn a_landed_glide_keeps_the_frames_coming_until_its_patience_runs_out() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _surface, pane, tile) = filled_tile(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let right = Rectangle::new(
        (tile.loc.x + tile.size.w / 2, tile.loc.y).into(),
        (tile.size.w / 2, tile.size.h).into(),
    );
    let start = state.clock.now();
    glide(
        &mut state,
        &window,
        right,
        Duration::from_millis(240),
        present::Curve::Linear,
        start,
    );
    state.clock.advance(Duration::from_millis(260));
    assert!(
        state.settle(state.clock.now()),
        "landed with its client silent, the frames keep coming"
    );
    assert!(
        state
            .panes
            .get(pane)
            .and_then(crate::present::course)
            .is_none(),
        "the premise: its transform is gone, so the transform is not what keeps them"
    );
    state.clock.advance(crate::resizing::PATIENCE);
    assert!(
        !state.settle(state.clock.now()),
        "once its patience is out, nothing is waiting"
    );

    // Back to the whole tile, whose width the client has had committed
    // all along: this glide lands on the size the client already has.
    let start = state.clock.now();
    glide(
        &mut state,
        &window,
        tile,
        Duration::from_millis(240),
        present::Curve::Linear,
        start,
    );
    assert_eq!(
        state.resize_pins(pane),
        (true, false),
        "the premise: this glide pins an edge too"
    );
    state.clock.advance(Duration::from_millis(260));
    assert!(
        !state.settle(state.clock.now()),
        "a client already at the size its glide landed on keeps nothing coming"
    );
}

/// **And a press on a gliding window lands on the pixel under it.**
///
/// `surface_under` used to map a point through `to_window_space`,
/// which reads the drawn rectangle as a scale of the pane's own. For
/// the glide above that is the new tile zoomed, so a press 600 pixels
/// into the drawn window reached the client 600 * 488 / 732 = 400
/// pixels in -- the zoom's picture, which is no longer what is drawn.
/// It goes through `render::place_client` now, and the glide here is
/// ten seconds long and linear so that the few milliseconds this test
/// takes to run cannot move it anywhere that matters.
#[test]
fn a_press_on_a_gliding_window_lands_on_the_pixel_under_it() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _surface, pane, tile) = filled_tile(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let half = Rectangle::new(tile.loc, (tile.size.w / 2, tile.size.h).into());
    let start = state.clock.now();
    glide(
        &mut state,
        &window,
        half,
        Duration::from_secs(10),
        present::Curve::Linear,
        start,
    );
    state.clock.advance(Duration::from_secs(5));

    let into = 600;
    let point: Point<f64, Logical> =
        (f64::from(tile.loc.x + into), f64::from(tile.loc.y + 100)).into();
    let outer = state.pane_outer_of(pane).expect("the pane is here");
    let frame = state.drawn(pane, outer);
    assert!(
        frame.covers(point) && point.x > f64::from(half.loc.x + half.size.w),
        "the point is on the gliding window and past its new tile: \
         {frame:?}, {point:?}"
    );

    let (surface, origin) = state
        .surface_under(point)
        .expect("the gliding window is under the point");
    assert_eq!(
        Some(surface),
        window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface().clone())
    );
    let within = point - origin;
    assert!(
        (within.x - f64::from(into)).abs() < 0.5 && (within.y - 100.0).abs() < 0.5,
        "the client is told the pixel the picture has under the pointer, \
         {into},100 at 1:1, and was told {within:?}"
    );
}

/// **#133 review, finding 5: a drag on a maximised tiled window starts
/// from its tile.**
///
/// A maximise takes the window out of `Pane::placed`, so that the
/// work area is not cut to the corner it was tiled in -- but it stays
/// a leaf of its tree, and nothing on the drag path asks about
/// maximised. `pane_laid_out` fell back to `pane_outer`, the whole
/// monitor, and the seam was handed the screen's far edge on the first
/// frame of a drag that had not moved. On stage `placed` still held
/// the tile. This is `a_client_that_rounds_its_size_does_not_move_the_seam`'s
/// drag, begun on a maximised window.
#[test]
fn a_drag_on_a_maximised_tiled_window_starts_from_its_tile() {
    use solium_layout::tree::{Edge, Tiling};

    tiled_fixture!(display, state, conn, queue, client, qh);
    let output = one_screen(&mut state);
    let screen = state
        .space
        .output_geometry(&output)
        .expect("the monitor is mapped");
    let (left, _left_toplevel, left_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (right, _right_toplevel, _right_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));
    let (area, settings) = (tiled_area(), tiled_settings());
    let mut tiling = Tiling::new();
    tiling.insert(left_id, None, None, area, settings);
    tiling.insert(right_id, Some(left_id), None, area, settings);
    sweep(&mut state, &tiling);
    let slot = leaf_of(&tiling, left_id);

    state.toggle_maximize(&left);
    commit_buffer(&client, &qh, &left_surface, screen.size.w, screen.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);
    let pane = state.panes.id_of(&left).expect("the pane is here");
    assert_eq!(
        state.pane_outer_of(pane),
        Some(screen),
        "the window is maximised, and not held in its tile"
    );

    let began = state.begin_resize(&left).expect("a mapped pane");
    let laid_out = state.pane_laid_out(&left).expect("a mapped pane");
    let grab: Point<f64, Logical> = (f64::from(began.loc.x + began.size.w) - 3.0, 300.0).into();
    let sent = crate::input::resize::dragged_edge(laid_out, ResizeEdge::Right, grab, grab);
    let mut unmoved = tiling.clone();
    unmoved.drag_seam(left_id, Edge::Right, sent, area, settings);
    let after = leaf_of(&unmoved, left_id);
    assert!(
        (after.x - slot.x).abs() < 0.5 && (after.w - slot.w).abs() < 0.5,
        "a drag that has not moved left the seam where it was: {slot:?} \
         became {after:?}, from an edge of {sent:?}"
    );
}

/// **#133 review, finding 9: a tiled window brought back onto a screen
/// is still tiled**, at the rectangle it was brought back to -- the
/// half of `Standing::Kept` that `a_floating_window_brought_back_onto_a_screen_is_not_given_a_tile`
/// cannot see. A client that then commits more than that rectangle is
/// held inside it.
#[test]
fn a_tiled_window_brought_back_onto_a_screen_is_still_tiled() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let _output = one_screen(&mut state);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (pane, tile) = tiled_alone(&mut state, &window);
    state.map_stacked(window.clone(), (-5000, 300), false);
    state.sync_panes();

    state.rescue_offscreen();
    let rescued = state.pane_outer_of(pane).expect("the pane is here");
    assert!(
        state.on_any_output(rescued),
        "the rescue brought the window back, or it proves nothing"
    );
    assert_eq!(
        state.panes.get(pane).and_then(Pane::placed),
        Some(rescued),
        "still in a tile, at the rectangle it was brought back to"
    );

    commit_buffer(&client, &qh, &surface, tile.size.w + 200, tile.size.h + 50);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);
    assert_eq!(
        state.pane_outer_of(pane),
        Some(rescued),
        "and a client that grows is held inside it"
    );
}

/// **And a maximised one brings the tile it left along with it.**
///
/// A window maximised on a monitor that goes away is rescued onto the
/// one that is left; the tile it is waiting to go back into was on the
/// departed monitor too, and `pane_laid_out` answers it for a drag on
/// the maximised window (see
/// `a_drag_on_a_maximised_tiled_window_starts_from_its_tile`). Left
/// behind, that drag would start from a rectangle on no screen.
#[test]
fn a_maximised_window_brought_back_onto_a_screen_brings_its_tile() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let _left = a_screen(&mut state, "rescue-left", (0, 0));
    let right = a_screen(&mut state, "rescue-right", (1920, 0));
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let pane = state.panes.id_of(&window).expect("the pane is here");
    let tile = at(2000, 100, 800, 600);
    let now = state.clock.now();
    glide(
        &mut state,
        &window,
        tile,
        Duration::ZERO,
        present::Curve::Linear,
        now,
    );
    state.toggle_maximize(&window);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);
    assert_eq!(
        state.panes.get(pane).and_then(Pane::left_tile),
        Some(tile),
        "maximised out of the tile on the right-hand monitor"
    );

    state.space.unmap_output(&right);
    state.settle_monitors();
    let rescued = state.pane_outer_of(pane).expect("the pane is here");
    assert!(
        state.on_any_output(rescued),
        "the maximised window was brought back: {rescued:?}"
    );

    let laid_out = state.pane_laid_out(&window).map(|laid_out| laid_out.0);
    assert_eq!(
        laid_out,
        Some(at(1920 - 800, 100, 800, 600)),
        "the tile it will go back into was brought back the way a window \
         is: onto the screen that is left, its size kept, clamped at the \
         edge it was beyond"
    );
}

/// **#133 review, finding 10: a tiled client is held inside its tile
/// with its frame taken off.**
///
/// Every other #133 test runs undecorated, where the client's share of
/// a tile *is* the tile, so `Solium::tile_of` forgetting the insets
/// passed all of them. A pane reserving a titlebar -- `Frame::Pending`,
/// which needs no Qt scene -- has a share `TITLEBAR_HEIGHT` shorter
/// than its tile, and a client committing the tile's full height has to
/// be held to that share: taken as the tile, it reaches a titlebar's
/// height into the tile below. Asserted of the pane, of the part of
/// the buffer a masked client is captured at, and of the hit test.
#[test]
fn a_titled_tiled_client_is_held_inside_its_tile_frame_and_all() {
    use solium_layout::tree::Tiling;

    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let pane = state.panes.id_of(&window).expect("the pane is here");
    state
        .panes
        .get_mut(pane)
        .expect("the pane is here")
        .set_frame(crate::pane::Frame::Pending);
    assert_eq!(
        state.insets_of(pane).top,
        TITLEBAR_HEIGHT,
        "a titlebar's worth reserved, or this is the undecorated case again"
    );
    let mut tiling = Tiling::new();
    tiling.insert(
        state.window_id(&window),
        None,
        None,
        tiled_area(),
        tiled_settings(),
    );
    sweep(&mut state, &tiling);
    let tile = state
        .panes
        .get(pane)
        .and_then(Pane::placed)
        .expect("the pane is in a tile");
    a_frame(&mut state);

    commit_buffer(&client, &qh, &surface, tile.size.w + 120, tile.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);

    assert_eq!(
        state.pane_outer_of(pane),
        Some(tile),
        "the frame and the client together are exactly the tile"
    );
    let held = state.panes.get(pane).expect("the pane is here");
    let frame = state.drawn(pane, tile);
    let drawn =
        crate::render::place_client(&state, held, &frame, tile.size, window.geometry().size);
    assert_eq!(
        drawn.fit.shown,
        Size::from((tile.size.w, tile.size.h - TITLEBAR_HEIGHT)),
        "a masked client is captured at the tile's share, under its titlebar"
    );
    let below: Point<f64, Logical> = (
        f64::from(tile.loc.x + 40),
        f64::from(tile.loc.y + tile.size.h + 4),
    )
        .into();
    assert_eq!(
        state.surface_under(below).map(|(surface, _)| surface),
        None,
        "and a point just past the tile's bottom edge is not the window's"
    );
}

/// **#133 review, findings 3 and 6: a warped window is captured at the
/// rectangle its warp is drawn over.**
///
/// `warp::mesh_part` spreads the whole of the capture over the frame's
/// rect, and that rect comes from `pane_outer_of`, which the cap holds
/// to the tile. The capture was sized from `outer_geometry` -- the
/// committed size -- so an oversized tiled client was squashed into
/// its tile for the length of a genie or a tilt, and its frame was told
/// the uncapped width while warped and the tile's once it landed.
/// `render::flat` is what both `offscreen::pane_job` and
/// `flat_window_elements` read, the texture size and the frame's
/// `Drawing.outer` alike.
#[test]
fn a_warped_window_is_captured_at_the_rect_its_warp_is_drawn_over() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (pane, tile) = tiled_alone(&mut state, &window);
    commit_buffer(&client, &qh, &surface, tile.size.w * 2, tile.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    a_frame(&mut state);
    assert_eq!(
        window.geometry().size.w,
        tile.size.w * 2,
        "the client really is twice as wide as its tile"
    );

    let flat = crate::render::flat(&state, &window).expect("the window is mapped");
    assert_eq!(
        Some(flat.outer),
        state.pane_outer_of(pane).map(|outer| outer.size),
        "captured at the rect the mesh is built over, so nothing is squashed"
    );
    assert_eq!(flat.outer, tile.size, "which is the tile");
    assert_eq!(flat.insets, state.insets_of(pane));
}

/// Open a popup on `parent` at `anchor` in its window's coordinates,
/// `size` big, and give it a buffer -- which means acking its first
/// configure, as xdg-shell requires before a popup commits anything.
#[expect(clippy::too_many_arguments, reason = "a fixture's plumbing")]
fn drawn_popup(
    display: &mut Display<Solium>,
    state: &mut Solium,
    conn: &Connection,
    qh: &QueueHandle<Client>,
    queue: &mut wayland_client::EventQueue<Client>,
    client: &mut Client,
    parent: &xdg_surface::XdgSurface,
    anchor: (i32, i32),
    size: (i32, i32),
) -> wl_surface::WlSurface {
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let wm_base = client.wm_base.clone().expect("xdg_wm_base bound");
    let surface = compositor.create_surface(qh, ());
    let xdg = wm_base.get_xdg_surface(&surface, qh, ());
    let positioner = wm_base.create_positioner(qh, ());
    positioner.set_size(size.0, size.1);
    positioner.set_anchor_rect(anchor.0, anchor.1, 1, 1);
    positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
    let _popup = xdg.get_popup(Some(parent), &positioner, qh, ());
    surface.commit();
    pump(display, state, conn, qh, queue, client);

    let id = wayland_client::Proxy::id(&xdg);
    let serial = client
        .surface_configures
        .iter()
        .rev()
        .find(|(to, _)| *to == id)
        .map(|&(_, serial)| serial)
        .expect("the popup was configured");
    xdg.ack_configure(serial);
    commit_buffer(client, qh, &surface, size.0, size.1);
    pump(display, state, conn, qh, queue, client);
    surface
}

/// **#133 review, findings 2 and 8: a toplevel is drawn without its
/// popups.**
///
/// Every path that draws a client draws its popups itself, through
/// `render::popup_elements` -- `elements` uncut above the sandwich, a
/// warped pane apart from its capture (`a_warped_panes_capture_holds_no_popups`)
/// -- and smithay's `Window::render_elements` draws them
/// *again*, ahead of the toplevel's own tree. On the ordinary path that
/// second copy went through the toplevel's fit and was cut to the
/// tile, one layer under the uncut one, so a translucent pixel of a
/// menu crossing the tile's edge was blended twice on one side of it
/// and once on the other. `render::toplevel_elements` is what all
/// three paths build the toplevel from, and it is generic over the
/// renderer so it runs here on smithay's `DummyRenderer`.
///
/// The first assertion is the control: smithay's own call does have
/// the popup in it, so the fixture has a popup that draws.
#[test]
fn a_toplevel_is_drawn_without_its_popups() {
    use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
    use smithay::backend::renderer::element::{Element as _, Id};
    use smithay::backend::renderer::test::DummyRenderer;

    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface, xdg_surface) =
        open_xdg(&mut display, &mut state, &conn, &client, &qh);
    let _popup = drawn_popup(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
        &xdg_surface,
        (10, 10),
        (40, 30),
    );
    let toplevel = window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone())
        .expect("an xdg toplevel");
    let popup = smithay::desktop::PopupManager::popups_for_surface(&toplevel)
        .next()
        .map(|(popup, _)| popup.wl_surface().clone())
        .expect("the popup is tracked on its parent");
    let (own_id, popup_id) = (
        Id::from_wayland_resource(&toplevel),
        Id::from_wayland_resource(&popup),
    );

    let mut renderer = DummyRenderer;
    let scale = smithay::utils::Scale::from(1.0);
    let whole: Vec<WaylandSurfaceRenderElement<DummyRenderer>> =
        smithay::backend::renderer::element::AsRenderElements::render_elements(
            &window,
            &mut renderer,
            (0, 0).into(),
            scale,
            1.0,
        );
    assert!(
        whole.iter().any(|element| element.id() == &popup_id),
        "smithay's whole-window call draws the popup, so there is a popup \
         here to leave out"
    );

    let own = crate::render::toplevel_elements(&mut renderer, &window, (0, 0).into(), scale, 1.0);
    assert!(
        own.iter().any(|element| element.id() == &own_id),
        "the toplevel itself is drawn"
    );
    assert!(
        own.iter().all(|element| element.id() != &popup_id),
        "and its popup is not, because every caller draws that itself"
    );
}

/// **A warped pane's capture holds no popups**: they are drawn in front of
/// it from a capture of their own, not under its titlebar.
#[test]
fn a_warped_panes_capture_holds_no_popups() {
    use smithay::backend::renderer::element::{Element as _, Id};
    use smithay::backend::renderer::test::DummyRenderer;
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface, xdg_surface) =
        open_xdg(&mut display, &mut state, &conn, &client, &qh);
    let _popup = drawn_popup(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
        &xdg_surface,
        (10, 10),
        (40, 30),
    );
    let toplevel = window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone())
        .expect("an xdg toplevel");
    let popup = smithay::desktop::PopupManager::popups_for_surface(&toplevel)
        .next()
        .map(|(popup, _)| popup.wl_surface().clone())
        .expect("the popup");
    let popup_id = Id::from_wayland_resource(&popup);
    let mut renderer = DummyRenderer;
    let scale = smithay::utils::Scale::from(1.0);
    let client_side =
        crate::render::client_piece(&mut renderer, &window, (0, 0).into(), scale, 1.0);
    assert!(
        client_side.iter().all(|element| element.id() != &popup_id),
        "the capture's client holds the popup"
    );
    let (popups, covered) =
        crate::render::popup_elements(&mut renderer, &window, (0, 0).into(), scale, 1.0);
    assert!(popups.iter().any(|element| element.id() == &popup_id));
    assert_eq!(covered.map(|rect| rect.size), Some((40, 30).into()));
}

/// A popup reaching past its window is captured whole: the rectangle its
/// capture is made at reaches past the window too.
#[test]
fn a_popup_past_the_window_is_captured_whole() {
    use smithay::backend::renderer::test::DummyRenderer;
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface, xdg_surface) =
        open_xdg(&mut display, &mut state, &conn, &client, &qh);
    let size = window.geometry().size;
    let _popup = drawn_popup(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
        &xdg_surface,
        (size.w - 10, size.h - 10),
        (60, 50),
    );
    let (_, covered) = crate::render::popup_elements(
        &mut DummyRenderer,
        &window,
        (0, 0).into(),
        smithay::utils::Scale::from(1.0),
        1.0,
    );
    let covered = covered.expect("the popup covers something");
    assert!(
        covered.loc.x + covered.size.w > size.w && covered.loc.y + covered.size.h > size.h,
        "{covered:?} stops at the window"
    );
}

/// **#232: a captured window ignores the client's window-geometry
/// offset**, so a client with client-side shadows (Firefox, GTK apps)
/// had its picture shifted away from its frame in every captured draw
/// -- warps, genies, close fades, tilted `sol.present` presentations.
/// `window_surface_origin` is now the one formula both the flat path
/// (`elements`) and the captured one (`flat_window_elements`, by way of
/// `client_piece`) draw a surface through, so this pins its contract
/// directly, against a window opened over the real protocol: a client
/// that sets no window geometry is unchanged, and one that sets an
/// offset through `xdg_surface.set_window_geometry` -- the request a
/// CSD shadow is declared with -- is shifted back by exactly that much,
/// the subtraction the captured path used to leave out.
#[test]
fn window_surface_origin_matches_a_csd_clients_shadow_offset() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface, xdg_surface) =
        open_xdg(&mut display, &mut state, &conn, &client, &qh);
    let scale = 1.0;
    let origin: smithay::utils::Point<i32, smithay::utils::Physical> = (100, 60).into();

    assert_eq!(
        window.geometry().loc,
        (0, 0).into(),
        "this fixture never calls set_window_geometry, so the committed \
         bounding box starts at the surface's own corner"
    );
    assert_eq!(
        crate::render::window_surface_origin(origin, &window, scale),
        origin,
        "no offset: the surface is drawn exactly where its frame reserves for it"
    );

    // A shadow outside the content: a CSD client's window geometry sits
    // inside its surface, (10, 15) in from the top-left corner here.
    xdg_surface.set_window_geometry(10, 15, 44, 49);
    surface.commit();
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    assert_eq!(window.geometry().loc, (10, 15).into());
    assert_eq!(
        crate::render::window_surface_origin(origin, &window, scale),
        (90, 45).into(),
        "a window-geometry offset of (10, 15) must shift the drawn surface \
         back by exactly that much, the same as the flat path always did"
    );
}

/// **A genie aimed at a surface aims at its instance on the window's own
/// monitor**, not on the monitor the pointer is on (#143). A missing scene
/// file builds no scene (`scripted.rs`, `Surface::sync`), so no Qt is
/// started.
#[test]
fn a_genie_aimed_at_a_surface_aims_at_its_instance_on_the_windows_own_monitor() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let _ = (&conn, &mut queue, &mut client);
    let (_left, _right) = side_by_side(&mut state, "aim-left");
    state.declare_surface(crate::scripted::Declaration::for_test(
        "dock",
        std::path::PathBuf::from("/nonexistent/solium-test-dock.qml"),
        crate::scripted::Layer::Top,
        crate::scripted::On::EveryMonitor,
    ));
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (2020, 100), false);
    let pane = state.panes.id_of(&window).expect("a pane for the window");
    let dock = state.surfaces.named("dock").expect("the dock is declared");
    let deform = crate::present::Deform {
        effect: crate::present::Geometry::for_test(crate::effect::settings::PresentFailed::Flat),
        anchor: crate::present::Anchor::Surface(dock),
    };
    let to = state
        .aimed_at_for(pane, Some(deform))
        .expect("the dock resolves")
        .to;
    assert!(
        to.loc.x >= 1920.0,
        "aimed at the dock on the pointer's monitor, at x = {}",
        to.loc.x
    );
}

/// **A capture whose surface tree has not committed is not drawn again.**
/// Its key is built from the same elements the capture draws, on
/// smithay's `DummyRenderer`, so this needs no GPU.
#[test]
fn a_capture_whose_surface_tree_has_not_committed_is_not_drawn_again() {
    use smithay::backend::renderer::test::DummyRenderer;
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (_pane, tile) = tiled_alone(&mut state, &window);
    commit_buffer(&client, &qh, &surface, tile.size.w, tile.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let mut renderer = DummyRenderer;
    let scale = smithay::utils::Scale::from(1.0);
    let size = (tile.size.w, tile.size.h).into();
    let key = |renderer: &mut DummyRenderer| {
        let elements =
            crate::render::toplevel_elements(renderer, &window, (0, 0).into(), scale, 1.0);
        crate::keyed::Inputs::of(crate::keyed::Kind::Pane, size, 1.0, &elements)
    };
    let mut capture = crate::keyed::Capture::<u32>::default();
    capture.drawn(1, key(&mut renderer));
    assert!(
        !capture.stale(&key(&mut renderer)),
        "nothing committed, and it would be drawn again"
    );
}

/// **A warped pane's capture is redrawn as its client fades** (#296). A
/// present with a turn and a `client_opacity` warps the pane, and the
/// frame its warp is planned from carries the client's opacity; the
/// capture draws the client at it, through the same `client_piece` and
/// `piece_alpha` `flat_pane` does, and that alpha is in the capture's
/// key, so a client fading under a warp is captured again each time it
/// changes and only then.
#[test]
fn a_warped_panes_capture_is_redrawn_as_its_client_fades() {
    use smithay::backend::renderer::{element::Element as _, test::DummyRenderer};
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (pane, tile) = tiled_alone(&mut state, &window);
    commit_buffer(&client, &qh, &surface, tile.size.w, tile.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let present = |state: &mut Solium, client_opacity: f32| {
        state.apply(crate::script::Outcome {
            commands: vec![crate::script::Command::Present {
                id: pane.get(),
                rect: None,
                opacity: None,
                client_opacity: Some(client_opacity),
                matrix: Some(crate::mat4::Mat4::rotate_z(0.2)),
                deform: None,
                z: 0.0,
                pivot: (0.5, 0.5),
                animation: crate::script::AnimationSpec {
                    duration: std::time::Duration::ZERO,
                    easing: crate::present::Curve::Linear,
                },
                channels: None,
            }],
            ..Default::default()
        });
    };
    let planned = |state: &Solium| {
        let outer = state.pane_outer_of(pane).expect("the pane is here");
        crate::render::warp_of(state, pane, outer, state.clock.now())
            .expect("a turned pane is warped")
            .0
            .client_opacity
    };
    let mut renderer = DummyRenderer;
    let scale = smithay::utils::Scale::from(1.0);
    let size = (tile.size.w, tile.size.h).into();
    let drawn = |renderer: &mut DummyRenderer, client_opacity: f32| {
        crate::render::client_piece(
            renderer,
            &window,
            (0, 0).into(),
            scale,
            crate::render::piece_alpha(1.0, client_opacity, crate::render::Piece::Client),
        )
    };
    let key =
        |elements: &[_]| crate::keyed::Inputs::of(crate::keyed::Kind::Pane, size, 1.0, elements);

    present(&mut state, 0.5);
    let half = planned(&state);
    assert!((half - 0.5).abs() < f32::EPSILON, "planned at {half}");
    let elements = drawn(&mut renderer, half);
    assert!(
        !elements.is_empty()
            && elements
                .iter()
                .all(|element| (element.alpha() - 0.5).abs() < f32::EPSILON),
        "the client is not drawn at half in the capture"
    );
    let mut capture = crate::keyed::Capture::<u32>::default();
    capture.drawn(1, key(&elements));
    assert!(
        !capture.stale(&key(&drawn(&mut renderer, planned(&state)))),
        "nothing changed, and it would be drawn again"
    );

    present(&mut state, 0.0);
    assert!(
        capture.stale(&key(&drawn(&mut renderer, planned(&state)))),
        "the client faded out and the capture still holds it at half"
    );
}

/// **A commit on a subsurface alone makes the capture stale**: "commits
/// anywhere in the surface tree" ([16] 0.4).
#[test]
fn a_commit_on_a_subsurface_alone_makes_the_capture_stale() {
    use smithay::backend::renderer::test::DummyRenderer;
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (_pane, tile) = tiled_alone(&mut state, &window);
    let compositor = client.compositor.clone().expect("wl_compositor bound");
    let subcompositor = client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &surface, &qh, ());
    subsurface.set_desync();
    commit_buffer(&client, &qh, &child, 40, 30);
    commit_buffer(&client, &qh, &surface, tile.size.w, tile.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let mut renderer = DummyRenderer;
    let scale = smithay::utils::Scale::from(1.0);
    let size = (tile.size.w, tile.size.h).into();
    let key = |renderer: &mut DummyRenderer| {
        let elements =
            crate::render::toplevel_elements(renderer, &window, (0, 0).into(), scale, 1.0);
        crate::keyed::Inputs::of(crate::keyed::Kind::Pane, size, 1.0, &elements)
    };
    let mut capture = crate::keyed::Capture::<u32>::default();
    capture.drawn(1, key(&mut renderer));
    commit_buffer(&client, &qh, &child, 40, 30);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        capture.stale(&key(&mut renderer)),
        "the subsurface committed and the capture did not notice"
    );
}

/// **A menu that commits during a flight is captured again**: the popups'
/// own capture is keyed on the popups' surfaces, so a warped window's menu
/// shows its live content, not the picture it had when the flight began.
#[test]
fn a_commit_on_a_popup_makes_the_popups_capture_stale() {
    use smithay::backend::renderer::test::DummyRenderer;
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface, xdg_surface) =
        open_xdg(&mut display, &mut state, &conn, &client, &qh);
    let popup = drawn_popup(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
        &xdg_surface,
        (10, 10),
        (40, 30),
    );
    let mut renderer = DummyRenderer;
    let scale = smithay::utils::Scale::from(1.0);
    let key = |renderer: &mut DummyRenderer| {
        let (elements, covered) =
            crate::render::popup_elements(renderer, &window, (0, 0).into(), scale, 1.0);
        let size = covered.map_or((1, 1).into(), |rect| (rect.size.w, rect.size.h).into());
        crate::keyed::Inputs::of(crate::keyed::Kind::Over, size, 1.0, &elements)
    };
    let mut capture = crate::keyed::Capture::<u32>::default();
    capture.drawn(1, key(&mut renderer));
    assert!(
        !capture.stale(&key(&mut renderer)),
        "nothing committed, and the menu would be drawn again"
    );
    commit_buffer(&client, &qh, &popup, 40, 30);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        capture.stale(&key(&mut renderer)),
        "the menu committed and its capture did not notice"
    );
}

/// FX2's real-client fixture: [`tiled_fixture!`]'s parts in one place,
/// one monitor for them to be on, and every window opened through it with
/// its client's objects, so a test can commit to it or ask for fullscreen
/// as the client. The style is `none`, for `tiled_fixture!`'s reason (the
/// #99 rule). Its scratch folder, named by `label`, goes with it.
struct Fixture {
    display: Display<Solium>,
    state: Solium,
    conn: Connection,
    queue: wayland_client::EventQueue<Client>,
    client: Client,
    qh: QueueHandle<Client>,
    label: &'static str,
    opened: Vec<(Window, xdg_toplevel::XdgToplevel, wl_surface::WlSurface)>,
    scratch: Option<std::path::PathBuf>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(scratch) = self.scratch.take() {
            let _ = std::fs::remove_dir_all(scratch);
        }
    }
}

/// `let mut fixture = Fixture { .. }`, made the way [`tiled_fixture!`]
/// makes its parts, with one 1920×1080 monitor at the origin named
/// `fx2-<label>` and one round trip, so the client starts having heard
/// everything.
macro_rules! fx2_fixture {
    ($fixture:ident, $label:literal) => {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let _ = a_screen(&mut state, concat!("fx2-", $label), (0, 0));
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let mut $fixture = Fixture {
            display,
            state,
            conn,
            queue,
            client,
            qh,
            label: $label,
            opened: Vec::new(),
            scratch: None,
        };
    };
}

/// [`pump`], with the fixture's parts.
fn pump_all(fixture: &mut Fixture) {
    pump(
        &mut fixture.display,
        &mut fixture.state,
        &fixture.conn,
        &fixture.qh,
        &mut fixture.queue,
        &mut fixture.client,
    );
}

/// A window opened through the real protocol, kept with its client's
/// objects.
fn open_kept(fixture: &mut Fixture) -> Window {
    let (window, toplevel, surface, _xdg) = open_xdg(
        &mut fixture.display,
        &mut fixture.state,
        &fixture.conn,
        &fixture.client,
        &fixture.qh,
    );
    fixture.opened.push((window.clone(), toplevel, surface));
    window
}

/// One window, tiled as [`tiled_alone`] tiles it.
fn one_window(fixture: &mut Fixture) -> (crate::pane::PaneId, Window) {
    let window = open_kept(fixture);
    let (pane, _tile) = tiled_alone(&mut fixture.state, &window);
    (pane, window)
}

/// Two windows tiled beside each other on the fixture's one monitor
/// (not [`side_by_side`], which is two monitors).
fn side_by_side_windows(fixture: &mut Fixture) -> (crate::pane::PaneId, crate::pane::PaneId) {
    use solium_layout::tree::Tiling;
    let (left, right) = (open_kept(fixture), open_kept(fixture));
    fixture.state.sync_panes();
    let (left_id, right_id) = (
        fixture.state.window_id(&left),
        fixture.state.window_id(&right),
    );
    let (area, settings) = (tiled_area(), tiled_settings());
    let mut tiling = Tiling::new();
    tiling.insert(left_id, None, None, area, settings);
    tiling.insert(right_id, Some(left_id), None, area, settings);
    sweep(&mut fixture.state, &tiling);
    fixture.state.settle(fixture.state.clock.now());
    let pane = |window: &Window| {
        fixture
            .state
            .panes
            .id_of(window)
            .expect("a client in the space has a pane")
    };
    (pane(&left), pane(&right))
}

/// The client's objects for a window this fixture opened.
fn kept(fixture: &Fixture, window: &Window) -> (xdg_toplevel::XdgToplevel, wl_surface::WlSurface) {
    fixture
        .opened
        .iter()
        .find(|(each, ..)| each == window)
        .map(|(_, toplevel, surface)| (toplevel.clone(), surface.clone()))
        .expect("a window this fixture opened")
}

/// A fresh buffer at the window's size, committed by its client.
fn commit_again(fixture: &mut Fixture, window: &Window) {
    let size = window.geometry().size;
    let (_, surface) = kept(fixture, window);
    commit_buffer(&fixture.client, &fixture.qh, &surface, size.w, size.h);
    pump_all(fixture);
}

/// The window's client asks for fullscreen and the request is handled:
/// what the compositor answers is the configure it then sends.
fn ask_fullscreen(fixture: &mut Fixture, window: &Window) {
    let (toplevel, _) = kept(fixture, window);
    toplevel.set_fullscreen(None);
    pump_all(fixture);
}

/// Give a pane's window the keyboard.
fn focus(fixture: &mut Fixture, pane: crate::pane::PaneId) {
    let window = fixture
        .state
        .panes
        .get(pane)
        .and_then(Pane::client)
        .cloned()
        .expect("a pane with a client");
    fixture.state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
}

/// The fixture's effects looked for in its scratch folder alone, which
/// holds Task 14's `tint`.
fn with_tint_folder(fixture: &mut Fixture) {
    let place = crate::effect::host::tests::scratch(&format!("fx2-{}", fixture.label));
    crate::effect::host::tests::folder(&place, "tint", TINT, &[("effect.frag", TINT_FRAG)]);
    fixture.state.effects = crate::effect::host::Host::new(crate::effect::host::Library::with(
        Some(place.clone()),
        place.join("none"),
    ));
    // No shipped effects, so no shipped `client_blur` to draw either.
    fixture.state.effect_settings.client_blur = None;
    fixture.scratch = Some(place);
}

/// `sol.effects` given these rules, taken.
fn apply_rules(fixture: &mut Fixture, lua: &str) {
    let rules = crate::effect::rules::parse(&tree(lua)).expect("parses");
    fixture.state.apply_effects(Ok(rules));
    assert!(
        !fixture.state.rules.is_empty(),
        "the rules were refused: {:?}",
        fixture.state.effects.problems()
    );
}

/// The fixture's effects looked for in the test folders
/// (`tests/fixtures/effects/`) and then the shipped ones.
fn fixture_effects(state: &mut Solium) {
    let fixtures = std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/effects"
    ));
    state.effects = crate::effect::host::Host::new(crate::effect::host::Library::with(
        Some(fixtures),
        crate::assets::effects(),
    ));
}

/// A copy of the shipped `effects/genie/` in a scratch folder the
/// fixture's effects read first: its folder.
fn scratch_genie(fixture: &mut Fixture) -> std::path::PathBuf {
    let place = crate::effect::host::tests::scratch(&format!("fx2-{}", fixture.label));
    let genie = crate::assets::effects().join("genie");
    let copy = place.join("genie");
    std::fs::create_dir_all(&copy).expect("a scratch folder");
    for entry in std::fs::read_dir(&genie).expect("the shipped genie") {
        let entry = entry.expect("an entry");
        std::fs::copy(entry.path(), copy.join(entry.file_name())).expect("copied");
    }
    fixture.state.effects = crate::effect::host::Host::new(crate::effect::host::Library::with(
        Some(place.clone()),
        crate::assets::effects(),
    ));
    fixture.scratch = Some(place);
    copy
}

/// `sol.present(<pane>, ...)` through `Solium::apply`, at once, as after
/// `sol.animate({ duration = 0 })`: a deform, a rect, or both.
fn present_now(
    fixture: &mut Fixture,
    pane: crate::pane::PaneId,
    deform: Option<&str>,
    rect: Option<(f64, f64, f64, f64)>,
) {
    let command = crate::script::Command::Present {
        id: pane.get(),
        rect: rect.map(|(x, y, w, h)| crate::script::Rect { x, y, w, h }),
        opacity: None,
        client_opacity: None,
        matrix: None,
        deform: deform.map(crate::script::deform_for_test),
        z: 0.0,
        pivot: (0.5, 0.5),
        animation: crate::script::AnimationSpec {
            duration: std::time::Duration::ZERO,
            easing: crate::present::Curve::OutCubic,
        },
        channels: None,
    };
    fixture.state.apply(crate::script::Outcome {
        commands: vec![command],
        ..Default::default()
    });
}

/// `sol.present(<pane>, { deform = <lua> })`, at once.
fn present_deform(fixture: &mut Fixture, pane: crate::pane::PaneId, lua: &str) {
    present_now(fixture, pane, Some(lua), None);
}

/// The geometry a pane's present asked for.
fn geometry_of(fixture: &Fixture, pane: crate::pane::PaneId) -> Option<crate::present::Geometry> {
    fixture
        .state
        .panes
        .get(pane)
        .and_then(crate::present::target_of)
        .and_then(|frame| frame.deform)
        .map(|deform| deform.effect)
}

/// What `prepare` would do with a pane this pass.
fn planned(fixture: &mut Fixture, pane: crate::pane::PaneId) -> crate::render::Planned {
    let outer = fixture.state.pane_outer_of(pane).expect("a pane on screen");
    let now = fixture.state.clock.now();
    crate::render::plan_warp(&mut fixture.state, pane, outer, 1.0, now)
}

/// `{ x = .., y = .., w = 64, h = 32 }` well below a pane's middle, as
/// Lua.
fn below(fixture: &Fixture, pane: crate::pane::PaneId) -> String {
    let outer = fixture.state.pane_outer_of(pane).expect("a pane on screen");
    format!(
        "{{ x = {}, y = {}, w = 64, h = 32 }}",
        outer.loc.x + outer.size.w / 2 - 32,
        outer.loc.y + outer.size.h + 200
    )
}

/// **A `sol.present` deform names an effect folder**: `genie` with
/// `axis = "auto"` toward a rect below the window is the folder's id,
/// pulled down, and `prepare` warps it through the folder's grid, 8 by
/// 48; a folder nobody has is a problem, carries no effect, and the
/// window is drawn undeformed (`effects.present.failed`'s default).
#[test]
fn a_present_deform_is_a_file() {
    fx2_fixture!(fixture, "present-file");
    let (pane, _) = one_window(&mut fixture);
    fixture_effects(&mut fixture.state);
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'genie', axis = 'auto', progress = 0.5, to = {to} }}"),
    );
    let geometry = geometry_of(&fixture, pane).expect("a geometry");
    assert_eq!(geometry.axis, solium_effects::Axis::Down);
    assert_eq!(Some(geometry.effect), fixture.state.effects.id("genie"));
    match planned(&mut fixture, pane) {
        crate::render::Planned::Warp {
            grid: Some(grid), ..
        } => assert_eq!((grid.cols, grid.rows), (8, 48)),
        other => panic!("not warped through the folder's grid: {other:?}"),
    }
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'nothing-here', progress = 0.5, to = {to} }}"),
    );
    assert_eq!(
        geometry_of(&fixture, pane).map(|geometry| geometry.effect),
        Some(crate::effect::host::EffectId::NONE)
    );
    assert!(
        fixture
            .state
            .effects
            .problems()
            .iter()
            .any(|problem| problem.effect == "nothing-here"),
        "{:?}",
        fixture.state.effects.problems()
    );
    assert!(
        matches!(planned(&mut fixture, pane), crate::render::Planned::Flat),
        "an unknown folder was not drawn undeformed"
    );
}

/// **`auto` is kept for the whole flight**: aimed at a window drawn to
/// its right, the genie is pulled right, and stays so once that window
/// is drawn below it, while the anchor itself follows it.
#[test]
fn auto_is_kept_for_the_whole_flight() {
    fx2_fixture!(fixture, "auto-kept");
    let (pane, target) = side_by_side_windows(&mut fixture);
    fixture_effects(&mut fixture.state);
    let outer = fixture.state.pane_outer_of(pane).expect("on screen");
    let (x, y, w, h) = (
        f64::from(outer.loc.x),
        f64::from(outer.loc.y),
        f64::from(outer.size.w),
        f64::from(outer.size.h),
    );
    present_now(
        &mut fixture,
        target,
        None,
        Some((x + w + 400.0, y + h / 2.0, 100.0, 50.0)),
    );
    present_deform(
        &mut fixture,
        pane,
        &format!(
            "{{ effect = 'genie', axis = 'auto', progress = 0.5, to = {{ window = {} }} }}",
            target.get()
        ),
    );
    assert_eq!(
        geometry_of(&fixture, pane).map(|geometry| geometry.axis),
        Some(solium_effects::Axis::Right)
    );
    present_now(
        &mut fixture,
        target,
        None,
        Some((x + w / 2.0 - 50.0, y + h + 100.0, 100.0, 50.0)),
    );
    let deform = fixture
        .state
        .panes
        .get(pane)
        .and_then(crate::present::target_of)
        .and_then(|frame| frame.deform);
    let aimed = fixture
        .state
        .aimed_at_for(pane, deform)
        .expect("the window is aimed at");
    assert!(
        aimed.to.loc.y > y + h,
        "the anchor did not follow its window: {:?}",
        aimed.to
    );
    assert_eq!(
        aimed.effect.axis,
        solium_effects::Axis::Right,
        "auto was picked again"
    );
}

/// **`effects.limits.params` is how many numbers a present may pack**: a
/// geometry with twelve params is refused at the default 8, with a
/// problem naming the key, and presents under `limits = { params = 16 }`
/// with all twelve, none cut.
#[test]
fn a_geometry_effect_with_twelve_params_presents_under_a_raised_limit() {
    fx2_fixture!(fixture, "twelve-params");
    let (pane, _) = one_window(&mut fixture);
    fixture_effects(&mut fixture.state);
    let deform = format!(
        "{{ effect = 'twelve', progress = 0.5, to = {} }}",
        below(&fixture, pane)
    );
    present_deform(&mut fixture, pane, &deform);
    assert!(
        geometry_of(&fixture, pane).is_none(),
        "twelve params packed under a limit of 8"
    );
    assert!(
        fixture
            .state
            .effects
            .problems()
            .iter()
            .any(|problem| problem.message.contains("effects.limits.params")),
        "{:?}",
        fixture.state.effects.problems()
    );
    apply_effects_lua(&mut fixture.state, "{ limits = { params = 16 } }");
    present_deform(&mut fixture, pane, &deform);
    assert_eq!(
        geometry_of(&fixture, pane).map(|geometry| geometry.params.as_slice().len()),
        Some(12),
        "the params were cut"
    );
    assert!(
        fixture
            .state
            .effects
            .problems()
            .iter()
            .all(|problem| !problem.message.contains("effects.limits.params")),
        "the present that packed did not mend the problem"
    );
}

/// **`effects.present` reaches a present, and a deform overrides it**:
/// with `present = { failed = "hide" }` a deform that says nothing
/// carries `Hide`, and its refused mesh draws nothing of the window; one
/// that says `failed = "flat"` carries `Flat`, and is drawn undeformed.
#[test]
fn effects_present_is_the_default_a_deform_overrides() {
    use crate::effect::settings::PresentFailed;
    fx2_fixture!(fixture, "present-policy");
    let (pane, _) = one_window(&mut fixture);
    fixture_effects(&mut fixture.state);
    let to = below(&fixture, pane);
    apply_effects_lua(&mut fixture.state, "{ present = { failed = 'hide' } }");
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'fail-mesh', progress = 0.5, to = {to} }}"),
    );
    assert_eq!(
        geometry_of(&fixture, pane).map(|geometry| geometry.failed),
        Some(PresentFailed::Hide)
    );
    assert!(
        matches!(planned(&mut fixture, pane), crate::render::Planned::Hidden),
        "a refused mesh under hide was drawn"
    );
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'fail-mesh', progress = 0.5, failed = 'flat', to = {to} }}"),
    );
    assert_eq!(
        geometry_of(&fixture, pane).map(|geometry| geometry.failed),
        Some(PresentFailed::Flat)
    );
    assert!(
        matches!(planned(&mut fixture, pane), crate::render::Planned::Flat),
        "a refused mesh under flat was not drawn undeformed"
    );
}

/// The reload `super+shift+r` makes (`Solium::reload_from`), of a
/// configuration that names no effect, written beside the fixture's
/// scratch effects.
fn reload(fixture: &mut Fixture) {
    reload_with(fixture, "sol.pane('none')\n");
}

/// [`reload`], of a configuration that reads `init`.
fn reload_with(fixture: &mut Fixture, init: &str) {
    let place = fixture.scratch.clone().expect("a scratch folder");
    let entry = place.join("init.lua");
    std::fs::write(&entry, init).expect("an init.lua");
    fixture.state.reload_from(&entry);
}

/// **A reload that leaves a present's folder unchanged keeps it**
/// (Ruling 7: an unrelated reload rebuilds nothing): a genie under way
/// under `on_reload = "flat"` still draws from its folder, through its
/// grid and under the same id, after a reload that did not edit it, so
/// only an edit makes it follow `on_reload`
/// (`a_reload_mid_present_follows_on_reload`).
#[test]
fn a_reload_that_leaves_a_presents_folder_unchanged_keeps_it() {
    fx2_fixture!(fixture, "present-unchanged");
    let (pane, _) = one_window(&mut fixture);
    let _ = scratch_genie(&mut fixture);
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'genie', progress = 0.5, on_reload = 'flat', to = {to} }}"),
    );
    let id = geometry_of(&fixture, pane).map(|geometry| geometry.effect);
    assert_eq!(id, fixture.state.effects.id("genie"), "the premise");
    reload(&mut fixture);
    assert!(
        crate::render::present_source(&fixture.state, pane).is_some(),
        "a reload that did not change the folder ended the present"
    );
    assert_eq!(
        fixture.state.effects.id("genie"),
        id,
        "the unchanged folder lost its id"
    );
    assert!(
        matches!(
            planned(&mut fixture, pane),
            crate::render::Planned::Warp { grid: Some(_), .. }
        ),
        "not drawn through its grid after the reload"
    );
}

/// **`sol.present_from` starts in its deform and lands without it**: a
/// launch growing out of its icon already in the icon's shape, relaxed
/// to the plain window as it arrives and then let go, as any
/// `present_from` is.
#[test]
fn a_present_from_starts_deformed_and_lands_plain() {
    fx2_fixture!(fixture, "present-from-deform");
    let (pane, _) = one_window(&mut fixture);
    let _ = scratch_genie(&mut fixture);
    let to = below(&fixture, pane);
    let start = fixture.state.clock.now();
    fixture.state.apply(crate::script::Outcome {
        commands: vec![crate::script::Command::PresentFrom {
            id: pane.get(),
            rect: crate::script::Rect {
                x: 10.0,
                y: 10.0,
                w: 60.0,
                h: 60.0,
            },
            opacity: None,
            client_opacity: None,
            animation: crate::script::AnimationSpec {
                duration: Duration::from_millis(100),
                easing: present::Curve::Linear,
            },
            channels: None,
            deform: Some(crate::script::deform_for_test(&format!(
                "{{ effect = 'genie', progress = 1, to = {to} }}"
            ))),
        }],
        ..Default::default()
    });
    let held = fixture.state.panes.get(pane).expect("the pane");
    let outer = fixture.state.pane_outer(held);
    let at = |ms| present::frame(held, outer, start + Duration::from_millis(ms));
    let first = at(0).deform.map(|deform| deform.effect.progress);
    assert_eq!(first, Some(1.0), "it starts all the way in its deform");
    let half = at(50).deform.map(|deform| deform.effect.progress);
    assert!(
        half.is_some_and(|progress| (progress - 0.5).abs() < 0.01),
        "relaxing as it goes, got {half:?}"
    );
    assert!(
        !present::settle(held, outer, start + Duration::from_millis(150)),
        "it has landed"
    );
    assert!(!present::transformed(held), "and is let go, deform and all");
}

/// **`effects.load` loads a folder before anything plays it** (#300): a
/// geometry only presents use is otherwise wanted only when the first
/// present names it, and that present draws the window flat. Named in
/// `load`, it is loaded with the configuration, and a reload that drops
/// it from `load` gives it back.
#[test]
fn effects_load_loads_a_folder_before_a_present_names_it() {
    fx2_fixture!(fixture, "effects-load");
    let _ = one_window(&mut fixture);
    let _ = scratch_genie(&mut fixture);
    reload_with(&mut fixture, "sol.effects({ rules = {} })\n");
    assert!(
        fixture.state.effects.effect("genie").is_none(),
        "the premise: nothing names the genie, so it is not loaded"
    );
    reload_with(
        &mut fixture,
        "sol.effects({ rules = {}, load = { 'genie' } })\n",
    );
    assert!(
        fixture.state.effects.effect("genie").is_some(),
        "named in effects.load, the genie is not loaded"
    );
    assert!(
        fixture.state.effects.problems().is_empty(),
        "{:?}",
        fixture.state.effects.problems()
    );
    reload_with(&mut fixture, "sol.effects({ rules = {} })\n");
    assert!(
        fixture.state.effects.effect("genie").is_none(),
        "dropped from effects.load, the genie is still held"
    );
}

/// **A reload that changes only `effects.sandbox` keeps a present**: the
/// new caps load the genie's unchanged folder again, under them, and
/// it keeps its id, so a window held at progress 0.5 under the default
/// `on_reload = "flat"` is still drawn through its grid; only an edit
/// makes it follow `on_reload` (`a_reload_mid_present_follows_on_reload`).
#[test]
fn a_reload_that_changes_only_the_sandbox_keeps_a_present() {
    fx2_fixture!(fixture, "present-caps");
    let (pane, _) = one_window(&mut fixture);
    let _ = scratch_genie(&mut fixture);
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'genie', progress = 0.5, on_reload = 'flat', to = {to} }}"),
    );
    let id = geometry_of(&fixture, pane).map(|geometry| geometry.effect);
    assert_eq!(id, fixture.state.effects.id("genie"), "the premise");
    let before = fixture.state.effects.caps();
    reload_with(
        &mut fixture,
        "sol.effects({ rules = {}, sandbox = { memory_mib = 32 } })\n",
    );
    assert!(
        fixture.state.effects.problems().is_empty(),
        "{:?}",
        fixture.state.effects.problems()
    );
    assert!(
        fixture.state.effects.caps() != before && fixture.state.effects.caps().memory == 32 << 20,
        "the premise: the reload changed the caps"
    );
    assert_eq!(
        fixture
            .state
            .effects
            .effect("genie")
            .map(|loaded| loaded.sandbox().caps()),
        Some(fixture.state.effects.caps()),
        "the genie that runs is not the one loaded under the new caps"
    );
    assert!(
        crate::render::present_source(&fixture.state, pane).is_some(),
        "a reload that changed only the caps ended the present"
    );
    assert_eq!(
        fixture.state.effects.id("genie"),
        id,
        "the unchanged folder lost its id to new caps"
    );
    assert!(
        matches!(
            planned(&mut fixture, pane),
            crate::render::Planned::Warp { grid: Some(_), .. }
        ),
        "not drawn through its grid after the reload"
    );
}

/// **A reload mid-present follows `on_reload`**: with the default
/// `"flat"` a present whose folder changed has no source for its grid,
/// and is drawn flat; with `"keep"` its grid comes from the version it
/// began with.
#[test]
fn a_reload_mid_present_follows_on_reload() {
    for (on_reload, kept) in [("flat", false), ("keep", true)] {
        fx2_fixture!(fixture, "present-reload");
        let (pane, _) = one_window(&mut fixture);
        let folder = scratch_genie(&mut fixture);
        let to = below(&fixture, pane);
        present_deform(
            &mut fixture,
            pane,
            &format!(
                "{{ effect = 'genie', progress = 0.5, on_reload = '{on_reload}', to = {to} }}"
            ),
        );
        assert!(
            crate::render::present_source(&fixture.state, pane).is_some(),
            "the premise: the present draws from its folder"
        );
        let file = folder.join("effect.lua");
        let edited = std::fs::read_to_string(&file).expect("read") + "-- edited\n";
        std::fs::write(&file, edited).expect("edited");
        reload(&mut fixture);
        assert_eq!(
            crate::render::present_source(&fixture.state, pane).is_some(),
            kept,
            "{on_reload}"
        );
        assert_eq!(
            matches!(
                planned(&mut fixture, pane),
                crate::render::Planned::Warp { grid: Some(_), .. }
            ),
            kept,
            "{on_reload}"
        );
    }
}

/// The pane's pinned version, if any.
fn pinned(fixture: &Fixture, pane: crate::pane::PaneId) -> bool {
    fixture
        .state
        .panes
        .get(pane)
        .is_some_and(|held| held.meshes().pinned.is_some())
}

/// **A pinned version is let go when its present lands**: a present
/// under `on_reload = "keep"` pins the version it began with, kept while
/// the window is held at its progress, and a present with no deform
/// that lands lets it go, so no pane holds a stale Lua state to the end
/// of its life.
#[test]
fn a_pinned_version_is_let_go_when_its_present_lands() {
    fx2_fixture!(fixture, "present-unpinned");
    let (pane, _) = one_window(&mut fixture);
    fixture_effects(&mut fixture.state);
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'genie', progress = 0.5, on_reload = 'keep', to = {to} }}"),
    );
    let now = fixture.state.clock.now();
    let _ = fixture.state.settle(now);
    assert!(
        pinned(&fixture, pane),
        "the version a held present began with was let go"
    );
    present_now(&mut fixture, pane, None, None);
    let now = fixture.state.clock.now();
    let _ = fixture.state.settle(now);
    assert!(
        !crate::present::transformed(fixture.state.panes.get(pane).expect("the pane")),
        "the premise: the present landed and was released"
    );
    assert!(
        !pinned(&fixture, pane),
        "a landed present still pins its version"
    );
}

/// **A geometry with a frag presents before it compiled**: its version
/// waits for the next `prepare`'s compile, and a present made before
/// then carries the id that version runs under, not none, so once it
/// compiled the window is drawn through its grid.
#[test]
fn a_geometry_with_a_frag_presents_before_it_compiled() {
    fx2_fixture!(fixture, "present-pending");
    let (pane, _) = one_window(&mut fixture);
    fixture_effects(&mut fixture.state);
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'mesh-frag', progress = 0.5, to = {to} }}"),
    );
    assert!(
        fixture.state.effects.has_pending("mesh-frag")
            && fixture.state.effects.effect("mesh-frag").is_none(),
        "the premise: its frag waits for a compile: {:?}",
        fixture.state.effects.problems()
    );
    let geometry = geometry_of(&fixture, pane).expect("a geometry");
    assert_ne!(
        geometry.effect,
        crate::effect::host::EffectId::NONE,
        "a version still compiling was presented as no effect"
    );
    fixture
        .state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    assert_eq!(Some(geometry.effect), fixture.state.effects.id("mesh-frag"));
    assert!(
        matches!(
            planned(&mut fixture, pane),
            crate::render::Planned::Warp { grid: Some(_), .. }
        ),
        "not drawn through its grid once it compiled"
    );
}

/// A copy of the fixture effect `name` in a scratch folder, which is
/// all the fixture's effects read besides the shipped ones: its folder.
fn scratch_fixture(fixture: &mut Fixture, name: &str) -> std::path::PathBuf {
    let from = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/effects"
    ))
    .join(name);
    let place = crate::effect::host::tests::scratch(&format!("fx2-{}", fixture.label));
    let copy = place.join(name);
    std::fs::create_dir_all(&copy).expect("a scratch folder");
    for entry in std::fs::read_dir(&from).expect("the fixture") {
        let entry = entry.expect("an entry");
        std::fs::copy(entry.path(), copy.join(entry.file_name())).expect("copied");
    }
    fixture.state.effects = crate::effect::host::Host::new(crate::effect::host::Library::with(
        Some(place.clone()),
        crate::assets::effects(),
    ));
    fixture.scratch = Some(place);
    copy
}

/// The pixels a pane's present draws through over the whole pane, its
/// capture 200 by 100.
fn pixels_of(
    fixture: &Fixture,
    pane: crate::pane::PaneId,
    geometry: crate::present::Geometry,
) -> Option<(
    crate::effect::gl::Program,
    crate::effect::gl::PixelUniforms,
    std::rc::Rc<[String]>,
)> {
    crate::render::present_pixels(
        &fixture.state,
        pane,
        geometry,
        (
            crate::warp::UnitRect::WHOLE,
            crate::render::content_in(crate::warp::UnitRect::WHOLE),
        ),
        (200, 100).into(),
    )
}

/// **A present draws its folder's frag as its pixels**: a geometry with
/// a frag beside its mesh is drawn, once that frag compiled into the
/// warp, through it, with the present's progress, a leaving's
/// direction, its seed, the capture's size and the part the capture is;
/// before then through nothing of its own.
#[test]
fn a_present_draws_its_folders_frag_as_its_pixels() {
    fx2_fixture!(fixture, "present-pixels");
    let (pane, _) = one_window(&mut fixture);
    fixture_effects(&mut fixture.state);
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'mesh-frag', progress = 0.5, seed = 0.25, to = {to} }}"),
    );
    let geometry = geometry_of(&fixture, pane).expect("a geometry");
    assert!(
        pixels_of(&fixture, pane, geometry).is_none(),
        "drawn through pixels that had not compiled"
    );
    fixture
        .state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    let (_, uniforms, names) =
        pixels_of(&fixture, pane, geometry).expect("its frag, once compiled");
    assert_eq!(
        (
            uniforms.progress,
            uniforms.clamped,
            uniforms.direction,
            uniforms.seed
        ),
        (0.5, 0.5, -1.0, 0.25)
    );
    assert_eq!(uniforms.size, [200.0, 100.0]);
    assert_eq!(uniforms.content, [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(uniforms.part, [0.0, 0.0, 1.0, 1.0]);
    assert!(names.is_empty() && uniforms.params.as_slice().is_empty());
}

/// A window still loading, its standing-in scene the shipped one built
/// on the Qt thread, over 400 by 300 on a monitor of `label`'s, its
/// effects the fixtures': what #302's tests present.
fn loading_with_its_scene(label: &str) -> (Solium, crate::pane::PaneId) {
    let display = Display::<Solium>::new().expect("creating a test wayland display");
    let mut state = Solium::new(display.handle());
    let _ = a_screen(&mut state, label, (0, 0));
    fixture_effects(&mut state);
    let scene = crate::surface::ShellSurface::new(
        crate::pane::loading_source(None),
        r#"{"program":"kitty","waited":0}"#,
    )
    .expect("the shipped loading scene builds");
    let now = state.clock.now();
    let pane = state.panes.open(Pane::loading(
        "kitty",
        None,
        Rectangle::new((200, 150).into(), (400, 300).into()),
        std::path::PathBuf::new(),
        Some(scene),
        now,
    ));
    (state, pane)
}

/// `sol.present(<pane>, { deform = <lua> })` at once, on `state`.
fn deform_now(state: &mut Solium, pane: crate::pane::PaneId, lua: &str) {
    state.apply(crate::script::Outcome {
        commands: vec![crate::script::Command::Present {
            id: pane.get(),
            rect: None,
            opacity: None,
            matrix: None,
            deform: Some(crate::script::deform_for_test(lua)),
            client_opacity: None,
            z: 0.0,
            pivot: (0.5, 0.5),
            animation: crate::script::AnimationSpec {
                duration: std::time::Duration::ZERO,
                easing: crate::present::Curve::OutCubic,
            },
            channels: None,
        }],
        ..Default::default()
    });
}

/// **A loading pane under a present is planned through the warp**
/// (#302): a window whose application has not arrived, under a genie,
/// is planned `Warp` through the genie's grid, as an arrived window is,
/// and is captured from its standing-in scene, so it is drawn bent and
/// not flat. No client, so the scene may be a real one (the #99 rule).
#[test]
fn a_loading_pane_under_a_present_is_planned_through_the_warp() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        if crate::qml::on_gpu() {
            return;
        }
        let (mut state, pane) = loading_with_its_scene("loading-warp");
        deform_now(
            &mut state,
            pane,
            "{ effect = 'genie', progress = 0.5, to = { x = 380, y = 900, w = 40, h = 40 } }",
        );
        crate::render::plan_warps(&mut state);
        assert!(
            matches!(
                state.presented.take(pane),
                Some(crate::render::Planned::Warp { grid: Some(_), .. })
            ),
            "a window still loading under a genie is not planned through its grid"
        );
        assert_eq!(
            crate::render::captured(&state, pane, None).map(|(what, _)| what),
            Some(crate::render::Captured::Standing),
            "its capture does not hold its scene"
        );
    });
}

/// **A loading pane at rest builds no capture** (#302): with no
/// present on it, a window still loading is planned flat, which
/// `prepare` captures nothing for, and is drawn as it always was.
#[test]
fn a_loading_pane_at_rest_builds_no_capture() {
    crate::qml::qt_test::on_the_qt_thread(|| {
        crate::qml::start().expect("Qt starts");
        if crate::qml::on_gpu() {
            return;
        }
        let (mut state, pane) = loading_with_its_scene("loading-rest");
        crate::render::plan_warps(&mut state);
        let planned = state.presented.take(pane);
        assert!(
            matches!(planned, Some(crate::render::Planned::Flat)),
            "a window still loading at rest is planned {planned:?}"
        );
        assert_eq!(
            crate::render::wanted_capture(crate::render::route(false, true)),
            None,
            "the premise: a pane planned flat wants no capture"
        );
    });
}

/// **A present on a loading pane survives its application's arrival**
/// (#302): the application maps into the window it was launched into,
/// and the genie on that window is the same one, planned through its
/// grid from the window's own capture now. The transform lives on the
/// pane, so the handover carries it.
#[test]
fn a_present_on_a_loading_pane_survives_its_applications_arrival() {
    fx2_fixture!(fixture, "loading-handover");
    fixture_effects(&mut fixture.state);
    let pane = fixture.state.open_loading(
        "app",
        Some(std::process::id()),
        crate::pane::loading_source(None),
        None,
    );
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'genie', progress = 0.5, to = {to} }}"),
    );
    let before = geometry_of(&fixture, pane).expect("a genie on the window still loading");
    let window = open_kept(&mut fixture);
    assert_eq!(
        fixture.state.panes.id_of(&window),
        Some(pane),
        "the premise: the application arrived in the window it was launched into"
    );
    assert_eq!(
        geometry_of(&fixture, pane),
        Some(before),
        "the handover dropped or changed the present"
    );
    crate::render::plan_warps(&mut fixture.state);
    assert!(
        matches!(
            fixture.state.presented.take(pane),
            Some(crate::render::Planned::Warp { grid: Some(_), .. })
        ),
        "the arrived window is not planned through the genie"
    );
}

/// **A present through a pixels effect reads the params it blends**:
/// `fly`'s `pixels` is `rounded`, so a deform's `radius` is `rounded`'s
/// and its `lift` `fly`'s, each packed with its own effect and blended
/// by the present, and `rounded`'s frag is drawn with `radius`. A param
/// neither has is a problem, and the window is drawn undeformed.
#[test]
fn a_present_through_a_pixels_effect_reads_the_params_it_blends() {
    fx2_fixture!(fixture, "present-pixels-effect");
    let (pane, _) = one_window(&mut fixture);
    fixture_effects(&mut fixture.state);
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'fly', progress = 0.5, radius = 12, lift = 0.25, to = {to} }}"),
    );
    let geometry = geometry_of(&fixture, pane).expect("a geometry");
    assert_eq!(geometry.params.as_slice(), [0.25]);
    assert_eq!(geometry.pixels.as_slice(), [12.0]);
    fixture
        .state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    let (_, uniforms, names) =
        pixels_of(&fixture, pane, geometry).expect("rounded's frag, once compiled");
    assert_eq!(&*names, ["p_radius".to_owned()]);
    assert_eq!(uniforms.params.as_slice(), [12.0]);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'fly', progress = 0.5, roundness = 1, to = {to} }}"),
    );
    assert!(geometry_of(&fixture, pane).is_none());
    assert!(
        fixture
            .state
            .effects
            .problems()
            .iter()
            .any(|problem| problem.message.contains("roundness")),
        "{:?}",
        fixture.state.effects.problems()
    );
}

/// **A present whose frag will not compile follows its `failed`**, and
/// the id it was given never names a later version: the version it
/// waited for is dropped, so under `failed = "hide"` nothing of the
/// window is drawn; once the frag is mended a new version runs under an
/// id of its own, and the present, whose folder a reload changed, is
/// drawn undeformed as `on_reload = "flat"` draws one, never through
/// the mended version's grid.
#[test]
fn a_present_whose_frag_will_not_compile_follows_its_failed() {
    fx2_fixture!(fixture, "present-frag-fails");
    let (pane, _) = one_window(&mut fixture);
    let folder = scratch_fixture(&mut fixture, "mesh-frag");
    let frag = folder.join("effect.frag");
    let good = std::fs::read_to_string(&frag).expect("its frag");
    std::fs::write(&frag, format!("{good}FAIL\n")).expect("broken");
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'mesh-frag', progress = 0.5, failed = 'hide', to = {to} }}"),
    );
    let geometry = geometry_of(&fixture, pane).expect("a geometry");
    assert_ne!(geometry.effect, crate::effect::host::EffectId::NONE);
    fixture
        .state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    assert!(
        matches!(planned(&mut fixture, pane), crate::render::Planned::Hidden),
        "a version that never ran was drawn undeformed, not by its `failed`"
    );
    std::fs::write(&frag, &good).expect("mended");
    fixture
        .state
        .effects
        .reload_keeping(["mesh-frag".to_owned()]);
    fixture
        .state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    assert!(
        fixture
            .state
            .effects
            .id("mesh-frag")
            .is_some_and(|id| id != geometry.effect),
        "the premise: a new version runs, under an id of its own"
    );
    assert!(
        matches!(planned(&mut fixture, pane), crate::render::Planned::Flat),
        "the dangling id came to name the mended version"
    );
}

/// **A present under `on_reload = "keep"` begun while its version
/// compiles keeps that version**: nothing runs to pin when it is made,
/// so the first pass whose id answers pins it, and a reload that
/// changes the folder leaves it drawn through its grid.
#[test]
fn a_kept_present_begun_while_its_version_compiles_keeps_it() {
    fx2_fixture!(fixture, "present-keep-pending");
    let (pane, _) = one_window(&mut fixture);
    let folder = scratch_fixture(&mut fixture, "mesh-frag");
    let to = below(&fixture, pane);
    present_deform(
        &mut fixture,
        pane,
        &format!("{{ effect = 'mesh-frag', progress = 0.5, on_reload = 'keep', to = {to} }}"),
    );
    assert!(
        fixture.state.effects.has_pending("mesh-frag"),
        "the premise: its version waits for a compile"
    );
    fixture
        .state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    assert!(matches!(
        planned(&mut fixture, pane),
        crate::render::Planned::Warp { grid: Some(_), .. }
    ));
    let file = folder.join("effect.lua");
    let edited = std::fs::read_to_string(&file).expect("read") + "-- edited\n";
    std::fs::write(&file, edited).expect("edited");
    fixture
        .state
        .effects
        .reload_keeping(["mesh-frag".to_owned()]);
    fixture
        .state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    assert!(
        matches!(
            planned(&mut fixture, pane),
            crate::render::Planned::Warp { grid: Some(_), .. }
        ),
        "a kept present lost its version to a reload"
    );
}

/// **A present refused for its popups alone is said once, and its
/// popups follow `failed`**: a mesh finite over the window and NaN past
/// its bottom edge warps the pane and is refused for a menu below it.
/// Over two passes the refusal is warned of once, though the pane's own
/// grid builds in each, and the popups are drawn undeformed (the
/// identity grid of their part) under `"flat"`, and not at all under
/// `"hide"`.
#[test]
fn a_present_refused_for_its_popups_alone_is_said_once() {
    use crate::warp::UnitRect;
    fx2_fixture!(fixture, "present-popups");
    let (pane, _) = one_window(&mut fixture);
    fixture_effects(&mut fixture.state);
    let to = below(&fixture, pane);
    let below_the_window = UnitRect {
        u0: 0.25,
        v0: 0.75,
        u1: 0.75,
        v1: 1.5,
    };
    for (failed, drawn) in [("flat", Some(None)), ("hide", None)] {
        present_deform(
            &mut fixture,
            pane,
            &format!("{{ effect = 'nan-past', progress = 0.5, failed = '{failed}', to = {to} }}"),
        );
        let outer = fixture.state.pane_outer_of(pane).expect("on screen");
        let warned = |fixture: &Fixture| {
            fixture
                .state
                .panes
                .get(pane)
                .map(|held| held.meshes().warned)
        };
        let before = warned(&fixture);
        for pass in 0..2 {
            assert!(
                matches!(
                    planned(&mut fixture, pane),
                    crate::render::Planned::Warp { grid: Some(_), .. }
                ),
                "{failed}, pass {pass}: the pane's own grid was refused"
            );
            let (frame, aimed) =
                crate::render::warp_of(&fixture.state, pane, outer, fixture.state.clock.now())
                    .expect("warped");
            let aimed = aimed.expect("aimed");
            let over = crate::render::over_grid(
                &mut fixture.state,
                pane,
                &frame,
                aimed,
                below_the_window,
                1.0,
            );
            assert_eq!(over, drawn, "{failed}, pass {pass}: the popups");
        }
        assert_eq!(
            warned(&fixture).zip(before).map(|(now, then)| now - then),
            Some(1),
            "{failed}: the popups' refusal was not said once"
        );
    }
}

/// **A rule on `focused` follows the keyboard**: two windows, the rule's
/// slot moves with the focus.
#[test]
fn a_rule_on_focused_follows_the_keyboard() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "rule-focused");
    let (first, second) = side_by_side_windows(&mut fixture);
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = { focused = true }, part = "client", slot = "front", effect = false },
             { match = { focused = true }, part = "client", slot = "behind", effect = "tint" } }"#,
    );
    focus(&mut fixture, first);
    let slots = crate::render::build_slots(&mut fixture.state);
    assert!(slots.wanted(&Owner::Pane(first, PaneSlot::Client), Slot::Behind));
    assert!(!slots.wanted(&Owner::Pane(second, PaneSlot::Client), Slot::Behind));
    assert!(
        !slots.wanted(&Owner::Pane(first, PaneSlot::Client), Slot::Front),
        "`effect = false` wants no slot"
    );
    focus(&mut fixture, second);
    let slots = crate::render::build_slots(&mut fixture.state);
    assert!(slots.wanted(&Owner::Pane(second, PaneSlot::Client), Slot::Behind));
    assert!(!slots.wanted(&Owner::Pane(first, PaneSlot::Client), Slot::Behind));
}

/// **A rule with a fade eases its slot in and out with the focus**: on
/// the window without the keyboard the slot is wanted from opacity 0 and
/// reaches 1 over the rule's 200 ms; once that window has the keyboard
/// its slot is still wanted, fading from 1, and wanted no more 200 ms on,
/// while the other window's comes in. A frame is asked for on every pass
/// a fade has yet to arrive, and on none once both have.
#[test]
fn a_rule_with_a_fade_eases_its_slot_in_and_out_with_the_focus() {
    use crate::effect::plan::{Owner, PaneSlot, Slots};
    use crate::effect::rules::Slot;
    use std::time::Duration;
    fx2_fixture!(fixture, "rule-fade");
    let (first, second) = side_by_side_windows(&mut fixture);
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = { focused = false }, part = "pane", slot = "front", effect = "tint",
               fade = { duration = 200, easing = "linear" } } }"#,
    );
    // How opaque each window's front slot is drawn, `None` when it is not
    // wanted, a pass at a time, 100 ms apart.
    let pass = |fixture: &mut Fixture, after: u64| {
        fixture.state.clock.advance(Duration::from_millis(after));
        fixture.state.redraw = false;
        let slots: Slots = crate::render::build_slots(&mut fixture.state);
        let on = |pane| {
            let owner = Owner::Pane(pane, PaneSlot::Pane);
            slots
                .wanted(&owner, Slot::Front)
                .then(|| slots.opacity(&owner, Slot::Front))
        };
        (on(first), on(second), fixture.state.redraw)
    };
    // Within 0.1: the clock goes on running between the passes.
    let near = |seen: Option<f32>, wanted: Option<f32>| match (seen, wanted) {
        (Some(seen), Some(wanted)) => (seen - wanted).abs() < 0.1,
        (seen, wanted) => seen.is_none() && wanted.is_none(),
    };
    focus(&mut fixture, first);
    for (after, wanted) in [
        (0, (None, Some(0.0), true)),
        (100, (None, Some(0.5), true)),
        (100, (None, Some(1.0), false)),
        (100, (None, Some(1.0), false)),
    ] {
        let seen = pass(&mut fixture, after);
        assert!(
            near(seen.0, wanted.0) && near(seen.1, wanted.1) && seen.2 == wanted.2,
            "{after} ms on, fading in: {seen:?}, wanted {wanted:?}"
        );
    }
    focus(&mut fixture, second);
    for (after, wanted) in [
        (0, (Some(0.0), Some(1.0), true)),
        (100, (Some(0.5), Some(0.5), true)),
        (100, (Some(1.0), None, false)),
        (100, (Some(1.0), None, false)),
    ] {
        let seen = pass(&mut fixture, after);
        assert!(
            near(seen.0, wanted.0) && near(seen.1, wanted.1) && seen.2 == wanted.2,
            "{after} ms on, the focus moved: {seen:?}, wanted {wanted:?}"
        );
    }
}

/// **Two `replace` rules on `focused` crossfade as the keyboard moves**
/// (#389): a window's own pixels at 0.95 with the keyboard and 0.80
/// without, each rule fading over 200 ms. The window losing the keyboard
/// has its `replace` slot wanted by the rule without it, whole, mixed
/// from the rule with it, wanted in the slot's `Leaving` twin, from 0 to
/// all the new one's over the 200 ms, and then that rule alone; the one
/// gaining it the other way round. The keyboard back half way, the first
/// window crossfades back from where it got to. Frames are owed while
/// any crossfade moves.
#[test]
fn two_replace_rules_on_focused_crossfade_as_the_keyboard_moves() {
    use crate::effect::plan::{Owner, PaneSlot, Slots};
    use crate::effect::rules::Slot;
    use std::time::Duration;
    fx2_fixture!(fixture, "rule-crossfade");
    let (first, second) = side_by_side_windows(&mut fixture);
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = { focused = true }, part = "client", slot = "replace",
               effect = { "tint", amount = 0.95 },
               fade = { duration = 200, easing = "linear" } },
             { match = { focused = false }, part = "client", slot = "replace",
               effect = { "tint", amount = 0.80 },
               fade = { duration = 200, easing = "linear" } } }"#,
    );
    // Each window's `replace`: the rule it shows, its crossfade's share
    // and the rule it crossfades from, a pass at a time.
    let pass = |fixture: &mut Fixture, after: u64| {
        fixture.state.clock.advance(Duration::from_millis(after));
        fixture.state.redraw = false;
        let slots: Slots = crate::render::build_slots(&mut fixture.state);
        let on = |pane| {
            let owner = Owner::Pane(pane, PaneSlot::Client);
            (
                slots.key(&owner, Slot::Replace).map(|key| key.index),
                slots
                    .mix(&owner, Slot::Replace)
                    .map(|share| (share * 10.0).round() / 10.0),
                slots.key(&owner, Slot::LeavingReplace).map(|key| key.index),
                slots.opacity(&owner, Slot::Replace),
            )
        };
        let crossfades = slots.crossfades().count();
        (on(first), on(second), crossfades, fixture.state.redraw)
    };
    focus(&mut fixture, first);
    let _ = pass(&mut fixture, 0);
    // Settled: each whole, crossfading from nothing.
    let settled = pass(&mut fixture, 1000);
    assert_eq!(
        settled,
        (
            (Some(0), None, None, 1.0),
            (Some(1), None, None, 1.0),
            0,
            false
        ),
        "settled"
    );
    focus(&mut fixture, second);
    for (after, wanted) in [
        (
            0,
            (
                (Some(1), Some(0.0), Some(0), 1.0),
                (Some(0), Some(0.0), Some(1), 1.0),
                2,
                true,
            ),
        ),
        (
            100,
            (
                (Some(1), Some(0.5), Some(0), 1.0),
                (Some(0), Some(0.5), Some(1), 1.0),
                2,
                true,
            ),
        ),
    ] {
        let seen = pass(&mut fixture, after);
        assert_eq!(seen, wanted, "{after} ms after the keyboard moved");
    }
    // Back half way: the first window crossfades back from there.
    focus(&mut fixture, first);
    let back = pass(&mut fixture, 0);
    assert_eq!(
        (back.0, back.1),
        (
            (Some(0), Some(0.5), Some(1), 1.0),
            (Some(1), Some(0.5), Some(0), 1.0)
        ),
        "turned round"
    );
    let done = pass(&mut fixture, 150);
    assert_eq!(
        done,
        (
            (Some(0), None, None, 1.0),
            (Some(1), None, None, 1.0),
            0,
            false
        ),
        "back where they were"
    );
}

/// **A `replace` rule coming and going crossfades with its part**
/// (#389, the window that went see-through to nothing as it went
/// fullscreen and then popped back): one `replace` rule on `focused`,
/// fading over 200 ms. The window losing the keyboard keeps its slot
/// wanted, its rule's result mixed with its part's own pixels, their
/// share rising from 0 to 1 over the 200 ms, never fading to nothing
/// (its opacity stays whole), and then nothing wanted at all; the one
/// gaining it comes in from its own pixels the same way. Turned round
/// half way, each goes back from where it got to. A `behind` with the
/// same fade still fades to nothing.
#[test]
fn a_replace_rule_coming_and_going_crossfades_with_its_part() {
    use crate::effect::plan::{Owner, PaneSlot, Slots};
    use crate::effect::rules::Slot;
    use std::time::Duration;
    fx2_fixture!(fixture, "rule-raw-crossfade");
    let (first, second) = side_by_side_windows(&mut fixture);
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = { focused = true }, part = "client", slot = "replace",
               effect = { "tint", amount = 0.9 },
               fade = { duration = 200, easing = "linear" } },
             { match = { focused = true }, part = "client", slot = "behind",
               effect = { "tint", amount = 0.5 },
               fade = { duration = 200, easing = "linear" } } }"#,
    );
    // Each window's `replace`: whether it is wanted, its part's share and
    // its opacity; and its `behind`'s opacity, a pass at a time.
    let pass = |fixture: &mut Fixture, after: u64| {
        fixture.state.clock.advance(Duration::from_millis(after));
        let slots: Slots = crate::render::build_slots(&mut fixture.state);
        let inputs = crate::render::self_inputs(&fixture.state, &slots);
        let on = |pane| {
            let owner = Owner::Pane(pane, PaneSlot::Client);
            let captured = inputs
                .iter()
                .any(|input| input.owner == owner && input.slot == Slot::Replace);
            (
                slots.wanted(&owner, Slot::Replace),
                slots
                    .raw(&owner, Slot::Replace)
                    .map(|share| (share * 10.0).round() / 10.0),
                slots.opacity(&owner, Slot::Replace),
                slots
                    .wanted(&owner, Slot::Behind)
                    .then(|| (slots.opacity(&owner, Slot::Behind) * 10.0).round() / 10.0),
                captured,
            )
        };
        (on(first), on(second))
    };
    focus(&mut fixture, first);
    assert_eq!(
        pass(&mut fixture, 0).0,
        (true, Some(1.0), 1.0, Some(0.0), true),
        "coming in from its own pixels, not from nothing"
    );
    assert_eq!(
        pass(&mut fixture, 1000),
        (
            (true, None, 1.0, Some(1.0), true),
            (false, None, 1.0, None, false)
        ),
        "settled"
    );
    focus(&mut fixture, second);
    assert_eq!(
        pass(&mut fixture, 0),
        (
            (true, None, 1.0, Some(1.0), true),
            (true, Some(1.0), 1.0, Some(0.0), true)
        ),
        "the keyboard moved"
    );
    assert_eq!(
        pass(&mut fixture, 40),
        (
            (true, Some(0.2), 1.0, Some(0.8), true),
            (true, Some(0.8), 1.0, Some(0.2), true)
        ),
        "a fifth of the way"
    );
    assert_eq!(
        pass(&mut fixture, 60),
        (
            (true, Some(0.5), 1.0, Some(0.5), true),
            (true, Some(0.5), 1.0, Some(0.5), true)
        ),
        "half way"
    );
    // Back half way: each goes back from where it got to.
    focus(&mut fixture, first);
    let _ = pass(&mut fixture, 0);
    assert_eq!(
        pass(&mut fixture, 40),
        (
            (true, Some(0.3), 1.0, Some(0.7), true),
            (true, Some(0.7), 1.0, Some(0.3), true)
        ),
        "turned round"
    );
    assert_eq!(
        pass(&mut fixture, 100),
        (
            (true, None, 1.0, Some(1.0), true),
            (false, None, 1.0, None, false)
        ),
        "back where they were, and nothing left on the window without the keyboard"
    );
}

/// **A rule on `fullscreen` applies from the configure that sets it**,
/// the state the compositor last told the client.
#[test]
fn a_rule_on_fullscreen_applies_from_the_configure_that_sets_it() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "rule-fullscreen");
    let (pane, window) = one_window(&mut fixture);
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = { fullscreen = true }, part = "client", slot = "behind", effect = "tint" } }"#,
    );
    assert!(
        !crate::render::build_slots(&mut fixture.state)
            .wanted(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind)
    );
    ask_fullscreen(&mut fixture, &window);
    assert!(
        crate::render::build_slots(&mut fixture.state)
            .wanted(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind)
    );
}

/// **A rule on `maximized` applies once the compositor maximises the
/// window, and lets go once it restores it** (#454): the state it last
/// told the client, as `fullscreen` is; `sol.windows()` says the same.
#[test]
fn a_rule_on_maximized_follows_the_window_being_maximised_and_restored() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "rule-maximized");
    let (pane, window) = one_window(&mut fixture);
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = { maximized = true }, part = "client", slot = "behind", effect = "tint" } }"#,
    );
    let wanted = |fixture: &mut Fixture| {
        crate::render::build_slots(&mut fixture.state)
            .wanted(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind)
    };
    assert!(!wanted(&mut fixture), "matched a window not maximised");
    let id = fixture.state.window_id(&window);
    let toggle = |fixture: &mut Fixture| {
        fixture.state.apply(crate::script::Outcome {
            commands: vec![crate::script::Command::ToggleMaximize { id }],
            ..Default::default()
        });
        pump_all(fixture);
    };
    toggle(&mut fixture);
    assert!(wanted(&mut fixture), "a maximised window was not matched");
    let snapshot = fixture.state.snapshot();
    assert!(snapshot.windows.first().expect("one window").maximized);
    toggle(&mut fixture);
    assert!(!wanted(&mut fixture), "a restored window was still matched");
}

/// **A rule on `values` follows what `sol.frame` hands the window**
/// (#454): the configuration tags a window, and the rule matches it from
/// the next pass, and stops when the tag is taken back.
#[test]
fn a_rule_on_values_follows_what_sol_frame_hands_the_window() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "rule-values");
    let (pane, window) = one_window(&mut fixture);
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = { values = { snapped = true } }, part = "client", slot = "behind", effect = "tint" } }"#,
    );
    let wanted = |fixture: &mut Fixture| {
        crate::render::build_slots(&mut fixture.state)
            .wanted(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind)
    };
    assert!(!wanted(&mut fixture), "matched a window never tagged");
    let id = fixture.state.window_id(&window);
    let tag = |fixture: &mut Fixture, snapped: bool| {
        fixture.state.apply(crate::script::Outcome {
            commands: vec![crate::script::Command::FrameValues {
                id,
                fields: [("snapped".to_owned(), crate::json::Json::Bool(snapped))]
                    .into_iter()
                    .collect(),
            }],
            ..Default::default()
        });
    };
    tag(&mut fixture, true);
    assert!(wanted(&mut fixture), "a tagged window was not matched");
    tag(&mut fixture, false);
    assert!(!wanted(&mut fixture), "a window untagged was still matched");
}

/// **A rule for another app leaves a window with an empty plan**, its
/// facts asked and matching nothing.
#[test]
fn a_rule_for_another_app_leaves_a_window_with_an_empty_plan() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "rule-other-app");
    let (pane, _) = one_window(&mut fixture);
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = { app_id = "mpv" }, part = "client", slot = "behind", effect = "tint" } }"#,
    );
    let slots = crate::render::build_slots(&mut fixture.state);
    assert!(!slots.wanted(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind));
    assert!(slots.is_empty());
    assert_eq!(
        slots.facts_gathered(),
        1,
        "the window's facts were not asked"
    );
}

/// **`sol.windows()` carries every window match key**: `fullscreen` and
/// `style` beside `app_id`, `title`, `focused` and `monitor`; and the
/// room its frame takes, `insets`.
#[test]
fn sol_windows_carries_every_window_match_key() {
    fx2_fixture!(fixture, "windows-keys");
    let (pane, window) = one_window(&mut fixture);
    let snapshot = fixture.state.snapshot();
    let row = snapshot.windows.first().expect("one window");
    assert!(!row.fullscreen);
    assert_eq!(
        row.style,
        fixture.state.decorations.style().unwrap_or("none")
    );
    // And what its frame takes, which the client is the rest of (#435).
    assert_eq!(row.insets, fixture.state.insets_of(pane));
    ask_fullscreen(&mut fixture, &window);
    let snapshot = fixture.state.snapshot();
    assert!(snapshot.windows.first().expect("one window").fullscreen);
}

/// **With no rules no slot is wanted and no fact is gathered** (spec
/// §8.4), though a window is on screen: the plan is empty and its counter
/// of facts gathered is 0.
#[test]
fn with_no_rules_no_slot_is_wanted_and_no_fact_is_gathered() {
    fx2_fixture!(fixture, "no-rules-no-facts");
    let _ = one_window(&mut fixture);
    let slots = crate::render::build_slots(&mut fixture.state);
    assert!(slots.is_empty());
    assert_eq!(
        slots.facts_gathered(),
        0,
        "facts were gathered for a pane no rule can match"
    );
}

/// **A fullscreen window with no rule takes today's path**: no slot is
/// wanted, it is not warped, it wants no capture, and with no slot ready
/// its walk is `PANE_ORDER` (`with_no_slots_the_pane_walk_is_pane_order`),
/// so `panes` builds the surface elements it builds today, which is what
/// smithay's DRM compositor assigns planes from (Ruling 24).
#[test]
fn a_fullscreen_window_with_no_rule_takes_todays_path() {
    use crate::effect::plan::PaneSlot;
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "fullscreen-no-rule");
    let (pane, window) = one_window(&mut fixture);
    ask_fullscreen(&mut fixture, &window);
    let (_, surface) = kept(&fixture, &window);
    commit_buffer(&fixture.client, &fixture.qh, &surface, 1920, 1080);
    pump_all(&mut fixture);
    fixture.state.clock.advance(Duration::from_secs(1));
    let now = fixture.state.clock.now();
    fixture.state.settle(now);
    pump_all(&mut fixture);
    assert_eq!(
        fixture.state.real_geometry(&window),
        Some(Rectangle::new((0, 0).into(), (1920, 1080).into())),
        "the premise: the window is fullscreen on the monitor"
    );
    let slots = crate::render::build_slots(&mut fixture.state);
    assert!(slots.is_empty());
    assert!(
        !crate::render::wants_warp(&fixture.state, pane),
        "a fullscreen window with no rule is warped"
    );
    assert_eq!(
        crate::render::wanted_capture(crate::render::Route::Flat),
        None
    );
    let ready = crate::render::slot_ready(&slots, pane, None);
    for part in [PaneSlot::Pane, PaneSlot::Client, PaneSlot::Popups] {
        for slot in [Slot::Behind, Slot::Front, Slot::Replace] {
            assert!(!ready(part, slot));
        }
    }
}

/// One window, tiled as [`one_window`] tiles it, with a 120×80 menu
/// drawn on it, so the window has a `popup` part.
fn one_window_with_a_menu(
    fixture: &mut Fixture,
) -> (crate::pane::PaneId, Window, wl_surface::WlSurface) {
    let (window, toplevel, surface, xdg) = open_xdg(
        &mut fixture.display,
        &mut fixture.state,
        &fixture.conn,
        &fixture.client,
        &fixture.qh,
    );
    fixture.opened.push((window.clone(), toplevel, surface));
    let (pane, _tile) = tiled_alone(&mut fixture.state, &window);
    let menu = drawn_popup(
        &mut fixture.display,
        &mut fixture.state,
        &fixture.conn,
        &fixture.qh,
        &mut fixture.queue,
        &mut fixture.client,
        &xdg,
        (20, 20),
        (120, 80),
    );
    (pane, window, menu)
}

/// **Every way an effect can fail leaves every part drawn** ([16] §5,
/// spec §8.4). Rules cover every part a window has here and every slot,
/// each set naming one effect that fails its own way: a Lua error and a
/// `p_` typo are refused at load, the set with them; a program that will
/// not compile, a GPU on which every run fails, and a self effect whose
/// capture was never drawn are wanted and never ready. In each, no slot is
/// ready, and the pane's walk is the walk with no rule at all.
#[test]
fn every_failure_leaves_the_part_drawn() {
    use crate::effect::plan::{Owner, PaneSlot, Slots};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "every-failure");
    let (pane, _window, _menu) = one_window_with_a_menu(&mut fixture);
    let fixtures = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/effects"
    ));
    fixture.state.effects = crate::effect::host::Host::new(crate::effect::host::Library::with(
        Some(fixtures.to_owned()),
        fixtures.join("none"),
    ));
    // No shipped effects, so no shipped `client_blur` to draw either.
    fixture.state.effect_settings.client_blur = None;
    let walk = |slots: &Slots| {
        let mut pieces = Vec::new();
        crate::render::pane_walk(
            &mut pieces,
            &crate::render::slot_ready(slots, pane, None),
            |into, piece| into.push(piece),
        );
        pieces
    };
    let plain = walk(&Slots::default());
    let every = |effect: &str, parts: &[&str]| {
        let mut lua = String::from("{");
        for part in parts {
            for slot in ["behind", "front", "replace"] {
                lua.push_str(&format!(
                    " {{ match = \"*\", part = \"{part}\", slot = \"{slot}\", effect = {effect} }},"
                ));
            }
        }
        lua.push('}');
        crate::effect::rules::parse(&tree(&lua)).expect("parses")
    };
    let all = [
        "pane",
        "client",
        "popup",
        "region:titlebar",
        "layer_shell:*",
    ];
    // A titlebar rule reading `self` is refused when it binds, until P15
    // (`effect::plan::tests::a_region_self_rule_is_refused_until_p15`),
    // which would keep the self effect's whole set out here.
    let unbanded = ["pane", "client", "popup", "layer_shell:*"];
    // Whether its chains get as far as a run: a program that would not
    // compile and a GPU that fails every run are each tried and fail; a
    // self effect with no capture is never run.
    for (effect, parts, refused, run) in [
        (r#""fail-lua""#, &all[..], true, false),
        (r#""fail-lint""#, &all[..], true, false),
        (r#""fail-compile""#, &all[..], false, true),
        (r#""ring""#, &all[..], false, true),
        (
            r#"{ "tint", source = "self" }"#,
            &unbanded[..],
            false,
            false,
        ),
    ] {
        let before = fixture.state.rules_generation;
        fixture.state.apply_effects(Ok(every(effect, parts)));
        assert_eq!(
            fixture.state.rules_generation == before,
            refused,
            "{effect}: refused at load or not ({:?})",
            fixture.state.effects.problems()
        );
        fixture
            .state
            .effects
            .compile_pending(&mut crate::effect::host::tests::Refusing);
        let mut slots = crate::render::build_slots(&mut fixture.state);
        for part in [PaneSlot::Pane, PaneSlot::Client, PaneSlot::Popups] {
            assert_eq!(
                slots.wanted(&Owner::Pane(pane, part), Slot::Replace),
                !refused,
                "{effect}: the premise, the {part:?}'s slot wanted or not"
            );
        }
        crate::render::record_boxes(&fixture.state, &mut slots);
        let Solium {
            store,
            effects,
            chains,
            memos,
            xrays,
            ..
        } = &mut fixture.state;
        let mut cx = crate::render::RunCx {
            store,
            effects: &*effects,
            chains,
            masked: None,
            now: 0.0,
            pass: 1,
            memos,
            xrays: &*xrays,
            motion: (std::time::Duration::ZERO, 0.0),
            waited: false,
            drawing: crate::drawing::Drawing::every(),
        };
        let mut runner = crate::render::tests::NoGpu::default();
        for nest in [crate::render::Nest::Inner, crate::render::Nest::Whole] {
            crate::render::run_slots(
                &mut cx,
                &mut crate::pool::Pool::new(0),
                &mut runner,
                &mut slots,
                &crate::effect::store::Drawn::default(),
                nest,
            );
        }
        assert_eq!(
            runner.runs > 0,
            run,
            "{effect}: the premise, its chains were run or not"
        );
        assert_eq!(
            slots.ready_count(),
            0,
            "{effect}: a failing effect made a slot ready"
        );
        assert_eq!(
            walk(&slots),
            plain,
            "{effect}: the pane's walk is not the walk with no rule"
        );
    }
}

/// **A scripted surface has a slot on each monitor it is on, and a
/// client layer surface one of its own**, each matched by its name: one
/// owner per instance per output, and one per layer surface whose
/// namespace the rule's glob takes.
#[test]
fn a_surface_has_a_slot_per_monitor_and_a_layer_surface_one_of_its_own() {
    use crate::effect::plan::Owner;
    use crate::effect::rules::Slot;
    use smithay::reexports::wayland_server::Resource as _;
    fx2_fixture!(fixture, "rule-surfaces");
    let _ = a_screen(&mut fixture.state, "fx2-rule-surfaces-right", (1920, 0));
    with_tint_folder(&mut fixture);
    for (name, layer) in [
        ("dock", crate::scripted::Layer::Top),
        ("wallpaper", crate::scripted::Layer::Background),
    ] {
        let _ = fixture
            .state
            .surfaces
            .declare(crate::scripted::Declaration::for_test(
                name,
                std::path::PathBuf::from(format!("{name}.qml")),
                layer,
                crate::scripted::On::EveryMonitor,
            ));
    }
    let _bar = bar(
        &mut fixture.display,
        &mut fixture.state,
        &fixture.conn,
        &fixture.client,
        &fixture.qh,
        30,
    );
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "surface:dock", slot = "behind", effect = "tint" },
             { match = "*", part = "layer_shell:restore-*", slot = "front", effect = "tint" } }"#,
    );
    let slots = crate::render::build_slots(&mut fixture.state);
    let dock = fixture.state.surfaces.named("dock").expect("declared");
    let wallpaper = fixture.state.surfaces.named("wallpaper").expect("declared");
    for monitor in ["fx2-rule-surfaces", "fx2-rule-surfaces-right"] {
        assert!(
            slots.wanted(&Owner::Surface(dock, monitor.to_owned()), Slot::Behind),
            "the dock on {monitor} has no slot"
        );
        assert!(!slots.wanted(&Owner::Surface(wallpaper, monitor.to_owned()), Slot::Behind));
    }
    let layered: Vec<_> = fixture
        .state
        .space
        .outputs()
        .flat_map(|output| {
            smithay::desktop::layer_map_for_output(output)
                .layers()
                .map(|layer| layer.wl_surface().id())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(layered.len(), 1, "the premise: one layer surface, the bar");
    assert!(slots.wanted(&Owner::LayerShell(layered[0].clone()), Slot::Front));
    assert_eq!(
        slots.wants().count(),
        3,
        "two docks and a bar, and nothing else"
    );
}

/// **A resolved rule whose chain is not bound wants no slot**: every
/// chain is bound at config load or when its style is applied, never in
/// a frame (Ruling 15), so a rule that could not bind (a style's, its
/// problem already on the overlay) leaves its slot empty.
#[test]
fn a_rule_whose_chain_is_not_bound_wants_no_slot() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::{Origin, RuleKey, Slot};
    fx2_fixture!(fixture, "rule-unbound");
    let (pane, _) = one_window(&mut fixture);
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "tint" } }"#,
    );
    let client = Owner::Pane(pane, PaneSlot::Client);
    assert!(crate::render::build_slots(&mut fixture.state).wanted(&client, Slot::Behind));
    fixture.state.chains.remove(RuleKey {
        origin: Origin::User,
        index: 0,
        generation: fixture.state.rules_generation,
    });
    assert!(!crate::render::build_slots(&mut fixture.state).wanted(&client, Slot::Behind));
}

/// The self input's key for a client, as `prepare` builds it.
fn self_key(fixture: &mut Fixture, window: &Window, pad: i32) -> crate::keyed::Inputs {
    use smithay::backend::renderer::test::DummyRenderer;
    let source = crate::offscreen::PartSource::Client(window);
    let size = crate::offscreen::part_size(&fixture.state, &source, 1.0, pad).expect("a size");
    let elements = crate::offscreen::part_elements(&mut DummyRenderer, &source, pad, 1.0);
    crate::keyed::Inputs::of(crate::keyed::Kind::Pane, size, 1.0, &elements)
}

/// A desynchronised subsurface of `window`'s surface with a `w` x `h`
/// buffer of its own, placed by a commit of its parent, as [fx0] Task
/// 17's subsurface steps make one.
fn subsurface_of(fixture: &mut Fixture, window: &Window, w: i32, h: i32) -> wl_surface::WlSurface {
    let (_, surface) = kept(fixture, window);
    let compositor = fixture
        .client
        .compositor
        .clone()
        .expect("wl_compositor bound");
    let subcompositor = fixture
        .client
        .subcompositor
        .clone()
        .expect("wl_subcompositor bound");
    let child = compositor.create_surface(&fixture.qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &surface, &fixture.qh, ());
    subsurface.set_desync();
    commit_buffer(&fixture.client, &fixture.qh, &child, w, h);
    commit_again(fixture, window);
    child
}

/// A fresh `w` x `h` buffer on a subsurface, committed by its client.
fn commit_child(fixture: &mut Fixture, child: &wl_surface::WlSurface, w: i32, h: i32) {
    commit_buffer(&fixture.client, &fixture.qh, child, w, h);
}

/// **A self rule captures the client once until it commits.**
#[test]
fn a_self_rule_captures_the_client_once_until_it_commits() {
    fx2_fixture!(fixture, "self-once");
    let (_pane, window) = one_window(&mut fixture);
    let mut capture = crate::keyed::Capture::<u32>::default();
    capture.drawn(1, self_key(&mut fixture, &window, 6));
    assert!(
        !capture.stale(&self_key(&mut fixture, &window, 6)),
        "recaptured with nothing committed"
    );
    commit_again(&mut fixture, &window);
    pump_all(&mut fixture);
    assert!(
        capture.stale(&self_key(&mut fixture, &window, 6)),
        "not recaptured after a commit"
    );
}

/// **A subsurface's commit recaptures its client**: the capture is keyed
/// on every surface of the tree ([fx0] Ruling 9).
#[test]
fn a_subsurface_commit_recaptures_its_client() {
    fx2_fixture!(fixture, "self-subsurface");
    let (_pane, window) = one_window(&mut fixture);
    let child = subsurface_of(&mut fixture, &window, 40, 30);
    let mut capture = crate::keyed::Capture::<u32>::default();
    capture.drawn(1, self_key(&mut fixture, &window, 6));
    commit_child(&mut fixture, &child, 40, 30);
    pump_all(&mut fixture);
    assert!(
        capture.stale(&self_key(&mut fixture, &window, 6)),
        "a subsurface's commit did not recapture the client"
    );
}

/// A client's layer surface on `layer`, its namespace `namespace`, across
/// the top of the fixture's monitor and 30 px tall, with a buffer: a part
/// `layer_shell:<namespace>` names. Its layer object comes with it.
fn layer_surface(
    fixture: &mut Fixture,
    namespace: &str,
    layer: zwlr_layer_shell_v1::Layer,
) -> (
    wl_surface::WlSurface,
    zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
) {
    use zwlr_layer_surface_v1::Anchor;
    let compositor = fixture
        .client
        .compositor
        .clone()
        .expect("wl_compositor bound");
    let shell = fixture
        .client
        .layer_shell
        .clone()
        .expect("zwlr_layer_shell_v1 bound");
    let surface = compositor.create_surface(&fixture.qh, ());
    let layered =
        shell.get_layer_surface(&surface, None, layer, namespace.to_owned(), &fixture.qh, ());
    layered.set_anchor(Anchor::Top | Anchor::Left | Anchor::Right);
    layered.set_size(0, 30);
    surface.commit();
    pump_all(fixture);
    commit_layer_again(fixture, &surface);
    (surface, layered)
}

/// A fresh 1920 x 30 buffer on a layer surface [`layer_surface`] made,
/// committed by its client.
fn commit_layer_again(fixture: &mut Fixture, surface: &wl_surface::WlSurface) {
    commit_buffer(&fixture.client, &fixture.qh, surface, 1920, 30);
    pump_all(fixture);
}

/// The key of an xray cut on the fixture's monitor, built as
/// `offscreen::xray_job` builds it, on smithay's `DummyRenderer`:
/// `SessionLayers`' entries, a layer surface's tree at its place as
/// `render::stacked_elements` draws it, and a pane's client tree should
/// one be listed, which `xray_holds_the_cuts_bands_and_nothing_else`
/// says none is.
fn xray_key(fixture: &mut Fixture, cut: crate::effect::xray::Cut) -> crate::keyed::Inputs {
    use crate::effect::xray::XraySource as _;
    use crate::render::Stacked;
    use smithay::backend::renderer::element::AsRenderElements as _;
    use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
    use smithay::backend::renderer::test::DummyRenderer;
    let output = fixture
        .state
        .space
        .outputs()
        .next()
        .cloned()
        .expect("the fixture's monitor");
    let scale = smithay::utils::Scale::from(1.0);
    let mut renderer = DummyRenderer;
    let mut elements: Vec<WaylandSurfaceRenderElement<DummyRenderer>> = Vec::new();
    for each in crate::effect::xray::SessionLayers.entries(&fixture.state, &output, cut) {
        match each {
            Stacked::Layer(surface, geometry) => elements.extend(surface.render_elements(
                &mut renderer,
                geometry.loc.to_physical_precise_round(1.0),
                scale,
                1.0,
            )),
            Stacked::Panes(nodes) => {
                for ((_, window, ..), _) in nodes {
                    if let Some(window) = window {
                        elements.extend(crate::render::toplevel_elements(
                            &mut renderer,
                            &window,
                            (0, 0).into(),
                            scale,
                            1.0,
                        ));
                    }
                }
            }
            Stacked::Surface(..) => {}
        }
    }
    let size = crate::offscreen::xray_size(&fixture.state, &output);
    crate::keyed::Inputs::of(crate::keyed::Kind::Xray, size, 1.0, &elements)
}

/// **The xray capture is keyed**: a layer surface in the cut that
/// commits makes it stale; a window that commits does not.
#[test]
fn the_xray_capture_is_stale_only_when_its_cut_changes() {
    fx2_fixture!(fixture, "xray-keyed");
    let (wallpaper, _layered) = layer_surface(
        &mut fixture,
        "wallpaper",
        zwlr_layer_shell_v1::Layer::Background,
    );
    let (_pane, window) = one_window(&mut fixture);
    pump_all(&mut fixture);
    let key = |fixture: &mut Fixture| xray_key(fixture, crate::effect::xray::Cut::Background);
    let mut capture = crate::keyed::Capture::<u32>::default();
    capture.drawn(1, key(&mut fixture));
    commit_again(&mut fixture, &window);
    pump_all(&mut fixture);
    assert!(
        !capture.stale(&key(&mut fixture)),
        "a window's commit staled xray"
    );
    commit_layer_again(&mut fixture, &wallpaper);
    pump_all(&mut fixture);
    assert!(
        capture.stale(&key(&mut fixture)),
        "the wallpaper's commit did not stale xray"
    );
}

/// **xray is drawn at the monitor's scale times `xray.scale`**: the
/// mode itself at `effects.xray.scale = 1`, half of it each way at 0.5.
#[test]
fn the_xray_target_is_the_monitor_at_its_scale_times_xray_scale() {
    fx2_fixture!(fixture, "xray-size");
    let output = fixture
        .state
        .space
        .outputs()
        .next()
        .cloned()
        .expect("an output");
    let mode = output.current_mode().expect("a mode").size;
    assert_eq!(crate::offscreen::xray_size(&fixture.state, &output), mode);
    fixture.state.effect_settings.xray.scale = 0.5;
    assert_eq!(
        crate::offscreen::xray_size(&fixture.state, &output),
        (mode.w / 2, mode.h / 2).into()
    );
}

/// **The xray rasterises its scenes at the monitor's scale**: what
/// `offscreen::xray_job` hands `render::stacked_elements` asks a hosted
/// scene for the picture the frame draws, at the monitor's own scale,
/// and places it at the cut's, at `effects.xray.scale` 0.5 as at 1.
#[test]
fn the_xray_rasterises_its_scenes_at_the_monitors_scale() {
    fx2_fixture!(fixture, "xray-raster");
    let output = fixture
        .state
        .space
        .outputs()
        .next()
        .cloned()
        .expect("an output");
    let scale = output.current_scale().fractional_scale();
    assert_eq!(
        crate::offscreen::xray_raster(&fixture.state, &output),
        crate::surface::Raster::at(scale)
    );
    fixture.state.effect_settings.xray.scale = 0.5;
    let raster = crate::offscreen::xray_raster(&fixture.state, &output);
    assert_eq!(
        raster.scale, scale,
        "rasterised at the cut's scale, not the monitor's"
    );
    assert!(
        (raster.place - scale * 0.5).abs() < f64::EPSILON,
        "placed at {}, not the cut's scale",
        raster.place
    );
}

/// A window whose root surface declares no opaque region: what a T2
/// slot behind it is for. `one_window`'s, which sets none.
fn translucent_window(fixture: &mut Fixture) -> (crate::pane::PaneId, Window) {
    let opened = one_window(fixture);
    fixture_effects(&mut fixture.state);
    opened
}

/// The one chain the user's rules bound.
fn only_user_chain(state: &Solium) -> &crate::effect::plan::BoundChain {
    state
        .chains
        .first_for_test(crate::effect::rules::Origin::User)
        .expect("a user chain")
}

/// `sol.effects(<lua table>)` through the compositor's `Command::Effects`
/// arm, the engine's own keys beside the rules, as the shipped `init.lua`
/// hands `config.effects` over (until Task 48's `lua/effects.lua`).
fn apply_options(fixture: &mut Fixture, lua: &str) {
    super::apply_effects_lua(&mut fixture.state, lua);
    assert!(
        fixture.state.effects.problems().is_empty(),
        "{:?}",
        fixture.state.effects.problems()
    );
}

/// A second 1920×1080 monitor to the right of the fixture's.
fn two_monitors(fixture: &mut Fixture) {
    let _ = a_screen(&mut fixture.state, "fx2-right", (1920, 0));
}

/// The fixture's own monitor, at the origin.
fn first_monitor(fixture: &Fixture) -> String {
    fixture
        .state
        .output_at((0, 0).into())
        .map(|output| output.name())
        .expect("the fixture's monitor")
}

/// The window moved so it lies across the seam between the fixture's
/// two monitors.
fn straddle(fixture: &mut Fixture, window: &Window) {
    fixture
        .state
        .space
        .map_element(window.clone(), (1880, 100), false);
    fixture.state.space.refresh();
    pump_all(fixture);
}

/// The monitor the layout puts a pane on, by its centre.
fn pane_monitor(fixture: &Fixture, pane: crate::pane::PaneId) -> String {
    fixture
        .state
        .pane_outer_of(pane)
        .and_then(|outer| fixture.state.output_of(outer))
        .map(|output| output.name())
        .expect("the pane's monitor")
}

/// Lock the session through the client's `ext_session_lock_v1`, every
/// monitor then showing the lock, so the client is told `locked` and
/// may unlock, as `lock_focus`' tests do.
fn lock_session(fixture: &mut Fixture) -> ext_session_lock_v1::ExtSessionLockV1 {
    let locks = fixture
        .client
        .locks
        .clone()
        .expect("ext_session_lock_manager_v1 bound");
    let lock = locks.lock(&fixture.qh, ());
    pump_all(fixture);
    assert!(fixture.state.lock.is_some(), "the session did not lock");
    let outputs: Vec<Output> = fixture.state.space.outputs().cloned().collect();
    for output in outputs {
        let frame = fixture.state.lock_frame();
        fixture.state.frame_presented(&output, frame);
    }
    pump_all(fixture);
    lock
}

/// Unlock what [`lock_session`] locked.
fn unlock_session(fixture: &mut Fixture, lock: ext_session_lock_v1::ExtSessionLockV1) {
    lock.unlock_and_destroy();
    pump_all(fixture);
    assert!(fixture.state.lock.is_none(), "the session did not unlock");
}

/// One pass's resolve: the pass's warps planned first, as `Gpu::resolve`
/// plans them, then the slots, their boxes and places, and the T2 cuts
/// and memos wanted, in the store's next pass.
fn resolved(fixture: &mut Fixture) -> crate::effect::plan::Slots {
    let pass = fixture.state.store.next_pass();
    crate::render::plan_warps(&mut fixture.state);
    let mut slots = crate::render::build_slots(&mut fixture.state);
    crate::render::record_boxes(&fixture.state, &mut slots);
    crate::render::want_t2(&mut fixture.state, &mut slots, pass);
    slots
}

/// The `T2Key` `run_slots` would build for the slot's first place this
/// pass, through the pure `render::t2_key`, with no self capture.
fn t2_key_of(
    fixture: &mut Fixture,
    owner: crate::effect::plan::Owner,
    slot: crate::effect::rules::Slot,
) -> Option<crate::effect::memo::T2Key> {
    let slots = resolved(fixture);
    let place = slots.places(&owner, slot).first()?.clone();
    let part = place.part;
    let chain = fixture.state.chains.get(slots.key(&owner, slot)?)?;
    let work = crate::render::t2_work(chain.memo()?, slots.shared_on(&owner, slot, &place.monitor));
    Some(crate::render::t2_key(
        &fixture.state.memos,
        &fixture.state.xrays,
        &place,
        chain,
        &work,
        part.content,
        part.radii,
        None,
    ))
}

/// **A backdrop blur binds as T2, its memo split**: `{ "blur" }` behind
/// a client, no `source`, is no longer refused, and its whole plan is
/// the memo.
#[test]
fn a_backdrop_blur_binds_as_t2_with_its_memo_split() {
    fx2_fixture!(fixture, "t2-binds");
    fixture_effects(&mut fixture.state);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    assert!(
        fixture.state.effects.problems().is_empty(),
        "{:?}",
        fixture.state.effects.problems()
    );
    let chain = only_user_chain(&fixture.state);
    assert_eq!(chain.tier, crate::effect::rules::Tier::Xray);
    let memo = chain.memo().expect("split");
    assert!(memo.suffix.steps.is_empty());
    assert_eq!(memo.prefix.as_ref().map(|p| p.steps.len()), Some(6));
    assert_eq!(chain.memo_keys().len(), 1, "one export, one memo");
}

/// **xray is wanted only on a monitor a T2 slot reads**, and only its
/// cut.
#[test]
fn xray_is_wanted_only_on_a_monitor_a_t2_slot_reads() {
    fx2_fixture!(fixture, "t2-lazy");
    two_monitors(&mut fixture);
    let (_pane, _window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let _slots = resolved(&mut fixture);
    assert_eq!(
        fixture.state.xrays.wanted(),
        vec![(
            first_monitor(&fixture),
            crate::effect::xray::Cut::Background
        )]
    );
}

/// **The first frame waits for its xray** (spec C14): with a T2 rule
/// wanted and every program compiled, the effects are not ready until
/// every wanted cut has been built once; with none wanted they are ready
/// as the host is.
#[test]
fn the_first_frame_waits_for_its_xray() {
    fx2_fixture!(fixture, "xray-warmup");
    assert_eq!(
        fixture.state.effects_ready(),
        fixture.state.effects.ready(),
        "no rule, no xray to wait for"
    );
    translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    // Every program asked compiled, so the xray is all that is waited for.
    fixture
        .state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    assert!(
        fixture.state.effects.ready(),
        "the premise: nothing to compile"
    );
    let _slots = resolved(&mut fixture);
    assert!(
        !fixture.state.effects_ready(),
        "ready before its xray was built"
    );
    let monitor = first_monitor(&fixture);
    fixture
        .state
        .xrays
        .drawn(&monitor, crate::effect::xray::Cut::Background, true);
    assert!(
        fixture.state.effects_ready(),
        "not ready once its xray was built"
    );
    assert_eq!(
        fixture.state.first_frame_ready(),
        fixture.state.effects_ready(),
        "one gate, the effects one of its terms"
    );
}

/// **A held first frame asks for the next pass, and the cap lets it
/// through**: with every program compiled and a T2 slot's cut not
/// built, the end of `prepare` holds (so `elements` draws only the
/// clear colour) and asks for another pass, since nothing else may
/// damage the screen to release it; at `startup.first_frame.max_ms` it
/// is drawn whatever is late, asking for nothing. With no rule nothing
/// is held and nothing is asked (spec §8.4).
#[test]
fn a_held_first_frame_asks_for_the_next_pass() {
    fx2_fixture!(fixture, "warm-hold");
    translucent_window(&mut fixture);
    fixture.state.redraw = false;
    assert!(
        !crate::render::hold_first_frame(&mut fixture.state),
        "held with no rule"
    );
    assert!(!fixture.state.redraw, "a pass asked for with no rule");
    // A session starting afresh, its first pass not asked yet.
    fixture.state.warm = crate::warm::Warm::default();
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    // Every program asked compiled, so only the cut holds the frame.
    fixture
        .state
        .effects
        .compile_pending(&mut crate::effect::host::tests::Refusing);
    let _slots = resolved(&mut fixture);
    fixture.state.redraw = false;
    assert!(crate::render::hold_first_frame(&mut fixture.state));
    assert!(fixture.state.redraw, "a held pass asked for no next one");
    fixture.state.redraw = false;
    fixture
        .state
        .clock
        .advance(std::time::Duration::from_millis(
            crate::warm::FirstFrame::default().max_ms,
        ));
    assert!(
        !crate::render::hold_first_frame(&mut fixture.state),
        "held past max_ms"
    );
    assert!(!fixture.state.redraw);
}

/// **A held memo is in the memory report** (Ruling F30): memos are held
/// outside the pool's free-list budget, so `memory_report` counts them
/// itself, as `memos_kb`.
#[test]
fn a_held_memo_is_in_the_memory_report() {
    fx2_fixture!(fixture, "memo-memory");
    assert_eq!(fixture.state.memory().memos_kb, 0, "no memo held");
    translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let _slots = resolved(&mut fixture);
    let held = fixture.state.memos.held_bytes();
    assert!(held > 1024, "the premise: a monitor's memo is held");
    assert_eq!(fixture.state.memory().memos_kb, held / 1024);
}

/// **Two windows with one blur want one memo.**
#[test]
fn two_windows_with_one_blur_want_one_memo() {
    fx2_fixture!(fixture, "t2-shared");
    fixture_effects(&mut fixture.state);
    let (left, right) = side_by_side_windows(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let slots = resolved(&mut fixture);
    for pane in [left, right] {
        assert_eq!(
            slots
                .places(
                    &crate::effect::plan::Owner::Pane(pane, crate::effect::plan::PaneSlot::Client),
                    crate::effect::rules::Slot::Behind
                )
                .len(),
            1,
            "the premise: each window has its place"
        );
    }
    assert_eq!(fixture.state.memos.len(), 1);
    assert_eq!(slots.memos().len(), 1, "the xray phase runs it once");
}

/// **While the session is locked nothing of it is captured, run or
/// read**: a self blur (T1) and a backdrop blur (T2) want no slot, no
/// self capture and no xray, and both come back on unlock ([16] §5,
/// *Review log* finding 6). Task 47 adds an alpha-masked T2 rule and
/// Task 58 a region rule to the same rules, each with its own check.
#[test]
fn nothing_of_the_session_is_captured_while_locked() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "session-locked");
    let (pane, _window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{
        { match = "*", part = "client", slot = "replace", effect = { "blur", source = "self" } },
        { match = "*", part = "client", slot = "behind", effect = "blur" },
    }"#,
    );
    let lock = lock_session(&mut fixture);
    let mut slots = crate::render::build_slots(&mut fixture.state);
    assert!(
        slots.is_empty(),
        "a slot of the session is wanted behind the lock"
    );
    crate::render::record_boxes(&fixture.state, &mut slots);
    assert!(
        crate::render::self_inputs(&fixture.state, &slots).is_empty(),
        "a client captured behind the lock"
    );
    crate::render::want_t2(&mut fixture.state, &mut slots, 1);
    assert!(fixture.state.xrays.wanted().is_empty());
    unlock_session(&mut fixture, lock);
    let slots = crate::render::build_slots(&mut fixture.state);
    assert!(slots.wanted(&Owner::Pane(pane, PaneSlot::Client), Slot::Replace));
    assert!(slots.wanted(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind));
}

/// **A reload that changes `xray.scale` rebuilds the cut once and runs
/// each of its memos once**: the capture's key holds its size, so the
/// kept capture is stale (*Review log* finding 8).
#[test]
fn a_reload_changing_xray_scale_rebuilds_its_cut_and_reruns_its_memos_once() {
    fx2_fixture!(fixture, "xray-reload");
    let _wallpaper = layer_surface(
        &mut fixture,
        "wallpaper",
        zwlr_layer_shell_v1::Layer::Background,
    );
    translucent_window(&mut fixture);
    let (cut, monitor) = (
        crate::effect::xray::Cut::Background,
        first_monitor(&fixture),
    );
    let rules = r#"rules = { { match = "*", part = "client", slot = "behind", effect = "blur" } }"#;
    let wanted = |fixture: &mut Fixture| {
        let _slots = resolved(fixture);
        fixture
            .state
            .memos
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
    };
    apply_options(&mut fixture, &format!("{{ {rules} }}"));
    let before = wanted(&mut fixture);
    assert!(!before.is_empty(), "the premise: a memo is wanted");
    let mut capture = crate::keyed::Capture::<u32>::default();
    capture.drawn(1, xray_key(&mut fixture, cut));
    fixture.state.xrays.drawn(&monitor, cut, true);
    let generation = fixture.state.xrays.generation(&monitor, cut).unwrap_or(0);
    for key in &before {
        fixture.state.memos.ran(key, generation);
    }
    let held: Vec<_> = before.iter().cloned().collect();
    assert!(
        crate::render::memos_to_run(&fixture.state.memos, &fixture.state.xrays, &held).is_empty(),
        "the premise: every memo ran over this cut"
    );
    apply_options(
        &mut fixture,
        &format!("{{ xray = {{ scale = 0.5 }}, {rules} }}"),
    );
    assert!(
        capture.stale(&xray_key(&mut fixture, cut)),
        "a new scale kept the old capture"
    );
    fixture.state.xrays.drawn(&monitor, cut, true);
    let after = wanted(&mut fixture);
    assert_eq!(after, before, "the same prefixes, the same memos");
    let after: Vec<_> = after.into_iter().collect();
    assert_eq!(
        crate::render::memos_to_run(&fixture.state.memos, &fixture.state.xrays, &after).len(),
        after.len(),
        "each memo runs once more"
    );
}

/// **A window across two monitors has one T2 place on each**, and one,
/// on its own pane's monitor, with `effects.xray.straddle = "own"`
/// (amended 2026-10-08, configurable).
#[test]
fn a_window_across_two_monitors_has_a_t2_place_on_each() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "t2-straddle");
    two_monitors(&mut fixture);
    let (pane, window) = translucent_window(&mut fixture);
    straddle(&mut fixture, &window);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let slots = resolved(&mut fixture);
    let places = slots.places(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind);
    assert_eq!(places.len(), 2, "{places:?}");
    assert_ne!(places[0].monitor, places[1].monitor);
    assert!(
        places.iter().all(|place| place.own == places[0].own),
        "a suffix runs at the part's own size on either monitor"
    );
    fixture.state.effect_settings.xray.straddle = crate::effect::xray::Straddle::Own;
    let slots = resolved(&mut fixture);
    let places = slots.places(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind);
    assert_eq!(
        places
            .iter()
            .map(|place| place.monitor.clone())
            .collect::<Vec<_>>(),
        vec![pane_monitor(&fixture, pane)]
    );
}

/// **`effects.memo.split` reaches the bound chain, and `memo = false`
/// is never split** (amended 2026-10-08, configurable): under `"off"`
/// a backdrop blur has no prefix; a rule saying `memo = false` has none
/// whatever the configuration says.
#[test]
fn the_memo_split_key_reaches_the_bound_chain() {
    fx2_fixture!(fixture, "memo-split-key");
    fixture_effects(&mut fixture.state);
    let rules = r#"rules = { { match = "*", part = "client", slot = "behind", effect = "blur" } }"#;
    apply_options(
        &mut fixture,
        &format!("{{ memo = {{ split = \"off\" }}, {rules} }}"),
    );
    assert_eq!(
        only_user_chain(&fixture.state)
            .memo()
            .map(|memo| memo.prefix.is_none()),
        Some(true)
    );
    apply_options(&mut fixture, &format!("{{ {rules} }}"));
    assert_eq!(
        only_user_chain(&fixture.state)
            .memo()
            .and_then(|memo| memo.prefix.as_ref())
            .map(|p| p.steps.len()),
        Some(6)
    );
}

#[test]
fn a_rule_with_memo_false_is_never_split() {
    fx2_fixture!(fixture, "memo-false");
    fixture_effects(&mut fixture.state);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur", memo = false } }"#,
    );
    let chain = only_user_chain(&fixture.state);
    assert_eq!(chain.memo().map(|memo| memo.prefix.is_none()), Some(true));
    assert!(chain.memo_keys().is_empty());
}

/// **Over `effects.memo.budget_mib` no memo is held and each place runs
/// the whole plan** (amended 2026-10-08, configurable): two translucent
/// windows behind a blur on a 1 MiB budget want no memo, and both
/// places are unshared, so `run_slots` runs the unsplit plan for each.
#[test]
fn over_the_memo_budget_a_slot_runs_its_whole_plan() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "memo-budget");
    fixture_effects(&mut fixture.state);
    let (a, b) = side_by_side_windows(&mut fixture);
    apply_options(
        &mut fixture,
        r#"{ memo = { budget_mib = 1 }, rules = { { match = "*", part = "client", slot = "behind", effect = "blur" } } }"#,
    );
    let slots = resolved(&mut fixture);
    assert_eq!(
        fixture.state.memos.len(),
        0,
        "a memo was held over the budget"
    );
    let monitor = first_monitor(&fixture);
    let chain = only_user_chain(&fixture.state);
    for pane in [a, b] {
        let owner = Owner::Pane(pane, PaneSlot::Client);
        assert_eq!(slots.places(&owner, Slot::Behind).len(), 1, "the premise");
        let shared = slots.shared_on(&owner, Slot::Behind, &monitor);
        assert_eq!(
            crate::render::t2_work(chain.memo().expect("T2"), shared),
            crate::render::T2Work::Whole
        );
    }
    assert!(slots.memos().is_empty(), "the xray phase runs no memo");
}

/// **A rule may read the wallpaper alone** (amended 2026-10-08,
/// configurable): with `effects.xray.layers = { "background", "bottom" }`
/// a window's rule wants `Cut::Bottom` by default and `Cut::Background`
/// with `cut = "background"`.
#[test]
fn a_rule_may_read_the_wallpaper_alone() {
    fx2_fixture!(fixture, "rule-cut");
    translucent_window(&mut fixture);
    let monitor = first_monitor(&fixture);
    for (cut, expected) in [
        ("below", crate::effect::xray::Cut::Bottom),
        ("background", crate::effect::xray::Cut::Background),
    ] {
        apply_options(
            &mut fixture,
            &format!(
                r#"{{ xray = {{ layers = {{ "background", "bottom" }} }}, rules = {{ {{ match = "*", part = "client", slot = "behind", effect = "blur", cut = "{cut}" }} }} }}"#
            ),
        );
        let _slots = resolved(&mut fixture);
        assert_eq!(
            fixture.state.xrays.wanted(),
            vec![(monitor.clone(), expected)],
            "cut = {cut}"
        );
    }
}

/// **A T2 slot that reads no memo re-runs when its xray is redrawn**
/// (fix round 2026-10-08): with `memo = false` the suffix is the whole
/// plan and samples `backdrop` itself, so its key carries the cut's
/// generation; nothing redrawn, nothing re-runs.
#[test]
fn a_t2_slot_with_memo_false_reruns_when_its_xray_is_redrawn() {
    fx2_fixture!(fixture, "memo-false-xray");
    let (pane, _) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur", memo = false } }"#,
    );
    reruns_when_its_xray_is_redrawn(
        &mut fixture,
        crate::effect::plan::Owner::Pane(pane, crate::effect::plan::PaneSlot::Client),
    );
}

/// **Over `effects.memo.budget_mib` too** (`T2Work::Whole`, its `memo`
/// 0).
#[test]
fn a_t2_slot_over_the_memo_budget_reruns_when_its_xray_is_redrawn() {
    fx2_fixture!(fixture, "memo-budget-xray");
    let (pane, _) = translucent_window(&mut fixture);
    apply_options(
        &mut fixture,
        r#"{ memo = { budget_mib = 1 }, rules = { { match = "*", part = "client", slot = "behind", effect = "blur" } } }"#,
    );
    reruns_when_its_xray_is_redrawn(
        &mut fixture,
        crate::effect::plan::Owner::Pane(pane, crate::effect::plan::PaneSlot::Client),
    );
}

fn reruns_when_its_xray_is_redrawn(fixture: &mut Fixture, owner: crate::effect::plan::Owner) {
    use crate::effect::rules::Slot;
    let before = t2_key_of(fixture, owner.clone(), Slot::Behind).expect("a key");
    assert_eq!(before.memo, 0, "this slot reads no memo");
    let state = crate::effect::store::T2State {
        key: Some(before),
        ..crate::effect::store::T2State::default()
    };
    assert!(
        !state.needs_run(t2_key_of(fixture, owner.clone(), Slot::Behind).expect("a key")),
        "nothing moved and it re-ran"
    );
    for (monitor, cut) in fixture.state.xrays.wanted() {
        fixture.state.xrays.drawn(&monitor, cut, true);
    }
    assert!(
        state.needs_run(t2_key_of(fixture, owner, Slot::Behind).expect("a key")),
        "a redrawn wallpaper left the slot's backdrop stale"
    );
}

/// **A dragged window's T2 key moves, and its memo does not**: the
/// suffix (here, a crop's placement) follows the window and the memo,
/// keyed by the monitor, the cut and the prefix alone, stays the one
/// memo (Ruling F37).
#[test]
fn a_dragged_windows_t2_key_moves_and_its_memo_does_not() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "t2-drag");
    let (pane, window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    let before = t2_key_of(&mut fixture, owner.clone(), Slot::Behind).expect("a key");
    let held = |fixture: &Fixture| {
        let mut keys: Vec<_> = fixture.state.memos.keys().cloned().collect();
        keys.sort();
        keys
    };
    let memos = held(&fixture);
    assert_eq!(memos.len(), 1, "the premise: one memo");
    let at = fixture
        .state
        .space
        .element_location(&window)
        .expect("mapped");
    fixture
        .state
        .space
        .map_element(window.clone(), (at.x + 1, at.y), false);
    fixture.state.space.refresh();
    let after = t2_key_of(&mut fixture, owner, Slot::Behind).expect("a key");
    assert_ne!(
        after.padded, before.padded,
        "a drag left the key where it was"
    );
    assert_eq!(held(&fixture), memos, "a drag made another memo");
}

/// **A drag finer than an xray pixel moves the T2 key** (Task 42's
/// review, carried into Task 44): at `effects.xray.scale = 0.5` a 1 px
/// drag that leaves the place's box in xray pixels where it was still
/// moves the key, which holds the box in the monitor's own pixels
/// (`T2Place.screen`), so the suffix re-runs (Ruling F37).
#[test]
fn a_drag_finer_than_an_xray_pixel_moves_the_t2_key_at_half_scale() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "t2-fine-drag");
    let (pane, window) = translucent_window(&mut fixture);
    apply_options(
        &mut fixture,
        r#"{ xray = { scale = 0.5 }, rules = { { match = "*", part = "client", slot = "behind", effect = "blur" } } }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    let in_xray = |fixture: &mut Fixture| {
        resolved(fixture)
            .places(&owner, Slot::Behind)
            .first()
            .map(|place| place.padded)
            .expect("a place")
    };
    // Of two 1 px drags in a row at half scale, one crosses an xray
    // pixel and the other does not, whichever column it starts at.
    let mut finer = 0;
    for _ in 0..2 {
        let was = in_xray(&mut fixture);
        let before = t2_key_of(&mut fixture, owner.clone(), Slot::Behind).expect("a key");
        let at = fixture
            .state
            .space
            .element_location(&window)
            .expect("mapped");
        fixture
            .state
            .space
            .map_element(window.clone(), (at.x + 1, at.y), false);
        fixture.state.space.refresh();
        if in_xray(&mut fixture) != was {
            continue;
        }
        finer += 1;
        let after = t2_key_of(&mut fixture, owner.clone(), Slot::Behind).expect("a key");
        assert_ne!(
            after.padded, before.padded,
            "a drag finer than an xray pixel left the key where it was"
        );
    }
    assert_eq!(
        finer, 1,
        "the premise: one of two 1 px drags stays inside an xray pixel"
    );
}

/// `window` placed at `to`, gliding there for `duration` from now on a
/// straight line, as `tiling.apply` places a window (`Solium::place`,
/// the call [`glide`] makes).
fn glide_to(
    fixture: &mut Fixture,
    window: &Window,
    to: Rectangle<i32, Logical>,
    duration: Duration,
) {
    let start = fixture.state.clock.now();
    glide(
        &mut fixture.state,
        window,
        to,
        duration,
        present::Curve::Linear,
        start,
    );
    pump_all(fixture);
}

/// `window` moved by `by` at once, as one step of a drag's `sol.place`.
fn place_by(fixture: &mut Fixture, window: &Window, by: (i32, i32)) {
    let pane = fixture.state.panes.id_of(window).expect("a pane");
    let at = fixture.state.pane_outer_of(pane).expect("on screen");
    glide_to(
        fixture,
        window,
        Rectangle::new(at.loc + Point::from(by), at.size),
        Duration::ZERO,
    );
}

/// One pass's resolve and the slot's T2 places.
fn places_of(
    fixture: &mut Fixture,
    owner: crate::effect::plan::Owner,
    slot: crate::effect::rules::Slot,
) -> Vec<crate::effect::plan::T2Place> {
    resolved(fixture).places(&owner, slot).to_vec()
}

/// `{ effect = "genie", progress = 0.5, … }` toward a strip below the
/// pane: a genie held half way, as `effects-check genie` holds one.
fn genie_half(fixture: &Fixture, pane: crate::pane::PaneId) -> String {
    format!(
        r#"{{ effect = "genie", progress = 0.5, axis = "down", to = {} }}"#,
        below(fixture, pane)
    )
}

/// **A gliding window's T2 place follows where it is drawn**, pass by
/// pass, not the place it glides to (Ruling F37, amended 2026-10-14,
/// live under motion and warps: the maintainer's capture of a blur
/// showing its destination's backdrop half way through a relayout).
#[test]
fn a_gliding_windows_t2_place_follows_its_drawn_rectangle() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "glide-place");
    let (pane, window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    let from = fixture.state.pane_outer_of(pane).expect("on screen");
    let scale = fixture.state.scale_of(from);
    let rest = places_of(&mut fixture, owner.clone(), Slot::Behind)[0].screen;
    glide_to(
        &mut fixture,
        &window,
        Rectangle::new(from.loc + Point::from((600, 0)), from.size),
        Duration::from_millis(1000),
    );
    for step in 1..=4 {
        fixture.state.clock.advance(Duration::from_millis(200));
        let outer = fixture.state.pane_outer_of(pane).expect("on screen");
        let along = fixture.state.drawn(pane, outer).rect.loc.x - f64::from(from.loc.x);
        assert!(
            along > 1.0 && along < 599.0,
            "pass {step}: the glide is not under way, so nothing was tested"
        );
        let at = places_of(&mut fixture, owner.clone(), Slot::Behind)[0].screen;
        assert!(
            (f64::from(at.loc.x - rest.loc.x) - along * scale).abs() <= 1.0,
            "pass {step}: the place is {} px along where the window is drawn {along} logical px along",
            at.loc.x - rest.loc.x
        );
        assert_eq!(
            at.size, rest.size,
            "pass {step}: a glide from rest resized the place"
        );
    }
    fixture.state.clock.advance(Duration::from_millis(400));
    let landed = places_of(&mut fixture, owner, Slot::Behind)[0].screen;
    assert!(
        (f64::from(landed.loc.x - rest.loc.x) - 600.0 * scale).abs() <= 1.0,
        "at rest the place is where the layout put it"
    );
}

/// **A gliding window's T2 place is measured at the pass's one
/// instant**, the one the walk draws at, not at whatever the clock says
/// later in `prepare` (amended 2026-10-14, live under motion and warps:
/// a fast relayout's blur lagged its window by its speed times the
/// pass's own time). 600 px in 100 ms, 4 ms of `prepare` between the
/// warps' plan and the places: 24 px apart.
#[test]
fn a_gliding_windows_t2_place_is_measured_at_the_passs_instant() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "glide-instant");
    let (pane, window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    let from = fixture.state.pane_outer_of(pane).expect("on screen");
    let scale = fixture.state.scale_of(from);
    let rest = places_of(&mut fixture, owner.clone(), Slot::Behind)[0].screen;
    glide_to(
        &mut fixture,
        &window,
        Rectangle::new(from.loc + Point::from((600, 0)), from.size),
        Duration::from_millis(100),
    );
    fixture.state.clock.advance(Duration::from_millis(30));
    let _ = fixture.state.store.next_pass();
    crate::render::plan_warps(&mut fixture.state);
    let at = fixture.state.presented.at().expect("the pass's instant");
    fixture.state.clock.advance(Duration::from_millis(4));
    let mut slots = crate::render::build_slots(&mut fixture.state);
    crate::render::record_boxes(&fixture.state, &mut slots);
    let outer = fixture.state.pane_outer_of(pane).expect("on screen");
    let walked = fixture.state.drawn_id_at(pane, outer, at).rect.loc.x - f64::from(from.loc.x);
    let later = fixture.state.drawn(pane, outer).rect.loc.x - f64::from(from.loc.x);
    assert!(
        later - walked > 10.0,
        "the clock did not move the glide, so nothing was tested"
    );
    let placed = slots.places(&owner, Slot::Behind)[0].screen;
    assert!(
        (f64::from(placed.loc.x - rest.loc.x) - walked * scale).abs() <= 1.0,
        "the place is {} px along where the walk draws the window {walked} logical px along",
        placed.loc.x - rest.loc.x
    );
}

/// **A plain blur on a gliding window moves its crop and runs
/// nothing**: its key's box moves every pass, its memo and xray do not,
/// and its crop of the memo moves with the window (amended 2026-10-14).
#[test]
fn a_plain_blur_on_a_gliding_window_moves_its_crop_and_runs_nothing() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "glide-crop");
    let (pane, window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    assert!(matches!(
        crate::render::t2_work(only_user_chain(&fixture.state).memo().expect("T2"), true),
        crate::render::T2Work::Crop(_)
    ));
    let from = fixture.state.pane_outer_of(pane).expect("on screen");
    glide_to(
        &mut fixture,
        &window,
        Rectangle::new(from.loc + Point::from((600, 0)), from.size),
        Duration::from_millis(1000),
    );
    fixture.state.clock.advance(Duration::from_millis(300));
    let one = t2_key_of(&mut fixture, owner.clone(), Slot::Behind).expect("a key");
    let crop_one = places_of(&mut fixture, owner.clone(), Slot::Behind)[0].map;
    fixture.state.clock.advance(Duration::from_millis(300));
    let two = t2_key_of(&mut fixture, owner.clone(), Slot::Behind).expect("a key");
    let crop_two = places_of(&mut fixture, owner, Slot::Behind)[0].map;
    assert_ne!(
        one.padded, two.padded,
        "the box did not move with the glide"
    );
    assert_eq!(
        (one.memo, one.xray),
        (two.memo, two.xray),
        "a glide moved the memo or the xray"
    );
    assert!(
        crop_two.offset[0] > crop_one.offset[0],
        "the crop did not move with the window"
    );
}

/// **A suffix re-runs while its window glides, and not at rest**
/// (`blur-tint`: the blur is the memo, the tint the suffix; amended
/// 2026-10-14).
#[test]
fn a_suffix_reruns_while_gliding_and_not_at_rest() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "glide-suffix");
    let (pane, window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur-tint" } }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    assert_eq!(
        crate::render::t2_work(only_user_chain(&fixture.state).memo().expect("T2"), true),
        crate::render::T2Work::Suffix
    );
    let from = fixture.state.pane_outer_of(pane).expect("on screen");
    glide_to(
        &mut fixture,
        &window,
        Rectangle::new(from.loc + Point::from((600, 0)), from.size),
        Duration::from_millis(1000),
    );
    let mut t2 = crate::effect::store::T2State::default();
    for step in 1..=4 {
        fixture.state.clock.advance(Duration::from_millis(200));
        let key = t2_key_of(&mut fixture, owner.clone(), Slot::Behind).expect("a key");
        assert!(
            t2.needs_run(key),
            "pass {step}: the suffix did not re-run while its window glided"
        );
        t2.key = Some(key);
    }
    fixture.state.clock.advance(Duration::from_millis(400));
    t2.key = t2_key_of(&mut fixture, owner.clone(), Slot::Behind);
    fixture.state.clock.advance(Duration::from_millis(100));
    let still = t2_key_of(&mut fixture, owner, Slot::Behind).expect("a key");
    assert!(!t2.needs_run(still), "the suffix re-ran at rest");
}

/// **A dragged window re-runs its suffix and not its memo**: its box in
/// the monitor moved, the xray under it did not.
#[test]
fn a_dragged_windows_suffix_reruns_and_its_memo_does_not() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "drag-suffix");
    let (pane, window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur-tint" } }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    let before = t2_key_of(&mut fixture, owner.clone(), Slot::Behind).expect("a key");
    place_by(&mut fixture, &window, (40, 0));
    let after = t2_key_of(&mut fixture, owner, Slot::Behind).expect("a key");
    assert_ne!(
        after.padded, before.padded,
        "the suffix's key did not move with the window"
    );
    assert_eq!(after.memo, before.memo, "the memo moved with a window");
    assert_eq!(after.xray, before.xray, "the xray moved with a window");
}

/// **A self effect is kept while its window moves without drawing**,
/// and re-runs on every commit while it moves (R4: live content); the
/// self capture is the same for any `source = "self"` chain (amended
/// 2026-10-14).
#[test]
fn a_self_effect_is_kept_while_moved_and_reruns_on_every_commit() {
    fx2_fixture!(fixture, "drag-self");
    let (_pane, window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "replace", effect = { "blur", source = "self" } } }"#,
    );
    let mut capture = crate::keyed::Capture::<u32>::default();
    capture.drawn(1, self_key(&mut fixture, &window, 64));
    place_by(&mut fixture, &window, (40, 0));
    assert!(
        !capture.stale(&self_key(&mut fixture, &window, 64)),
        "a move without a commit recaptured"
    );
    for _ in 0..3 {
        commit_again(&mut fixture, &window);
        place_by(&mut fixture, &window, (10, 0));
        assert!(
            capture.stale(&self_key(&mut fixture, &window, 64)),
            "a commit while moving was not recaptured"
        );
        capture.drawn(1, self_key(&mut fixture, &window, 64));
    }
}

/// **A warped pane's behind slot is drawn beneath its warp, live**
/// (Ruling F37, amended 2026-10-14, live under motion and warps): held
/// half way through a genie, its client's behind slot is beneath, and
/// its place is the bounds of the client's cells of the genie's mesh,
/// reaching below the pane, not the pane at rest; its front slot stays
/// in the capture at rest; and with `effects.xray.warped = "rest"`
/// nothing is beneath and the place is at rest, the first draft's
/// behaviour ([16] decision 3).
#[test]
fn a_warped_panes_behind_slot_is_drawn_beneath_its_warp() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "warp-beneath");
    let (pane, _window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{
            { match = "*", part = "client", slot = "behind", effect = "blur" },
            { match = "*", part = "client", slot = "front", effect = "blur-tint" },
        }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    let flat_behind = places_of(&mut fixture, owner.clone(), Slot::Behind)[0].screen;
    let flat_front = places_of(&mut fixture, owner.clone(), Slot::Front)[0].screen;
    let genie = genie_half(&fixture, pane);
    present_deform(&mut fixture, pane, &genie);
    let slots = resolved(&mut fixture);
    assert!(
        matches!(
            fixture.state.presented.get(pane),
            Some(crate::render::Planned::Warp { .. })
        ),
        "the genie is not planned, so nothing was tested"
    );
    assert!(
        slots.is_beneath(&owner, Slot::Behind),
        "a genied pane's behind slot stayed in its capture"
    );
    assert!(
        !slots.is_beneath(&owner, Slot::Front),
        "a front slot left the capture"
    );
    let bent = slots.places(&owner, Slot::Behind)[0].screen;
    assert!(
        bent.loc.y + bent.size.h > flat_behind.loc.y + flat_behind.size.h,
        "the behind slot reads the backdrop where the pane rests, not down the funnel"
    );
    assert_eq!(
        slots.places(&owner, Slot::Front)[0].screen,
        flat_front,
        "a front slot left its rest"
    );
    fixture.state.effect_settings.xray.warped = crate::effect::xray::Warped::Rest;
    let slots = resolved(&mut fixture);
    assert!(
        !slots.is_beneath(&owner, Slot::Behind),
        "warped = rest drew the slot beneath"
    );
    assert_eq!(
        slots.places(&owner, Slot::Behind)[0].screen,
        flat_behind,
        "warped = rest moved the place"
    );
}

/// **Under `straddle = "own"` a warped slot across two monitors stays
/// in its capture**: its one place cannot feed the beneath draw on the
/// other monitor, where the capture's hole would show the screen below
/// (amended 2026-10-14, live under motion and warps).
#[test]
fn a_warped_slot_across_two_monitors_under_own_stays_in_its_capture() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "warp-own");
    two_monitors(&mut fixture);
    let (pane, window) = translucent_window(&mut fixture);
    straddle(&mut fixture, &window);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    let genie = genie_half(&fixture, pane);
    present_deform(&mut fixture, pane, &genie);
    assert!(
        resolved(&mut fixture).is_beneath(&owner, Slot::Behind),
        "under each the straddling genie is not beneath, so nothing was tested"
    );
    fixture.state.effect_settings.xray.straddle = crate::effect::xray::Straddle::Own;
    assert!(
        !resolved(&mut fixture).is_beneath(&owner, Slot::Behind),
        "under own a slot with one place was drawn beneath two monitors"
    );
}

/// **Two parts that cannot both hold their memo do not trade it while
/// their keys move every pass** (Task 42's re-review, carried into Task
/// 44): a window's client and its pane, each with its own backdrop blur
/// on one monitor and cut, under a budget that holds one memo; their
/// memo keys move every pass, as an animated prefix uniform moves them,
/// the lesser of the two flipping between them; the part admitted first
/// keeps the memo, since `want_t2` says it read it last pass.
#[test]
fn two_parts_under_a_one_memo_budget_do_not_trade_it_as_their_keys_move() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    const MIB: u64 = 1024 * 1024;
    fx2_fixture!(fixture, "memo-no-trade");
    let (pane, _window) = translucent_window(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" },
             { match = "*", part = "pane", slot = "behind", effect = "blur" } }"#,
    );
    let owners = [
        Owner::Pane(pane, PaneSlot::Client),
        Owner::Pane(pane, PaneSlot::Pane),
    ];
    let monitor = first_monitor(&fixture);
    let slots = resolved(&mut fixture);
    let rules: Vec<_> = owners
        .iter()
        .map(|owner| {
            slots
                .key(owner, Slot::Behind)
                .expect("the premise: each part's slot is wanted")
        })
        .collect();
    assert_ne!(rules[0], rules[1], "the premise: two rules");
    for owner in &owners {
        assert_eq!(
            slots
                .places(owner, Slot::Behind)
                .iter()
                .map(|place| place.monitor.clone())
                .collect::<Vec<_>>(),
            vec![monitor.clone()],
            "the premise: one place, on the one monitor"
        );
    }
    assert_eq!(
        fixture.state.memos.len(),
        1,
        "the premise: one blur is one memo, with no budget"
    );
    let one = fixture.state.memos.held_bytes();
    let budget = one.div_ceil(MIB);
    assert!(
        2 * one > budget * MIB,
        "the premise: {budget} MiB holds one memo of {one} bytes, not two"
    );
    fixture.state.effect_settings.memo.budget_mib = budget;
    let mut holder: Option<usize> = None;
    for pass in 0..1000_u64 {
        // The lesser key is the client's on even passes, the pane's on
        // odd ones, and no key comes back.
        let (low, high) = (2 * pass + 1, 2 * pass + 2);
        let keys = if pass % 2 == 0 {
            [low, high]
        } else {
            [high, low]
        };
        for (rule, key) in rules.iter().zip(keys) {
            fixture
                .state
                .chains
                .get_mut_for_test(*rule)
                .expect("bound")
                .set_memo_keys_for_test(vec![key]);
        }
        let slots = resolved(&mut fixture);
        let shared: Vec<usize> = owners
            .iter()
            .enumerate()
            .filter(|(_, owner)| slots.shared_on(owner, Slot::Behind, &monitor))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(shared.len(), 1, "pass {pass}: one memo is held");
        match holder {
            None => holder = Some(shared[0]),
            Some(was) => assert_eq!(shared[0], was, "pass {pass}: the memo changed hands"),
        }
    }
}

/// **A part that left a monitor gives its T2 result there back** as
/// `run_slots` runs it (`effect::store::SlotState::keep_t2`): a window
/// across two monitors has a T2 state on each; moved onto one, a pass
/// keeps only that monitor's.
#[test]
fn a_part_that_left_a_monitor_gives_its_t2_result_there_back() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "t2-left");
    two_monitors(&mut fixture);
    let (pane, window) = translucent_window(&mut fixture);
    straddle(&mut fixture, &window);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "blur" } }"#,
    );
    let owner = Owner::Pane(pane, PaneSlot::Client);
    let held_on = |fixture: &Fixture| -> Vec<String> {
        fixture
            .state
            .store
            .get(&owner, Slot::Behind)
            .map(|held| held.t2.keys().cloned().collect())
            .unwrap_or_default()
    };
    let _slots = resolved(&mut fixture);
    assert_eq!(
        held_on(&fixture).len(),
        2,
        "the premise: a T2 state on each monitor"
    );
    fixture
        .state
        .space
        .map_element(window.clone(), (100, 100), false);
    fixture.state.space.refresh();
    pump_all(&mut fixture);
    let mut slots = resolved(&mut fixture);
    assert_eq!(
        slots.places(&owner, Slot::Behind).len(),
        1,
        "the premise: the window is on one monitor"
    );
    let Solium {
        store,
        effects,
        chains,
        memos,
        xrays,
        ..
    } = &mut fixture.state;
    let mut cx = crate::render::RunCx {
        store,
        effects: &*effects,
        chains,
        masked: None,
        now: 0.0,
        pass: 2,
        memos,
        xrays: &*xrays,
        motion: (std::time::Duration::ZERO, 0.0),
        waited: false,
        drawing: crate::drawing::Drawing::every(),
    };
    let mut runner = crate::render::tests::NoGpu::default();
    for nest in [crate::render::Nest::Inner, crate::render::Nest::Whole] {
        crate::render::run_slots(
            &mut cx,
            &mut crate::pool::Pool::new(0),
            &mut runner,
            &mut slots,
            &crate::effect::store::Drawn::default(),
            nest,
        );
    }
    assert_eq!(
        held_on(&fixture),
        vec![first_monitor(&fixture)],
        "the monitor the window left kept its T2 result"
    );
}

/// **The self capture is padded by the effect's reach**: client pixels
/// plus twice the reach, each way, the client's corner at the reach.
#[test]
fn the_self_capture_is_padded_by_the_effects_reach() {
    use smithay::backend::renderer::test::DummyRenderer;
    fx2_fixture!(fixture, "self-padded");
    let (_pane, window) = one_window(&mut fixture);
    let source = crate::offscreen::PartSource::Client(&window);
    let size = crate::offscreen::part_size(&fixture.state, &source, 1.0, 6).expect("a size");
    let plain = crate::offscreen::client_pixels(
        crate::render::flat(&fixture.state, &window)
            .expect("flat")
            .client
            .size,
        1.0,
    );
    assert_eq!(size, (plain.w + 12, plain.h + 12).into());
    let elements = crate::offscreen::part_elements(&mut DummyRenderer, &source, 6, 1.0);
    let corner = elements.first().map(|element| {
        smithay::backend::renderer::element::Element::geometry(
            element,
            smithay::utils::Scale::from(1.0),
        )
        .loc
    });
    assert_eq!(
        corner,
        Some((6, 6).into()),
        "the client's corner is not at the pad"
    );
}

/// **Every wanted slot has its part's box, padded by its chain's reach**:
/// a client's, a scripted surface's on its monitor and a layer
/// surface's, each its own pixels plus twice the reach each way, the
/// part inside it; with no rule, none. A chain that draws further than
/// it reads is padded by its bleed instead, so a border drawn outside
/// its part has the room to be drawn in (#375).
#[test]
fn every_wanted_slot_has_its_part_box_padded_by_its_reach() {
    use crate::effect::plan::{Owner, PaneSlot, PartBox};
    use crate::effect::rules::Slot;
    use smithay::reexports::wayland_server::Resource as _;
    fx2_fixture!(fixture, "self-boxes");
    let (pane, window) = one_window(&mut fixture);
    with_tint_folder(&mut fixture);
    let _ = fixture
        .state
        .surfaces
        .declare(crate::scripted::Declaration::for_test(
            "dock",
            std::path::PathBuf::from("dock.qml"),
            crate::scripted::Layer::Top,
            crate::scripted::On::EveryMonitor,
        ));
    let _bar = bar(
        &mut fixture.display,
        &mut fixture.state,
        &fixture.conn,
        &fixture.client,
        &fixture.qh,
        30,
    );
    let mut none = crate::render::build_slots(&mut fixture.state);
    crate::render::record_boxes(&fixture.state, &mut none);
    assert!(
        none.boxed(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind)
            .is_none()
    );
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = { "tint", reach = 6 } },
             { match = "*", part = "client", slot = "front", effect = { "tint", bleed = 3 } },
             { match = "*", part = "surface:dock", slot = "behind", effect = { "tint", reach = 2.5 } },
             { match = "*", part = "layer_shell:restore-*", slot = "front", effect = "tint" } }"#,
    );
    let mut slots = crate::render::build_slots(&mut fixture.state);
    crate::render::record_boxes(&fixture.state, &mut slots);
    let client = crate::render::flat(&fixture.state, &window)
        .expect("flat")
        .client
        .size;
    assert_eq!(
        slots.boxed(&Owner::Pane(pane, PaneSlot::Client), Slot::Behind),
        Some(PartBox::around(
            crate::offscreen::client_pixels(client, 1.0),
            6,
            [0.0; 4]
        ))
    );
    assert_eq!(
        slots.boxed(&Owner::Pane(pane, PaneSlot::Client), Slot::Front),
        Some(PartBox::around(
            crate::offscreen::client_pixels(client, 1.0),
            3,
            [0.0; 4]
        )),
        "a bleed of 3 and no reach pads by 3"
    );
    let dock = fixture.state.surfaces.named("dock").expect("declared");
    assert_eq!(
        slots.boxed(
            &Owner::Surface(dock, "fx2-self-boxes".to_owned()),
            Slot::Behind
        ),
        Some(PartBox::around((1920, 1080).into(), 3, [0.0; 4])),
        "a reach of 2.5 pads by a whole pixel more"
    );
    let (layer, geometry) = fixture
        .state
        .space
        .outputs()
        .find_map(|output| {
            let map = smithay::desktop::layer_map_for_output(output);
            let layer = map.layers().next()?.clone();
            let geometry = map.layer_geometry(&layer)?;
            Some((layer, geometry))
        })
        .expect("the bar is mapped");
    assert_eq!(
        slots.boxed(&Owner::LayerShell(layer.wl_surface().id()), Slot::Front),
        Some(PartBox::around(
            crate::offscreen::client_pixels(geometry.size, 1.0),
            0,
            [0.0; 4]
        ))
    );
}

/// **Only a chain reading its part asks for a capture**: a T1 chain
/// (`tint` reads `self`) does, a T0 one (`ring` reads only `shape`)
/// does not, wherever it is.
#[test]
fn only_a_chain_reading_its_part_asks_for_a_capture() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "self-only-t1");
    let (pane, _window) = one_window(&mut fixture);
    with_tint_folder(&mut fixture);
    let place = fixture.scratch.clone().expect("the tint folder's place");
    crate::effect::host::tests::folder(
        &place,
        "ring",
        "return { api = 1, inputs = { 'shape' }, frag = 'effect.frag' }",
        &[(
            "effect.frag",
            "vec4 sol_effect(vec2 uv) { return vec4(sol_shape(uv)); }\n",
        )],
    );
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "tint" },
             { match = "*", part = "client", slot = "front", effect = "ring" },
             { match = "*", part = "pane", slot = "behind", effect = "ring" } }"#,
    );
    let slots = crate::render::build_slots(&mut fixture.state);
    assert_eq!(slots.wants().count(), 3, "the premise: three slots wanted");
    let asked: Vec<_> = crate::render::self_inputs(&fixture.state, &slots)
        .into_iter()
        .map(|input| (input.owner, input.slot))
        .collect();
    assert_eq!(asked, [(Owner::Pane(pane, PaneSlot::Client), Slot::Behind)]);
}

/// **A pane no monitor shows is resolved for nothing**, the cull
/// `prepare` makes: a window parked a screen away, as a hidden
/// workspace's are, gathers no fact and wants no slot though a rule
/// matches it, while the window on screen does.
#[test]
fn a_pane_no_monitor_shows_gathers_no_fact_and_wants_no_slot() {
    use crate::effect::plan::{Owner, PaneSlot};
    use crate::effect::rules::Slot;
    fx2_fixture!(fixture, "rule-offscreen");
    let (shown, _) = one_window(&mut fixture);
    let parked = open_kept(&mut fixture);
    fixture
        .state
        .map_stacked(parked.clone(), (-5000, 300), false);
    fixture.state.sync_panes();
    let parked = fixture.state.panes.id_of(&parked).expect("a pane");
    with_tint_folder(&mut fixture);
    apply_rules(
        &mut fixture,
        r#"{ { match = "*", part = "client", slot = "behind", effect = "tint" } }"#,
    );
    let slots = crate::render::build_slots(&mut fixture.state);
    assert!(slots.wanted(&Owner::Pane(shown, PaneSlot::Client), Slot::Behind));
    assert!(!slots.wanted(&Owner::Pane(parked, PaneSlot::Client), Slot::Behind));
    assert_eq!(
        slots.facts_gathered(),
        1,
        "the parked window's facts were gathered"
    );
}

/// **#133 review, finding 8: a menu past its parent's tile takes the
/// press.**
///
/// A popup is drawn uncut, reaching past its parent's tile, and since
/// #133 the parent's hit-test rectangle stops at the tile. So a menu
/// opened near the right edge of an oversized client drew items over
/// the neighbouring tile that could not be pointed at: `surface_under`
/// gated every pane on its own frame, the point fell through to the
/// neighbour, and a press there dismisses the menu's grab instead of
/// choosing the item. The window with the menu is raised, as a click
/// that opened one leaves it.
#[test]
fn a_menu_past_its_parents_tile_takes_the_press() {
    use solium_layout::tree::Tiling;

    tiled_fixture!(display, state, conn, queue, client, qh);
    let (left, _left_toplevel, left_surface, left_xdg) =
        open_xdg(&mut display, &mut state, &conn, &client, &qh);
    let (right, _right_toplevel, right_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));
    let (area, settings) = (tiled_area(), tiled_settings());
    let mut tiling = Tiling::new();
    tiling.insert(left_id, None, None, area, settings);
    tiling.insert(right_id, Some(left_id), None, area, settings);
    sweep(&mut state, &tiling);
    let pane = state.panes.id_of(&left).expect("the pane is here");
    let tile = state
        .panes
        .get(pane)
        .and_then(Pane::placed)
        .expect("the pane is in a tile");
    let beside = state
        .panes
        .id_of(&right)
        .and_then(|pane| state.panes.get(pane))
        .and_then(Pane::placed)
        .expect("the neighbour is in a tile");
    commit_buffer(&client, &qh, &right_surface, beside.size.w, beside.size.h);
    commit_buffer(&client, &qh, &left_surface, tile.size.w + 200, tile.size.h);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let _popup = drawn_popup(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
        &left_xdg,
        (tile.size.w - 20, 50),
        (120, 80),
    );
    state.space.raise_element(&left, false);
    a_frame(&mut state);

    let point: Point<f64, Logical> = (
        f64::from(tile.loc.x + tile.size.w + 40),
        f64::from(tile.loc.y + 90),
    )
        .into();
    assert!(
        beside.to_f64().contains(point),
        "the point is over the neighbour's tile, where the menu reaches"
    );
    let menu = left.toplevel().and_then(|toplevel| {
        smithay::desktop::PopupManager::popups_for_surface(toplevel.wl_surface())
            .next()
            .map(|(popup, _)| popup.wl_surface().clone())
    });
    assert!(menu.is_some(), "the menu is tracked on its parent");
    assert_eq!(
        state.surface_under(point).map(|(surface, _)| surface),
        menu,
        "the menu has the pixel it drew, not the window under it"
    );
}

/// **A script's thumbnail and a script's open are pictures of the
/// whole window**, for a tiled one as for any other.
///
/// `Solium::apply` gives the frames `sol.present` and
/// `sol.present_from` build a zoom of their rectangle over the
/// window's own. That zoom is what tells `render::fit` the rectangle is
/// the window made smaller, so a tiled window that fits its tile is
/// scaled into an overview slot or an open's first frame whole, as it
/// was before #133 -- where a frame at zoom 1.0 that small would read
/// as a tile that narrow, and the window would be cut to its top-left
/// corner instead.
#[test]
fn a_scripts_thumbnail_and_open_are_pictures_of_the_whole_window() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _surface, pane, tile) = filled_tile(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let id = state.window_id(&window);
    let instant = AnimationSpec {
        duration: Duration::ZERO,
        ..AnimationSpec::default()
    };
    let committed = window.geometry().size;
    let thumbnail = Rect {
        x: f64::from(tile.loc.x) + 40.0,
        y: f64::from(tile.loc.y) + 30.0,
        w: f64::from(tile.size.w) / 4.0,
        h: f64::from(tile.size.h) / 4.0,
    };
    let placed_now = |state: &Solium| {
        let held = state.panes.get(pane).expect("the pane is here");
        let outer = state.pane_outer(held);
        let frame = state.drawn(pane, outer);
        (
            frame,
            crate::render::place_client(state, held, &frame, outer.size, committed),
        )
    };

    state.apply(Outcome {
        commands: vec![Command::Present {
            id,
            rect: Some(thumbnail),
            opacity: None,
            client_opacity: None,
            matrix: None,
            deform: None,
            z: 0.0,
            pivot: (0.5, 0.5),
            animation: instant,
            channels: None,
        }],
        ..Outcome::default()
    });
    let (frame, drawn) = placed_now(&state);
    assert_eq!(frame.zoom, (0.25, 0.25), "a quarter of the window");
    assert_eq!(
        (drawn.fit.factor, drawn.fit.crop),
        (smithay::utils::Scale::from((0.25, 0.25)), None),
        "the whole window at a quarter, with nothing cut off it"
    );

    state.apply(Outcome {
        commands: vec![Command::Clear {
            id,
            animation: instant,
        }],
        ..Outcome::default()
    });
    a_frame(&mut state);
    let shrunk = Rect {
        x: f64::from(tile.loc.x) + f64::from(tile.size.w) * 0.06,
        y: f64::from(tile.loc.y) + f64::from(tile.size.h) * 0.06,
        w: f64::from(tile.size.w) * 0.88,
        h: f64::from(tile.size.h) * 0.88,
    };
    let start = state.clock.now();
    state.apply(Outcome {
        commands: vec![Command::PresentFrom {
            id,
            rect: shrunk,
            opacity: Some(0.0),
            client_opacity: None,
            animation: AnimationSpec {
                duration: Duration::from_secs(10),
                easing: present::Curve::Linear,
            },
            channels: None,
            deform: None,
        }],
        ..Outcome::default()
    });
    let held = state.panes.get(pane).expect("the pane is here");
    let outer = state.pane_outer(held);
    let first = state.drawn_at(held, outer, start);
    let opened = crate::render::place_client(&state, held, &first, outer.size, committed);
    assert!(
        (first.zoom.0 - 0.88).abs() < 1e-9 && (first.zoom.1 - 0.88).abs() < 1e-9,
        "an open's first frame is the window at 0.88: {:?}",
        first.zoom
    );
    assert!(
        (opened.fit.factor.x - 0.88).abs() < 1e-9 && opened.fit.crop.is_none(),
        "the whole window at 0.88, with nothing cut off it: {:?}",
        opened.fit
    );
}

/// **A press on a window lands where its client is drawn, frame and
/// all.**
///
/// `elements` places the client under the frame's share from
/// `insets_of`, which answers for a `Frame::Pending` pane -- one still
/// reserving the titlebar a frame is on its way to fill. The hit test
/// took its inset from `frame_insets`, which answers nothing for that
/// pane because it is not decorated yet, so a press there reached the
/// client a titlebar's height above the pixel under it. Both read the
/// one `render::place_client` now.
#[test]
fn a_press_on_a_window_reserving_a_titlebar_lands_on_the_pixel_under_it() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state.panes.id_of(&window).expect("the pane is here");
    state
        .panes
        .get_mut(pane)
        .expect("the pane is here")
        .set_frame(crate::pane::Frame::Pending);
    commit_buffer(&client, &qh, &surface, 400, 300);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    // Past the open animation its first commit started, which begins
    // at opacity zero -- and a pane nobody can see takes no press.
    state.clock.advance(Duration::from_secs(1));
    a_frame(&mut state);
    let outer = state.pane_outer_of(pane).expect("the pane is here");
    assert_eq!(
        outer,
        at(400, 300 - TITLEBAR_HEIGHT, 400, 300 + TITLEBAR_HEIGHT),
        "the client at 400,300 with a titlebar's worth reserved above it"
    );

    let point: Point<f64, Logical> = (450.0, 310.0).into();
    let (_, origin) = state
        .surface_under(point)
        .expect("the client is under the point");
    let within = point - origin;
    assert!(
        (within.x - 50.0).abs() < 0.5 && (within.y - 10.0).abs() < 0.5,
        "ten pixels into the client, as drawn, and was told {within:?}"
    );
}

/// **A press on a window whose drag is still being answered lands on
/// the pixel its picture has there.**
///
/// Under a resize hold the client's last buffer is stretched into the
/// rectangle the drag has reached, and the hold outlives the gesture
/// by up to `resizing::PATIENCE`. The hit test mapped the point 1:1
/// against the dragged rectangle, so a press on the stretched picture
/// reached the client somewhere else -- here, a buffer 400 wide drawn
/// 200 wide, and a press 150 pixels in, which is pixel 300 of the
/// picture and was delivered as pixel 150. It inverts the same fit the
/// picture is drawn with now.
#[test]
fn a_press_on_a_held_window_lands_on_the_pixel_its_picture_has() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state.panes.id_of(&window).expect("the pane is here");
    commit_buffer(&client, &qh, &surface, 400, 300);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    state.sync_panes();

    let outer = at(400, 300, 200, 150);
    let request = ResizeRequest {
        window: window.clone(),
        wanted: outer,
        edge_at: (600.0, 450.0),
        edges: ResizeEdge::BottomRight,
    };
    // A stretch, which is the fill whose picture and rectangle differ
    // in scale and so the one a 1:1 mapping got wrong. Not the default
    // since #233.
    state.resizing.fill = crate::resizing::Fill::Stretch;
    state.begin_resize(&window);
    tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
    assert!(state.holding_resize(pane), "the drag is holding the pane");
    assert_eq!(
        state.resize_fill(pane),
        crate::resizing::Fill::Stretch,
        "and stretching the last buffer into the dragged rectangle"
    );

    let point: Point<f64, Logical> = (550.0, 400.0).into();
    let (_, origin) = state
        .surface_under(point)
        .expect("the window is under the point");
    let within = point - origin;
    assert!(
        (within.x - 300.0).abs() < 0.5 && (within.y - 200.0).abs() < 0.5,
        "the picture has buffer pixel 300,200 there, and the client was \
         told {within:?}"
    );
}

/// **#233: a press on what an anchored picture cuts away is not the
/// client's.**
///
/// A window under a titlebar has its top edge dragged down by 100. Its
/// client has not answered, so under `anchor` its 300-tall
/// picture stays against the bottom edge, which is standing still, and
/// the top 100 of it is cut away -- under the titlebar that came down
/// with the edge. The hit test inverted the fit without its cut, so a
/// point on the titlebar landed in that hidden band and the client was
/// handed the pointer over pixels it is not showing. Below the cut the
/// press still lands on the pixel the picture has there.
#[test]
fn a_press_on_what_an_anchored_picture_cuts_away_is_not_the_clients() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    state.resizing.fill = crate::resizing::Fill::Anchor;
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state.panes.id_of(&window).expect("the pane is here");
    state
        .panes
        .get_mut(pane)
        .expect("the pane is here")
        .set_frame(crate::pane::Frame::Pending);
    commit_buffer(&client, &qh, &surface, 400, 300);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    state.clock.advance(Duration::from_secs(1));
    a_frame(&mut state);
    let toplevel = window
        .toplevel()
        .map(|toplevel| toplevel.wl_surface().clone());

    let outer = at(400, 400 - TITLEBAR_HEIGHT, 400, 200 + TITLEBAR_HEIGHT);
    let request = ResizeRequest {
        window: window.clone(),
        wanted: outer,
        edge_at: (500.0, f64::from(400 - TITLEBAR_HEIGHT)),
        edges: ResizeEdge::Top,
    };
    state.begin_resize(&window);
    let now = state.clock.now();
    tiled_frame(&mut state, &request, pane, outer, now);
    assert_eq!(
        state.resize_fill(pane),
        crate::resizing::Fill::Anchor,
        "the premise: the anchored fill"
    );
    assert_eq!(
        state.resize_pins(pane),
        (false, true),
        "the top edge moves and the bottom stands still"
    );

    let titlebar: Point<f64, Logical> = (450.0, f64::from(400 - TITLEBAR_HEIGHT / 2)).into();
    assert!(
        state.drawn(pane, outer).covers(titlebar),
        "the point is on the window's titlebar"
    );
    assert_ne!(
        state.surface_under(titlebar).map(|(surface, _)| surface),
        toplevel,
        "the band of the picture under the titlebar is cut away, and a \
         press there is not the client's"
    );

    let point: Point<f64, Logical> = (450.0, 450.0).into();
    let (under, origin) = state
        .surface_under(point)
        .expect("the window is under the point");
    assert_eq!(Some(under), toplevel);
    let within = point - origin;
    assert!(
        (within.x - 50.0).abs() < 0.5 && (within.y - 150.0).abs() < 0.5,
        "50 below the cut is buffer row 150 of the picture held against \
         the bottom edge, and the client was told {within:?}"
    );
}

/// The work area every tiled-tree fixture in this module lays out over.
fn tiled_area() -> solium_layout::Rect {
    solium_layout::Rect::new(0.0, 0.0, 1000.0, 600.0)
}

/// The shipped gap, because at `gap: 0` the two sides of a seam are one
/// line and half the arithmetic under test disappears.
fn tiled_settings() -> solium_layout::Settings {
    solium_layout::Settings {
        gap: 12.0,
        split: 0.5,
        ..solium_layout::Settings::default()
    }
}

/// Where a tree has put one window. `rect_at` in the layout crate's own
/// suite, which is not public.
fn leaf_of(tiling: &solium_layout::tree::Tiling, id: u64) -> solium_layout::Rect {
    tiling
        .layout(tiled_area(), tiled_settings())
        .into_iter()
        .find(|(other, _)| *other == id)
        .expect("the window is in the tree")
        .1
}

/// One layout sweep, which is what `tiling.apply` is in Lua: every leaf
/// placed, every frame, whether it moved or not.
///
/// `sol.place` is `Solium::place`, and a script's rect is the pane's
/// *outer* rectangle — `place` subtracts the insets itself.
fn sweep(state: &mut Solium, tiling: &solium_layout::tree::Tiling) {
    for (id, rect) in tiling.layout(tiled_area(), tiled_settings()) {
        state.place(
            id,
            Rect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
            },
            AnimationSpec {
                duration: Duration::ZERO,
                ..AnimationSpec::default()
            },
            Duration::ZERO,
            Standing::Tile,
        );
    }
}

/// **A tiled drag measures from the grab, not from the last frame.**
///
/// `ResizeGrab` freezes two rectangles at the press and this pins why
/// the second of them has to be one of them. `laid_out` is where the
/// layout had this pane when the button went down, and the layout moves
/// the pane on *every frame of the drag* — that is what a seam moving
/// means. So a version that asked `Solium::pane_laid_out` afresh each
/// frame and added the gesture's total travel to whatever came back
/// would add the previous frame's travel a second time, and the frame
/// before that a third. Not drift: the whole of the motion, compounding,
/// for as long as the button is held.
///
/// It is an easy thing to write, because "ask the layout where the pane
/// is now" reads like the honest version. The first half below is the
/// fixture earning its keep — the compositor really is moving this pane
/// under the drag — and the second is the runaway, computed by doing
/// exactly that to a second copy of the same tree.
///
/// A *position* built from a frozen base and a total delta, which is
/// what `ResizeRequest` promises and what keeps `drag_seam` idempotent.
/// Both halves of that matter and neither implies the other: the base
/// is frozen so the drag does not read its own output back, and the
/// delta is total rather than per-frame so it does not accumulate
/// rounding. See `crate::input::resize::dragged_edge`.
#[test]
fn a_tiled_drag_measures_from_where_the_grab_began_and_not_from_the_last_frame() {
    use solium_layout::tree::{Edge, Tiling};

    tiled_fixture!(display, state, conn, queue, client, qh);
    let (left, _left_toplevel, _left_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (right, _right_toplevel, _right_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.sync_panes();
    let (left_id, right_id) = (state.window_id(&left), state.window_id(&right));

    let (area, settings) = (tiled_area(), tiled_settings());
    let mut tiling = Tiling::new();
    tiling.insert(left_id, None, None, area, settings);
    tiling.insert(right_id, Some(left_id), None, area, settings);
    sweep(&mut state, &tiling);

    let began = leaf_of(&tiling, left_id);
    let frozen = state
        .pane_laid_out(&left)
        .expect("a pane a layout has placed");
    let from: Point<f64, Logical> = (f64::from(frozen.0.loc.x + frozen.0.size.w), 300.0).into();

    // Three frames of one gesture. The numbers are the pointer's total
    // travel from the press, which is what a grab has -- never the step
    // since the last frame.
    let mut the_layout_moved_it = false;
    for total in [10.0, 40.0, 90.0] {
        let now: Point<f64, Logical> = (from.x + total, from.y).into();
        let sent = crate::input::resize::dragged_edge(frozen, ResizeEdge::Right, from, now);
        tiling.drag_seam(left_id, Edge::Right, sent, area, settings);
        sweep(&mut state, &tiling);
        // A whole frame, configures and all. The client is never made
        // to answer here -- what this test is about happens whether it
        // does or not -- but a sweep that never reached the wire would
        // be a different code path from the one a drag takes.
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.sync_panes();
        the_layout_moved_it |= state.pane_laid_out(&left) != Some(frozen);
    }
    assert!(
        the_layout_moved_it,
        "the layout has to actually move this pane under the drag, or \
         freezing its rectangle costs nothing and this test pins nothing"
    );

    let ended = leaf_of(&tiling, left_id);
    assert!(
        (ended.w - (began.w + 90.0)).abs() < 0.5,
        "the pointer travelled 90 from the press, so the edge is 90 \
         from where it was: {began:?} became {ended:?}"
    );

    // And the same three frames against a base re-read each time, which
    // is the mistake this is here to keep out.
    let mut runaway = Tiling::new();
    runaway.insert(left_id, None, None, area, settings);
    runaway.insert(right_id, Some(left_id), None, area, settings);
    for total in [10.0, 40.0, 90.0] {
        let live = leaf_of(&runaway, left_id);
        runaway.drag_seam(
            left_id,
            Edge::Right,
            (live.x + live.w + total, 300.0),
            area,
            settings,
        );
    }
    let flew = leaf_of(&runaway, left_id);
    assert!(
        flew.w - began.w > 130.0,
        "re-reading the layout's rectangle each frame adds every \
         earlier frame's travel again -- 140 rather than 90 here, and \
         unbounded on a real gesture: {began:?} became {flew:?}"
    );
}

/// **A drag tells its client on every frame it moves, by default.**
///
/// The same six frames as the capped test below, with no
/// `resize.tell_hz` configured: six offers, six configures. Told a tenth of
/// a second apart, every client measured on 2026-10-16 -- kitty, Dolphin,
/// a GTK4 window and Firefox -- answered within 13 ms and still trailed
/// the pointer by 150 px and more, because the interval was the lag.
#[test]
fn a_tiled_drag_configures_on_every_frame_it_moves_by_default() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);
    for (millis, width) in [
        (0, 300),
        (16, 302),
        (32, 304),
        (48, 306),
        (64, 308),
        (80, 310),
    ] {
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, width, 200),
            Duration::from_millis(millis),
        );
    }
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.configures.len(),
        6,
        "six frames of a drag that moved each time are six configures when \
         nothing caps them; got {:?}",
        client.configures
    );
}

/// **Issue #123, the rate half: a tiled drag configured its client once
/// per frame.**
///
/// The throttle — a capped `resize.tell_hz` since every frame became the
/// default — is consulted by `Hold::dragged` and by nothing else, and
/// until now a tiled pane had no hold — so `move_pane`
/// sent a configure on every frame of every drag, which is sixty a
/// second. `resizing.rs` says in its own words why that fails: a client
/// that cannot render at that rate does not try, it falls behind, and
/// the window stutters against the pointer rather than following it.
///
/// **Every frame asks for a different width**, deliberately. Smithay
/// deduplicates a configure that repeats the size it last sent, so a
/// test that dragged a window to the same place twice would be green
/// against a compositor with no throttle at all.
#[test]
fn a_tiled_drag_configures_on_the_throttle_and_not_once_per_frame() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    // A capped `resize.tell_hz`: what this test is about is a throttle's
    // own behaviour, and since 2026-10-16 a drag is told every frame unless
    // one is configured.
    state.resizing.tell = Duration::from_millis(100);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");

    // Opening a window configures it, and that configure is not this
    // test's subject.
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);

    // Six frames at sixteen milliseconds, which is what a drag at sixty
    // hertz is and is entirely inside one hundred-millisecond interval.
    for (millis, width) in [
        (0, 300),
        (16, 302),
        (32, 304),
        (48, 306),
        (64, 308),
        (80, 310),
    ] {
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, width, 200),
            Duration::from_millis(millis),
        );
    }
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.configures.len(),
        1,
        "six frames of a claimed drag inside one capped interval must be \
         one configure, not six: the first offer goes out on the frame \
         it is decided and the rest wait. Got {:?}",
        client.configures
    );

    // And past the interval the client hears again, because the throttle
    // is a rate and not a gate: a drag that went on for a second with a
    // single configure in it would end a long way from the pointer.
    tiled_frame(
        &mut state,
        &request,
        pane,
        at(400, 300, 340, 200),
        Duration::from_millis(120),
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.configures.len(),
        2,
        "the interval has passed, so the next frame's rectangle is sent"
    );
    assert_eq!(
        client.configures.last().map(|&(_, w, h)| (w, h)),
        Some((340, 200)),
        "and what is sent is the rectangle of the frame that sent it, \
         not a stale one the throttle had been sitting on"
    );
}

/// Two windows side by side, 400x300 each, the seam between them at
/// x = 400: what every round test drags.
struct Seam {
    display: Display<Solium>,
    state: Solium,
    conn: Connection,
    queue: wayland_client::EventQueue<Client>,
    client: Client,
    qh: QueueHandle<Client>,
    left: (
        Window,
        xdg_toplevel::XdgToplevel,
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
    ),
    right: (
        Window,
        xdg_toplevel::XdgToplevel,
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
    ),
    panes: (crate::pane::PaneId, crate::pane::PaneId),
}

impl Seam {
    fn new() -> Self {
        let mut display = Display::<Solium>::new().expect("creating a test wayland display");
        let mut state = Solium::new(display.handle());
        state
            .decorations
            .set_style(&mut state.panes, Some("none".to_string()));
        let (conn, mut queue, mut client) = connect(&mut display, &mut state);
        let qh = queue.handle();
        let left = open_xdg(&mut display, &mut state, &conn, &client, &qh);
        let right = open_xdg(&mut display, &mut state, &conn, &client, &qh);
        commit_buffer(&client, &qh, &left.2, 400, 300);
        commit_buffer(&client, &qh, &right.2, 400, 300);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.map_stacked(left.0.clone(), (0, 0), false);
        state.map_stacked(right.0.clone(), (400, 0), false);
        state.sync_panes();
        let panes = (
            state
                .panes
                .id_of(&left.0)
                .expect("the left window has a pane"),
            state
                .panes
                .id_of(&right.0)
                .expect("the right window has a pane"),
        );
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();
        Self {
            display,
            state,
            conn,
            queue,
            client,
            qh,
            left,
            right,
            panes,
        }
    }

    fn pump(&mut self) {
        pump(
            &mut self.display,
            &mut self.state,
            &self.conn,
            &self.qh,
            &mut self.queue,
            &mut self.client,
        );
    }

    /// The drag on the left window's right edge.
    fn request(&self) -> ResizeRequest {
        ResizeRequest {
            window: self.left.0.clone(),
            wanted: at(0, 0, 400, 300),
            edge_at: (400.0, 300.0),
            edges: ResizeEdge::Right,
        }
    }

    /// A frame of the drag that put the seam at `x`: both panes moved
    /// by the layout, then the rest of `settle_resize`.
    fn seam_at(&mut self, x: i32, now: Duration) {
        let request = self.request();
        self.state.arm_resize_gesture(&request);
        for (pane, outer) in [
            (self.panes.0, at(0, 0, x, 300)),
            (self.panes.1, at(x, 0, 800 - x, 300)),
        ] {
            let was = self
                .state
                .pane_outer_of(pane)
                .expect("a mapped pane has a rectangle");
            self.state.move_pane(
                pane,
                outer,
                was,
                AnimationSpec {
                    duration: Duration::ZERO,
                    ..AnimationSpec::default()
                },
                now,
                Standing::Tile,
            );
        }
        self.state.resize_gesture = None;
        self.state.drop_resize_hold_for(&request.window);
        paused_frame(&mut self.state, now);
    }

    /// A frame with no motion in it.
    fn frame(&mut self, now: Duration) {
        paused_frame(&mut self.state, now);
    }

    /// One side's client acknowledging the last configure it was sent
    /// and drawing at `width`.
    fn answer(&mut self, right: bool, width: i32) {
        let (xdg, surface) = if right {
            (self.right.3.clone(), self.right.2.clone())
        } else {
            (self.left.3.clone(), self.left.2.clone())
        };
        let id = wayland_client::Proxy::id(&xdg);
        let serial = self
            .client
            .surface_configures
            .iter()
            .rev()
            .find(|(to, _)| *to == id)
            .map(|&(_, serial)| serial)
            .expect("that window was configured");
        xdg.ack_configure(serial);
        commit_buffer(&self.client, &self.qh, &surface, width, 300);
        self.pump();
    }

    /// Where each pane is drawn: its frame as the renderer resolves it,
    /// transform and all, and not only the rectangle under it -- a glide
    /// the layout starts at every move is drawn instead of that.
    fn drawn(&self) -> (Rectangle<i32, Logical>, Rectangle<i32, Logical>) {
        let of = |pane| {
            let held = self.state.panes.get(pane).expect("the pane is here");
            let rect = self
                .state
                .drawn_at(held, self.state.pane_outer(held), self.state.clock.now())
                .rect;
            Rectangle::new(
                (rect.loc.x.round() as i32, rect.loc.y.round() as i32).into(),
                (rect.size.w.round() as i32, rect.size.h.round() as i32).into(),
            )
        };
        (of(self.panes.0), of(self.panes.1))
    }

    /// The size each client has committed, as far as the compositor has
    /// applied it.
    fn committed(&self) -> (Size<i32, Logical>, Size<i32, Logical>) {
        (self.left.0.geometry().size, self.right.0.geometry().size)
    }

    /// The widths each client has been told, in order: a configure that
    /// only changes a state -- the release taking `resizing` away -- is
    /// not a new width, and is not counted again.
    fn told(&self, right: bool) -> Vec<i32> {
        let id = wayland_client::Proxy::id(if right { &self.right.1 } else { &self.left.1 });
        let mut widths: Vec<i32> = self
            .client
            .configures
            .iter()
            .filter(|(to, _, _)| *to == id)
            .map(|&(_, width, _)| width)
            .collect();
        widths.dedup();
        widths
    }
}

/// **A seam moves once both of its clients have drawn (`resize.sync`).**
///
/// Both panes are told their new sizes together, and drawn where they
/// were until both have answered: the left one's answer, in first, is
/// held back rather than shown against a seam its neighbour has not
/// reached, and both land in the same pass once the right one's is in.
/// Moved at once instead, the frame between the two answers draws the
/// left window's new picture cut by the old seam and the right window's
/// old picture short of the new one -- the grey band of 2026-10-16.
#[test]
fn a_seam_moves_once_both_of_its_clients_have_drawn() {
    let mut seam = Seam::new();
    seam.state.resizing.sync = Duration::from_millis(200);
    seam.state.begin_resize(&seam.left.0);
    seam.seam_at(450, Duration::ZERO);
    seam.pump();
    assert_eq!(
        (seam.told(false), seam.told(true)),
        (vec![450], vec![350]),
        "both sides are told their new size in the same round"
    );
    assert_eq!(
        seam.drawn(),
        (at(0, 0, 400, 300), at(400, 0, 400, 300)),
        "and both are drawn where their pictures are until they answer"
    );

    seam.answer(false, 450);
    assert_eq!(
        seam.committed().0,
        Size::from((400, 300)),
        "the left client's answer is held back while the right one's is not in"
    );
    seam.frame(Duration::from_millis(16));
    assert_eq!(
        seam.drawn(),
        (at(0, 0, 400, 300), at(400, 0, 400, 300)),
        "a frame with one answer in still draws the old seam"
    );

    seam.answer(true, 350);
    seam.frame(Duration::from_millis(32));
    assert_eq!(
        seam.committed(),
        (Size::from((450, 300)), Size::from((350, 300))),
        "with both answers in, both are applied together"
    );
    assert_eq!(
        seam.drawn(),
        (at(0, 0, 450, 300), at(450, 0, 350, 300)),
        "and both panes move to the new seam in the same pass"
    );
}

/// **A slow client holds a seam up for `resize.sync` and no longer.**
/// Past it the round ends with whatever is in: the answered side is
/// applied, and both are drawn at their new rectangles, the slow one's
/// old picture fitted into its own as the bridge always fitted one.
#[test]
fn a_seam_waits_for_a_slow_client_no_longer_than_resize_sync() {
    let mut seam = Seam::new();
    seam.state.resizing.sync = Duration::from_millis(200);
    seam.state.begin_resize(&seam.left.0);
    seam.seam_at(450, Duration::ZERO);
    seam.pump();
    seam.answer(false, 450);
    seam.frame(Duration::from_millis(199));
    assert_eq!(
        seam.drawn(),
        (at(0, 0, 400, 300), at(400, 0, 400, 300)),
        "inside `resize.sync` the seam waits for its slow side"
    );
    seam.frame(Duration::from_millis(200));
    assert_eq!(
        seam.committed().0,
        Size::from((450, 300)),
        "at `resize.sync` the answer that is in is applied"
    );
    assert_eq!(
        seam.drawn(),
        (at(0, 0, 450, 300), at(450, 0, 350, 300)),
        "and the seam moves without its slow side"
    );
}

/// **A seam that moves on during a round is told in the next one**,
/// never mid-round: a client told twice before its neighbour answered
/// once would answer a seam the neighbour is never shown at.
#[test]
fn a_seam_moved_during_a_round_is_told_in_the_next() {
    let mut seam = Seam::new();
    seam.state.resizing.sync = Duration::from_millis(200);
    seam.state.begin_resize(&seam.left.0);
    seam.seam_at(450, Duration::ZERO);
    seam.seam_at(470, Duration::from_millis(16));
    seam.pump();
    assert_eq!(
        (seam.told(false), seam.told(true)),
        (vec![450], vec![350]),
        "the second frame's seam waits for the first round"
    );
    seam.answer(false, 450);
    seam.answer(true, 350);
    seam.frame(Duration::from_millis(32));
    seam.pump();
    assert_eq!(
        (seam.told(false), seam.told(true)),
        (vec![450, 470], vec![350, 330]),
        "the round that ends starts the next, from where the seam has got to"
    );
    assert_eq!(
        seam.drawn(),
        (at(0, 0, 450, 300), at(450, 0, 350, 300)),
        "drawn where the round that ended put them"
    );
}

/// **A seam let go of during a round ends where it was let go**: the
/// release waits for the round under way like any other move, and the
/// gesture ends once the last round's answers are in.
#[test]
fn a_seam_let_go_of_mid_round_ends_where_it_was_let_go() {
    let mut seam = Seam::new();
    seam.state.resizing.sync = Duration::from_millis(200);
    seam.state.begin_resize(&seam.left.0);
    seam.seam_at(450, Duration::ZERO);
    seam.seam_at(470, Duration::from_millis(16));
    seam.state.release_resize(&seam.left.0);
    seam.pump();
    assert_eq!(
        (seam.told(false), seam.told(true)),
        (vec![450], vec![350]),
        "the release does not tell anybody past the round under way"
    );
    seam.answer(false, 450);
    seam.answer(true, 350);
    seam.frame(Duration::from_millis(32));
    seam.pump();
    seam.answer(false, 470);
    seam.answer(true, 330);
    seam.frame(Duration::from_millis(48));
    assert_eq!(
        seam.drawn(),
        (at(0, 0, 470, 300), at(470, 0, 330, 300)),
        "the gesture ends where the seam was let go"
    );
    assert!(
        !seam.state.holding_resize(seam.panes.0) && !seam.state.holding_resize(seam.panes.1),
        "and once both answers are in, nothing is held any more"
    );
}

/// **`resize.sync = 0` moves each pane to the layout's rectangle at
/// once**, as the bridge always did: told on every frame, drawn at its
/// slot.
#[test]
fn with_resize_sync_zero_a_seam_is_drawn_where_the_layout_put_it_at_once() {
    let mut seam = Seam::new();
    seam.state.resizing.sync = Duration::ZERO;
    seam.state.begin_resize(&seam.left.0);
    seam.seam_at(450, Duration::ZERO);
    seam.seam_at(470, Duration::from_millis(16));
    seam.pump();
    assert_eq!(
        (seam.told(false), seam.told(true)),
        (vec![450, 470], vec![350, 330]),
        "every frame tells both sides"
    );
    assert_eq!(
        seam.drawn(),
        (at(0, 0, 470, 300), at(470, 0, 330, 300)),
        "and both are drawn at the layout's rectangles straight away"
    );
}

/// **Issue #123, the flicker half: a tiled pane drew the size its client
/// last committed.**
///
/// `pane_geometry` lets a pane's slot outrank its client only while
/// `holding_resize` is true, which was false for every tiled drag, so
/// `pane_outer` and its twenty-odd callers read `real_geometry` — the
/// size the client last *agreed to*, which during a drag is frames
/// behind the layout.
///
/// Two assertions and they are not the same one twice. The first is the
/// inversion. The second is `sync_panes`, which copies the space into
/// every pane's slot once a frame and so used to undo the layout's
/// rectangle between the frame that drew it and the next one: that
/// alternation — the layout's rectangle on a frame carrying a motion,
/// the client's on a frame without one — is what the user sees as
/// stutter.
#[test]
fn a_tiled_pane_draws_the_layouts_rectangle_while_its_client_lags() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);
    let outer = at(400, 300, 300, 200);
    tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);

    assert_eq!(
        window.geometry().size,
        Size::from((64, 64)),
        "the fixture's client never answers a configure, which is what \
         makes it a stand-in for one that is merely slow -- if it had \
         answered there would be no disagreement to test"
    );
    let geometry = |state: &Solium| {
        state.pane_geometry(state.panes.get(pane).expect("the pane is still here"))
    };
    assert_eq!(
        geometry(&state),
        outer,
        "the layout's rectangle is what the pane is, immediately. \
         Falling back to the client's committed size draws a window \
         the size it was before the drag started at the position the \
         drag has reached"
    );

    state.sync_panes();
    assert_eq!(
        geometry(&state),
        outer,
        "and it survives the frame. `sync_panes` writes the space into \
         every pane's slot, and the space's size is whatever the client \
         last committed, so without `held_slot` the layout's authority \
         lasts exactly until the next frame's reconciliation"
    );
}

/// **The trap `crate::resizing` is built around, on the tiled path: a
/// client that refuses must not be stretched for ever.**
///
/// A client with a minimum size — Firefox has one, a terminal rounds to
/// its cell grid — answers a configure with a size of its own. Nothing
/// on the tiled path noticed: `resize_fill` answered `None`, `factor`
/// took the free-to-grow arm, and the buffer was scaled without limit
/// towards a rectangle its client had already walked away from. That is
/// the permanently-soft window this module's documentation says cost a
/// bug once already, and #115 — reading the client's minimum — is still
/// unread, so this is the only thing standing between a refusal and a
/// blur.
///
/// Both halves are asserted, because they happen at different times: the
/// stretch stops *during* the gesture, and the pane lands on the
/// client's own size when the gesture ends.
#[test]
fn a_tiled_client_that_refuses_a_size_stops_the_stretch_and_ends_the_bridge() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    // A configured stretch, which is the fill a refusal takes away; the
    // default since #233 has no stretch to take.
    state.resizing.fill = crate::resizing::Fill::Stretch;
    state.begin_resize(&window);
    let outer = at(400, 300, 300, 200);
    tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
    assert_eq!(
        state.resize_fill(pane),
        crate::resizing::Fill::Stretch,
        "nothing has been refused yet, so the configured fill stands"
    );

    // The answer, and it is not the one that was asked for: 120x90
    // where 300x200 was offered. A real client saying "this is my
    // minimum" says it exactly this way.
    commit_buffer(&client, &qh, &surface, 120, 90);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        window.geometry().size,
        Size::from((120, 90)),
        "the client committed a size of its own, which is the whole \
         scenario"
    );

    tiled_frame(&mut state, &request, pane, outer, Duration::from_millis(16));
    assert_eq!(
        state.resize_fill(pane),
        crate::resizing::Fill::Anchor,
        "a refusal overrides the configured fill. A stretch towards a \
         size the client has walked away from never returns to 1, so \
         the window stays soft until something else resizes it"
    );

    state.release_resize(&window);
    state.settle_resize();
    assert!(
        !state.holding_resize(pane),
        "any answer ends the bridge, and a refusal is an answer -- \
         waiting out `PATIENCE` for a size that has already been \
         declined only adds a quarter second of squashed window"
    );
    assert_eq!(
        state
            .panes
            .get(pane)
            .expect("the pane is still here")
            .slot()
            .size,
        Size::from((120, 90)),
        "and the pane lands on the size the client chose rather than \
         keeping a tile its client will never fill"
    );
}

/// **A window merely pushed aside by someone else's drag still needs its
/// configure.**
///
/// `move_pane` runs for every pane a layout touches, not just the one
/// under the pointer: `tiling.apply` emits a placement for every leaf on
/// every visible monitor, and a seam moving means at least two of them
/// have genuinely changed. So the throttle has to be per pane. Retarget
/// the single hold at "the pane the layout moved" instead — the obvious
/// smaller fix — and the interval opened by the dragged window swallows
/// the neighbour's one and only configure, which is a window that never
/// hears its new size at all.
///
/// Both counts are asserted from one drag, because they are the two
/// halves of the same rule: throttled is not silenced, and silent is not
/// throttled.
#[test]
fn a_pane_pushed_aside_by_another_panes_drag_is_configured_at_once() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    // A capped `resize.tell_hz`: what this test is about is a throttle's
    // own behaviour, and since 2026-10-16 a drag is told every frame unless
    // one is configured.
    state.resizing.tell = Duration::from_millis(100);
    let (dragged, dragged_toplevel, _dragged_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (beside, beside_toplevel, _beside_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(dragged.clone(), (400, 300), false);
    state.map_stacked(beside.clone(), (700, 300), false);
    state.sync_panes();
    let dragged_pane = state
        .panes
        .id_of(&dragged)
        .expect("a client in the space has a pane");
    let beside_pane = state
        .panes
        .id_of(&beside)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    let request = ResizeRequest {
        window: dragged.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&dragged);

    // The seam has not reached the neighbour yet, so the layout moves
    // one pane.
    tiled_frame(
        &mut state,
        &request,
        dragged_pane,
        at(400, 300, 300, 200),
        Duration::ZERO,
    );
    // Fifty milliseconds later — half an interval — the seam moves both.
    let later = Duration::from_millis(50);
    tiled_frame(
        &mut state,
        &request,
        dragged_pane,
        at(400, 300, 320, 200),
        later,
    );
    tiled_frame(
        &mut state,
        &request,
        beside_pane,
        at(720, 300, 180, 200),
        later,
    );

    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let counted = |toplevel: &xdg_toplevel::XdgToplevel| {
        let id = wayland_client::Proxy::id(toplevel);
        client
            .configures
            .iter()
            .filter(|(sent_to, ..)| sent_to == &id)
            .count()
    };
    assert_eq!(
        counted(&dragged_toplevel),
        1,
        "the dragged pane moved on both frames and both were inside one \
         interval, so its client hears once"
    );
    assert_eq!(
        counted(&beside_toplevel),
        1,
        "and the pane beside it moved for the first time on the second \
         frame, so it hears at once -- its interval starts when it \
         moves, not when somebody else did"
    );
}

/// **#402: after a fast seam drag, a pane across the seam is in its
/// tile, however late its client answers.**
///
/// The seam is dragged into the far pane faster than its client draws: it
/// is configured to 388 wide, then 288, and answers the 388 after the 288
/// has gone out. That answer was judged against the newest ask, read as a
/// refusal of it, and the release adopted the stale 388 at once, pinned
/// against the pane's far edge -- 100 to the left of its tile, over the
/// dragged pane -- where it stayed once the client drew the 288 it had been
/// asked for. It is the browser half over a terminal with bare desktop
/// beside it. Judged against the ask it answered, that answer is the
/// client keeping up and the pane waits for the next one.
///
/// And a client that answers nothing more before its patience runs out is
/// landed at its tile's corner, its stale size cut to the tile, so the
/// answer that comes later fills the tile rather than leaving the pane
/// where the stale size had put it.
#[test]
fn a_pane_across_a_fast_seam_drag_lands_in_its_tile_however_late_its_client_answers() {
    for answers in [true, false] {
        tiled_fixture!(display, state, conn, queue, client, qh);
        let (dragged, _dragged_toplevel, _dragged_surface, _dragged_xdg) =
            open_xdg(&mut display, &mut state, &conn, &client, &qh);
        let (far, far_toplevel, far_surface, far_xdg) =
            open_xdg(&mut display, &mut state, &conn, &client, &qh);
        state.map_stacked(dragged.clone(), (0, 0), false);
        state.map_stacked(far.clone(), (512, 0), false);
        state.sync_panes();
        let dragged_pane = state.panes.id_of(&dragged).expect("a pane");
        let far_pane = state.panes.id_of(&far).expect("a pane");
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        client.configures.clear();
        client.surface_configures.clear();

        let request = ResizeRequest {
            window: dragged.clone(),
            wanted: at(0, 0, 700, 600),
            edge_at: (700.0, 300.0),
            edges: ResizeEdge::Right,
        };
        state.begin_resize(&dragged);
        let start = state.clock.now();
        // Three frames of one quick gesture, each moving the seam, the
        // last a whole throttle interval after the others: it is told
        // the tile the drag ends on.
        let mut tile = at(512, 0, 488, 600);
        for (after, seam) in [(0, 600), (40, 650), (200, 700)] {
            let now = start + Duration::from_millis(after);
            state.clock.advance(now.saturating_sub(state.clock.now()));
            tile = at(seam + 12, 0, 1000 - seam - 12, 600);
            // The far pane first: the frame's flush, run with the other,
            // then finds it placed already.
            tiled_frame(&mut state, &request, far_pane, tile, now);
            tiled_frame(&mut state, &request, dragged_pane, at(0, 0, seam, 600), now);
        }
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        let far_id = wayland_client::Proxy::id(&far_toplevel);
        let asked: Vec<(i32, i32)> = client
            .configures
            .iter()
            .filter(|(to, ..)| to == &far_id)
            .map(|(_, w, h)| (*w, *h))
            .collect();
        assert!(
            asked.first() == Some(&(388, 600)) && asked.last() == Some(&(288, 600)),
            "the premise: the far client was asked for 388, and last for 288: {asked:?}"
        );
        let xdg_id = wayland_client::Proxy::id(&far_xdg);
        let serials: Vec<u32> = client
            .surface_configures
            .iter()
            .filter(|(to, _)| to == &xdg_id)
            .map(|(_, serial)| *serial)
            .collect();

        // It answers the first ask, the second already sent.
        far_xdg.ack_configure(serials[0]);
        commit_buffer(&client, &qh, &far_surface, 388, 600);
        pump(
            &mut display,
            &mut state,
            &conn,
            &qh,
            &mut queue,
            &mut client,
        );
        state.release_resize(&dragged);
        let released = state.clock.now();
        paused_frame(&mut state, released);
        state.sync_panes();

        let outer = |state: &Solium| {
            state.pane_outer(state.panes.get(far_pane).expect("the pane is still here"))
        };
        if answers {
            assert!(
                state.holding_resize(far_pane),
                "an answer to the earlier ask is the client keeping up, not a \
                 refusal of the newest, so the pane waits for the next"
            );
            assert_eq!(outer(&state), tile, "and waits in its tile");
            far_xdg.ack_configure(serials[serials.len() - 1]);
            commit_buffer(&client, &qh, &far_surface, 288, 600);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
        } else {
            state.clock.advance(crate::resizing::PATIENCE);
            let now = state.clock.now();
            paused_frame(&mut state, now);
            state.sync_panes();
            assert!(
                !state.holding_resize(far_pane),
                "the premise: its patience ran out"
            );
            assert_eq!(
                outer(&state),
                tile,
                "landed on its stale size, it is cut to its tile at the tile's \
                 corner rather than pinned against its far edge over its neighbour"
            );
            far_xdg.ack_configure(serials[serials.len() - 1]);
            commit_buffer(&client, &qh, &far_surface, 288, 600);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
        }
        let now = state.clock.now();
        paused_frame(&mut state, now);
        state.sync_panes();
        assert!(
            !state.holding_resize(far_pane),
            "answered, nothing holds it"
        );
        assert_eq!(
            outer(&state),
            tile,
            "answers {answers}: once its client has drawn the size it was asked \
             for, the pane across the seam is its tile"
        );
    }
}

/// **#405: a pane a drag is holding keeps its outer rectangle across a
/// style change.** Tiling turned off mid seam drag swaps the border-only
/// frame for one with a titlebar. Every other window keeps its outer
/// rectangle and its client gives the titlebar its room; the pane across
/// the seam, held by the drag, kept its client's rectangle instead and the
/// titlebar went on outside it, above the top of the screen.
#[test]
fn a_held_pane_keeps_its_outer_rectangle_when_its_frame_changes() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (dragged, _dragged_toplevel, _dragged_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    let (far, far_toplevel, far_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(dragged.clone(), (0, 0), false);
    state.map_stacked(far.clone(), (512, 0), false);
    state.sync_panes();
    let dragged_pane = state.panes.id_of(&dragged).expect("a pane");
    let far_pane = state.panes.id_of(&far).expect("a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let request = ResizeRequest {
        window: dragged.clone(),
        wanted: at(0, 0, 600, 600),
        edge_at: (600.0, 300.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&dragged);
    let tile = at(612, 0, 388, 600);
    let now = state.clock.now();
    tiled_frame(&mut state, &request, dragged_pane, at(0, 0, 600, 600), now);
    tiled_frame(&mut state, &request, far_pane, tile, now);
    assert!(
        state.holding_resize(far_pane),
        "the premise: the drag holds it"
    );
    let outer = |state: &Solium| {
        state.pane_outer(state.panes.get(far_pane).expect("the pane is still here"))
    };
    assert_eq!(outer(&state), tile, "the premise: it is its tile");
    client.configures.clear();

    // The frame changes under it: one reserving a titlebar, as `pane =`
    // a titled style does. No Qt here to build one, and a frame on its
    // way reserves the same room.
    let outers = state.outers_before_restyle();
    for pane in [dragged_pane, far_pane] {
        if let Some(held) = state.panes.get_mut(pane) {
            held.set_frame(crate::pane::Frame::Pending);
        }
    }
    state.keep_outers(outers, now);
    state.sync_panes();
    assert_eq!(
        outer(&state),
        tile,
        "the new frame comes out of the rectangle it held, and does not go on \
         outside it"
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let far_id = wayland_client::Proxy::id(&far_toplevel);
    assert_eq!(
        client
            .configures
            .iter()
            .filter(|(to, ..)| to == &far_id)
            .map(|(_, w, h)| (*w, *h))
            .next_back(),
        Some((388, 600 - TITLEBAR_HEIGHT)),
        "its client is told the room the new frame leaves it"
    );

    // Let go, and the client answers what it was told.
    state.release_resize(&dragged);
    commit_buffer(&client, &qh, &far_surface, 388, 600 - TITLEBAR_HEIGHT);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let now = state.clock.now();
    paused_frame(&mut state, now);
    state.sync_panes();
    assert!(
        !state.holding_resize(far_pane),
        "answered, nothing holds it"
    );
    assert_eq!(outer(&state), tile, "and it stays in the rectangle it had");
}

/// **#226: a window still answering a placement keeps the placed size
/// across a style change.** Tiling turned off, a configuration glides
/// every window back where it floated and changes the frame style in the
/// same breath. The client had not drawn its floating size yet, so the
/// space still had its tile's size; the restyle read that and asked the
/// client for it again, and the window went back to the size of its tile
/// a moment after it had left it.
#[test]
fn a_window_still_answering_a_placement_keeps_the_placed_size_when_its_frame_changes() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (0, 0), false);
    state.sync_panes();
    let pane = state.panes.id_of(&window).expect("a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let floated = at(100, 100, 800, 500);
    let now = state.clock.now();
    let was = state.pane_outer_of(pane).expect("a mapped pane");
    state.move_pane(
        pane,
        floated,
        was,
        AnimationSpec::default(),
        now,
        Standing::Free,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_ne!(
        window.geometry().size,
        floated.size,
        "the premise: its client has not drawn the placed size yet"
    );

    // The restyle's own two steps around a frame change. No Qt here to
    // build a frame, so the room it leaves is the same on both sides, and
    // what is checked is the size: the newest ask must still be the
    // placed one, whatever the client last drew.
    let outers = state.outers_before_restyle();
    state.keep_outers(outers, now);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let id = wayland_client::Proxy::id(&toplevel);
    assert_eq!(
        client
            .configures
            .iter()
            .filter(|(to, ..)| to == &id)
            .map(|(_, w, h)| (*w, *h))
            .next_back(),
        Some((800, 500)),
        "its client is last asked for the placed size, not the size it is \
         leaving"
    );
}

/// **#431: a script reads a window back where it put it.** A layout or
/// a configuration that places a window and reads `sol.windows()` before
/// the client has answered read the size the client was still drawing:
/// toggling tiling and floating quickly, a configuration that remembers
/// where each window floated remembered a size it was leaving, and the
/// windows drifted a little each time.
#[test]
fn a_script_reads_a_window_still_answering_a_placement_at_the_placed_size() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (0, 0), false);
    state.sync_panes();
    let pane = state.panes.id_of(&window).expect("a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let floated = at(100, 100, 800, 500);
    let now = state.clock.now();
    let was = state.pane_outer_of(pane).expect("a mapped pane");
    state.move_pane(
        pane,
        floated,
        was,
        AnimationSpec::default(),
        now,
        Standing::Free,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_ne!(
        window.geometry().size,
        inner(floated, state.insets_of(pane)).size,
        "the premise: its client has not drawn the placed size yet"
    );
    let row = state
        .window_infos()
        .into_iter()
        .find(|row| row.id == pane.get())
        .expect("the window is listed");
    assert_eq!(
        (row.rect.x, row.rect.y, row.rect.w, row.rect.h),
        (100.0, 100.0, 800.0, 500.0),
        "a script reads the rectangle it placed the window in"
    );
}

/// **Only a live gesture arms a bridge.**
///
/// The keyboard `nudge` reaches `move_pane` through `Scripts::key` and
/// never through `settle_resize`, and so do a config reload, a monitor
/// change, a decoration restyle, a workspace switch and
/// `rescue_offscreen`. A hold armed by any of them could never be let
/// go of: `release_resize` has exactly one caller and it is the pointer
/// grab, so `Hold::settle` would answer `Waiting` for ever and the
/// pane's slot and the space would be held apart for the life of the
/// window — the never-resolved disagreement `PATIENCE` exists to
/// prevent.
///
/// The second assertion is the one that keeps the first honest: a fix
/// that arms nothing by arming nothing is not a fix. The client still
/// has to be told.
#[test]
fn a_keyboard_nudge_arms_no_bridge_and_still_reaches_its_client() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    // No `arm_resize_gesture`, because nothing in a key dispatch calls
    // it. This is the whole of what a nudge is by the time it arrives.
    let was = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    state.move_pane(
        pane,
        at(400, 300, 300, 200),
        was,
        AnimationSpec::default(),
        Duration::ZERO,
        Standing::Tile,
    );

    assert!(
        !state.holding_resize(pane),
        "a keypress is not a gesture and has nothing that could ever \
         end a hold"
    );
    assert!(
        state.resize_bridge.is_none(),
        "and no bridge was opened for it either"
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.configures.len(),
        1,
        "the window still has to hear its new size: arming nothing is \
         only correct if it costs nothing"
    );
}

/// **A layout re-placing a pane exactly where it already is says
/// nothing to its client.**
///
/// `tree:layout` emits every leaf whether or not it moved, once per
/// visible monitor, and a drag runs it once a frame — so in a dwindle
/// tree the eight panes that did not move were configured sixty times a
/// second each for the length of every gesture. Smithay hides half of
/// that: `send_pending_configure` drops a configure that repeats the
/// size it last sent, so on the wire the xdg clients saw nothing. The
/// X11 arm has no such check and sent a real `ConfigureWindow` every
/// time, for every X11 window in the layout.
///
/// **Asserted against `offers_size` rather than against the wire**, and
/// that is forced rather than chosen: the fixture speaks xdg, which is
/// exactly the protocol whose own deduplication makes the defect
/// invisible from the client's side. A test that counted configures
/// here would pass against the unfixed compositor and prove nothing.
#[test]
fn a_layout_replacing_a_pane_where_it_already_is_tells_its_client_nothing() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    // Where the client actually is: the fixture's buffer is 64x64 and
    // it was mapped at (400, 300).
    let settled = at(400, 300, 64, 64);
    assert_eq!(
        state.real_geometry(&window),
        Some(settled),
        "the client and the space agree before the sweep, which is the \
         state every unchanged leaf is in on every frame of a drag"
    );
    assert!(
        !state.offers_size(pane, &window, settled, Duration::ZERO),
        "a leaf the layout re-placed where it already was has nothing \
         to be told"
    );
    // A position it does not have is a real change, even at the same
    // size: for an X11 window `size_window` is the only thing that
    // carries a position at all, so deduplicating on size alone would
    // leave one told to stay where it no longer is.
    assert!(
        state.offers_size(pane, &window, at(500, 300, 64, 64), Duration::ZERO),
        "a move with no resize still has to reach the client"
    );
}

/// **The throttle had no trailing edge, so the end of every drag was
/// never sent.**
///
/// `Hold::dragged` is reached from `move_pane` and from `hold_resize`,
/// and `settle_resize` reaches either only on a frame whose
/// `pending_resize` carried a motion. So the offers a drag makes inside
/// its last throttled interval are written into the pane's slot, drawn from
/// the pane's slot, and never put on the wire: the client sits at a size
/// up to a tenth of a second stale while the pane is drawn where the
/// pointer is, and the bridge between the two is the visible gap.
///
/// **A paused pointer is the ordinary end of a drag**, not an edge
/// case — people stop moving before they let go — and the gap is held
/// for as long as the pause lasts. Before #123 the tiled path
/// configured on every frame and had no tail at all, which makes this
/// the regression of exactly the symptom that was reported.
///
/// The pause is driven with `paused_frame`, which is the whole of what
/// `settle_resize` runs on a frame with no motion. Against the code
/// before this fix that call is `settle_resize_bridge` alone, and
/// nothing in it sends anything.
#[test]
fn a_drag_that_pauses_still_tells_its_client_where_it_stopped() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    // A capped `resize.tell_hz`: what this test is about is a throttle's
    // own behaviour, and since 2026-10-16 a drag is told every frame unless
    // one is configured.
    state.resizing.tell = Duration::from_millis(100);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);

    // Three frames of a drag well inside one interval. The first is the
    // pane joining the bridge, which goes out at once; the other two are
    // throttled, and the third is where the pointer stops.
    for (millis, width) in [(0, 300), (16, 340), (32, 380)] {
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, width, 200),
            Duration::from_millis(millis),
        );
    }
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.configures.last().map(|&(_, w, _)| w),
        Some(300),
        "the control: while the interval is running the client is still \
         at the first offer, and the 80 pixels since are the gap"
    );

    // The pointer has stopped. No motion reaches `settle_resize`, so
    // nothing calls `move_pane` again — these are the frames a paused
    // drag is made of.
    for millis in [48, 64, 80, 96, 112, 128] {
        paused_frame(&mut state, Duration::from_millis(millis));
    }
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.configures.last().map(|&(_, w, h)| (w, h)),
        Some((380, 200)),
        "once the interval passes the pane's rectangle has to reach its \
         client whether or not the pointer moved again. Without a \
         trailing flush the client stays 80 pixels behind a pane that is \
         drawn where the pointer stopped, for as long as the pause \
         lasts. Got {:?}",
        client.configures
    );
    assert_eq!(
        client.configures.len(),
        2,
        "and once, not once a frame: the flush is the throttle's \
         trailing edge and not a way around it. Got {:?}",
        client.configures
    );
}

/// **A pane given a different client mid-gesture was configured through
/// the old one.**
///
/// `settle_resize_hold`, `settle_resize_bridge` and `release_bridge`
/// all check `panes.get(pane).and_then(Pane::client)` against the window
/// their hold remembers before touching anything. `flush_resize` did
/// not, and it is the one that runs *first* on every frame — so a pane
/// whose content was replaced while its hold was still alive sent the
/// new client's slot to the old client, and read the old client's
/// committed size back into the throttle's bookkeeping while it was
/// there. `Pane::adopt` is how a pane's content is replaced without the
/// pane changing: same id, same slot, different window, which is
/// exactly the state the guard is about.
///
/// **The control is
/// [`a_drag_that_pauses_still_tells_its_client_where_it_stopped`],
/// which is this test without the adoption**: the same fixture, the
/// same two frames inside one interval, the same paused frame past it,
/// and there the configure does arrive. A test that only asserts
/// silence passes against a flush that was never going to fire, so the
/// slot is asserted here as well — the pane is still holding the offer
/// the throttle swallowed, which is the thing that would have been sent
/// to the wrong client.
#[test]
fn a_pane_given_a_different_client_is_not_flushed_through_the_old_one() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);
    // Two frames inside one interval: the first joins the bridge and
    // goes out at once, the second is throttled and is what a trailing
    // flush exists to send.
    for (millis, width) in [(0, 300), (16, 340)] {
        tiled_frame(
            &mut state,
            &request,
            pane,
            at(400, 300, width, 200),
            Duration::from_millis(millis),
        );
    }

    // The pane's content is replaced. Its id and its slot are the same
    // ones the hold is holding; the window inside it is not.
    let (other, _other_toplevel, _other_surface) =
        open_surface(&mut display, &mut state, &conn, &client, &qh);
    state
        .panes
        .get_mut(pane)
        .expect("the pane is still here")
        .adopt(other);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    // Past the interval, which is the frame the flush would fire on.
    paused_frame(&mut state, Duration::from_millis(120));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let id = wayland_client::Proxy::id(&toplevel);
    assert!(
        !client.configures.iter().any(|(sent_to, ..)| sent_to == &id),
        "the window this hold remembers no longer owns the pane, so the \
         slot being flushed is not its rectangle to be told about. Got \
         {:?}",
        client.configures
    );
    assert_eq!(
        state
            .panes
            .get(pane)
            .expect("the pane is still here")
            .slot()
            .size,
        Size::from((340, 200)),
        "the control: the slot still carries the offer the throttle \
         swallowed, so there was something for the flush to send"
    );
}

/// **The same tail on the floating path**, which `settle_resize_hold`
/// looks like it covers and does not.
///
/// `Hold::settle` decides whether a hold is over; it has never sent a
/// configure and does not know how. So a paused floating drag sat on
/// its last offer exactly as a tiled one did — the only thing that ever
/// sent unconditionally was the release. Older than #123 rather than a
/// regression of it, and fixed by the same flush, so it is asserted
/// here rather than left to be rediscovered.
#[test]
fn a_floating_drag_that_pauses_also_tells_its_client() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();
    state.begin_resize(&window);

    let dragged = |state: &mut Solium, width: i32, millis: u64| {
        let request = ResizeRequest {
            window: window.clone(),
            wanted: at(400, 300, width, 200),
            edge_at: (f64::from(400 + width), 500.0),
            edges: ResizeEdge::Right,
        };
        state.hold_resize(&request, Duration::from_millis(millis));
    };
    dragged(&mut state, 300, 0);
    dragged(&mut state, 340, 16);
    dragged(&mut state, 380, 32);
    for millis in [48, 64, 80, 96, 112, 128] {
        paused_frame(&mut state, Duration::from_millis(millis));
    }
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        state.holding_resize(pane),
        "the gesture is still live, which is what makes the pause a \
         pause rather than an ending"
    );
    assert_eq!(
        client.configures.last().map(|&(_, w, h)| (w, h)),
        Some((380, 200)),
        "a floating drag's tail is the same tail. Got {:?}",
        client.configures
    );
}

/// **A pane the layout only *moved* was never told, for the whole of
/// the gesture.**
///
/// `offers_size`'s case (2) documents why the whole rectangle is
/// compared and not just the size: `size_window` is the only thing that
/// carries a position to an X11 client, because `map_stacked` moves the
/// window in the space and says nothing to anybody. Case (1) compared
/// sizes — `Hold::dragged` took one — so a bridged pane whose slot
/// translates answered "nothing to say" and went on answering it until
/// the button came up. `scrolling.lua`'s `widen` shifts every column
/// sideways at an unchanged width, so that is the whole of an X11
/// window's drag in the scrolling layout: stale geometry, and pointer
/// coordinates routed to where the window used to be.
///
/// **Asserted against `offers_size` rather than against the wire**, and
/// forced rather than chosen, for the reason
/// `a_layout_replacing_a_pane_where_it_already_is_tells_its_client_nothing`
/// gives at length: the fixture speaks xdg, and smithay deduplicates a
/// configure that repeats the last size it sent — which is every
/// configure this test is about. Counting them would prove nothing.
///
/// The existing neighbour test cannot see this: it moves the pane it is
/// about *and* resizes it, and it moves it for the first time, so it
/// never reaches case (1) at all.
#[test]
fn a_bridged_pane_that_only_moves_is_still_told_where_it_went() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);
    // The pane joins the bridge here, so what follows exercises case (1).
    tiled_frame(
        &mut state,
        &request,
        pane,
        at(400, 300, 300, 200),
        Duration::ZERO,
    );
    assert!(
        state.holding_resize(pane),
        "the pane has to be bridged, or this is testing case (2)"
    );

    // A whole interval later, so the throttle is not what is being
    // measured: the column slides 40 pixels sideways at exactly the
    // width it already had.
    assert!(
        state.offers_size(
            pane,
            &window,
            at(440, 300, 300, 200),
            crate::resizing::PATIENCE,
        ),
        "a bridged pane that translates has moved, and for an X11 client \
         `size_window` is the only thing that will ever say so"
    );
    // And the dedup it must not have cost: the same rectangle twice is
    // still nothing to say.
    assert!(
        !state.offers_size(
            pane,
            &window,
            at(440, 300, 300, 200),
            crate::resizing::PATIENCE * 2,
        ),
        "comparing the whole rectangle must not turn into telling the \
         client on every frame"
    );
}

/// **A terminal rounds; it does not refuse. `declined` could not tell
/// the difference, so kitty never got the fill the user configured.**
///
/// `Hold::note` records a decline for any answer that is not exactly
/// the ask, and `fill` forced `Fill::Hold` (`Fill::Anchor` since #233)
/// for the rest of the gesture
/// on the strength of it. A terminal answers with a whole number of
/// character cells and so is a few pixels out on its very first answer
/// and every one after it — which makes kitty, the ordinary tiled
/// client, `declined` from the first frame of every seam drag.
/// `Fill::Hold` deliberately left an uncovered strip while a pane
/// grew, so what the user saw in place of their `stretch` was a band
/// of background: a different wrong-looking frame rather than none, on
/// the drag whose symptom is "frames with the wrong size".
///
/// Both directions, because a fix that stops calling anything a refusal
/// would ship the permanent blur this module was built to prevent.
#[test]
fn a_cell_grid_rounding_is_not_the_refusal_that_takes_the_stretch_away() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    // The user's stretch, which is what a rounding must not take away.
    state.resizing.fill = crate::resizing::Fill::Stretch;
    state.begin_resize(&window);
    let outer = at(400, 300, 300, 200);
    tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);

    // What kitty answers 300x200 with: the nearest whole number of
    // cells, which at an ordinary font is a handful of pixels short on
    // each axis.
    commit_buffer(&client, &qh, &surface, 294, 190);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        window.geometry().size,
        Size::from((294, 190)),
        "the client answered with a size of its own, which is what a \
         cell grid always does"
    );
    tiled_frame(&mut state, &request, pane, outer, Duration::from_millis(16));
    assert_eq!(
        state.resize_fill(pane),
        crate::resizing::Fill::Stretch,
        "six pixels on a three-hundred pixel pane is a rounding, and \
         the user's configured fill stands. Calling it a refusal costs \
         the stretch on every terminal drag there will ever be"
    );

    // And the same client, refusing for real: a minimum width, which is
    // nothing like six pixels out.
    commit_buffer(&client, &qh, &surface, 120, 90);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    tiled_frame(&mut state, &request, pane, outer, Duration::from_millis(32));
    assert_eq!(
        state.resize_fill(pane),
        crate::resizing::Fill::Anchor,
        "a client that has walked away from the ask still loses the \
         stretch: it will never bring the factor back to 1, and a \
         window that never un-stretches is the trap this module is \
         built around"
    );
}

/// **The drag's throttle was applied to every caller of `move_pane`.**
///
/// A bridge outlives its gesture by up to `PATIENCE`, and `move_pane`
/// is reached by a config reload, a `modes.use` from a keybinding, a
/// workspace switch and `rescue_offscreen` as well as by a layout
/// sweep. Case (1) looked the pane up in the bridge without asking
/// whose sweep this was, so one of those landing inside a live bridge
/// had its one and only configure swallowed by an interval somebody
/// else opened — and nothing would resend it, because a keypress has no
/// next frame. `move_pane` went on writing the slot regardless, so the
/// pane was then drawn, stretched, at a rectangle its client had never
/// been told about for the rest of the gesture.
///
/// `resize_gesture` is set for the length of `trigger_resize` and by
/// nothing else, which is exactly the question "is the sweep reaching
/// this pane the live drag's own".
#[test]
fn a_reload_inside_a_live_bridge_is_not_throttled_by_the_drag() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);
    tiled_frame(
        &mut state,
        &request,
        pane,
        at(400, 300, 300, 200),
        Duration::ZERO,
    );

    // Sixteen milliseconds later — deep inside the interval the drag
    // just opened — something that is not the drag places this pane.
    // No `arm_resize_gesture`, because a reload does not run one: this
    // is the whole of what a `move_pane` from a key dispatch is.
    let was = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    state.move_pane(
        pane,
        at(400, 300, 260, 180),
        was,
        AnimationSpec::default(),
        Duration::from_millis(16),
        Standing::Tile,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.configures.last().map(|&(_, w, h)| (w, h)),
        Some((260, 180)),
        "a reload's one configure must not be swallowed by a drag's \
         interval: there is no second frame to resend it, and the pane \
         has already taken the rectangle. Got {:?}",
        client.configures
    );
    assert!(
        state.holding_resize(pane),
        "and the bridge is still the bridge -- the reload was sent \
         through the hold, not around it, so `asked` still names what \
         the client last heard"
    );
}

/// **A pending change must not wait out the drag's interval.**
///
/// `size_window` is a pending size *and* a `send_pending_configure`, so
/// a throttled frame skips the flush as well. A maximise, a fullscreen
/// or a decoration mode agreed by somebody else is then blocked behind
/// an interval for exactly the windows a drag is touching — which is
/// the one place case (1) short-circuited before the check that exists
/// to catch it.
///
/// The change here is a decoration mode. A maximised or fullscreen
/// window is no layout's to place, so a drag's frame does not reach one:
/// `a_layout_leaves_a_fullscreen_or_maximised_window_where_it_is`.
#[test]
fn a_pending_change_is_flushed_even_while_the_drag_is_throttled() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);
    tiled_frame(
        &mut state,
        &request,
        pane,
        at(400, 300, 300, 200),
        Duration::ZERO,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    let before = client.configures.len();

    // Somebody else agrees a state change and is waiting on the
    // configure that carries it.
    window
        .toplevel()
        .expect("the fixture's window is an xdg toplevel")
        .with_pending_state(|state| {
            use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as Decoration;
            state.decoration_mode = Some(if state.decoration_mode == Some(Decoration::ServerSide) {
                Decoration::ClientSide
            } else {
                Decoration::ServerSide
            });
        });
    // And a throttled frame of the drag arrives before the interval is
    // up. This is the frame that used to swallow it.
    tiled_frame(
        &mut state,
        &request,
        pane,
        at(400, 300, 310, 200),
        Duration::from_millis(16),
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        client.configures.len() > before,
        "the throttle governs how often a drag asks for a size, not \
         whether a window blocked on somebody else's change ever hears \
         about it. Got {:?}",
        client.configures
    );
}

/// **A press that pauses let the previous gesture's deadline expire.**
///
/// `begin_resize` reconciles the floating hold and clears
/// `resize_ended`, and `arm_resize_gesture` rearms the bridge — but
/// that runs on the first *motion*, and `settle_resize_bridge` runs on
/// every frame. So pressing on a border, holding still for a quarter of
/// a second and then dragging let the previous drag's `PATIENCE` run
/// out with the button already down: every pane that drag had moved was
/// adopted off its tile, which is precisely the snap `Hold::rearm`
/// exists to prevent, arriving between the press and the first pixel of
/// motion.
#[test]
fn a_press_stops_the_previous_drags_deadline_before_it_can_expire() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);
    let outer = at(400, 300, 300, 200);
    tiled_frame(&mut state, &request, pane, outer, Duration::ZERO);
    // The first gesture ends. Its holds are now waiting out `PATIENCE`
    // for an answer the fixture's client never gives.
    let ended = Duration::from_millis(100);
    state.release_bridge(&window, ended);

    // The second gesture presses, and the hand stays still. Every frame
    // of that pause runs `settle_resize_bridge`.
    state.begin_resize(&window);
    paused_frame(&mut state, ended + crate::resizing::PATIENCE);
    paused_frame(
        &mut state,
        ended + crate::resizing::PATIENCE + Duration::from_millis(50),
    );

    assert!(
        state.holding_resize(pane),
        "the new gesture owns this pane and will keep placing it, so \
         the old gesture's deadline must stop at the press rather than \
         at the first motion"
    );
    assert_eq!(
        state
            .panes
            .get(pane)
            .expect("the pane is still here")
            .slot(),
        outer,
        "and the pane is still on the tile the last drag left it on \
         rather than snapped back to its client's own size"
    );
}

/// **A claimed/unclaimed flip cost two unthrottled configures and reset
/// the interval.**
///
/// `settle_resize` forks per frame, so a handler whose answer changes
/// between frames hands one pane back and forth between the bridge and
/// the floating hold. Each direction built a fresh `Hold` with a fresh
/// `told`, and a fresh hold is told immediately by design — so an
/// alternating handler restored the sixty configures a second
/// a capped `resize.tell_hz` exists to impose. Carrying the hold across the boundary
/// is what stops that: it is the same client in the same gesture.
///
/// The fixture has no scripts, so `trigger_resize` returns false and
/// `settle_resize`'s own unclaimed branch is what `hold_resize` is
/// reached through here; `tiled_frame` is the claimed one.
#[test]
fn a_layout_changing_its_mind_does_not_buy_a_configure_each_way() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    // A capped `resize.tell_hz`: what this test is about is a throttle's
    // own behaviour, and since 2026-10-16 a drag is told every frame unless
    // one is configured.
    state.resizing.tell = Duration::from_millis(100);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    client.configures.clear();

    let asking = |width: i32| ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, width, 200),
        edge_at: (f64::from(400 + width), 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);

    // Claimed: the pane joins the bridge and is told at once.
    tiled_frame(
        &mut state,
        &asking(300),
        pane,
        at(400, 300, 300, 200),
        Duration::ZERO,
    );
    // Unclaimed, sixteen milliseconds later. Every frame asks for a
    // different width, because smithay drops a configure that repeats
    // the last size it sent and a test that dragged to the same place
    // twice would be green against a compositor with no throttle at all.
    state.hold_resize(&asking(310), Duration::from_millis(16));
    // Claimed again.
    tiled_frame(
        &mut state,
        &asking(320),
        pane,
        at(400, 300, 320, 200),
        Duration::from_millis(32),
    );
    // And unclaimed again.
    state.hold_resize(&asking(330), Duration::from_millis(48));

    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.configures.len(),
        1,
        "four frames inside one interval are one configure however many \
         times the layout changed its mind: the pane's throttle belongs \
         to the gesture, not to whichever authority happens to be \
         placing it. Got {:?}",
        client.configures
    );
    assert!(
        state.holding_resize(pane),
        "and exactly one authority is holding it at the end"
    );
}

/// **A pane's moved edge was derived from where its *client* is, not
/// from where the pane was.**
///
/// `moved_edges` asks which of this pane's edges a placement moved, and
/// a pane's previous rectangle is its slot. `real_geometry` is the
/// client's — the space's position at the size the client last
/// committed — which during a drag is frames behind the slot, so the
/// client's latency leaked into the answer: an edge that did not move
/// looks moved because the client has not caught up to where it already
/// is. `Hold::pins` then hangs a held picture against the wrong side of
/// the window, and `anchored` gives a refusal's pixels back on an edge
/// the user never touched.
///
/// The arrangement below is the ordinary one: a client that has
/// answered nothing, a pane a seam has already widened once, and a
/// second placement that pulls the pane's *left* edge. Measured from
/// the slot that is a left pull and nothing else. Measured from the
/// client's 64x64 buffer both horizontal edges look moved — which names
/// neither — and the vertical bottom looks moved as well.
#[test]
fn a_panes_moved_edge_is_measured_from_its_own_slot_not_from_its_client() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        window.geometry().size,
        Size::from((64, 64)),
        "the client has answered nothing, which is what makes its \
         rectangle the wrong thing to measure against"
    );

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Left,
    };
    state.begin_resize(&window);
    tiled_frame(
        &mut state,
        &request,
        pane,
        at(400, 300, 300, 200),
        Duration::ZERO,
    );

    // The layout stops placing this pane for a frame and then places it
    // again, which is what makes the edges be derived a second time.
    // Driven directly for the reason `tiled_frame` gives: a Lua handler
    // that changed its mind between frames would be testing mlua.
    state.drop_bridged(pane);
    // A left pull from the slot: the left edge moves in by 40 and the
    // right edge — at 700 — does not move at all.
    tiled_frame(
        &mut state,
        &request,
        pane,
        at(440, 300, 260, 200),
        Duration::from_millis(16),
    );

    assert_eq!(
        state.resize_pins(pane),
        (true, false),
        "the left edge is the one that moved and the right one is \
         standing still, so a held picture has to stay against the \
         right. Measured from the client's committed 64x64 instead, \
         both horizontal edges look moved and the pane is told it has \
         no stationary edge at all"
    );
}

/// **A window the space does not have still needs the throttle.**
///
/// Arming required `real_geometry` to answer, and it answers `None` for
/// a window that is not in the space. The rectangle still counted as
/// changed — `None != Some(client)` — so the client was told on every
/// frame and no hold was ever armed to say when to stop: the one path
/// left running at the pre-#123 configure rate, for the length of every
/// gesture that touched such a window.
///
/// A pane's own slot is the previous rectangle the edges want anyway,
/// so nothing needs the space's answer to arm; only case (2)'s "is the
/// client already here" does, and `None` there means "no idea", which
/// is a reason to send rather than a reason not to hold.
#[test]
fn a_pane_whose_window_left_the_space_is_still_bridged() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, _toplevel, _surface) = open_surface(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(window.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    state.space.unmap_elem(&window);
    assert_eq!(
        state.real_geometry(&window),
        None,
        "the space no longer has it, which is the whole precondition"
    );

    let request = ResizeRequest {
        window: window.clone(),
        wanted: at(400, 300, 300, 200),
        edge_at: (700.0, 500.0),
        edges: ResizeEdge::Right,
    };
    state.begin_resize(&window);
    tiled_frame(
        &mut state,
        &request,
        pane,
        at(400, 300, 300, 200),
        Duration::ZERO,
    );
    assert!(
        state.holding_resize(pane),
        "a pane a live gesture moved is bridged whether or not the \
         space can say where its client was: without a hold nothing \
         throttles it and nothing ever ends it"
    );
}

mod lock_focus;

mod client_sizes;

mod launch_splash;

mod reflow_on_close;

/// A window opened, mapped at a known place, and known to the panes.
///
/// The three closing tests below all start here, and all three need a
/// *real* client rather than a loading pane: `settle_closing` removes a
/// pane with no client outright — "the one case where closing is
/// entirely ours to decide" — so a fixture without one never reaches
/// `send_close`, never stamps `asked_at`, and so cannot see either of
/// the two faults that live after the request goes out.
fn opened_at(
    display: &mut Display<Solium>,
    state: &mut Solium,
    conn: &Connection,
    client: &Client,
    qh: &QueueHandle<Client>,
    place: (i32, i32),
) -> (Window, crate::pane::PaneId) {
    let (window, _toplevel) = open_window(display, state, conn, client, qh);
    state.map_stacked(window.clone(), place, false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&window)
        .expect("a client in the space has a pane");
    (window, pane)
}

/// How this pane is drawn at `now`, through the same call the renderer
/// makes.
fn drawn_now(state: &Solium, pane: crate::pane::PaneId, now: Duration) -> Frame {
    let outer = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    state.drawn_id_at(pane, outer, now)
}

/// **Issue #127, fault 1: a layout sweep during a close put the dying
/// window back at full opacity, and it then vanished with no
/// animation.**
///
/// `Pane::closing_at` had three production readers and `move_pane` was
/// not one of them, so the unconditional `present::from` at the end of
/// every placement overwrote the closing transform with a *released*
/// one aimed at `Frame::real` — full size, full opacity. Any sweep
/// inside the 190 ms `CLOSING` window did it, and a sweep inside that
/// window is ordinary: another window opening, a layer surface's first
/// configure, a GTK4 `set_parent`. That is the "sometimes close
/// animations does not even play" the issue was filed for.
///
/// **Three assertions, and they are three different claims.** The
/// opacity is the fault itself. The position is the *decision* — a
/// closing pane animates out from where it was, and does not slide to
/// the slot the layout has just given it; see `move_pane`, which argues
/// it. And `placed` is the half that must keep working: only the
/// transform is suppressed, the layout's bookkeeping is untouched, and
/// #124 reads `placed`.
#[test]
fn a_layout_sweep_does_not_cancel_a_close_that_is_already_playing() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (_window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let was = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    // Sampled before the call, which samples it again for itself: the
    // gap is microseconds and every instant below is offset by enough
    // milliseconds that it cannot matter.
    let pressed = state.clock.now();
    state.close_pane(pane);
    assert!(
        drawn_now(&state, pane, pressed + Duration::from_millis(95)).opacity < 0.9,
        "the close animation is under way; if it were not, nothing \
         below is testing anything"
    );

    // The sweep. A window opening on the other half of the screen is
    // the commonest way to get one, and `tiling.apply` re-places every
    // leaf on every visible monitor — including this one, which is
    // still in the tree because its client has not gone yet.
    let elsewhere = at(1000, 300, 64, 64);
    state.move_pane(
        pane,
        elsewhere,
        was,
        AnimationSpec::default(),
        pressed + Duration::from_millis(95),
        Standing::Tile,
    );

    let landed = drawn_now(&state, pane, pressed + Duration::from_millis(400));
    assert!(
        landed.opacity.abs() < f32::EPSILON,
        "a pane the layout moved mid-close must still finish its \
         close: it was drawn at opacity {} instead",
        landed.opacity
    );
    // 400 + 64 * (1 - 0.86) / 2: `Frame::scaled` is about the centre,
    // so the shrunk rectangle sits inside the one the window was
    // closed at. The claim is which of the two places it is near, and
    // 600 logical pixels separate them.
    assert!(
        landed.rect.loc.x < 500.0,
        "a closing pane animates out from where it was, not from the \
         slot it will never occupy: it was drawn at x={}",
        landed.rect.loc.x
    );

    let placed = state
        .panes
        .get(pane)
        .and_then(Pane::placed)
        .expect("the sweep placed this pane");
    assert_eq!(
        placed, elsewhere,
        "only the transform is suppressed. The layout's own answer is \
         still written, because the layout is still right about where \
         this pane lives and #124 reads it"
    );

    // **And the close still completes.** The assertions above are all
    // about the frame at T+400, which a suppression that also swallowed
    // the request would satisfy perfectly: the window would sit at
    // opacity 0 for ever and no client would ever be told. Driving
    // `settle_closing` past the deadline is what separates "still
    // animating out" from "stuck invisible", and it is the claim the
    // sentence "a swept close still sends its one request and stays
    // gone" was making with nothing behind it.
    state.settle_closing(pressed + Duration::from_millis(400));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.closes.len(),
        1,
        "a close the layout swept still sends its one request"
    );
    assert!(
        drawn_now(&state, pane, pressed + Duration::from_millis(500))
            .opacity
            .abs()
            < f32::EPSILON,
        "and the window stays gone afterwards rather than being handed \
         back by the sweep"
    );
}

/// **Issue #127, fault 2: a second close restarted an invisible
/// animation and sent a second `send_close`.**
///
/// `close_pane`'s guard was `closing_at().is_some()`, and
/// `settle_closing` clears `closing_at` before it stamps `asked_at` —
/// so from the moment the request goes out until `settle_refused` gives
/// up on it, a pane already on its way out answered "not closing" to
/// the one question that was asked about it. A second `super+q` in that
/// window restarted `present::close` from the held opacity-0 frame
/// (invisible to invisible, so nothing to see) and asked the client
/// again.
///
/// Asking twice is not harmless. A client showing "save your work?" is
/// a client that received the first request and is acting on it; a
/// second one is a second dialog.
#[test]
fn a_second_close_inside_the_grace_neither_restarts_nor_asks_again() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (_window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        client.closes.is_empty(),
        "opening a window asks nothing to close"
    );

    let pressed = state.clock.now();
    state.close_pane(pane);
    let asked = pressed + present::CLOSING + Duration::from_millis(10);
    state.settle_closing(asked);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(client.closes.len(), 1, "the press asked, once");
    assert!(
        drawn_now(&state, pane, asked).opacity.abs() < f32::EPSILON,
        "and the window is held invisible while the client decides, \
         which is what makes a second press invisible too"
    );

    // The second `super+q`, well inside the grace period. There is
    // nothing on screen for the user to have aimed it at, which is
    // exactly why it happens: the window went and the client has not.
    state.close_pane(pane);
    assert!(
        state.panes.get(pane).and_then(Pane::closing_at).is_none(),
        "a pane already on its way out must not have its animation \
         restarted -- and restarting it from an invisible frame to an \
         invisible frame is 190ms of nothing"
    );

    // Far enough past a second `CLOSING` that a restarted timer would
    // have come due and sent its request.
    state.settle_closing(asked + present::CLOSING + Duration::from_millis(10));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.closes.len(),
        1,
        "and the client is asked once per window, not once per press"
    );
}

/// **Issue #127, fault 3: a slow client faded out, faded back in, then
/// popped.**
///
/// `settle_refused` measures its grace from the instant the request
/// went out and brings the window back when it expires. At 400 ms that
/// is not a deadline on *refusal* — it is a deadline on slowness, and
/// Electron's `before-quit`, the JVM's window listeners and Firefox's
/// session flush all run past it. What the user saw was the window fade
/// away, come back, and then vanish with no animation at all when the
/// client finally did close.
///
/// **Written against a client latency, not against the constant.** 600
/// ms is the claim: a client that takes that long to honour a close is
/// never shown again. A test that restated `GRACE` would pass at any
/// value including the one that caused the bug — this fails for every
/// grace period shorter than 600 ms, whatever it is called.
///
/// **And the other direction, in the same test**, because the recovery
/// is right and deleting it would otherwise turn this green: a window
/// that never answers is still brought back, and soon enough that it
/// reads as an answer to the press. Together the two halves pin the
/// grace period into a range rather than onto a number.
#[test]
fn a_client_that_takes_six_hundred_milliseconds_to_close_is_never_shown_again() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (_window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let pressed = state.clock.now();
    state.close_pane(pane);
    let asked = pressed + present::CLOSING + Duration::from_millis(10);
    state.settle_closing(asked);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(client.closes.len(), 1, "the request went out");

    /// What a heavy client costs between receiving `xdg_toplevel.close`
    /// and destroying its toplevel. The fixture's client never destroys
    /// anything, so this is simply how long the test waits before
    /// declaring the window safely gone.
    const SLOW: Duration = Duration::from_millis(600);
    /// A frame at 60Hz. The fault is a frame the user can see, so the
    /// assertion is made at the rate frames are drawn.
    const FRAME: Duration = Duration::from_millis(16);

    /// By when a window that never answers must be back on screen and
    /// fully opaque. Not arithmetic: it is the span in which a window
    /// reappearing still reads as caused by the press rather than as
    /// the session doing something by itself, and it has to cover the
    /// recovery's own fade as well as the wait before it.
    const LOST: Duration = Duration::from_millis(1500);

    let mut waited = Duration::ZERO;
    // The loop that ships: `Solium::settle` calls this once a frame.
    // Sampling the frame at the same instant is what makes the
    // assertion about what is on screen rather than about a timer.
    while waited <= LOST {
        let now = asked + waited;
        state.settle_refused(now);
        let opacity = drawn_now(&state, pane, now).opacity;
        if waited <= SLOW {
            assert!(
                opacity.abs() < f32::EPSILON,
                "{}ms after the request the window was drawn at opacity \
                 {opacity}; a client this slow is closing, not \
                 refusing, and bringing it back means fade out, fade \
                 in, pop",
                waited.as_millis()
            );
        }
        waited += FRAME;
    }

    // And the other direction. The recovery is right and must survive:
    // a window animated away that then refuses to close would
    // otherwise be invisible and alive, holding its place in the
    // layout, with nothing to bring it back.
    let back = drawn_now(&state, pane, asked + LOST).opacity;
    assert!(
        (back - 1.0).abs() < f32::EPSILON,
        "a window that never answers is still brought back, and within \
         a second and a half of being asked: it was drawn at opacity \
         {back}"
    );
}

/// **#127 review, finding 1: an invisible closing pane went on winning
/// every hit test at the rectangle it used to occupy.**
///
/// `window_under`, `surface_under`, `pane_chrome` and `decorated_under`
/// all asked `drawn_at(..).rect.contains(location)` and nothing else.
/// `present::close` ends at opacity 0 and is written with
/// `release: false` on purpose, so the transform *holds* there — for
/// `CLOSING` plus the whole grace period, which #127 took from about
/// 590 ms to about 1190 ms. For all of it the dead window was the
/// topmost thing at a rectangle the layout had already given to
/// somebody else.
///
/// **The trade #127 made without noticing.** It removed a cosmetic
/// flicker — the window fading back in and popping — and what that
/// flicker had been doing was *telling the user the window was still
/// there*. Silencing it while leaving the hit test alone turns a
/// visible glitch into an invisible one: the sibling has reflowed into
/// the space and is what is on screen, a click there lands in the dead
/// window, and every keystroke after it follows the focus that click
/// set. For exactly the slow-but-honest clients the grace bump was
/// written for.
///
/// **Both halves of a press, because they are two walks and either
/// alone would leave the other broken.** `window_under` is what
/// click-to-focus raises and focuses — the keystroke half, since focus
/// is what the typing follows — and `surface_under` is what the pointer
/// event is actually delivered to. `chrome_under` is asserted beside
/// them because an invisible titlebar is still a titlebar to a walk
/// that only measures rectangles.
///
/// The pane being closed is opened *second* so that it is above the
/// survivor in the stack: a test where the right answer is also the
/// topmost one is not testing the walk.
#[test]
fn a_press_where_a_closed_window_used_to_be_reaches_what_is_drawn_there() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (kept, survivor) = opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
    let (doomed, closing) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    // Past both windows' opening animations, and *settled*, which is
    // what the render loop does once a frame. Two reasons, and the test
    // needs both. `present::open` starts at opacity 0 and eases up, so
    // a window in its first frame is genuinely not on screen yet and
    // these walks correctly decline it. And `open`'s target was
    // captured before `opened_at` moved the window, so until the
    // released transform is retired the pane is still drawn at the
    // rectangle it mapped at rather than the one it now lives at.
    state.clock.advance(Duration::from_millis(300));
    state.settle(state.clock.now());

    let vacated = state
        .pane_outer_of(closing)
        .expect("a mapped pane has a rectangle");
    let was = state
        .pane_outer_of(survivor)
        .expect("a mapped pane has a rectangle");
    // The middle of the window the user is about to close, which is
    // where the window that replaces it will be too.
    let point = Point::<f64, Logical>::from((
        f64::from(vacated.loc.x) + f64::from(vacated.size.w) / 2.0,
        f64::from(vacated.loc.y) + f64::from(vacated.size.h) / 2.0,
    ));

    // The premise. If the closing window did not own this point to
    // begin with, nothing below is about anything.
    assert_eq!(
        state.window_under(point).map(|(window, _)| window),
        Some(doomed.clone()),
        "before the close, the point belongs to the window that is \
         about to be closed"
    );

    state.close_pane(closing);
    // Past `CLOSING`: the animation has landed, the request has gone
    // out, and the fixture's client never destroys anything -- which is
    // what a client still running its quit handlers looks like from
    // here.
    state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = state.clock.now();
    state.settle_closing(asked);
    assert_eq!(client.closes.len(), 0, "the client has not answered yet");

    // The layout reflows into the space, which is the ordinary
    // consequence of a close and the reason the stale rectangle matters
    // at all.
    state.move_pane(
        survivor,
        vacated,
        was,
        AnimationSpec::default(),
        asked,
        Standing::Tile,
    );
    // Past the survivor's own move, so what is drawn at the point is
    // the survivor itself rather than a frame of it in transit.
    state.clock.advance(Duration::from_millis(400));

    // The premise for the second half: the closing pane is invisible
    // and its rectangle still covers the point. Without this the walk
    // could be answering correctly for the wrong reason.
    let dead = drawn_now(&state, closing, state.clock.now());
    assert!(
        dead.opacity.abs() < f32::EPSILON && dead.rect.contains(point),
        "the closing pane is held at opacity 0 over the point -- \
         opacity {}, rect {:?}. That is the situation under test",
        dead.opacity,
        dead.rect
    );

    assert_eq!(
        state.window_under(point).map(|(window, _)| window),
        Some(kept.clone()),
        "a click where a closed window used to be belongs to the \
         window that is drawn there now. `window_under` is what \
         click-to-focus focuses, so answering the dead window sends \
         every keystroke after the click into a window that is not on \
         screen"
    );

    let surface = state
        .surface_under(point)
        .map(|(surface, _)| surface)
        .expect("the survivor is drawn at this point and has a surface");
    assert_eq!(
        Some(&surface),
        kept.wl_surface().as_deref(),
        "and the pointer event is delivered to that window's surface, \
         not to the dead one's"
    );

    // The compositor's own chrome, by the same rule: an invisible
    // titlebar has no buttons and an invisible edge cannot be dragged.
    // A `Halo` would be wrong here too, which is why `chrome_offered`
    // is gated and not only `covers`.
    assert!(
        state
            .chrome_under(point)
            .is_none_or(|under| under.pane != closing),
        "no chrome of the closed window is under the point either"
    );
}

/// **#127 review, finding 2: the guard covered the transform and left
/// the client-facing half of a placement running.**
///
/// `move_pane` suppressed `present::from` for a leaving pane and went on
/// calling `offers_size`, `size_window` and `map_stacked`. The configure
/// is the visible one: it asks a client that is tearing itself down to
/// re-lay-out at a size nobody will ever see, and if the client answers,
/// `real_geometry` moves under a `frame.rect` that `present::close` has
/// pinned — which is exactly the pair `resizing::factor` divides, so the
/// dying buffer is stretched to fill a rectangle it was never painted
/// for, mid-fade.
///
/// Counted at the client, for the reason `Client::configures` gives:
/// from the server's own side a compositor that sends a configure looks
/// identical to one that does not.
#[test]
fn a_layout_sweep_does_not_configure_a_window_that_is_closing() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (_window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let was = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    let pressed = state.clock.now();
    state.close_pane(pane);

    let before = client.configures.len();
    // A different size, not just a different place: `offers_size`
    // deduplicates on the whole rectangle, so a sweep that only moved
    // the pane would send nothing even without the guard and the test
    // would pass against the bug.
    state.move_pane(
        pane,
        at(1000, 300, 250, 180),
        was,
        AnimationSpec::default(),
        pressed + Duration::from_millis(95),
        Standing::Tile,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    assert_eq!(
        client.configures.len(),
        before,
        "a client that has been asked to close is not asked to \
         re-lay-out at a size that will never be drawn"
    );

    // And the half that must keep working, for the same reason the
    // sweep test asserts it: only what a client can observe is
    // suppressed, and #124 reads `placed`.
    let placed = state
        .panes
        .get(pane)
        .and_then(Pane::placed)
        .expect("the sweep placed this pane");
    assert_eq!(
        placed,
        at(1000, 300, 250, 180),
        "the layout's own answer is still written for a leaving pane"
    );
}

/// **#127 review, finding 3: `GRACE` inverts for the client that
/// refuses on purpose.**
///
/// The grace period was lengthened on the argument that too long only
/// makes a genuinely refused window wait. That holds for the
/// honest-but-slow client and reverses for this one: "save your changes
/// before closing?" is a refusal delivered as a question, and under a
/// flat deadline the parent was a hole for the whole second with the
/// dialog floating over nothing to read. A second `super+q` could not
/// clear it either — correctly, since `Pane::leaving` declines to start
/// a second close on a pane already in one.
///
/// A new window parented to the one being closed is an answer, and it
/// is the safe kind to act on: being wrong gives a window back that was
/// going to leave anyway. See `Solium::refused_with_a_dialog`.
///
/// **Asserted well inside `GRACE`**, which is the whole claim. Sampling
/// after it would pass against the plain deadline and test nothing.
#[test]
fn a_window_that_answers_a_close_with_a_dialog_comes_straight_back() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    // `opened_at`'s body, inlined for the one thing it discards: the
    // client-side toplevel proxy, which is the only end `set_parent`
    // can be sent from.
    let (parent, parent_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(parent.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&parent)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    state.close_pane(pane);
    state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = state.clock.now();
    state.settle_closing(asked);
    assert!(
        state
            .panes
            .get(pane)
            .is_some_and(|pane| pane.asked_at().is_some()),
        "the request has gone out and the client has not answered"
    );
    assert!(
        drawn_now(&state, pane, asked).opacity.abs() < f32::EPSILON,
        "and the window is invisible, which is the hole the dialog \
         would otherwise float over"
    );

    // The client's answer: not a destroy, a dialog. `set_parent` is
    // what says the dialog is about *this* window, and it is the
    // request `parent_changed` fires on.
    let (dialog, dialog_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    dialog_toplevel.set_parent(Some(&parent_toplevel));
    conn.flush().expect("flushing set_parent");
    display
        .dispatch_clients(&mut state)
        .expect("dispatching set_parent");

    // The fixture is doing what it claims: the compositor read the
    // parent back, so this is a dialog about the window being closed
    // and not merely another window.
    assert_eq!(
        state.parent_of(&dialog),
        Parentage::Window(pane.get()),
        "the client called set_parent and the compositor did not read \
         it back, so nothing below is about a dialog for this window"
    );

    let back = drawn_now(&state, pane, state.clock.now() + Duration::from_millis(200));
    assert!(
        (back.opacity - 1.0).abs() < f32::EPSILON,
        "a window whose client answered with a dialog is back on \
         screen without waiting out the grace period: it was drawn at \
         opacity {}",
        back.opacity
    );
    assert!(
        state.panes.get(pane).is_some_and(|pane| !pane.leaving()),
        "and it is no longer leaving, so a second super+q can close it"
    );
}

/// **#127 third review, finding 1: an X11 tooltip could cancel a
/// close.**
///
/// The rule is `refused_with_a_dialog`'s own: a window that places
/// itself — a menu, a tooltip, a splash, a notification, an
/// override-redirect window — appearing over a window that is closing
/// says nothing about whether the close was refused. It stood as a
/// comment at one of the three call sites, and the `TransientFor` hook
/// added beside it did not repeat it. Every one of those windows is in
/// `self.space` and receives `PROPERTY_CHANGE`, so any of them that set
/// `WM_TRANSIENT_FOR` after mapping brought its parent back: `super+q`,
/// the window fades out, returns, and never closes.
///
/// **Asserted at rule level, and the limit is stated rather than
/// implied.** An `X11Surface` cannot be built without a live XWayland
/// and nothing in this suite has one, so the X11 event that carries the
/// case cannot be produced here. What *can* be produced is the fact the
/// gate turns on, which is not an X11 fact at all: `Pane::managed`,
/// false for exactly those windows and set in one place —
/// `take_unmanaged_pane`, which both of XWayland's self-placing branches
/// call. So the child here is given the unmanaged pane a tooltip gets,
/// and driven through the one caller this fixture can drive. That pins
/// the gate; it does not pin the X11 plumbing above it.
#[test]
fn a_window_that_places_itself_does_not_cancel_a_close() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (parent, parent_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(parent.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&parent)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    state.close_pane(pane);
    state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = state.clock.now();
    state.settle_closing(asked);
    assert!(
        state
            .panes
            .get(pane)
            .is_some_and(|pane| pane.asked_at().is_some()),
        "the request has gone out and the client has not answered, \
         which is the state a tooltip must not end"
    );

    // The tooltip. Everything about it is ordinary except its pane,
    // which is the whole of what makes it one.
    let (tip, tip_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    let tip_pane = state
        .panes
        .id_of(&tip)
        .expect("a client in the space has a pane");
    if let Some(unmanaged) = state.panes.get_mut(tip_pane) {
        unmanaged.unmanage();
    }
    assert!(
        state
            .panes
            .get(tip_pane)
            .is_some_and(|pane| !pane.managed()),
        "the premise: this child holds the unmanaged pane every menu, \
         tooltip, splash and override-redirect window is given, and \
         without it this test is about an ordinary dialog"
    );

    tip_toplevel.set_parent(Some(&parent_toplevel));
    conn.flush().expect("flushing set_parent");
    display
        .dispatch_clients(&mut state)
        .expect("dispatching set_parent");

    // The second premise: the compositor really did read the parent
    // back, so the gate is what declined and not a parent nobody found.
    assert_eq!(
        state.parent_of(&tip),
        Parentage::Window(pane.get()),
        "the tooltip named the closing window as its parent and the \
         compositor did not read it back, so nothing below is about \
         the gate"
    );

    assert!(
        state.panes.get(pane).is_some_and(Pane::leaving),
        "a window that places itself is not an answer to anything: the \
         close must still be running"
    );
    let still = drawn_now(&state, pane, state.clock.now() + Duration::from_millis(200));
    assert!(
        still.opacity.abs() < f32::EPSILON,
        "and the closing window must still be held invisible rather \
         than faded back up by a tooltip: it was drawn at opacity {}",
        still.opacity
    );
}

/// **#127 third review, finding 3: `refused_with_a_dialog` threw away
/// `give_back`'s answer.**
///
/// `give_back` reports whether `present::clear` took, because
/// `with_slot` declines rather than panics when the transform slot is
/// already borrowed. On a declined frame nothing is retired — and
/// inside `CLOSING` there is no `asked_at`, so `settle_refused` is not
/// looking at this pane and never will be. The close then ran to its
/// deadline and `settle_closing` sent the request, closing the parent
/// out from under the dialog that had just answered for it.
///
/// **The observation is at the client, because that is the only end
/// that can tell.** `send_close` is a call into smithay and a
/// compositor that made it looks, from its own side, exactly like one
/// that did not. See [`Client::closes`].
///
/// **What the jam costs this test, said plainly.** `present::jam_slot`
/// is one-way, so the give-back retries for ever here and there is no
/// frame on which it succeeds; `present::frame` also falls back to real
/// geometry while the slot is busy, so opacity says nothing either.
/// What this asserts is the half that was actually lost — the request
/// that must not go out, and the close staying owed rather than
/// forgotten. The other half, a give-back on a free slot bringing the
/// window back, is
/// `a_window_that_answers_a_close_with_a_dialog_comes_straight_back`.
#[test]
fn a_dialog_whose_give_back_is_declined_does_not_lose_its_parent() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (parent, parent_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    state.map_stacked(parent.clone(), (400, 300), false);
    state.sync_panes();
    let pane = state
        .panes
        .id_of(&parent)
        .expect("a client in the space has a pane");
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    state.close_pane(pane);
    // Inside `CLOSING`, which is where a file chooser arrives and is
    // the state `settle_refused` cannot see: the request has not gone
    // out, so there is no `asked_at` for it to be due on.
    state.clock.advance(Duration::from_millis(50));
    assert!(
        state
            .panes
            .get(pane)
            .is_some_and(|pane| pane.closing_at().is_some() && pane.asked_at().is_none()),
        "the premise: the animation is playing and the request has not \
         gone out, which is the only window in which this fault exists"
    );

    // The busy frame. Nothing un-jams this, which is why it is taken
    // after the close has started and before the dialog arrives.
    if let Some(busy) = state.panes.get(pane) {
        present::jam_slot(busy);
    }

    let (dialog, dialog_toplevel) = open_window(&mut display, &mut state, &conn, &client, &qh);
    dialog_toplevel.set_parent(Some(&parent_toplevel));
    conn.flush().expect("flushing set_parent");
    display
        .dispatch_clients(&mut state)
        .expect("dispatching set_parent");
    assert_eq!(
        state.parent_of(&dialog),
        Parentage::Window(pane.get()),
        "the client called set_parent and the compositor did not read \
         it back, so nothing below is about a dialog for this window"
    );

    // Past the deadline the request would go out on.
    state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let due = state.clock.now();
    let still_going = state.settle_closing(due);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    assert!(
        client.closes.is_empty(),
        "a client that answered a close with a dialog was asked to \
         close anyway, on the frame its give-back was declined: the \
         parent is shut out from under its own prompt"
    );
    assert!(
        still_going,
        "and the close is still owed, so the backend keeps drawing and \
         the give-back is retried -- a deadline that answered false \
         here would strand the window instead"
    );
    assert!(
        state.panes.get(pane).is_some_and(Pane::leaving),
        "the pane is still leaving, so a second super+q cannot start a \
         second close over the top of this one"
    );

    // And it is a retry rather than one reprieve: another frame, and
    // the request still does not go out.
    state.clock.advance(Duration::from_millis(100));
    let later = state.clock.now();
    assert!(
        state.settle_closing(later),
        "the retry is still live a frame later"
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        client.closes.is_empty(),
        "the reprieve lasts as long as the give-back is owed, rather \
         than for one frame"
    );
}

/// **#127 third review, finding 2: focus could land on a window nobody
/// can see.**
///
/// `settle_focus`'s topmost arm was taught to skip a pane that is
/// `leaving` and nothing else, while the pointer arm it falls back from
/// goes through `window_under` and so asks `Frame::covers` — opacity
/// *and* the rectangle. A hidden workspace is parked a screen away
/// rather than unmapped (`workspaces.lua`: a switch moves the view, not
/// the windows), so its panes are perfectly visible to a filter that
/// only asks about the close.
///
/// Closing the only window on the workspace in view therefore handed
/// the keyboard to a desk the user cannot see, and with it
/// `focus_window`'s `trigger_focus`, which is what a workspace script
/// acts on. The window that then refused to close came back to a
/// session where `give_back`'s `settle_focus` declines — something is
/// focused — so it was permanent. That is #127's own symptom by a
/// second route.
///
/// **Asserted at the client**, for the reason
/// `typing_after_a_close_reaches_the_window_that_is_drawn` gives: from
/// the compositor's own side a seat holding an off-stage surface looks
/// exactly like one holding a visible one. Here the right answer is
/// that the keystroke goes *nowhere* — an idle keyboard loses no
/// characters to the wrong application. This test stops at the
/// request; that the window which then comes back takes the keyboard is
/// `a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back`'s
/// to show, and until #127's fourth review nothing did.
#[test]
fn a_close_does_not_hand_the_keyboard_to_a_workspace_nobody_can_see() {
    tiled_fixture!(display, state, conn, queue, client, qh);

    // One monitor, because "off screen" is a question with no answer
    // without one -- `nothing_on_stage` says so itself.
    let screen = Output::new(
        "hidden-desk-test".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "solium".to_string(),
            model: "test".to_string(),
        },
    );
    screen.change_current_state(
        Some(Mode {
            size: (1920, 1080).into(),
            refresh: 60_000,
        }),
        None,
        Some(Scale::Fractional(1.0)),
        None,
    );
    state.space.map_output(&screen, (0, 0));

    let (parked, parked_pane) =
        opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
    let (visible, closing) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    // Twice, for the reason `typing_after_a_close...` gives: the
    // keyboard is a request the client makes in answer to the seat's
    // capabilities.
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        client.keyboard.is_some(),
        "the client bound a keyboard; without one this test cannot \
         observe anything"
    );

    // Past both opening animations and settled, so no pane is still
    // drawn back near the origin where the untouched pointer is.
    state.clock.advance(Duration::from_millis(300));
    state.settle(state.clock.now());
    assert!(
        state.window_under((0.0, 0.0).into()).is_none(),
        "the pointer has not been moved and there is nothing under it, \
         which is what makes this the keyboard's question"
    );

    // Workspace 2 goes away: one selection, carried a screen and a bit
    // to the left. `workspaces.lua`'s own arrangement and its own
    // numbers.
    let desk = state.window_id(&parked);
    let switch = state.clock.now();
    state.groups.declare(
        "desk-2",
        crate::group::Selection {
            members: vec![crate::group::Member::Window(desk)],
            on: None,
        },
        switch,
    );
    state.groups.present(
        "desk-2",
        crate::group::Shift {
            dx: -1920.0 * 1.06,
            ..crate::group::Shift::NONE
        },
        switch,
        Duration::from_millis(300),
        present::Curve::OutCubic,
    );
    state.clock.advance(Duration::from_millis(400));
    state.settle(state.clock.now());

    // The premise, both halves of it: the parked window is fully
    // opaque -- it is not hidden by being faded out, which the
    // `shows()` half of the gate would have caught on its own -- and
    // its own rectangle is still on the monitor, because a hidden
    // workspace is parked rather than unmapped. Only the *drawn*
    // rectangle knows it is gone.
    let landed = state.clock.now();
    let away = drawn_now(&state, parked_pane, landed);
    assert!(
        (away.opacity - 1.0).abs() < f32::EPSILON,
        "the parked window is fully opaque, so opacity alone cannot be \
         what declines it"
    );
    let real = state
        .pane_outer_of(parked_pane)
        .expect("a mapped pane has a rectangle");
    assert!(
        state.on_any_output(real),
        "and it still lives on the monitor -- a workspace switch moves \
         the view, not the windows -- so `pane_outer` cannot be what \
         declines it either"
    );
    let screens: Vec<Rectangle<i32, Logical>> = state
        .space
        .outputs()
        .filter_map(|output| state.space.output_geometry(output))
        .collect();
    assert_eq!(
        nothing_on_stage([(real, away.rect)], &screens),
        Some(true),
        "the parked window is drawn off every monitor, which is the \
         one thing about it that is true"
    );

    // The window the user is working in, and the premise that this
    // fixture can see where typing goes at all.
    state.focus_window(
        &visible,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    types(&mut state, KEY_A);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.typed,
        vec![(Some(surface_id(&visible)), KEY_A)],
        "the premise: typing reaches the focused window"
    );

    // And the user closes the only window they can see.
    state.close_pane(closing);
    state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = state.clock.now();
    state.settle_closing(asked);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    // The client first, because it is the only end that can tell, and
    // the compositor's own view of the seat second.
    let before = client.typed.len();
    types(&mut state, KEY_A);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.typed.len(),
        before,
        "a keystroke after the last visible window closed goes \
         nowhere, rather than onto a workspace the user cannot see: it \
         reached {:?}, and the parked window's surface is {}",
        client.typed.last(),
        surface_id(&parked)
    );
    assert!(
        state.focused_window().is_none(),
        "and the seat is holding nothing at all -- it must, because \
         `give_back`'s `settle_focus` declines when something already \
         has focus, which is what would make this permanent rather \
         than a wrong answer for one second"
    );
}

/// Whether any of this pane's drawn rectangle at `at` reaches a screen,
/// and whether it paints anything there — the two halves of "on stage",
/// measured the way the premises below need them measured.
fn drawn_on_stage(state: &Solium, pane: crate::pane::PaneId, at: Duration) -> bool {
    let frame = drawn_now(state, pane, at);
    let slot = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    let screens: Vec<Rectangle<i32, Logical>> = state
        .space
        .outputs()
        .filter_map(|output| state.space.output_geometry(output))
        .collect();
    frame.shows() && nothing_on_stage([(slot, frame.rect)], &screens) == Some(false)
}

/// **#127 fourth review, NEW-1: a refused window came back with the
/// keyboard on nothing.**
///
/// `give_back` calls `settle_focus` straight after `present::clear`, so
/// the restore it has just started is at progress zero, and at progress
/// zero a transform answers its `from` — which past `CLOSING` is
/// `present::close`'s opacity-zero end. Judged by the frame being
/// drawn, the window being given back was invisible, so the topmost arm
/// declined the only window there was to focus, and the window stood
/// back up at full opacity with the seat holding nothing.
///
/// **Deterministic, where the fault was not.** In the running
/// compositor `settle_focus` reads the clock a little after the frame's
/// `now`, and the window escaped whenever enough real time had passed
/// between the two for the fade to clear one step of eight bits. This
/// hands `settle_refused` an instant the clock has not reached, so
/// every reading `settle_focus` takes falls at or before the restore's
/// start, where `Animation::progress` is exactly zero. The failing case,
/// made certain rather than likely.
///
/// Nothing else is open, so `hand_off_keyboard` empties the seat at the
/// request — asserted, because that is the path the fault needs.
/// Asserted at the client as well as the seat, for the reason
/// `typing_after_a_close_reaches_the_window_that_is_drawn` gives.
#[test]
fn a_refused_close_gives_the_keyboard_back_to_the_window_it_brings_back() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    one_screen(&mut state);
    let (window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    // Twice: the keyboard is a request the client makes in answer to
    // the seat's capabilities. See `typing_after_a_close...`.
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        client.keyboard.is_some(),
        "the client bound a keyboard; without one this test cannot \
         observe anything"
    );
    // Past the opening animation and settled, so the untouched pointer
    // at the origin is over nothing and the topmost arm is the one
    // asked.
    state.clock.advance(Duration::from_millis(300));
    state.settle(state.clock.now());
    assert!(
        state.window_under((0.0, 0.0).into()).is_none(),
        "the pointer is over nothing, which makes this the keyboard's \
         question and not the mouse's"
    );

    state.focus_window(
        &window,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    state.close_pane(pane);
    state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = state.clock.now();
    state.settle_closing(asked);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(client.closes.len(), 1, "the request went out");
    assert!(
        state.focused_window().is_none(),
        "the premise: with nothing else open, handing the keyboard off \
         at the request leaves the seat holding nothing"
    );

    // The refusal, at an instant the clock has not reached. Past the
    // grace period, so the window is due.
    let refused = asked + Duration::from_millis(1500);
    state.settle_refused(refused);
    assert!(
        !drawn_now(&state, pane, state.clock.now()).shows(),
        "the premise: at every instant `settle_focus` could have read, \
         the window being given back is still drawn at nothing"
    );
    assert!(
        drawn_on_stage(&state, pane, refused + Duration::from_millis(200)),
        "and it is on its way back: once the restore lands it is on \
         screen and opaque"
    );

    assert_eq!(
        state.focused_window().as_ref(),
        Some(&window),
        "a window given back to a session whose keyboard is idle takes \
         the keyboard, even on the frame its restore starts"
    );
    let before = client.typed.len();
    types(&mut state, KEY_A);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.typed.get(before),
        Some(&(Some(surface_id(&window)), KEY_A)),
        "and typing reaches it, rather than going nowhere until the \
         user clicks"
    );
}

/// How far a hidden desk is carried: a screen and a bit to the left,
/// `workspaces.lua`'s own arrangement and its own numbers.
const AWAY: f64 = -1920.0 * 1.06;

/// Two desks with one window each, and a switch between them on its
/// first frame.
///
/// Returns `(leaving, arriving)`. `leaving` is on the desk in view and
/// is opened second, so it is the topmost pane: a walk whose right
/// answer is also the topmost one is not testing the walk. `arriving`
/// starts parked `AWAY` and settled there.
///
/// **The switch starts at an instant the clock has not reached**, so
/// every reading `settle_focus` takes falls at or before its start,
/// where `Animation::progress` is exactly zero — the first frame, made
/// certain rather than likely. Its premises are asserted here, both
/// ends of them: on that frame `leaving` is drawn on stage and
/// `arriving` off it, and once the switch lands it is the other way
/// round. Focus that follows the drawn frame and focus that follows the
/// destination give different answers only because of this.
///
/// The seat is emptied by hand at the end. How it came to be empty is
/// not the question — a close, a lock, a window going — what
/// `settle_focus` does about it is.
fn a_switch_on_its_first_frame(
    display: &mut Display<Solium>,
    state: &mut Solium,
    conn: &Connection,
    qh: &QueueHandle<Client>,
    queue: &mut wayland_client::EventQueue<Client>,
    client: &mut Client,
) -> ((Window, crate::pane::PaneId), (Window, crate::pane::PaneId)) {
    one_screen(state);
    let arriving = opened_at(display, state, conn, client, qh, (1000, 300));
    let leaving = opened_at(display, state, conn, client, qh, (400, 300));
    pump(display, state, conn, qh, queue, client);
    pump(display, state, conn, qh, queue, client);
    assert!(
        client.keyboard.is_some(),
        "the client bound a keyboard; without one this test cannot \
         observe anything"
    );
    state.clock.advance(Duration::from_millis(300));
    state.settle(state.clock.now());

    let parked = state.clock.now();
    for (name, window) in [("desk-1", &leaving.0), ("desk-2", &arriving.0)] {
        let id = state.window_id(window);
        state.groups.declare(
            name,
            crate::group::Selection {
                members: vec![crate::group::Member::Window(id)],
                on: None,
            },
            parked,
        );
    }
    state.groups.present(
        "desk-2",
        crate::group::Shift {
            dx: AWAY,
            ..crate::group::Shift::NONE
        },
        parked,
        Duration::from_millis(300),
        present::Curve::OutCubic,
    );
    state.clock.advance(Duration::from_millis(400));
    state.settle(state.clock.now());

    let switch = state.clock.now() + Duration::from_millis(50);
    state.groups.present(
        "desk-1",
        crate::group::Shift {
            dx: AWAY,
            ..crate::group::Shift::NONE
        },
        switch,
        Duration::from_millis(300),
        present::Curve::OutCubic,
    );
    state.groups.present(
        "desk-2",
        crate::group::Shift::NONE,
        switch,
        Duration::from_millis(300),
        present::Curve::OutCubic,
    );

    let first = state.clock.now();
    let landed = switch + Duration::from_millis(400);
    assert!(
        drawn_on_stage(state, leaving.1, first) && !drawn_on_stage(state, arriving.1, first),
        "the premise: on the switch's first frame the desk being left is \
         still the one drawn"
    );
    assert!(
        !drawn_on_stage(state, leaving.1, landed) && drawn_on_stage(state, arriving.1, landed),
        "and once it lands the desk switched to is"
    );

    assert!(
        state.seat.get_keyboard().is_some(),
        "the fixture's seat has a keyboard; without one there is no \
         focus to be wrong about"
    );
    state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    assert!(state.focused_window().is_none(), "the seat is empty");
    (leaving, arriving)
}

/// **#127 fourth review, NEW-2: on a workspace switch's first frame,
/// focus judged by where the windows had been.**
///
/// `on_stage` sampled the instant it was called, while
/// `everything_is_off_stage` — the one other caller asking the same
/// question — samples `SETTLED` ahead, because a group transform that
/// has just started is at progress zero. So `settle_focus` inside the
/// first frames of a switch handed the keyboard to the desk being left
/// and passed over the desk being switched to. The pointer is over
/// nothing, so this is the topmost arm.
#[test]
fn the_first_frame_of_a_workspace_switch_focuses_the_desk_switched_to() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let ((leaving, _), (arriving, _)) = a_switch_on_its_first_frame(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        state.window_under((0.0, 0.0).into()).is_none(),
        "the pointer is over nothing, so the topmost arm is the one \
         asked"
    );

    state.settle_focus();
    assert_eq!(
        state.focused_window().as_ref(),
        Some(&arriving),
        "focus goes to the desk being switched to, not to the one being \
         left (surface {}), which is topmost and still drawn",
        surface_id(&leaving)
    );
    let before = client.typed.len();
    types(&mut state, KEY_A);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.typed.get(before),
        Some(&(Some(surface_id(&arriving)), KEY_A)),
        "and typing reaches it"
    );
}

/// **The pointer arm of the same question, and where the two rules
/// meet.**
///
/// `settle_focus` asks the window under the pointer first, because with
/// focus-follows-mouse that is where focus would land the moment the
/// pointer moved — and *the moment it moved* is after the switch has
/// landed, not on this frame. So a pointer resting where the desk being
/// left is drawn must not hand that desk the keyboard.
///
/// **And the click on the same pixel still goes to what is drawn**,
/// which is the half that must not change: `window_under` is the hit
/// test, a press lands on what is on screen this frame, and on this
/// frame that is the window being left. Both are asserted at the same
/// point on the same frame, so a fix that moved the hit test instead
/// fails here.
#[test]
fn a_pointer_over_the_desk_being_left_does_not_hand_it_the_keyboard() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let ((leaving, _), (arriving, _)) = a_switch_on_its_first_frame(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    // The middle of the window being left, as it is drawn this frame.
    let point: Point<f64, Logical> = (432.0, 332.0).into();
    let pointer = state.seat.get_pointer().expect(
        "the fixture's seat has a pointer; without one there is no \
         pointer arm to test",
    );
    pointer.motion(
        &mut state,
        None,
        &smithay::input::pointer::MotionEvent {
            location: point,
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    pointer.frame(&mut state);
    assert_eq!(
        state.window_under(point).map(|(window, _)| window).as_ref(),
        Some(&leaving),
        "a press on this pixel this frame lands on the window drawn \
         there, which is the desk being left -- the hit test judges \
         the present frame, and that is right"
    );

    state.settle_focus();
    assert_eq!(
        state.focused_window().as_ref(),
        Some(&arriving),
        "but the keyboard goes where the desks are settling: the window \
         under the pointer is sliding off stage (surface {}), so the \
         desk switched to takes it",
        surface_id(&leaving)
    );
}

/// **A closing window under the pointer, while it is still fading.**
///
/// The case [`SETTLED`] lists first. `settle_focus`'s pointer arm used
/// to get "never a window on its way out" from `window_under`, which
/// declines a pane only once it shows nothing — so for the 190 ms a
/// close is still playing, a seat left empty by anything else handed
/// the keyboard to the window being closed, if the pointer was resting
/// on it. The topmost arm's `leaving()` filter never reached it: the
/// pointer arm answers first.
///
/// Judged at the destination, a closing pane is at opacity zero from
/// the press, so it is no candidate; and the same press on the same
/// frame still lands on it, because it is still drawn and a half-faded
/// window keeps its clicks. Both are asserted.
#[test]
fn a_window_mid_close_under_the_pointer_is_not_handed_the_keyboard() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    one_screen(&mut state);
    let (kept, _) = opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
    let (doomed, closing) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    state.clock.advance(Duration::from_millis(300));
    state.settle(state.clock.now());

    let point: Point<f64, Logical> = (432.0, 332.0).into();
    let pointer = state.seat.get_pointer().expect(
        "the fixture's seat has a pointer; without one there is no \
         pointer arm to test",
    );
    pointer.motion(
        &mut state,
        None,
        &smithay::input::pointer::MotionEvent {
            location: point,
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    pointer.frame(&mut state);

    // A quarter of the way into the fade: drawn, visibly, and still
    // taking its clicks.
    state.close_pane(closing);
    state.clock.advance(Duration::from_millis(50));
    assert!(
        drawn_now(&state, closing, state.clock.now()).shows(),
        "the premise: the window being closed is still on screen"
    );
    assert_eq!(
        state.window_under(point).map(|(window, _)| window).as_ref(),
        Some(&doomed),
        "and a press on it this frame lands on it, which is right"
    );

    assert!(
        state.seat.get_keyboard().is_some(),
        "the fixture's seat has a keyboard; without one there is no \
         focus to be wrong about"
    );
    state.give_keyboard(None, SERIAL_COUNTER.next_serial());
    state.settle_focus();
    assert_eq!(
        state.focused_window().as_ref(),
        Some(&kept),
        "but the keyboard is not handed to a window that is on its way \
         out (surface {}): the one staying takes it",
        surface_id(&doomed)
    );
}

/// The protocol id of a window's surface.
///
/// The one number the two ends of this fixture share. A `wl_surface` is
/// created by the client, so the id it picked is the id the server
/// knows it by, and a test holding the server's `Window` can say which
/// surface a `wl_keyboard.enter` named without threading the client's
/// proxy through every helper.
fn surface_id(window: &Window) -> u32 {
    window
        .wl_surface()
        .expect("a mapped client has a surface")
        .id()
        .protocol_id()
}

/// One keystroke, through the same `KeyboardHandle::input` that
/// `crate::input::keyboard` ends in.
///
/// `Forward` unconditionally: whether a combination is a binding is
/// `input::keyboard`'s question and not this one's.
///
/// `key` is the evdev code, which is what an input backend reports and
/// what the client is sent. The `+ 8` in the middle is the X11 offset
/// libxkbcommon works in and smithay unwinds again on the wire, so the
/// number that goes in here is the number that comes out at the other
/// end — and a test asserting on a different one would be asserting on
/// this fixture's arithmetic rather than on the compositor's.
fn types(state: &mut Solium, key: u32) {
    let keyboard = state.seat.get_keyboard().expect(
        "the fixture's seat has a keyboard; without one there is no \
         typing to be wrong about",
    );
    keyboard.input::<(), _>(
        state,
        smithay::input::keyboard::Keycode::new(key + 8),
        smithay::backend::input::KeyState::Pressed,
        SERIAL_COUNTER.next_serial(),
        0,
        |_, _, _| smithay::input::keyboard::FilterResult::Forward,
    );
}

/// `KEY_A`, as evdev and the client both spell it. See [`types`].
const KEY_A: u32 = 30;

/// **#127 second review, finding 1: the keystroke fix only worked via a
/// click.**
///
/// The review before this one stopped an invisible closing pane from
/// winning `window_under`, and `window_under` is what click-to-focus
/// focuses — so a press where the dead window used to be goes to the
/// window that is drawn there, and the typing after it follows. That is
/// the whole of the fix, and it requires the user to touch the mouse.
///
/// **Nothing moved focus otherwise.** `close_pane` does not, and
/// `settle_focus` runs from `sync_panes` only when the pane set
/// changes, which a close that has been asked and not yet answered does
/// not do. So `super+q` and carry on typing — the ordinary way anyone
/// meets this — put every character into a window at opacity zero for
/// the 190 ms animation *and* the 1000 ms grace after it, while the
/// sibling that reflowed into the space sat on screen looking like the
/// thing being typed into.
///
/// **Asserted at the client, which is the only end that can tell.**
/// From the compositor's own side a seat holding an invisible surface
/// looks exactly like one holding a visible one; `give_keyboard`'s
/// `true` says only that the lock's rule allowed the surface, and
/// nothing complains about opacity. So this binds a `wl_keyboard` and
/// asks where the key came out. See [`Client::typed`].
///
/// The premise is typed first, before the close: a test whose second
/// half passes because the keyboard was never wired at all would be
/// worth nothing, and this makes that failure loud instead.
#[test]
fn typing_after_a_close_reaches_the_window_that_is_drawn() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (kept, _survivor) = opened_at(&mut display, &mut state, &conn, &client, &qh, (1000, 300));
    let (doomed, closing) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    // Twice: the first round trip carries the seat's capabilities to
    // the client, and `get_keyboard` is a request it makes in answer to
    // them, so the keyboard does not exist on the server until the
    // second.
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        client.keyboard.is_some(),
        "the client bound a keyboard; without one this test cannot \
         observe anything"
    );

    // Past both opening animations, and *settled*, which is what the
    // render loop does once a frame.
    //
    // Not housekeeping: `present::open`'s target was captured before
    // `opened_at` moved each window, so until that released transform
    // is retired a pane is still drawn near the origin — where the
    // pointer is sitting, having never been moved. `settle_focus` asks
    // `window_under` first, so without this it answers from the pointer
    // and the topmost-pane arm that this test is about is never
    // reached. It passed that way, for a reason that had nothing to do
    // with the fix.
    state.clock.advance(Duration::from_millis(300));
    state.settle(state.clock.now());
    assert!(
        state.window_under((0.0, 0.0).into()).is_none(),
        "the pointer has not been moved and there is nothing under it, \
         which is what makes this the keyboard's question and not the \
         mouse's"
    );

    // The window the user is working in, which is the one they are
    // about to close.
    state.focus_window(
        &doomed,
        SERIAL_COUNTER.next_serial(),
        crate::script::FocusSource::Script,
    );
    types(&mut state, KEY_A);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.typed,
        vec![(Some(surface_id(&doomed)), KEY_A)],
        "the premise: typing reaches the focused window, and this \
         fixture can see it happen"
    );

    state.close_pane(closing);
    // Past `CLOSING`: the animation has landed, the request has gone
    // out, and the fixture's client never destroys anything — a client
    // still running its quit handlers, which is what the longer grace
    // period exists for.
    state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = state.clock.now();
    state.settle_closing(asked);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    // The situation under test, stated rather than assumed: the window
    // that was closed is drawn at nothing, and the user has not touched
    // the mouse — there has been no press for `window_under` to answer.
    assert!(
        drawn_now(&state, closing, asked).opacity.abs() < f32::EPSILON,
        "the closed window is invisible, which is what makes typing \
         into it silent"
    );

    // And the user keeps typing, which is the whole of the case.
    types(&mut state, KEY_A);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.typed.last(),
        Some(&(Some(surface_id(&kept)), KEY_A)),
        "a keystroke after a close belongs to the window that is drawn, \
         not to the one that was closed: it went to surface {:?}, and \
         the closed window's is {}",
        client.typed.last().and_then(|(surface, _)| *surface),
        surface_id(&doomed)
    );
}

/// **#127 second review, finding 2: suppressing `map_stacked` left the
/// three copies of a pane's position disagreeing.**
///
/// The guard `move_pane` needs is over the *configure*: a client that
/// answers a new size while `present::close` holds `frame.rect` pinned
/// gets its last buffer stretched mid-fade, because those two
/// rectangles are the pair `resizing::factor` divides. `map_stacked`
/// carries a location and no size, so it is no part of that — and
/// suppressing it anyway cost what `move_pane`'s own opening paragraph
/// warns about: *setting the slot without telling the space is undone
/// before the next frame is drawn, silently*. `pane_geometry` reads
/// `real_geometry` for a mapped client, so `sync_panes` copied the
/// stale rectangle straight back over `set_slot`.
///
/// **`sync_panes` is the line that made it a defect**, and a test that
/// only looked at the frame `move_pane` returns on would pass against
/// the bug. The sweep is a workspace switch, a `rescue_offscreen` or a
/// config reload; the refusal is a client that goes on living. Between
/// them the window came back where it had been *closed*, while
/// `Pane::placed` said the layout had moved it — which is the pair
/// `pane_laid_out` hands #124's edge drag.
///
/// **And the last assertion is the one this replaces a comment with.**
/// The client's *size* really is one configure behind while the pane is
/// leaving, because that configure is deliberately never sent. The
/// claim made for that is that the next sweep after the window comes
/// back sends it — the suppression happened before `offers_size` could
/// record the size as told, so re-placing at the same rectangle is
/// still a change. That is asserted here rather than promised there.
#[test]
fn a_window_the_layout_moved_mid_close_comes_back_where_it_was_put() {
    tiled_fixture!(display, state, conn, queue, client, qh);
    let (window, pane) = opened_at(&mut display, &mut state, &conn, &client, &qh, (400, 300));
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );

    let was = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    state.close_pane(pane);
    state
        .clock
        .advance(present::CLOSING + Duration::from_millis(10));
    let asked = state.clock.now();
    state.settle_closing(asked);
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(client.closes.len(), 1, "the request went out");

    // The sweep, mid-close. A different size as well as a different
    // place, so that the configure guard is still being exercised and
    // this is not quietly testing a move nobody suppressed.
    let elsewhere = at(1000, 500, 250, 180);
    let configures = client.configures.len();
    state.move_pane(
        pane,
        elsewhere,
        was,
        AnimationSpec::default(),
        asked,
        Standing::Tile,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert_eq!(
        client.configures.len(),
        configures,
        "the leaving client is still told nothing, which is the guard \
         that belongs here"
    );

    // The next frame. This is where the defect lived: the space was
    // never told, so the space's stale rectangle came back over the
    // slot the sweep had just written.
    state.sync_panes();

    let outer = state
        .pane_outer_of(pane)
        .expect("a mapped pane has a rectangle");
    assert_eq!(
        outer.loc, elsewhere.loc,
        "a pane the layout moved during a close lives where the layout \
         put it, one frame later as well as on the frame it was moved"
    );
    let laid_out = state
        .pane_laid_out(&window)
        .expect("a pane the layout has placed answers this");
    assert_eq!(
        laid_out.0.loc, outer.loc,
        "and the rectangle #124's edge drag starts from is one the \
         window is actually at: `pane_laid_out` says {:?} and the pane \
         is at {:?}",
        laid_out.0.loc, outer.loc
    );

    // The refusal. The client never destroyed anything, so the deadline
    // is what brings the window back.
    let refused = asked + Duration::from_millis(1500);
    state.settle_refused(refused);
    let back = drawn_now(&state, pane, refused + Duration::from_millis(200));
    assert!(
        (back.opacity - 1.0).abs() < f32::EPSILON,
        "the premise for the rest: the window is back on screen, at \
         opacity {}",
        back.opacity
    );
    assert_eq!(
        state
            .pane_outer_of(pane)
            .expect("a mapped pane has a rectangle")
            .loc,
        elsewhere.loc,
        "and it comes back where the layout left it rather than where \
         it was closed"
    );

    // The residue, and the claim made about it. The size is still the
    // one the client last committed, because the configure that would
    // have changed it was suppressed — and the next sweep sends it.
    let configures = client.configures.len();
    state.move_pane(
        pane,
        elsewhere,
        elsewhere,
        AnimationSpec::default(),
        refused + Duration::from_millis(200),
        Standing::Tile,
    );
    pump(
        &mut display,
        &mut state,
        &conn,
        &qh,
        &mut queue,
        &mut client,
    );
    assert!(
        client.configures.len() > configures,
        "the configure the close swallowed is sent by the first sweep \
         after the window comes back, which is what makes the \
         suppression a delay rather than a loss"
    );
}

mod follow;

/// **#364: an X11 window's pointer**, against a real client and a real
/// cursor surface.
///
/// The test client stands in for XWayland: which client XWayland is, is
/// all `Pointer` is told of it, and this binary cannot start XWayland
/// (see `a_managed_x11_window_is_offered_the_keyboard_as_an_xdg_one_is`).
/// What the X server says the cursor is called arrives through
/// `Pointer::x11`, which is all the connection in `cursor::x11` does with
/// a name once it has one.
mod x11_cursor {
    use super::*;

    /// Two monitors side by side, one client's window on the left-hand
    /// one with the pointer over it, and the client's cursor surface set.
    struct Desk {
        display: Display<Solium>,
        state: Solium,
        conn: Connection,
        queue: wayland_client::EventQueue<Client>,
        qh: QueueHandle<Client>,
        client: Client,
        window: Window,
        pointer: wl_pointer::WlPointer,
        cursor: wl_surface::WlSurface,
        /// The serial of the pointer's `enter`, which sets a cursor.
        serial: u32,
        /// The monitor the pointer is on, and the other.
        left: Output,
        right: Output,
    }

    impl Desk {
        fn new() -> Self {
            let mut display = Display::<Solium>::new().expect("creating a test wayland display");
            let mut state = Solium::new(display.handle());
            // See the #99 test: a Qt scene in a process holding a
            // libwayland connection of its own aborts the test binary.
            state
                .decorations
                .set_style(&mut state.panes, Some("none".to_string()));
            let (left, right) = side_by_side(&mut state, "x11-cursor-test");
            let (conn, mut queue, mut client) = connect(&mut display, &mut state);
            let qh = queue.handle();
            let (window, _toplevel, _surface) =
                open_surface(&mut display, &mut state, &conn, &client, &qh);
            state.sync_panes();
            state.space.map_element(window.clone(), (600, 500), false);
            state.space.refresh();
            state.clock.advance(Duration::from_secs(1));
            let now = state.clock.now();
            state.settle(now);
            let pointer = client.seat_pointer(&qh);
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            let over = state
                .space
                .element_geometry(&window)
                .expect("the window is in the space")
                .to_f64();
            move_pointer(
                &mut state,
                (
                    over.loc.x + over.size.w / 2.0,
                    over.loc.y + over.size.h / 2.0,
                ),
                1_000,
            );
            pump(
                &mut display,
                &mut state,
                &conn,
                &qh,
                &mut queue,
                &mut client,
            );
            let serial = client
                .pointer_entered
                .expect("the premise: the window was told the pointer entered");
            let compositor = client.compositor.clone().expect("wl_compositor bound");
            let cursor = compositor.create_surface(&qh, ());
            pointer.set_cursor(serial, Some(&cursor), 0, 0);
            let mut desk = Self {
                display,
                state,
                conn,
                queue,
                qh,
                client,
                window,
                pointer,
                cursor,
                serial,
                left,
                right,
            };
            desk.pump();
            assert!(
                matches!(desk.state.pointer.showing(), CursorImageStatus::Surface(_)),
                "the premise: the client's cursor surface is shown"
            );
            desk
        }

        fn pump(&mut self) {
            pump(
                &mut self.display,
                &mut self.state,
                &self.conn,
                &self.qh,
                &mut self.queue,
                &mut self.client,
            );
        }

        /// This client is XWayland.
        fn be_xwayland(&mut self) {
            let owner = self
                .window
                .wl_surface()
                .and_then(|surface| surface.client())
                .expect("the window has a client")
                .id();
            self.state.pointer.xwayland(owner);
        }

        /// What is drawn, as a shape or as the client's own pixels.
        fn drawn(&mut self) -> (Option<CursorIcon>, bool) {
            let shown = match self.state.pointer.showing() {
                CursorImageStatus::Named(icon) => Some(icon),
                _ => None,
            };
            let pixels = matches!(self.state.pointer.showing(), CursorImageStatus::Surface(_));
            assert_eq!(
                self.state.pointer.shape(),
                shown,
                "the pointer scene was told something other than what is drawn"
            );
            (shown, pixels)
        }
    }

    /// **XWayland's cursor surface is drawn as the shape its X name stands
    /// for**: a name known while the surface is another client's changes
    /// nothing; once the client is XWayland, the surface is drawn as the
    /// shape and the pointer scene is told it; with no name known it is
    /// the client's own pixels again; and a pointer the client hides
    /// stays hidden whatever the X cursor is called.
    #[test]
    fn xwaylands_cursor_surface_is_drawn_as_the_shape_its_x_name_stands_for() {
        let mut desk = Desk::new();
        let text = Some(CursorIcon::Text);
        assert!(
            desk.state.pointer.x11(text),
            "a new name did not ask for a frame"
        );
        assert!(
            !desk.state.pointer.x11(text),
            "the same name asked for a frame"
        );
        assert_eq!(
            desk.drawn(),
            (None, true),
            "a surface not XWayland's was drawn as the X cursor's shape"
        );

        desk.be_xwayland();
        assert_eq!(
            desk.drawn(),
            (text, false),
            "XWayland's surface was not drawn as the X cursor's shape"
        );
        assert!(
            matches!(desk.state.pointer.said(), CursorImageStatus::Surface(_)),
            "the surface was thrown away for the shape"
        );

        assert!(desk.state.pointer.x11(None));
        assert_eq!(
            desk.drawn(),
            (None, true),
            "a cursor with no name known was not the client's own pixels"
        );

        desk.state.pointer.x11(text);
        let serial = desk.serial;
        desk.pointer.set_cursor(serial, None, 0, 0);
        desk.pump();
        assert!(
            matches!(desk.state.pointer.showing(), CursorImageStatus::Hidden)
                && desk.state.pointer.shape().is_none(),
            "a pointer XWayland hid was drawn"
        );
    }

    /// **A cursor surface is told to draw on the pointer's monitor**, and
    /// only there, including while a named shape is drawn in its place:
    /// XWayland attaches the next picture only once the last one's frame
    /// is done, so a surface never told would be stale the moment its X
    /// cursor lost its name.
    ///
    /// With cursor surfaces left out of `Solium::drawn_only_on`, fails at
    /// "the pointer's monitor did not tell the cursor surface".
    #[test]
    fn a_cursor_surface_is_told_to_draw_on_the_pointers_monitor() {
        let mut desk = Desk::new();
        desk.be_xwayland();
        desk.state.pointer.x11(Some(CursorIcon::Text));
        assert_eq!(desk.drawn(), (Some(CursorIcon::Text), false));
        let (left, right) = (desk.left.clone(), desk.right.clone());
        let at = desk
            .state
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default();
        assert_eq!(
            desk.state.output_at(at.to_i32_round()).as_ref(),
            Some(&left),
            "the premise: the pointer is on the left-hand monitor"
        );
        desk.cursor.frame(&desk.qh, FrameDone);
        desk.cursor.commit();
        desk.pump();
        let refresh = Duration::from_millis(16);

        desk.state
            .send_frames_on(&right, Duration::from_millis(16), refresh);
        desk.pump();
        assert_eq!(
            desk.client.frames_done, 0,
            "a monitor the pointer is not on told the cursor surface to draw"
        );
        desk.state
            .send_frames_on(&left, Duration::from_millis(32), refresh);
        desk.pump();
        assert_eq!(
            desk.client.frames_done, 1,
            "the pointer's monitor did not tell the cursor surface to draw"
        );
    }
}
