//! Window frames, drawn by the compositor from QML.
//!
//! One QML scene per decorated window, rendered by the same host as the bar —
//! see `qml.rs`. Each scene re-rasterises only when Qt says it changed, so a
//! screen full of idle windows costs a comparison per window per frame rather
//! than a rasterisation per window per frame. That is the whole reason this can
//! be one scene per window and not one shared atlas.
//!
//! **The frame reserves its height.** A window's *outer* rect is its client
//! rect grown upward by [`TITLEBAR_HEIGHT`], the client is placed below the
//! frame, and every presentation transform applies to the outer rect. Frame and
//! window therefore scale, move and animate as one object — in overview a
//! thumbnail carries its own titlebar — and the client area is never covered.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use anyhow::Result;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    reexports::wayland_server::backend::ObjectId,
    utils::{Buffer as BufferCoords, Logical, Rectangle, Size, Transform},
};

use crate::qml;

/// How tall a window frame is, and so how much of a window's slot is not
/// client area.
pub(crate) const TITLEBAR_HEIGHT: i32 = 32;

/// Identical renders in a row before a frame is left alone.
///
/// Small enough that an idle window costs almost nothing, large enough that an
/// animation which has been started but has not yet moved a pixel is not
/// mistaken for one that has finished.
const SETTLED: u8 = 4;

/// How often a frame's own animation is advanced.
///
/// Chrome, not content: a sheen crossing a titlebar reads the same at 60 as at
/// 260, and the scene is rasterised on the CPU, so the difference is most of
/// what an animating decoration costs. Anything *set* on a frame still renders
/// at once -- what is rate-limited is time passing, not the user acting.
const ANIMATION_INTERVAL: Duration = Duration::from_micros(16_666);

/// What a frame button asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Close,
    ToggleMaximize,
}

impl Action {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "close" => Some(Self::Close),
            "maximize" => Some(Self::ToggleMaximize),
            _ => {
                tracing::warn!(name, "the titlebar asked for something unknown");
                None
            }
        }
    }
}

/// How much of a window's slot the frame takes, on each side.
///
/// The decoration decides this, not the compositor: a bar along the left, a
/// bar underneath and a plain border are the same mechanism with different
/// numbers, and hardcoding "32 pixels at the top" is what stops them being
/// writable as ordinary QML.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Insets {
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
    pub(crate) left: i32,
}

impl Insets {
    /// No frame at all: an undecorated window, or one whose decoration draws
    /// only inside the window's own bounds.
    pub(crate) const NONE: Self = Self {
        top: 0,
        right: 0,
        bottom: 0,
        left: 0,
    };

    /// Width taken from the client.
    pub(crate) const fn horizontal(self) -> i32 {
        self.left + self.right
    }

    /// Height taken from the client.
    pub(crate) const fn vertical(self) -> i32 {
        self.top + self.bottom
    }

    /// The frame's own area, as up to four rectangles around a client of
    /// `width` by `height` including the insets.
    ///
    /// Corners belong to the top and bottom bands, so the sides do not overlap
    /// them: a region copied twice is a region copied twice.
    pub(crate) fn bands(self, width: i32, height: i32) -> Vec<Rectangle<i32, BufferCoords>> {
        let middle = (height - self.vertical()).max(0);
        [
            Rectangle::new((0, 0).into(), (width, self.top).into()),
            Rectangle::new(
                (0, height - self.bottom).into(),
                (width, self.bottom).into(),
            ),
            Rectangle::new((0, self.top).into(), (self.left, middle).into()),
            Rectangle::new(
                (width - self.right, self.top).into(),
                (self.right, middle).into(),
            ),
        ]
        .into_iter()
        .filter(|band| band.size.w > 0 && band.size.h > 0)
        .collect()
    }

    /// Whether there is any frame to draw.
    pub(crate) const fn any(self) -> bool {
        self.top != 0 || self.right != 0 || self.bottom != 0 || self.left != 0
    }
}

/// What a frame is told about the window it belongs to.
///
/// One argument rather than three: they are set together, compared together,
/// and a decoration that grows a fourth thing to know should not change every
/// call site.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Look<'a> {
    pub(crate) title: &'a str,
    pub(crate) focused: bool,
    /// Whether the pointer is anywhere over the window, frame or client.
    pub(crate) pointer_inside: bool,
}

