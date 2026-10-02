//! `zwp_text_input_v3` (text-input-unstable-v3): where the focused text field
//! is, and where its caret is.
//!
//! A client with a text field focused says so with `enable`, and says where
//! its caret is with `set_cursor_rectangle`, in the coordinates of the surface
//! it was told it has text-input focus on; both are applied on `commit`. That
//! is all this keeps: which text field is live, on which window, and its caret.
//! The compositor publishes it as data, in the global space for the
//! configuration (`sol.text_input()`, `sol.on("text_input", ...)`) and in a
//! pane's own space for its decoration (`caret`), and draws nothing of its own
//! with it. `tests::an_enabled_field_has_its_caret_in_the_global_space`.
//!
//! ## Why not smithay's
//!
//! Smithay 0.7 has a text-input module, `wayland/text_input/`, and it is half
//! of an input method: it discards every request while no input method client
//! is connected (`text_input_handle.rs`, "discarding text-input request
//! without IME running"), sends `enter` only once there is one, and hands the
//! cursor rectangle to the input method's popup and to nobody else. With no
//! input method, which is Solium today, a client could never enable a field
//! and its caret would never be known. So the protocol is answered here, from
//! the generated bindings, and it is small: text-input focus follows the
//! keyboard (`enter` and `leave`), and the rest is three double-buffered
//! requests. No text is ever sent to a client: that needs an input method,
//! which is #26's other half, and would be bridged to these same objects.

use smithay::{
    desktop::{PopupManager, Window, find_popup_root_surface},
    reexports::{
        wayland_protocols::wp::text_input::zv3::server::{
            zwp_text_input_manager_v3::{self, ZwpTextInputManagerV3},
            zwp_text_input_v3::{self, ZwpTextInputV3},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point, Rectangle},
    wayland::compositor::{SubsurfaceCachedState, get_parent, with_states},
};

use crate::{pane::PaneId, state::Solium};

/// The global, every text-input object a client holds, and the surface they
/// follow.
#[derive(Debug)]
pub(crate) struct TextInputs {
    #[expect(
        dead_code,
        reason = "registers zwp_text_input_manager_v3; dropping it would remove the global"
    )]
    global: GlobalId,
    instances: Vec<Instance>,
    /// The surface the keyboard is on, which text-input focus follows.
    focus: Option<WlSurface>,
    /// The caret in the pane-local space of the pane whose window has it, as
    /// last worked out for the decorations: once a frame, by
    /// [`Solium::settle_caret`].
    pane_caret: Option<(PaneId, Rectangle<i32, Logical>)>,
}

/// One `zwp_text_input_v3`.
#[derive(Debug)]
struct Instance {
    object: ZwpTextInputV3,
    /// What the requests since the last `commit` asked for.
    pending: Pending,
    /// Whether its last commit left it enabled, so a text field is focused.
    enabled: bool,
    /// The caret its last commit left, in the coordinates of `entered`.
    cursor: Option<Rectangle<i32, Logical>>,
    /// The surface it was sent `enter` for and not yet `leave`. Requests from
    /// an object that has none are ignored, as the protocol says.
    entered: Option<WlSurface>,
}

/// Double-buffered state, applied on `commit`.
#[derive(Debug, Default)]
struct Pending {
    enable: Option<bool>,
    cursor: Option<Rectangle<i32, Logical>>,
}

/// The focused text field, as the configuration is told it: the window it is
/// in, by the id scripts hold windows by, and its caret in the global space,
/// once the client has said where that is.
/// `tests::an_enabled_field_has_its_caret_in_the_global_space`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Field {
    pub(crate) window: u64,
    pub(crate) caret: Option<Rectangle<f64, Logical>>,
}

impl TextInputs {
    /// Registers `zwp_text_input_manager_v3` for every client: a client saying
    /// where its caret is grants it nothing.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        Self {
            global: display.create_global::<Solium, ZwpTextInputManagerV3, _>(1, ()),
            instances: Vec::new(),
            focus: None,
            pane_caret: None,
        }
    }

    /// The enabled text input on the surface with text-input focus, and that
    /// surface.
    fn active(&self) -> Option<(&Instance, &WlSurface)> {
        let focus = self.focus.as_ref()?;
        self.instances
            .iter()
            .find(|instance| instance.enabled && instance.entered.as_ref() == Some(focus))
            .map(|instance| (instance, focus))
    }
}

