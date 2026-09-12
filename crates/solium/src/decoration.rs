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

use std::{collections::HashMap, path::PathBuf};

use crate::pane::PaneId;

use anyhow::Result;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
            gles::GlesRenderer,
        },
    },
    utils::{Buffer as BufferCoords, Logical, Rectangle, Size, Transform},
};

use crate::{
    qml::{
        self,
        paint::{Gpu, Placement},
    },
    render::{Drawn, Element},
};

/// How tall a window frame is, and so how much of a window's slot is not
/// client area.
pub(crate) const TITLEBAR_HEIGHT: i32 = 32;

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

    /// The same bands, with the insets taken in device pixels.
    ///
    /// `width` and `height` are the buffer's own pixels, so the insets — which
    /// are logical — have to be scaled to match. Reusing `bands` on a 2x screen
    /// would copy a band half as tall as the titlebar and leave the rest of it
    /// showing whatever was in the buffer before.
    pub(crate) fn bands_at(
        self,
        width: i32,
        height: i32,
        scale: f64,
    ) -> Vec<Rectangle<i32, BufferCoords>> {
        #[expect(clippy::cast_possible_truncation, reason = "an inset on this screen")]
        let up = |edge: i32| (f64::from(edge) * scale).ceil() as i32;
        Self {
            top: up(self.top),
            right: up(self.right),
            bottom: up(self.bottom),
            left: up(self.left),
        }
        .bands(width, height)
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

/// How a rasterised frame reaches the screen.
///
/// Not a preference, and not this module's decision: Qt fixes its scene graph
/// for the life of the process and a host that came up on one backend refuses
/// scenes of the other kind, so this follows `qml::on_gpu` — see
/// [`qml::Scene::for_host`], which is where it is actually decided.
#[derive(Debug)]
enum Backing {
    /// Rasterised on the CPU into a shared-memory buffer the compositor
    /// uploads, band by band.
    Memory(Option<MemoryRenderBuffer>),
    /// Drawn by Qt into a dmabuf we allocated, which the compositor samples. A
    /// window resize is a new buffer, which [`Gpu`] answers by rebinding rather
    /// than by rebuilding the scene: a frame rebuilt on every frame of a resize
    /// is a frame whose own animations never advance.
    Gpu(Gpu),
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
    backing: Backing,
    /// The device-pixel size the frame was last drawn at.
    ///
    /// Kept on both paths, because `client_size` is asked for it while a style
    /// is being swapped and the answer must not depend on which path this is.
    buffer_size: (i32, i32),
    shown: Shown,
    /// Where the window was before it was maximised. `Some` means maximised —
    /// one field rather than a flag and a rect that can disagree.
    pub(crate) restore: Option<Rectangle<i32, Logical>>,
}

/// A decoration animates on its own clock -- a border easing to a new colour, a
/// bar sliding out, a sheen crossing a titlebar -- and the compositor only draws
/// when something has damaged the screen. Nothing the client did damaged it, so
/// unless the frame says it is still moving, rendering stops and the animation
/// freezes wherever it happened to be.
impl crate::render::Painted for Decoration {
    fn something_new_to_draw(&self) -> bool {
        self.scene.needs_render()
    }

    fn animation_in_flight(&self) -> bool {
        self.scene.animation_in_flight()
    }
}

impl Decoration {
    fn new(path: &std::path::Path, width: i32, height: i32) -> Result<Self> {
        qml::start()?;
        // Built before it can be asked anything, because what it reserves is a
        // property of the scene and there is no scene to ask until it exists.
        //
        // At the client's size on the software path, where the scene is about
        // to be laid out at the outer rect anyway and a `QImage` of either size
        // costs the same to throw away. **At 1x1 on the GPU path**, which
        // `ShellSurface::new` does for the same reason: a GPU scene's buffer is
        // a GBM allocation, the first `frame` call rebinds onto the outer rect
        // in the monitor's pixels regardless, and building at the client size
        // means ~3.9 MB allocated, handed to Qt, imported and freed again for
        // every window that opens.
        //
        // Nothing is lost by it. The insets are read once here and never
        // re-read, and the scene is then laid out at a size that is not this
        // one either, so a decoration whose insets depended on its size has
        // already been getting an answer computed from the wrong size on both
        // paths — they are constants by contract, and every decoration that
        // ships declares them as literals.
        //
        // **What did change, and is worth knowing before someone relaxes that
        // contract.** Before this, both paths read the insets from a
        // client-sized scene, so a size-dependent inset gave the *same* wrong
        // answer on each. Now the GPU path reads them from a 1x1 scene and the
        // software path still reads them from a client-sized one, so it would
        // give two *different* wrong answers — a window whose frame reserved
        // one strip of space on a TTY and another under winit, from one QML
        // file. Benign today, and only because all eight shipped decorations
        // declare `insetTop`/`Right`/`Bottom`/`Left` and `overlay` as literals.
        // The day one of them binds an inset to `width` or `height`, this stops
        // being a shared inaccuracy and becomes a divergence between the two
        // paths, which is much harder to see: each path is self-consistent.
        let built = if qml::on_gpu() {
            (1, 1)
        } else {
            (width.max(1), height.max(1))
        };
        let mut scene = qml::Scene::for_host(path, built.0, built.1, None)?;
        let insets = Insets {
            top: scene.get_int("insetTop").max(0),
            right: scene.get_int("insetRight").max(0),
            bottom: scene.get_int("insetBottom").max(0),
            left: scene.get_int("insetLeft").max(0),
        };
        // A frame with nothing reserved has nowhere else to paint but over
        // the client, so it is an overlay whether it says so or not.
        let overlay = scene.get_bool("overlay") || !insets.any();
        let on_gpu = qml::on_gpu();
        if !on_gpu {
            // The software half of the size decision above.
            // ...and then grown to the whole outer rect, which is what it
            // draws: the client area within it is simply left transparent.
            // At 1x, because the insets were just read from a scene laid out
            // that way and the first `frame` call resizes it to the monitor it
            // lands on anyway. The insets themselves are logical and do not
            // change with the scale, which is the point of laying QML out in
            // logical units.
            //
            // Skipped entirely on the GPU path, and not as an optimisation: a
            // GPU scene's pixel size *is* its buffer's, so the host refuses to
            // change it here and would say so in a warning for every window
            // that ever opened.
            scene.resize(
                (width + insets.horizontal()).max(1),
                (height + insets.vertical()).max(1),
                1.0,
            );
        }
        Ok(Self {
            scene,
            insets,
            overlay,
            backing: if on_gpu {
                // `(0, 0)` and not the client size the scene really is, so the
                // first frame takes the rebind branch. What this has to end up
                // on is the *outer* rect at the monitor's scale, and both of
                // those arrive with the first draw.
                Backing::Gpu(Gpu::new((0, 0)))
            } else {
                Backing::Memory(None)
            },
            buffer_size: (0, 0),
            shown: Shown::default(),
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

    /// What this frame reserves around its client.
    pub(crate) fn insets(&self) -> Insets {
        self.insets
    }

    /// Draw the frame over the whole of `rect`, at `outer` pixels.
    ///
    /// `rect` is where the window is drawn, which a presentation transform may
    /// have scaled; `outer` is its unscaled size, which is what the scene is
    /// rasterised at. No time is passed: the clock belongs to the process, and
    /// `qml::tick` advances it once for the whole frame. Keeping them apart is what lets a frame scale with its
    /// window in overview without the text being re-laid out every frame.
    /// The frame, drawn across `rect`, at `alpha`.
    ///
    /// The alpha is the window's, not the frame's. Every other part of a window
    /// is drawn through the presentation transform's opacity — the client's
    /// surface, its popups, the warped texture — and the frame was the one
    /// thing that was not. So a window closing faded away underneath a titlebar
    /// that stayed perfectly solid until the pane was retired, which is a bar
    /// hanging in the air with nothing under it at the exact moment the user is
    /// least willing to forgive one.
    ///
    /// Concrete on `GlesRenderer` rather than generic since the GPU path
    /// arrived, for the reason `ShellSurface::element` gives: taking the
    /// thread's EGL context back off Qt is `EGLContext::make_current`, and
    /// nothing on the `Renderer` traits says where the context is.
    ///
    /// Returns whether the frame is still animating as well as what to draw,
    /// and the two come out together on purpose — see [`crate::render::Drawn`].
    pub(crate) fn frame(
        &mut self,
        renderer: &mut GlesRenderer,
        rect: Rectangle<f64, Logical>,
        outer: Size<i32, Logical>,
        look: &Look<'_>,
        alpha: f32,
        scale: f64,
    ) -> Drawn {
        let width = outer.w.max(1);
        let height = outer.h.max(1);
        // QML rasterises in device pixels, so on a 2x monitor a frame drawn at
        // its logical size is drawn at half the resolution of the screen it
        // lands on and then stretched. That is the whole of what a blurry
        // HiDPI desktop *is*, and a compositor's own chrome being the blurry
        // part is the least forgivable version of it.
        //
        // So the buffer is `logical * scale` pixels and the element maps it
        // back down to the logical rect, which the output scale then takes
        // back up to exactly these pixels. One to one.
        let pixels = |logical: i32| {
            #[expect(clippy::cast_possible_truncation, reason = "a frame on this screen")]
            let scaled = (f64::from(logical) * scale).round() as i32;
            scaled.max(1)
        };
        let size = (pixels(width), pixels(height));

        // Before anything renders, on either path.
        self.tell(look, width, height);

        let resized = self.buffer_size != size;
        self.buffer_size = size;

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

        // Physical, which is what the parameter has always been: at 1x a
        // logical position was the same number and it did not matter.
        let position = (rect.loc.x * scale, rect.loc.y * scale);

        // The flag is read here and not after, because the draw below spends
        // it. `tell` has already run, so a property this frame wrote -- a new
        // title, a focus change -- counts as something to draw; an animation
        // step marked by `qml::tick` in `render::prepare` counts too, and that
        // is the one that keeps the loop going. See `render::Drawn`.
        Drawn::drawing(self, |this| {
            // Two fields of one struct, borrowed at once: the scene is what
            // renders and the backing is what holds the result.
            let Self { scene, backing, .. } = this;
            match backing {
                // A window resize is a *rebind* here and not a rebuild, which
                // is the whole of Task 6: a decoration is sized from an
                // animating rectangle for the length of every window
                // animation, and a scene rebuilt once per frame is a scene
                // whose own animations restart once per frame and therefore
                // never advance. `Gpu` does it.
                Backing::Gpu(gpu) => gpu
                    .element(
                        scene,
                        renderer,
                        size,
                        scale,
                        Placement {
                            position,
                            size: drawn,
                            alpha,
                            kind: Kind::Unspecified,
                        },
                    )
                    .map(Element::Screen),
                Backing::Memory(_) => this
                    .in_memory(renderer, size, resized, position, drawn, alpha, scale)
                    .map(Element::Chrome),
            }
        })
    }

    /// Hand the frame everything it is told about its window.
    ///
    /// Compared field by field rather than by building a `Shown`: this runs for
    /// every window of every frame, and the title is the one thing here that
    /// allocates.
    fn tell(&mut self, look: &Look<'_>, width: i32, height: i32) {
        let Look {
            title,
            focused,
            pointer_inside,
        } = *look;
        if self.shown.title == title
            && self.shown.focused == focused
            && self.shown.pointer_inside == pointer_inside
        {
            return;
        }
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
    }

    /// The software path, unchanged: Qt rasterises into a `QImage` and the
    /// compositor uploads the bands of it that can have changed.
    #[expect(
        clippy::too_many_arguments,
        reason = "the placement and the size are the caller's, computed once for both paths"
    )]
    fn in_memory(
        &mut self,
        renderer: &mut GlesRenderer,
        size: (i32, i32),
        resized: bool,
        position: (f64, f64),
        drawn: Size<i32, Logical>,
        alpha: f32,
        scale: f64,
    ) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
        self.scene.resize(size.0, size.1, scale);

        // The buffer is made *before* the scene is rendered, so the render can
        // be copied straight into it. It used to be the other way round, which
        // meant staging the whole image in a Vec first: a window-sized
        // allocation and copy every frame, 3.9MB of it on an ordinary window,
        // which dwarfed everything else this function does.
        let overlay = self.overlay;
        let insets = self.insets;
        let Self {
            scene,
            backing: Backing::Memory(slot),
            ..
        } = self
        else {
            return None;
        };
        if slot.is_none() || resized {
            *slot = Some(MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                size,
                1,
                Transform::Normal,
                None,
            ));
        }

        if scene.needs_render() || resized {
            // Only the parts of the frame that can have changed. A titlebar on
            // a 1150x850 window is 4% of it, and the other 96% has nothing in
            // it to copy or upload.
            let regions = if overlay || resized {
                vec![Rectangle::from_size(size.into())]
            } else {
                // In buffer pixels, like everything else here: the bands are a
                // copy optimisation over the image, not a logical rect.
                insets.bands_at(size.0, size.1, scale)
            };
            let rendered = match scene.render() {
                Ok(rendered) => rendered,
                Err(err) => {
                    tracing::warn!(?err, "a window frame did not render");
                    return None;
                }
            };
            if rendered.changed || resized {
                let buffer = slot.as_mut()?;
                let stride = rendered.stride;
                let pixels = rendered.pixels;
                let mut context = buffer.render();
                let copy = context.draw(|target| {
                    let row_bytes = usize::try_from(size.0.max(0)).unwrap_or_default() * 4;
                    for region in &regions {
                        let width = usize::try_from(region.size.w.max(0)).unwrap_or_default() * 4;
                        let left = usize::try_from(region.loc.x.max(0)).unwrap_or_default() * 4;
                        let top = region.loc.y.max(0);
                        for row in top..top.saturating_add(region.size.h.max(0)) {
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
        }
        let buffer = slot.as_mut()?;

        // `src` must be given whenever `size` is. Smithay defaults it to the
        // *drawn* size, which crops the buffer to its top-left corner instead
        // of scaling it — at full size the two are equal and it looks correct,
        // and it only shows up once a mode scales the frame down: the title
        // slides right and the buttons vanish off the edge.
        //
        // The whole buffer, in its own pixels — the buffer's scale is 1, so
        // its "logical" size is its pixel size.
        let source = Rectangle::from_size((f64::from(size.0), f64::from(size.1)).into());

        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            position,
            buffer,
            Some(alpha),
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
/// Keyed by *pane*, not by surface, and *presence means decorated*: a client
/// that negotiated client-side decorations has no entry, so it draws its own
/// frame and the compositor draws none. Two frames on one window is what
/// happens when this is a flag instead of a lookup.
///
/// The key is the pane because a pane outlives the arrival of its surface. A
/// frame drawn around a window whose application is still starting is the same
/// frame, with the same animation still running in it, the moment the client
/// maps — nothing is rebuilt, so nothing restarts or flickers.
#[derive(Debug, Default)]
pub(crate) struct Decorations {
    frames: HashMap<PaneId, Decoration>,
    /// Panes that will never have a frame, as opposed to not having one yet.
    ///
    /// A client drawing its own decorations, and an override-redirect menu.
    /// The distinction exists so `Solium::insets_of` knows whether to keep
    /// reserving room for a frame that is coming.
    bare: std::collections::HashSet<PaneId>,
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
        if bare(self.style.as_deref()) {
            // Every frame goes, and the clients are resized to the room they
            // now have -- which the caller does, because it is the one holding
            // the windows.
            self.frames.clear();
            return true;
        }
        let path = qml_path(self.style.as_deref());
        let existing: Vec<(PaneId, (i32, i32))> = self
            .frames
            .iter()
            .map(|(id, frame)| (*id, frame.client_size()))
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

    pub(crate) fn insert(&mut self, id: PaneId, width: i32, height: i32) {
        if self.frames.contains_key(&id) {
            return;
        }
        if bare(self.style.as_deref()) {
            self.bare.insert(id);
            return;
        }
        self.bare.remove(&id);
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

    /// Drop every frame whose pane has gone.
    pub(crate) fn retain(&mut self, keep: impl Fn(PaneId) -> bool) {
        let before = self.frames.len();
        self.frames.retain(|id, _| keep(*id));
        self.bare.retain(|id| keep(*id));
        let dropped = before - self.frames.len();
        if dropped > 0 {
            tracing::debug!(dropped, "dropped window frames with their panes");
        }
    }

    /// This pane will never have a frame. See the `bare` field.
    pub(crate) fn set_bare(&mut self, id: PaneId) {
        self.bare.insert(id);
    }

    /// It may have a frame again — leaving fullscreen, or a client changing
    /// its mind about drawing its own.
    pub(crate) fn unset_bare(&mut self, id: PaneId) {
        self.bare.remove(&id);
    }

    pub(crate) fn remove(&mut self, id: PaneId) {
        if self.frames.remove(&id).is_some() {
            tracing::debug!("dropped a window frame");
        }
    }

    /// Which decoration is in use, if a script chose one.
    pub(crate) fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }

    /// The live scene a pane's frame is drawn from.
    ///
    /// The last reason this table is still the authority. A `Decoration` owns
    /// a Qt scene, which is not `Clone` and of which there is exactly one — so
    /// the readers that want the decoration *itself* rather than a fact about
    /// it (drawing it, giving it the pointer, taking its button presses, its
    /// pre-maximise rectangle) cannot move onto the pane until the decoration
    /// is **moved** there. That is Task 4. Everything that only wants to know
    /// what the frame *is* has already gone: see `Solium::insets_of`.
    pub(crate) fn get_mut(&mut self, id: PaneId) -> Option<&mut Decoration> {
        self.frames.get_mut(&id)
    }

    /// How many frames are being kept. For leak diagnostics: this should
    /// return to what it was once every window is closed.
    ///
    /// Deliberately still counting the *table* and not the panes. The two
    /// agree for every live pane, which is what makes counting panes look
    /// equivalent — but an entry left behind by a pane that has gone belongs
    /// to neither, and that entry is the leak this number exists to show.
    pub(crate) fn len(&self) -> usize {
        self.frames.len()
    }

    /// What the tables say this pane's frame is.
    ///
    /// The shadow half of the strangler: the value a pane is given after every
    /// write to `frames` or `bare`, so that the pane's own answer is derived
    /// from the authority rather than worked out a second time beside it. A
    /// writer that forgets to call this leaves a pane disagreeing with the
    /// tables, which is exactly what the next task's assertion is for.
    ///
    /// **`frames` is asked first, on purpose.** That is the order
    /// `Solium::insets_of` reads them in, so a pane in both tables shadows as
    /// `Styled` here for the same reason it reserves a frame's insets there.
    /// Reproducing today's precedence is the point; correcting it would be a
    /// behaviour change wearing a refactor's clothes.
    pub(crate) fn frame_of(&self, id: PaneId) -> crate::pane::Frame {
        if let Some(decoration) = self.frames.get(&id) {
            return crate::pane::Frame::Styled(decoration.insets());
        }
        if self.bare.contains(&id) {
            return crate::pane::Frame::None;
        }
        // In neither table. Not yet built -- which includes the two ways that
        // can be permanent and look temporary: a decoration whose QML would not
        // load (`insert` logs "leaving it bare" and does not mark it so), and a
        // window that was already framed when the style became `none`
        // (`set_style` clears `frames` and marks nothing). Both keep reserving
        // room for a frame that is not coming. Shadowed as it stands.
        crate::pane::Frame::Pending
    }

    /// The tables and the pane must agree about this pane's frame.
    ///
    /// The other half of the strangler, and the reason the readers can be
    /// moved over one at a time instead of all at once: every reader that now
    /// asks the *pane* checks first that the tables would have said the same,
    /// so a writer that forgot to shadow is a failed assertion at the moment
    /// it is read rather than a window with the wrong shape three tasks later.
    ///
    /// It checks **agreement, not correctness**. Two of the ways a pane lands
    /// in neither table are permanent rather than temporary (see `frame_of`),
    /// and in both the tables and the pane agree and are wrong together. This
    /// will not catch them, and is not meant to.
    ///
    /// Debug-only, and it exists for exactly as long as both answers do: Task
    /// 4 deletes it with the tables.
    #[cfg(debug_assertions)]
    pub(crate) fn agree(&self, id: PaneId, pane: &crate::pane::Pane) {
        // Named rather than compared as values because `Frame` is not `Eq` --
        // and should not be, once it owns a `Decoration` -- and because a
        // failure that reads `"styled" != "none"` says what went wrong.
        let table = if self.frames.contains_key(&id) {
            "styled"
        } else if self.bare.contains(&id) {
            "none"
        } else {
            "pending"
        };
        let owned = match pane.frame() {
            crate::pane::Frame::Styled(_) => "styled",
            crate::pane::Frame::None => "none",
            crate::pane::Frame::Pending => "pending",
        };
        debug_assert_eq!(table, owned, "pane {id:?} disagrees with the tables");

        // And, for a framed pane, *how much* it reserves. A writer that
        // replaced one decoration with another and forgot to shadow leaves
        // both answers reading "styled" and only the numbers apart -- which is
        // the whole of what `insets_of` returns, so the classification check
        // above would pass while every layout using it was wrong.
        if let (Some(decoration), crate::pane::Frame::Styled(insets)) =
            (self.frames.get(&id), pane.frame())
        {
            debug_assert_eq!(
                decoration.insets(),
                *insets,
                "pane {id:?} holds insets the table has moved on from"
            );
        }
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

/// Whether this style means "draw no frame at all".
///
/// A real setting and not only a diagnostic: some people want a desktop with
/// no window furniture, and a tiling layout with a bar of its own has no use
/// for a titlebar on every window.
///
/// It is also the arm that isolates the per-window leak in #33. A bare window
/// still maps a toplevel, still opens and closes a pane, still has its buffers
/// imported — it simply never builds a `qml::Scene`. Whether the leak survives
/// that is the difference between blaming the QML host and blaming the surface.
fn bare(style: Option<&str>) -> bool {
    let chosen = std::env::var("SOLIUM_DECORATION")
        .ok()
        .or_else(|| style.map(ToOwned::to_owned));
    matches!(
        chosen.as_deref().map(str::trim),
        Some("none" | "None" | "NONE")
    )
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

    /// A pane with no client and no history. Nothing here builds a
    /// `Decoration` — that wants a Qt scene, and so a GPU and a display.
    #[cfg(debug_assertions)]
    fn undecorated_pane() -> crate::pane::Pane {
        crate::pane::Pane::loading(
            "kitty",
            None,
            Rectangle::new((0, 0).into(), (300, 200).into()),
            PathBuf::new(),
            None,
            std::time::Duration::ZERO,
        )
    }

    #[test]
    #[cfg(debug_assertions)]
    fn a_pane_nobody_has_decorated_agrees_with_the_empty_tables() {
        // Both answers read "pending": in neither table, and `Frame::Pending`
        // by default. The agreeing case has to be asserted too, or the test
        // below only proves that *something* panics.
        let pane = undecorated_pane();
        Decorations::default().agree(pane.id(), &pane);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "disagrees with the tables")]
    fn the_assertion_bites_when_a_writer_forgets_to_shadow() {
        // Exactly what a missed writer looks like: `set_bare` ran, and the
        // pane was never told. Asserted because a `debug_assert` that cannot
        // fire and one that never has to are indistinguishable from a green
        // gate, and this one is the whole safety net under Task 2.
        let pane = undecorated_pane();
        let mut decorations = Decorations::default();
        decorations.set_bare(pane.id());
        decorations.agree(pane.id(), &pane);
    }
}