/// What a frame shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Shown {
    title: String,
    focused: bool,
    /// Whether the pointer is anywhere over the window, frame or client. A
    /// border that lights up when you approach the window needs to know this
    /// even while the pointer is over the client, which is not ours.
    pointer_inside: bool,
}

/// One window's frame.
#[derive(Debug)]
pub(crate) struct Decoration {
    scene: qml::Scene,
    /// What the QML asked to reserve, read once when it was built.
    insets: Insets,
    /// Whether the frame paints outside the space it reserved.
    ///
    /// A frame that stays inside its own bands only has to have those bands
    /// copied and uploaded each time it changes; one that draws over the
    /// client -- a bar floating above the window, a glow across it -- has to
    /// have all of it copied, because anything in it may have moved.
    overlay: bool,
    buffer: Option<MemoryRenderBuffer>,
    buffer_size: (i32, i32),
    shown: Shown,
    /// How many consecutive renders have produced nothing new.
    ///
    /// Not a flag, and the difference matters: the frame after a property is
    /// set looks exactly like the frame before it, because an animation
    /// started by that property has not moved yet. Stopping on the first
    /// unchanged render therefore freezes every transition at its starting
    /// value -- a border that never lights, a bar that never slides out. It
    /// takes a few identical frames in a row to mean "settled".
    still: u8,
    /// Pixels rendered but not yet copied into the buffer, because the buffer
    /// is created after the render: stride and rows, as the scene gave them.
    pending: Option<(usize, Vec<u8>)>,
    /// When the scene was last driven, for `ANIMATION_INTERVAL`.
    advanced_at: Duration,
    /// Where the window was before it was maximised. `Some` means maximised —
    /// one field rather than a flag and a rect that can disagree.
    pub(crate) restore: Option<Rectangle<i32, Logical>>,
}

impl Decoration {
    fn new(path: &std::path::Path, width: i32, height: i32) -> Result<Self> {
        qml::start()?;
        // Built at the client's size first, because what it reserves is a
        // property of the scene and there is no scene to ask until it exists.
        let mut scene = qml::Scene::new(path, width.max(1), height.max(1))?;
        let insets = Insets {
            top: scene.get_int("insetTop").max(0),
            right: scene.get_int("insetRight").max(0),
            bottom: scene.get_int("insetBottom").max(0),
            left: scene.get_int("insetLeft").max(0),
        };
        // A frame with nothing reserved has nowhere else to paint but over
        // the client, so it is an overlay whether it says so or not.
        let overlay = scene.get_bool("overlay") || !insets.any();
        // ...and then grown to the whole outer rect, which is what it draws:
        // the client area within it is simply left transparent.
        scene.resize(
            (width + insets.horizontal()).max(1),
            (height + insets.vertical()).max(1),
        );
        Ok(Self {
            scene,
            insets,
            overlay,
            buffer: None,
            buffer_size: (0, 0),
            shown: Shown::default(),
            still: 0,
            pending: None,
            advanced_at: Duration::ZERO,
            restore: None,
        })
    }

    /// Draw the frame and return it as a render element.
    ///
    /// `rect` is where the frame is *drawn*, which in a mode is not where the
    /// window lives; the scene is always rasterised at its unscaled size and
    /// the element scales it, so a thumbnail's titlebar costs no more than a
    /// full-size one.
    /// The client's size, inside whatever this frame reserves.
    fn client_size(&self) -> (i32, i32) {
        (
            (self.buffer_size.0 - self.insets.horizontal()).max(1),
            (self.buffer_size.1 - self.insets.vertical()).max(1),
        )
    }

    /// Whether the frame's own animations still have somewhere to go.
    ///
    /// A decoration animates on its own clock -- a border easing to a new
    /// colour, a bar sliding out, a sheen crossing a titlebar -- and the
    /// compositor only draws when something has damaged the screen. Nothing
    /// the client did damaged it, so unless the frame says it is still moving,
    /// rendering stops and the animation freezes wherever it happened to be.
    pub(crate) fn animating(&self) -> bool {
        self.still < SETTLED
    }