impl GlobalDispatch<ZwpTextInputManagerV3, ()> for Solium {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpTextInputManagerV3>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwpTextInputManagerV3, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwpTextInputManagerV3,
        request: zwp_text_input_manager_v3::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let zwp_text_input_manager_v3::Request::GetTextInput { id, .. } = request else {
            return;
        };
        let object = data_init.init(id, ());
        // A client may bind late, after its surface already has the keyboard,
        // and is told so at once.
        // `tests::a_text_input_made_after_the_keyboard_arrived_is_entered_at_once`.
        let entered = state
            .text_inputs
            .focus
            .clone()
            .filter(|focus| focus.id().same_client_as(&object.id()));
        if let Some(surface) = entered.as_ref() {
            object.enter(surface);
        }
        state.text_inputs.instances.push(Instance {
            object,
            pending: Pending::default(),
            enabled: false,
            cursor: None,
            entered,
        });
    }
}

impl Dispatch<ZwpTextInputV3, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        object: &ZwpTextInputV3,
        request: zwp_text_input_v3::Request,
        (): &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let Some(instance) = state
            .text_inputs
            .instances
            .iter_mut()
            .find(|instance| &instance.object == object)
        else {
            return;
        };
        match request {
            // Enabling is a new field, so it starts with no caret.
            zwp_text_input_v3::Request::Enable => {
                instance.pending = Pending {
                    enable: Some(true),
                    cursor: None,
                };
            }
            zwp_text_input_v3::Request::Disable => instance.pending.enable = Some(false),
            zwp_text_input_v3::Request::SetCursorRectangle {
                x,
                y,
                width,
                height,
            } => {
                instance.pending.cursor =
                    Some(Rectangle::new((x, y).into(), (width, height).into()));
            }
            zwp_text_input_v3::Request::Commit => {
                let pending = std::mem::take(&mut instance.pending);
                // Not focused, nothing it says is applied.
                // `tests::a_field_whose_window_loses_the_keyboard_is_gone`.
                if instance.entered.is_none() {
                    return;
                }
                let was = (instance.enabled, instance.cursor);
                match pending.enable {
                    Some(true) => (instance.enabled, instance.cursor) = (true, None),
                    Some(false) => (instance.enabled, instance.cursor) = (false, None),
                    None => {}
                }
                if instance.enabled && pending.cursor.is_some() {
                    instance.cursor = pending.cursor;
                }
                tracing::debug!(
                    enabled = instance.enabled,
                    cursor = ?instance.cursor,
                    "a text field committed"
                );
                let changed = was != (instance.enabled, instance.cursor);
                let enabled = pending.enable == Some(true) && instance.enabled;
                state.text_field_changed(changed, enabled);
            }
            // The surrounding text, its change cause and the content type are
            // an input method's to read, and there is none.
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _client: ClientId, object: &ZwpTextInputV3, (): &()) {
        let before = state.text_inputs.active().is_some();
        state
            .text_inputs
            .instances
            .retain(|instance| &instance.object != object);
        // `tests::a_field_whose_window_loses_the_keyboard_is_gone`.
        let gone = before && state.text_inputs.active().is_none();
        state.text_field_changed(gone, false);
    }
}

impl Solium {
    /// Text-input focus follows the keyboard: `leave` for the surface it is
    /// leaving, which takes every field there away, and `enter` for every
    /// text input of the client it arrives at. Called from the seat's
    /// `focus_changed`, so wherever the keyboard goes, this goes.
    /// `tests::a_field_whose_window_loses_the_keyboard_is_gone`,
    /// `tests::a_field_focused_again_is_told_again`.
    pub(crate) fn text_input_focus(&mut self, focused: Option<&WlSurface>) {
        if self.text_inputs.focus.as_ref() == focused {
            return;
        }
        let had = self.text_inputs.active().is_some();
        self.text_inputs.focus = focused.cloned();
        for instance in &mut self.text_inputs.instances {
            if let Some(left) = instance.entered.take() {
                instance.object.leave(&left);
            }
            instance.enabled = false;
            instance.cursor = None;
            instance.pending = Pending::default();
            if let Some(surface) = focused
                && surface.id().same_client_as(&instance.object.id())
            {
                instance.object.enter(surface);
                instance.entered = Some(surface.clone());
            }
        }
        self.text_field_changed(had, false);
    }

    /// What a text field changing means: a frame, so a decoration drawing
    /// at the caret moves with it, and a `text_input` event for the
    /// configuration when a field was enabled or focused.
    /// `tests::text_input_is_told_when_a_field_is_enabled_and_when_it_is_focused`.
    fn text_field_changed(&mut self, changed: bool, enabled: bool) {
        if changed || enabled {
            self.redraw = true;
        }
        if enabled && self.text_field().is_some() {
            self.trigger_text_input();
        }
    }