    /// What this frame reserves around its client.
    pub(crate) fn insets(&self) -> Insets {
        self.insets
    }

    /// Draw the frame over the whole of `rect`, at `outer` pixels.
    ///
    /// `rect` is where the window is drawn, which a presentation transform may
    /// have scaled; `outer` is its unscaled size, which is what the scene is
    /// rasterised at. Keeping them apart is what lets a frame scale with its
    /// window in overview without the text being re-laid out every frame.
    pub(crate) fn frame<R>(
        &mut self,
        renderer: &mut R,
        rect: Rectangle<f64, Logical>,
        outer: Size<i32, Logical>,
        look: &Look<'_>,
        now: Duration,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let width = outer.w.max(1);
        let height = outer.h.max(1);
        self.scene.resize(width, height);

        // Compared field by field rather than by building a `Shown`: this runs
        // for every window of every frame, and the title is the one thing here
        // that allocates.
        let Look {
            title,
            focused,
            pointer_inside,
        } = *look;
        // Whether anything was set on the frame this time round. What the user
        // did is drawn immediately; what the clock did can wait for the next
        // interval.
        let mut told = false;
        if self.shown.title != title
            || self.shown.focused != focused
            || self.shown.pointer_inside != pointer_inside
        {
            self.scene.set_string("title", title);
            self.scene.set_bool("focused", focused);
            self.scene.set_bool("pointerInside", pointer_inside);
            // The client's own size, so a decoration can place things against
            // the window rather than against itself.
            self.scene
                .set_int("contentWidth", width - self.insets.horizontal());
            self.scene
                .set_int("contentHeight", height - self.insets.vertical());
            self.shown.title.clear();
            self.shown.title.push_str(title);
            self.shown.focused = focused;
            self.shown.pointer_inside = pointer_inside;
            self.still = 0;
            told = true;
        }

        let size = (width, height);
        let resized = self.buffer_size != size;
        if resized {
            self.still = 0;
        }

        // Driving Qt's scene graph every frame for a screen full of idle
        // windows buys identical pixels, so a frame that has stopped moving
        // stops being driven -- but only after several identical frames in a
        // row, for the reason `still` explains.
        let due = now.saturating_sub(self.advanced_at) >= ANIMATION_INTERVAL;
        if self.animating() && (told || resized || due) {
            self.advanced_at = now;
            self.scene.advance(now);
            match self.scene.render() {
                Ok(rendered) => {
                    if rendered.changed || resized {
                        self.pending = Some((rendered.stride, rendered.pixels.to_vec()));
                    }
                    self.still = if rendered.changed {
                        0
                    } else {
                        self.still.saturating_add(1)
                    };
                }
                Err(err) => {
                    tracing::warn!(?err, "a window frame did not render");
                    return None;
                }
            }
            // A loop with a pause in it produces identical frames while it
            // waits, which is indistinguishable from having finished -- so a
            // decoration that keeps going says so, and is kept driven. Read
            // after rendering because it is a binding: it can only answer once
            // the scene has been evaluated.
            if self.scene.get_bool("animating") {
                self.still = 0;
            }
        }

        if self.buffer.is_none() || resized {
            self.buffer = Some(MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                size,
                1,
                Transform::Normal,
                None,
            ));
            self.buffer_size = size;
        }
        let buffer = self.buffer.as_mut()?;

        if let Some((stride, pixels)) = self.pending.take() {
            // Only the parts of the frame that can have changed. A titlebar on
            // a 1150x850 window is 4% of it; copying and uploading the other
            // 96% every frame is what an animating decoration used to cost,
            // and it costs the same whether anything happened there or not.
            let regions = if self.overlay || resized {
                vec![Rectangle::from_size(size.into())]
            } else {
                self.insets.bands(size.0, size.1)
            };
            let mut context = buffer.render();
            let copy = context.draw(|target| {
                let row_bytes = usize::try_from(size.0.max(0)).unwrap_or_default() * 4;
                for region in &regions {
                    let (x, y) = (region.loc.x.max(0), region.loc.y.max(0));
                    let width = usize::try_from(region.size.w.max(0)).unwrap_or_default() * 4;
                    let left = usize::try_from(x).unwrap_or_default() * 4;
                    for row in y..y.saturating_add(region.size.h.max(0)) {
                        let row = usize::try_from(row).unwrap_or_default();
                        let from = row * stride + left;
                        let into = row * row_bytes + left;
                        let (Some(source), Some(destination)) = (
                            pixels.get(from..from + width),
                            target.get_mut(into..into + width),
                        ) else {
                            return Err(());
                        };
                        destination.copy_from_slice(source);
                    }
                }
                Ok(regions.clone())
            });
            if copy.is_err() {
                tracing::warn!("a frame image was smaller than its buffer");
                return None;
            }
        }