    /// Run the `text_input` listeners, and apply what they asked for.
    fn trigger_text_input(&mut self) {
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            return;
        };
        let outcome = scripts.text_input(snapshot);
        self.scripts = Some(scripts);
        self.apply(outcome);
    }

    /// The focused text field, with its caret in the global space: the
    /// caret's place in its window, through the window's place on screen and
    /// the scale it is drawn at, presentation transforms included, the way a
    /// press is mapped back into it. A 3D tilt is not followed, as a press
    /// is not. `None` when no window has a text field enabled.
    /// `tests::an_enabled_field_has_its_caret_in_the_global_space`,
    /// `tests::the_caret_follows_its_window_moved_and_presented`.
    pub(crate) fn text_field(&self) -> Option<Field> {
        let (instance, surface) = self.text_inputs.active()?;
        let (window, offset) = surface_in_window(self, surface)?;
        let id = self.panes.id_of(&window)?;
        let pane = self.panes.get(id)?;
        let caret = instance.cursor.map(|cursor| {
            let outer = self.pane_outer(pane);
            let frame = self.drawn(id, outer);
            let placed =
                crate::render::place_client(self, pane, &frame, outer.size, window.geometry().size);
            let factor = placed.fit.factor;
            let at = offset + cursor.loc;
            Rectangle::new(
                (
                    placed.origin.x + f64::from(at.x) * factor.x,
                    placed.origin.y + f64::from(at.y) * factor.y,
                )
                    .into(),
                (
                    f64::from(cursor.size.w) * factor.x,
                    f64::from(cursor.size.h) * factor.y,
                )
                    .into(),
            )
        });
        Some(Field {
            window: id.get(),
            caret,
        })
    }

    /// The caret in its pane's own space, the space a decoration is laid out
    /// in: past the frame's insets, and before any presentation transform,
    /// which the decoration is drawn through with its window. Worked out
    /// once a frame, for every decoration to ask [`Self::caret_in`].
    /// `tests::only_the_pane_whose_window_has_the_caret_is_given_it`.
    pub(crate) fn settle_caret(&mut self) {
        self.text_inputs.pane_caret = self.pane_caret();
    }

    fn pane_caret(&self) -> Option<(PaneId, Rectangle<i32, Logical>)> {
        let (instance, surface) = self.text_inputs.active()?;
        let cursor = instance.cursor?;
        let (window, offset) = surface_in_window(self, surface)?;
        let id = self.panes.id_of(&window)?;
        let insets = self.insets_of(id);
        let at = offset + cursor.loc + Point::from((insets.left, insets.top));
        Some((id, Rectangle::new(at, cursor.size)))
    }

    /// The caret, if it is in this pane's window, in the pane's own space.
    /// `tests::only_the_pane_whose_window_has_the_caret_is_given_it`.
    pub(crate) fn caret_in(&self, pane: PaneId) -> Option<Rectangle<i32, Logical>> {
        self.text_inputs
            .pane_caret
            .filter(|(id, _)| *id == pane)
            .map(|(_, caret)| caret)
    }
}

/// The window `surface` belongs to, and where the surface's origin is in that
/// window's own space, the one its geometry is measured from: down through
/// every subsurface to its root, and from a popup's root to the toplevel it
/// was opened from, as the renderer places them.
/// `tests::a_caret_in_a_popup_is_where_the_popup_is`.
fn surface_in_window(state: &Solium, surface: &WlSurface) -> Option<(Window, Point<i32, Logical>)> {
    let mut offset = Point::<i32, Logical>::default();
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        offset += with_states(&root, |states| {
            states
                .cached_state
                .get::<SubsurfaceCachedState>()
                .current()
                .location
        });
        root = parent;
    }
    if let Some(window) = state.window_for(&root) {
        let geometry = window.geometry().loc;
        return Some((window, offset - geometry));
    }
    let popup = state.popups.find_popup(&root)?;
    let toplevel = find_popup_root_surface(&popup).ok()?;
    let window = state.window_for(&toplevel)?;
    let (_, at) =
        PopupManager::popups_for_surface(&toplevel).find(|(each, _)| each.wl_surface() == &root)?;
    Some((window, offset + at - popup.geometry().loc))
}

#[cfg(test)]
pub(crate) mod tests;