        // The drawn size scales the frame with the window it belongs to.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a frame is at most an output wide"
        )]
        let drawn: Size<i32, Logical> = (
            rect.size.w.round() as i32,
            rect.size.h.round().max(1.0) as i32,
        )
            .into();

        // `src` must be given whenever `size` is. Smithay defaults it to the
        // *drawn* size, which crops the buffer to its top-left corner instead
        // of scaling it — at full size the two are equal and it looks correct,
        // and it only shows up once a mode scales the frame down: the title
        // slides right and the buttons vanish off the edge.
        let source = Rectangle::from_size((f64::from(width), f64::from(height)).into());

        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (rect.loc.x, rect.loc.y),
            buffer,
            None,
            Some(source),
            Some(drawn),
            Kind::Unspecified,
        )
        .inspect_err(|err| tracing::warn!(?err, "could not upload a window frame"))
        .ok()
    }

    /// Tell the frame the pointer has left the window altogether.
    ///
    /// Sent as a position rather than a flag as well, because QML's hover
    /// handling is positional: a `MouseArea` that never sees the pointer leave
    /// stays hovered forever, and a border lit by proximity stays lit.
    pub(crate) fn pointer_left(&mut self) {
        self.scene.pointer(-1.0, -1.0, None);
        self.scene.set_bool("pointerInside", false);
        self.shown.pointer_inside = false;
        self.still = 0;
    }

    /// Pointer input in frame-local coordinates.
    pub(crate) fn pointer(&mut self, x: f64, y: f64, pressed: Option<bool>) {
        self.scene.pointer(x, y, pressed);
    }

    /// Whether the pointer is over a button.
    ///
    /// Asked of QML rather than worked out from coordinates: QML owns the
    /// button layout, so a copy of it here would be a second authority that
    /// drifts the first time the frame is restyled.
    pub(crate) fn on_button(&self) -> bool {
        self.scene.get_bool("onButton")
    }

    /// What a button asked for since the last call, if anything.
    pub(crate) fn take_action(&mut self) -> Option<Action> {
        self.scene
            .take_string("action")
            .as_deref()
            .and_then(Action::parse)
    }
}

/// Every decorated window's frame.
///
/// Keyed by the toplevel's surface id, and *presence means decorated*: a client
/// that negotiated client-side decorations has no entry, so it draws its own
/// frame and the compositor draws none. Two frames on one window is what
/// happens when this is a flag instead of a lookup.
#[derive(Debug, Default)]
pub(crate) struct Decorations {
    frames: HashMap<ObjectId, Decoration>,
    /// Which decoration to build, as a script named it. `None` is whatever
    /// the environment or the default says.
    style: Option<String>,
}

impl Decorations {
    /// Start decorating a window, if it is not decorated already.
    /// Choose the decoration every window is framed with.
    ///
    /// Existing frames are rebuilt from the new file rather than dropped:
    /// nothing re-creates a frame on its own -- they are made when a client
    /// negotiates its decoration mode, once -- so dropping them would leave
    /// every open window bare until it was reopened. Rebuilding is what makes
    /// this a live setting a script can change and watch happen.
    pub(crate) fn set_style(&mut self, style: Option<String>) -> bool {
        if self.style == style {
            return false;
        }
        self.style = style;
        let path = qml_path(self.style.as_deref());
        let existing: Vec<(ObjectId, (i32, i32))> = self
            .frames
            .iter()
            .map(|(id, frame)| (id.clone(), frame.client_size()))
            .collect();
        for (id, (width, height)) in existing {
            match Decoration::new(&path, width, height) {
                Ok(fresh) => {
                    self.frames.insert(id, fresh);
                }
                Err(err) => {
                    tracing::error!(?err, "could not load the new decoration, leaving it bare");
                    self.frames.remove(&id);
                }
            }
        }
        true
    }

    pub(crate) fn insert(&mut self, id: ObjectId, width: i32, height: i32) {
        if self.frames.contains_key(&id) {
            return;
        }
        match Decoration::new(&qml_path(self.style.as_deref()), width, height) {
            Ok(decoration) => {
                self.frames.insert(id, decoration);
            }
            Err(err) => {
                // An undecorated window is worse than a decorated one and much
                // better than no window.
                tracing::error!(?err, "could not load a window frame, leaving it bare");
            }
        }
    }

    pub(crate) fn remove(&mut self, id: &ObjectId) {
        if self.frames.remove(id).is_some() {
            tracing::debug!("dropped a window frame");
        }
    }

    /// Which decoration is in use, if a script chose one.
    pub(crate) fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }

    pub(crate) fn get(&self, id: &ObjectId) -> Option<&Decoration> {
        self.frames.get(id)
    }

    pub(crate) fn get_mut(&mut self, id: &ObjectId) -> Option<&mut Decoration> {
        self.frames.get_mut(id)
    }

    /// How many frames are being kept. For leak diagnostics: this should
    /// return to what it was once every window is closed.
    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    pub(crate) fn contains(&self, id: &ObjectId) -> bool {
        self.frames.contains_key(id)
    }
}

/// Where the frame's QML lives.
///
/// Overridable so a frame can be restyled and reloaded without a rebuild,
/// which is most of the point of authoring it in QML.
/// Which QML file draws window frames.
///
/// A name picks one of the decorations that ship with the compositor; a path
/// picks anyone else's. Writing a decoration is writing a QML file and setting
/// this -- there is nothing else to build, and no compositor code to touch.
///
/// ```sh
/// SOLIUM_DECORATION=left          # qml/decorations/left.qml
/// SOLIUM_DECORATION=~/mine.qml    # anywhere
/// ```
fn qml_path(style: Option<&str>) -> PathBuf {
    // The old name still works: it was a path to a titlebar, and it is a path
    // to a decoration now.
    if let Some(path) = std::env::var_os("SOLIUM_QML_TITLEBAR") {
        return PathBuf::from(path);
    }
    // The environment wins over the configuration, not the other way round.
    // The configuration always names something -- the shipped one says "top"
    // -- so a script's choice losing to nothing would be one thing, but a
    // script's choice *winning* leaves `SOLIUM_DECORATION` with no effect at
    // all. It is set by whoever started this particular run, to try one
    // decoration for one session, and that is the more specific intent.
    let chosen = std::env::var("SOLIUM_DECORATION")
        .ok()
        .or_else(|| style.map(ToOwned::to_owned));
    let Some(name) = chosen else {
        return shipped_decoration("top");
    };
    if name.contains('/') || name.ends_with(".qml") {
        return PathBuf::from(shellexpand(&name));
    }
    // A file of the same name in the user's own directory shadows the one that
    // ships, so `decoration = "top"` can mean the user's idea of a top bar.
    if let Some(user) = qml::user_qml_dir() {
        let theirs = user.join("decorations").join(format!("{name}.qml"));
        if theirs.is_file() {
            return theirs;
        }
    }
    shipped_decoration(&name)
}

/// One of the decorations that ship with the compositor, by name.
fn shipped_decoration(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/decorations"))
        .join(format!("{name}.qml"))
}

/// Expand a leading `~`, since this is read from an environment variable and
/// nothing else will have done it.
fn shellexpand(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => {
            std::env::var("HOME").map_or_else(|_| path.to_owned(), |home| format!("{home}/{rest}"))
        }
        None => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_names_map_to_actions() {
        assert_eq!(Action::parse("close"), Some(Action::Close));
        assert_eq!(Action::parse("maximize"), Some(Action::ToggleMaximize));
        // Unknown names are ignored rather than guessed at: a typo in QML must
        // not close a window.
        assert_eq!(Action::parse("clos"), None);
        assert_eq!(Action::parse(""), None);
    }
}
