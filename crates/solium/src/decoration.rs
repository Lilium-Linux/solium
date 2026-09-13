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
//!
//! **A frame is a list of layers**, each its own scene and each its own element
//! in the frame, so a client's surface can be placed *between* two of them —
//! see [`crate::style`], which reads the list out of a bundle's `Pane.qml`. A
//! decoration built from a single QML file is that list with one entry at
//! [`Depth::Frame`]: the identity case is this machinery holding one thing
//! rather than a second path beside it.

use std::path::{Path, PathBuf};

use crate::{
    pane::{Frame, Pane, PaneId, Panes},
    style::{Depth, LayerSpec, Style},
};

use anyhow::{Context as _, Result};
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

/// Where and how a pane's layers are drawn this frame.
///
/// One argument rather than four: they are only ever passed together, and a
/// second `f64` on a call that already takes a rectangle and a size is the kind
/// of thing that gets given in the wrong order once and then stays wrong.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drawing {
    /// Where the window is drawn, which a presentation transform may have
    /// moved or scaled.
    pub(crate) rect: Rectangle<f64, Logical>,
    /// Its unscaled size, which is what the scenes are rasterised at. Keeping
    /// the two apart is what lets a frame scale with its window in overview
    /// without the text being re-laid out every frame.
    pub(crate) outer: Size<i32, Logical>,
    /// The **window's** opacity, not the frame's.
    ///
    /// Every other part of a window is drawn through the presentation
    /// transform's opacity — the client's surface, its popups, the warped
    /// texture — and the frame was the one thing that was not. So a window
    /// closing faded away underneath a titlebar that stayed perfectly solid
    /// until the pane was retired, which is a bar hanging in the air with
    /// nothing under it at the exact moment the user is least willing to
    /// forgive one.
    pub(crate) alpha: f32,
    /// Device pixels per logical one, on the monitor this is drawn on.
    pub(crate) scale: f64,
}

/// One layer of a style, as a live scene.
///
/// Separate scenes rather than one image with passes, because nothing else lets
/// a client's surface sit *between* two layers the same style produced — which
/// is the whole reason layers exist. See `render::PANE_ORDER`.
#[derive(Debug)]
struct LayerScene {
    /// Where the client sits relative to this layer.
    depth: Depth,
    /// Which layer this is, for diagnostics and for the order tests.
    name: String,
    scene: qml::Scene,
    backing: Backing,
    /// Whether this layer paints outside the space the style reserved.
    ///
    /// A layer that stays inside the insets only has to have those bands
    /// copied and uploaded each time it changes; one that draws over the
    /// client -- a bar floating above the window, a glow across it -- has to
    /// have all of it copied, because anything in it may have moved. A layer
    /// at `behind` or `above` is one by definition.
    overlay: bool,
    /// The device-pixel size **this layer** was last drawn at.
    ///
    /// Per layer and not per decoration, because a frame asks for one depth at
    /// a time: a single size compared at the decoration would be brought up to
    /// date by whichever depth was drawn first, and the other two would read
    /// "not resized" on the one frame they had to copy everything.
    buffer_size: (i32, i32),
}

/// A decoration animates on its own clock -- a border easing to a new colour, a
/// bar sliding out, a sheen crossing a titlebar -- and the compositor only draws
/// when something has damaged the screen. Nothing the client did damaged it, so
/// unless the frame says it is still moving, rendering stops and the animation
/// freezes wherever it happened to be.
///
/// Per **layer** and not per decoration, because [`Drawn::drawing`] spends the
/// flag it reads: a decoration-wide answer would report the dirt of layers this
/// draw is not going to touch, and clear none of it.
impl crate::render::Painted for LayerScene {
    fn something_new_to_draw(&self) -> bool {
        self.scene.needs_render()
    }

    fn animation_in_flight(&self) -> bool {
        self.scene.animation_in_flight()
    }
}

/// The layers of one depth, in the order they go into the frame.
///
/// Later-declared layers draw on top, which is QML's own rule for siblings, and
/// the element list is topmost first — so one depth is walked backwards.
///
/// Generic over the borrow so that the draw, which needs the scenes mutably,
/// and the question "what is at this depth", which needs no renderer at all,
/// are one statement rather than two that happen to agree. `&LayerScene` and
/// `&mut LayerScene` both deref to one.
fn at<L: std::ops::Deref<Target = LayerScene>>(
    layers: impl DoubleEndedIterator<Item = L>,
    depth: Depth,
) -> impl Iterator<Item = L> {
    layers.rev().filter(move |layer| layer.depth == depth)
}

/// One window's frame: every layer its style declares, each its own scene.
#[derive(Debug)]
pub(crate) struct Decoration {
    /// One scene per declared layer, in declaration order.
    ///
    /// A decoration built from a single QML file is a list of one at
    /// [`Depth::Frame`]. That is the identity case and it has to stay as cheap
    /// as it was: one scene, one buffer, one element, and two depths that find
    /// nothing and return.
    layers: Vec<LayerScene>,
    /// What the style asked to reserve, read once when it was built.
    insets: Insets,
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

impl Decoration {
    fn new(path: &Path, width: i32, height: i32) -> Result<Self> {
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
            layers: vec![LayerScene {
                // One QML file is one layer, where decorations already are.
                // There is no second path for it: a `frame` layer is what a
                // decoration has always been, and Task 7 moves the eight
                // shipped ones into bundles without changing a pixel.
                depth: Depth::Frame,
                name: String::new(),
                scene,
                backing: Backing::for_host(on_gpu),
                overlay,
                buffer_size: (0, 0),
            }],
            insets,
            buffer_size: (0, 0),
            shown: Shown::default(),
            restore: None,
        })
    }

    /// Every layer a style declares, each as its own scene.
    ///
    /// The scenes are built here and never rebuilt: a layer is a live QML
    /// object tree, and rebuilding one restarts every animation in it — which
    /// is the defect `Gpu`'s rebind exists to avoid, one level down.
    ///
    /// A layer with `source:` loads that file. An **inline** layer has no file
    /// of its own, so the manifest is loaded again and told which of its layers
    /// this instance is drawing, through `layerIndex`. The alternative — one
    /// scene with the others hidden inside it — cannot be spelled from a
    /// `Layer`: see `PaneStyle.showOneLayer`.
    pub(crate) fn from_style(style: &Style, width: i32, height: i32) -> Result<Self> {
        qml::start()?;
        let mut layers = Vec::with_capacity(style.layers.len());
        for spec in &style.layers {
            layers.push(
                LayerScene::build(spec, style, width, height).with_context(|| {
                    format!(
                        "building layer {} (`{}`) of {}",
                        spec.index,
                        spec.name,
                        style.dir.display()
                    )
                })?,
            );
        }
        Ok(Self {
            layers,
            // From the style and not from a scene. `insets` is declared on
            // `PaneStyle` once for the whole bundle, which is the ruling this
            // format rests on: the client is placed once, so three layers each
            // answering would be three answers to one question.
            insets: style.insets,
            buffer_size: (0, 0),
            shown: Shown::default(),
            restore: None,
        })
    }

    /// The client's size, inside whatever this frame reserves.
    fn client_size(&self) -> (i32, i32) {
        (
            (self.buffer_size.0 - self.insets.horizontal()).max(1),
            (self.buffer_size.1 - self.insets.vertical()).max(1),
        )
    }

    /// What this frame reserves around its client.
    pub(crate) const fn insets(&self) -> Insets {
        self.insets
    }

    /// The layers of this style at one depth, drawn into `into`, topmost first.
    ///
    /// Called **once per depth** for every pane, from the one place that states
    /// where the client goes among them: `render::PANE_ORDER`. A pane with a
    /// plain decoration finds its one layer at [`Depth::Frame`] and nothing at
    /// the other two, so the identity case costs one draw and two walks of a
    /// one-element list.
    ///
    /// `drawing.rect` is where the window is drawn, which in a mode is not
    /// where it lives; the scenes are always rasterised at `drawing.outer` and
    /// the elements scale them, so a thumbnail's titlebar costs no more than a
    /// full-size one. No time is passed: the clock belongs to the process, and
    /// `qml::tick` advances it once for the whole frame.
    ///
    /// Concrete on `GlesRenderer` rather than generic since the GPU path
    /// arrived, for the reason `ShellSurface::element` gives: taking the
    /// thread's EGL context back off Qt is `EGLContext::make_current`, and
    /// nothing on the `Renderer` traits says where the context is.
    ///
    /// Returns whether any layer drawn here is still animating, which comes out
    /// of the draw rather than from a question after it — see
    /// [`crate::render::Drawn`].
    pub(crate) fn layer_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        depth: Depth,
        look: &Look<'_>,
        drawing: Drawing,
        into: &mut Vec<Element>,
    ) -> bool {
        let width = drawing.outer.w.max(1);
        let height = drawing.outer.h.max(1);
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
            let scaled = (f64::from(logical) * drawing.scale).round() as i32;
            scaled.max(1)
        };
        let size = (pixels(width), pixels(height));

        // Before anything renders, on either path — and for **every** layer,
        // not the ones about to be drawn. This runs once per depth and the
        // write is guarded by `shown`, so telling only this depth's layers
        // would tell whichever depth came first and silently skip the rest.
        self.tell(look, width, height);
        self.buffer_size = size;

        // The drawn size scales the frame with the window it belongs to.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a frame is at most an output wide"
        )]
        let drawn: Size<i32, Logical> = (
            drawing.rect.size.w.round() as i32,
            drawing.rect.size.h.round().max(1.0) as i32,
        )
            .into();
        let placement = Placement {
            // Physical, which is what the parameter has always been: at 1x a
            // logical position was the same number and it did not matter.
            position: (
                drawing.rect.loc.x * drawing.scale,
                drawing.rect.loc.y * drawing.scale,
            ),
            size: drawn,
            alpha: drawing.alpha,
            kind: Kind::Unspecified,
        };

        let insets = self.insets;
        let mut animating = false;
        for layer in at(self.layers.iter_mut(), depth) {
            let drawn = layer.element(renderer, insets, size, placement, drawing.scale);
            into.extend(drawn.element);
            animating |= drawn.animating;
        }
        animating
    }

    /// The names of the layers at one depth, in the order they are drawn.
    ///
    /// What [`Decoration::layer_elements`] draws, without a renderer — the same
    /// selection through the same [`at`], so the two cannot come to disagree
    /// about which layers a depth has or which of them is on top.
    pub(crate) fn layers_at(&self, depth: Depth) -> impl Iterator<Item = &str> {
        at(self.layers.iter(), depth).map(|layer| layer.name.as_str())
    }

    /// Hand every layer everything it is told about its window.
    ///
    /// Compared field by field rather than by building a `Shown`: this runs for
    /// every window of every frame, and the title is the one thing here that
    /// allocates.
    ///
    /// Every layer and not only the frame: a glow behind a window that cannot
    /// tell whether the window is focused is a glow that is always on. The five
    /// properties are declared on `PaneStyle` as well as on a delegated layer's
    /// own root, so an inline layer can bind to them too.
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
        // The client's own size, so a layer can place things against the
        // window rather than against itself.
        let content = (
            width - self.insets.horizontal(),
            height - self.insets.vertical(),
        );
        for layer in &mut self.layers {
            layer.scene.set_string("title", title);
            layer.scene.set_bool("focused", focused);
            layer.scene.set_bool("pointerInside", pointer_inside);
            layer.scene.set_int("contentWidth", content.0);
            layer.scene.set_int("contentHeight", content.1);
        }
        self.shown.title.clear();
        self.shown.title.push_str(title);
        self.shown.focused = focused;
        self.shown.pointer_inside = pointer_inside;
    }

    /// Tell the frame the pointer has left the window altogether.
    ///
    /// Sent as a position rather than a flag as well, because QML's hover
    /// handling is positional: a `MouseArea` that never sees the pointer leave
    /// stays hovered forever, and a border lit by proximity stays lit.
    pub(crate) fn pointer_left(&mut self) {
        for layer in &mut self.layers {
            layer.scene.pointer(-1.0, -1.0, None);
            layer.scene.set_bool("pointerInside", false);
        }
        self.shown.pointer_inside = false;
    }

    /// Pointer input in frame-local coordinates.
    ///
    /// To **every** layer, which is the honest answer until input is scoped. A
    /// layer is the size of the pane and the coordinates are the pane's, so a
    /// press under a button in one layer is under whatever happens to be at
    /// that spot in every other — and hover state that is never delivered is a
    /// border that stays lit. Deciding which layer *wants* the press needs the
    /// scene to say whether a `MouseArea` accepted it, which the spec's *Input,
    /// scoped* section is about and which nothing here can answer yet.
    pub(crate) fn pointer(&mut self, x: f64, y: f64, pressed: Option<bool>) {
        for layer in &mut self.layers {
            layer.scene.pointer(x, y, pressed);
        }
    }

    /// Whether the pointer is over a button.
    ///
    /// Asked of QML rather than worked out from coordinates: QML owns the
    /// button layout, so a copy of it here would be a second authority that
    /// drifts the first time the frame is restyled.
    pub(crate) fn on_button(&self) -> bool {
        self.layers
            .iter()
            .any(|layer| layer.scene.get_bool("onButton"))
    }

    /// What a button asked for since the last call, if anything.
    ///
    /// Every layer is asked even once one has answered, because `take_string`
    /// is what *clears* the property: a layer left unasked keeps its action and
    /// fires it on whichever later frame something else happens to ask.
    pub(crate) fn take_action(&mut self) -> Option<Action> {
        let mut action = None;
        for layer in &mut self.layers {
            let asked = layer
                .scene
                .take_string("action")
                .as_deref()
                .and_then(Action::parse);
            action = action.or(asked);
        }
        action
    }
}

impl Backing {
    /// Whichever kind of buffer this process's scenes reach the screen through.
    ///
    /// Not a preference: [`qml::Scene::for_host`] has already decided, and a
    /// backing of the other kind would be a buffer nothing ever writes into.
    fn for_host(on_gpu: bool) -> Self {
        if on_gpu {
            // `(0, 0)` and not the size the scene really is, so the first frame
            // takes the rebind branch. What this has to end up on is the
            // *outer* rect at the monitor's scale, and both of those arrive
            // with the first draw.
            Self::Gpu(Gpu::new((0, 0)))
        } else {
            Self::Memory(None)
        }
    }
}

impl LayerScene {
    /// One declared layer, brought up as a scene of its own.
    fn build(spec: &LayerSpec, style: &Style, width: i32, height: i32) -> Result<Self> {
        // The insets are the style's and are known before anything is built,
        // so a layer is laid out at the outer rect from the start — where
        // `Decoration::new` has to build at the client size first and resize,
        // because the only place its insets exist is inside the scene.
        //
        // **At 1x1 on the GPU path**, for the reason `Decoration::new` gives at
        // length: a GPU scene's buffer is a GBM allocation, the first draw
        // rebinds onto the outer rect in the monitor's pixels regardless, and
        // building at the real size means ~3.9 MB allocated, handed to Qt,
        // imported and freed again for every layer of every window that opens.
        let on_gpu = qml::on_gpu();
        let built = if on_gpu {
            (1, 1)
        } else {
            (
                (width + style.insets.horizontal()).max(1),
                (height + style.insets.vertical()).max(1),
            )
        };
        // An inline layer has no file of its own, so its scene is the manifest
        // loaded again with `layerIndex` pointing at the one child it draws.
        // Set at *creation* rather than after it, so `Component.onCompleted`
        // already knows which layer this instance is — see
        // `PaneStyle.showOneLayer`, which parents that child and unparents the
        // rest.
        let inline = format!(r#"{{"layerIndex":{}}}"#, spec.index);
        let manifest;
        let (path, initial): (&Path, Option<&str>) = match spec.source.as_deref() {
            Some(file) => (file, None),
            None => {
                manifest = style.dir.join("Pane.qml");
                (&manifest, Some(inline.as_str()))
            }
        };
        let scene = qml::Scene::for_host(path, built.0, built.1, initial)?;
        // A layer at `behind` or `above` paints over the client by definition,
        // and so does any layer of a style that reserved nothing: there is
        // nowhere else for it to paint. Only a `frame` layer inside real insets
        // can be copied band by band, and only if it says it stays inside them.
        let overlay =
            spec.depth != Depth::Frame || !style.insets.any() || scene.get_bool("overlay");
        Ok(Self {
            depth: spec.depth,
            name: spec.name.clone(),
            scene,
            backing: Backing::for_host(on_gpu),
            overlay,
            buffer_size: (0, 0),
        })
    }

    /// This layer, drawn across `placement`, as an element.
    ///
    /// The two backings differ only in what holds the result, which is why they
    /// share a [`Placement`]: the position, the drawn size and the alpha are
    /// the caller's and are computed once for the whole pane.
    fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        insets: Insets,
        size: (i32, i32),
        placement: Placement,
        scale: f64,
    ) -> Drawn {
        let resized = self.buffer_size != size;
        self.buffer_size = size;

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
                    .element(scene, renderer, size, scale, placement)
                    .map(Element::Screen),
                Backing::Memory(_) => this
                    .in_memory(renderer, insets, size, resized, placement, scale)
                    .map(Element::Chrome),
            }
        })
    }

    /// The software path, unchanged: Qt rasterises into a `QImage` and the
    /// compositor uploads the bands of it that can have changed.
    fn in_memory(
        &mut self,
        renderer: &mut GlesRenderer,
        insets: Insets,
        size: (i32, i32),
        resized: bool,
        placement: Placement,
        scale: f64,
    ) -> Option<MemoryRenderBufferRenderElement<GlesRenderer>> {
        self.scene.resize(size.0, size.1, scale);

        // The buffer is made *before* the scene is rendered, so the render can
        // be copied straight into it. It used to be the other way round, which
        // meant staging the whole image in a Vec first: a window-sized
        // allocation and copy every frame, 3.9MB of it on an ordinary window,
        // which dwarfed everything else this function does.
        let overlay = self.overlay;
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
            placement.position,
            buffer,
            Some(placement.alpha),
            Some(source),
            Some(placement.size),
            placement.kind,
        )
        .inspect_err(|err| tracing::warn!(?err, "could not upload a window frame"))
        .ok()
    }
}

/// How windows are framed: which decoration, and the building of it.
///
/// **It holds no frames.** It used to hold two tables keyed by `PaneId` — a
/// `HashMap<PaneId, Decoration>` and a `HashSet<PaneId>` of panes that would
/// never have one — which meant a frame outlived its window unless something
/// remembered to reconcile them, and meant two tables could answer the same
/// question differently. A frame is now [`crate::pane::Frame`], a field of the
/// pane it is drawn around, and it leaves when the pane does.
///
/// So what is left here is the *policy*: which QML file a frame is built from,
/// and what "built" means when the style says `none` or the file will not load.
/// Every mutator takes `&mut Panes` and writes its answer onto the pane, which
/// is why they are still methods here rather than free functions — `insert`
/// alone picks between four outcomes, and the call site cannot know which.
#[derive(Debug, Default)]
pub(crate) struct Decorations {
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
    pub(crate) fn set_style(&mut self, panes: &mut Panes, style: Option<String>) -> bool {
        if self.style == style {
            return false;
        }
        self.style = style;
        if bare(self.style.as_deref()) {
            // Every frame goes, and the clients are resized to the room they
            // now have -- which the caller does, because it is the one holding
            // the windows.
            //
            // **`Pending` and not `None`**, which is not what it should be and
            // is what it was: this was `self.frames.clear()`, and a pane that
            // is in neither table reserves a titlebar's worth of room for a
            // frame that is not coming. So `decoration = "none"` only takes
            // effect for windows opened *after* it -- those go through
            // `insert`, which marks them properly. That is half of #90, it is
            // reproduced here deliberately, and the one-word fix belongs in
            // the commit that fixes it rather than in the one that moved it.
            let framed: Vec<PaneId> = panes
                .iter()
                .filter(|pane| pane.decoration().is_some())
                .map(Pane::id)
                .collect();
            for id in framed {
                if let Some(pane) = panes.get_mut(id) {
                    pane.set_frame(Frame::Pending);
                }
            }
            return true;
        }
        // Measured first and rebuilt after, because each new frame is sized
        // from the one it replaces. Collected for the same reason the
        // `frames.iter()` this replaces was: the walk cannot be holding the
        // panes while the loop writes to them.
        let existing: Vec<(PaneId, (i32, i32))> = panes
            .iter()
            .filter_map(|pane| Some((pane.id(), pane.decoration()?.client_size())))
            .collect();
        for (id, (width, height)) in existing {
            match build(self.style.as_deref(), width, height) {
                Ok(fresh) => {
                    if let Some(pane) = panes.get_mut(id) {
                        pane.set_frame(Frame::Styled(fresh));
                    }
                }
                Err(err) => {
                    tracing::error!(?err, "could not load the new decoration, leaving it bare");
                    if let Some(pane) = panes.get_mut(id) {
                        pane.set_frame(Frame::Pending);
                    }
                }
            }
        }
        true
    }

    /// Build this pane's frame, unless it has one or the style says not to.
    pub(crate) fn insert(&mut self, panes: &mut Panes, id: PaneId, width: i32, height: i32) {
        let Some(pane) = panes.get_mut(id) else {
            // No pane to give it to. Unreachable from every call site, all of
            // which hold a live pane -- and the reason a `Decoration` is not
            // built first and placed after: a frame built for a pane that has
            // gone is a Qt scene nothing will ever free.
            return;
        };
        if matches!(pane.frame(), Frame::Styled(_)) {
            return;
        }
        if bare(self.style.as_deref()) {
            pane.set_frame(Frame::None);
            return;
        }
        // Cleared *before* the build, which is where `self.bare.remove(&id)`
        // stood. It matters only in the failure arm below, and there it is the
        // other half of #90: a pane that was bare and whose new frame will not
        // load ends up `Pending` -- reserving room for a frame that is not
        // coming -- rather than back where it started. Kept, so that this
        // commit changes nothing; #90 changes it on purpose.
        pane.set_frame(Frame::Pending);
        match build(self.style.as_deref(), width, height) {
            Ok(decoration) => pane.set_frame(Frame::Styled(decoration)),
            Err(err) => {
                // An undecorated window is worse than a decorated one and much
                // better than no window.
                tracing::error!(?err, "could not load a window frame, leaving it bare");
            }
        }
    }

    /// This pane will never have a frame: a client drawing its own, or an
    /// override-redirect menu. Not the same as not having one *yet*, which is
    /// what decides whether room is still reserved for one.
    ///
    /// Unconditional, where `bare.insert(id)` left an existing frame standing.
    /// The two tables could say "framed" and "bare" at once and nothing could
    /// act on it: `insets_of` read `frames` first, so `bare` was ignored. No
    /// caller could reach that state either — each of the three either has a
    /// pane that has never been framed or calls `remove` on the line above.
    pub(crate) fn set_bare(&mut self, panes: &mut Panes, id: PaneId) {
        if let Some(pane) = panes.get_mut(id) {
            pane.set_frame(Frame::None);
        }
    }

    /// It may have a frame again — leaving fullscreen, or a client changing
    /// its mind about drawing its own.
    ///
    /// Back to *pending*, not to a frame: this only lifts the ban, and the
    /// caller follows it with `insert` to build one. A pane that already has a
    /// frame keeps it, which is what `bare.remove` did — the scene must not be
    /// dropped by a call that was never about it.
    pub(crate) fn unset_bare(&mut self, panes: &mut Panes, id: PaneId) {
        if let Some(pane) = panes.get_mut(id)
            && matches!(pane.frame(), Frame::None)
        {
            pane.set_frame(Frame::Pending);
        }
    }

    /// Drop this pane's frame, if it has one.
    ///
    /// To `Pending` rather than `None`, because that is what `frames.remove`
    /// left behind: the pane is in neither table, so room is still reserved.
    /// Both callers follow it with `set_bare`, which is what actually says
    /// "and none is coming".
    pub(crate) fn remove(&mut self, panes: &mut Panes, id: PaneId) {
        if let Some(pane) = panes.get_mut(id)
            && matches!(pane.frame(), Frame::Styled(_))
        {
            pane.set_frame(Frame::Pending);
            tracing::debug!("dropped a window frame");
        }
    }

    /// Which decoration is in use, if a script chose one.
    pub(crate) fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }
}

/// Build one pane's frame from whatever the setting names.
///
/// **A style bundle first, a single QML file second.** A name that is a folder
/// under `panes/` is a style with layers and is loaded as one; anything else is
/// the one file decorations have always been. That order is what makes Task 7's
/// conversion a move of files rather than a change of setting — `decoration =
/// "top"` draws `decorations/top.qml` today and `panes/top/` the day that
/// folder exists, and nothing in `config.lua` changes on either side of it.
///
/// The two are the same thing at different sizes, which is why there is one
/// entry point and not two: a decoration is a style with one `frame` layer.
fn build(style: Option<&str>, width: i32, height: i32) -> Result<Decoration> {
    let Some(dir) = bundle(style) else {
        return Decoration::new(&qml_path(style), width, height);
    };
    // Through `style::load`, which is the only door: it is what checks
    // `requires` against this process's scene graph, and a second way to
    // produce a `Style` would be a style drawn on a path that has already said
    // it cannot draw it. See `style::requirements`.
    let declared = crate::style::load(&dir)
        .with_context(|| format!("loading the style bundle {}", dir.display()))?;
    let decoration = Decoration::from_style(&declared, width, height)?;
    say_what_was_built(&dir, &decoration);
    Ok(decoration)
}

/// Say what a bundle built, in the order it will be drawn.
///
/// At `debug`, once per window, and it earns the line because what this feature
/// adds is an *order*: "the spikes are behind the window" is otherwise
/// diagnosed by looking at the screen and guessing which of the depth string,
/// the declaration order and the element list was wrong.
///
/// Built through the same [`crate::render::PANE_ORDER`] the frame is, so it
/// cannot report an order the compositor does not draw — which is the whole
/// value of a diagnostic like this and the usual way one stops being worth
/// reading.
fn say_what_was_built(dir: &Path, decoration: &Decoration) {
    let mut order: Vec<&str> = Vec::new();
    crate::render::pane_pieces(&mut order, |into, piece| match piece {
        crate::render::Piece::Layers(depth) => into.extend(decoration.layers_at(depth)),
        crate::render::Piece::Client => into.push("<the client>"),
    });
    tracing::debug!(
        bundle = %dir.display(),
        ?order,
        "built a pane style; topmost first, and the client is where it says"
    );
}

/// The style bundle this setting asks for, if it asks for one at all.
///
/// `SOLIUM_QML_TITLEBAR` and any name ending in `.qml` are a *file* and never a
/// bundle — `style::find` takes anything with a separator in it as a path, so
/// without this a `SOLIUM_DECORATION=~/mine.qml` would be handed back as a
/// directory and then reported as having no `Pane.qml` in it.
fn bundle(style: Option<&str>) -> Option<PathBuf> {
    if std::env::var_os("SOLIUM_QML_TITLEBAR").is_some() {
        return None;
    }
    let name = chosen(style)?;
    if name.ends_with(".qml") {
        return None;
    }
    crate::style::find(&name)
}

/// What names the frame: the environment, and then the configuration.
///
/// The environment wins over the configuration, not the other way round. The
/// configuration always names something -- the shipped one says "top" -- so a
/// script's choice losing to nothing would be one thing, but a script's choice
/// *winning* leaves `SOLIUM_DECORATION` with no effect at all. It is set by
/// whoever started this particular run, to try one decoration for one session,
/// and that is the more specific intent.
fn chosen(style: Option<&str>) -> Option<String> {
    std::env::var("SOLIUM_DECORATION")
        .ok()
        .or_else(|| style.map(ToOwned::to_owned))
}

/// Where the frame's QML lives, when it is one file rather than a bundle.
///
/// Overridable so a frame can be restyled and reloaded without a rebuild,
/// which is most of the point of authoring it in QML.
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
    let Some(name) = chosen(style) else {
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
    matches!(
        chosen(style).as_deref().map(str::trim),
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

    /// Everything Qt-touching in this file's tests goes through this. It moved
    /// to `qml` when `style` needed it too — the rule it encodes is the QML
    /// engine's thread affinity, which is not a fact about decorations.
    use crate::qml::qt_test::on_the_qt_thread;

    #[test]
    fn button_names_map_to_actions() {
        assert_eq!(Action::parse("close"), Some(Action::Close));
        assert_eq!(Action::parse("maximize"), Some(Action::ToggleMaximize));
        // Unknown names are ignored rather than guessed at: a typo in QML must
        // not close a window.
        assert_eq!(Action::parse("clos"), None);
        assert_eq!(Action::parse(""), None);
    }

    /// One pane, with no client and no frame.
    ///
    /// A `Decoration` **can** be built from here, which is worth stating
    /// because it was written down twice that it could not: the claim was that
    /// a frame wants a Qt scene and so a GPU and a display. It wants Qt, and
    /// nothing else. `solium_qml_start` brings the software host up on the
    /// `offscreen` QPA platform with the `software` Quick backend — see
    /// `qml/host.cpp`, where both are `qputenv`ed before the `QGuiApplication`
    /// — because no window is ever created and the scene renders into a
    /// `QImage`. That is the same headless path `--check-qml` uses. Measured in
    /// the gate's container: `top.qml` builds and reads back `insets.top = 32`.
    ///
    /// What is still out of reach here is *drawing* one, which wants a
    /// `GlesRenderer`. That is `dev/wirecheck`'s, and the compositor's.
    fn one_pane() -> (Panes, PaneId) {
        let mut panes = Panes::default();
        let id = panes.open(Pane::loading(
            "kitty",
            None,
            Rectangle::new((0, 0).into(), (300, 200).into()),
            PathBuf::new(),
            None,
            std::time::Duration::ZERO,
        ));
        (panes, id)
    }

    /// Whether this session has already decided which decoration to use.
    ///
    /// `SOLIUM_QML_TITLEBAR` replaces the path outright and `SOLIUM_DECORATION`
    /// is read before the style, so either one turns a test that builds a frame
    /// into a test of whatever that variable points at — a file that may not
    /// exist, or `none`, which builds nothing at all. Tests share a process and
    /// an environment, so the answer is to stand down rather than to unset it.
    fn the_environment_has_already_chosen() -> bool {
        std::env::var_os("SOLIUM_QML_TITLEBAR").is_some()
            || std::env::var_os("SOLIUM_DECORATION").is_some()
    }

    #[test]
    fn a_pane_that_will_never_have_a_frame_says_so_rather_than_waiting() {
        // The distinction the two tables existed to make, and the one that is
        // easiest to lose in one value: `None` reserves nothing, `Pending`
        // reserves a titlebar for a frame that is still coming. A menu marked
        // bare must not sit under a strip of dead space for ever.
        let (mut panes, id) = one_pane();
        let mut decorations = Decorations::default();
        assert!(matches!(
            panes.get(id).map(Pane::frame),
            Some(Frame::Pending)
        ));

        decorations.set_bare(&mut panes, id);
        assert!(matches!(panes.get(id).map(Pane::frame), Some(Frame::None)));

        // And back to *pending*, not to a frame. `unset_bare` only lifts the
        // ban; the `insert` that follows it is what builds anything.
        decorations.unset_bare(&mut panes, id);
        assert!(matches!(
            panes.get(id).map(Pane::frame),
            Some(Frame::Pending)
        ));
    }

    #[test]
    fn dropping_a_frame_nobody_built_leaves_the_pane_where_it_was() {
        // `remove` was `frames.remove(&id)`, which did not touch `bare`. So a
        // pane that will never have a frame is still one afterwards -- and the
        // two callers that mean "and none is coming" say so by calling
        // `set_bare` on the next line, which is why this must not.
        let (mut panes, id) = one_pane();
        let mut decorations = Decorations::default();
        decorations.set_bare(&mut panes, id);
        decorations.remove(&mut panes, id);
        assert!(matches!(panes.get(id).map(Pane::frame), Some(Frame::None)));
    }

    #[test]
    fn a_window_opened_while_the_style_is_none_is_bare_from_the_start() {
        // The half of #90 that works, pinned because the half that does not is
        // one line away in the same function: a window opened *after*
        // `decoration = "none"` goes through `insert`, which marks it, while
        // one already framed when the style changed falls to `Pending` and
        // keeps reserving room. See `set_style`'s bare arm.
        if std::env::var_os("SOLIUM_DECORATION").is_some() {
            // `bare()` reads the environment before the style, so a session
            // that set it decides this rather than the test does.
            return;
        }
        let (mut panes, id) = one_pane();
        let mut decorations = Decorations::default();
        assert!(decorations.set_style(&mut panes, Some("none".to_owned())));
        decorations.insert(&mut panes, id, 300, 200);
        assert!(matches!(panes.get(id).map(Pane::frame), Some(Frame::None)));
    }

    #[test]
    fn a_style_that_has_not_changed_rebuilds_nothing() {
        // The early return, which is load-bearing rather than an
        // optimisation: `reload` calls `set_style(None)` and then
        // `set_style(style)` precisely to defeat it, and every other caller
        // relies on it to leave live Qt scenes alone.
        let (mut panes, _) = one_pane();
        let mut decorations = Decorations::default();
        assert!(!decorations.set_style(&mut panes, None));
        assert!(decorations.set_style(&mut panes, Some("top".to_owned())));
        assert!(!decorations.set_style(&mut panes, Some("top".to_owned())));
    }

    #[test]
    fn a_pane_cannot_be_framed_and_bare_at_once() {
        // The illegal state, taken through the one mutator that could have
        // written it, on a pane with a real frame on it.
        //
        // It was two tables: `frames: HashMap<PaneId, Decoration>` and `bare:
        // HashSet<PaneId>`, and nothing stopped a pane being in both.
        // `insets_of` read `frames` first, so a pane in both was framed and its
        // `bare` was silently ignored — and `set_bare` was `bare.insert(id)`,
        // which left an existing frame standing, so this call is exactly how
        // that state would have been written.
        //
        // There is nowhere for it to go now. `Frame` is an enum, a pane holds
        // one, and the `Frame::None` this writes *drops* the `Decoration` it
        // replaces. "Still has a scene" and "will never have a frame" cannot
        // both be true, because they are the same field.
        if the_environment_has_already_chosen() {
            return;
        }
        on_the_qt_thread(|| {
            let (mut panes, id) = one_pane();
            let mut decorations = Decorations::default();
            decorations.insert(&mut panes, id, 300, 200);
            assert!(
                panes.get(id).and_then(Pane::decoration).is_some(),
                "the claim is about a pane that really is framed, so building \
                 one has to have worked -- if this is what failed, Qt did not \
                 come up, not the thing under test"
            );

            decorations.set_bare(&mut panes, id);

            assert!(
                matches!(panes.get(id).map(Pane::frame), Some(Frame::None)),
                "and marking it bare is unconditional: a frame does not survive \
                 a call whose whole meaning is that there will never be one"
            );
            assert!(
                panes.get(id).and_then(Pane::decoration).is_none(),
                "the scene is gone, not standing behind a flag that no reader \
                 looked at"
            );
        });
    }

    #[test]
    fn a_reload_rebuilds_a_frame_rather_than_dropping_it() {
        // What `Solium::reload` does to every window that is open while it runs
        // — and until now the only thing standing behind it was the comment on
        // `set_style`. Nothing re-creates a frame on its own: they are built
        // once, when a client negotiates its decoration mode, so a reload that
        // *dropped* them instead of rebuilding them would leave every open
        // window bare until it was reopened. The window that is open during a
        // reload is the one being worked in.
        //
        // The sequence below is `reload`'s, line for line: clear the QML cache,
        // read the style back, set it to `None` and then to what it was. The
        // two calls are how it defeats `set_style`'s "nothing changed" guard —
        // see `a_style_that_has_not_changed_rebuilds_nothing`, which is the
        // other half of this.
        if the_environment_has_already_chosen() {
            return;
        }
        on_the_qt_thread(|| {
            // Three panes, because a reload has to leave two of them alone.
            // Only the first is ever framed.
            let (mut panes, framed) = one_pane();
            let slot = Rectangle::new((0, 0).into(), (300, 200).into());
            let bare_pane = panes.open(Pane::loading(
                "menu",
                None,
                slot,
                PathBuf::new(),
                None,
                std::time::Duration::ZERO,
            ));
            let waiting = panes.open(Pane::loading(
                "firefox",
                None,
                slot,
                PathBuf::new(),
                None,
                std::time::Duration::ZERO,
            ));

            let mut decorations = Decorations::default();
            assert!(decorations.set_style(&mut panes, Some("top".to_owned())));
            decorations.insert(&mut panes, framed, 300, 200);
            // A client drawing its own decorations, or an override-redirect menu.
            decorations.set_bare(&mut panes, bare_pane);
            // `waiting` is left as it was born: a client that has not arrived.

            if let Some(decoration) = panes.get_mut(framed).and_then(Pane::decoration_mut) {
                // What a rendered frame would have left behind, so that the rebuild
                // below is measured at a real size rather than at the 1x1 a frame
                // that has never been drawn reports. Realism and not an assertion:
                // the frame that comes back has its own `buffer_size` of zero
                // again, so there is nothing here that can read the size it was
                // rebuilt at.
                decoration.buffer_size = (300, 200 + TITLEBAR_HEIGHT);
                // And something only *this* `Decoration` knows, so that "there is
                // still a frame" can be told apart from "there is still the same
                // frame". A freshly built one carries `Shown::default()`.
                decoration.tell(
                    &Look {
                        title: "before the reload",
                        focused: true,
                        pointer_inside: false,
                    },
                    300,
                    200,
                );
            }
            assert_eq!(
                panes
                    .get(framed)
                    .and_then(Pane::decoration)
                    .map(|frame| frame.shown.title.as_str()),
                Some("before the reload"),
                "the claim is about a pane that really is framed, so building one \
                 has to have worked -- if this is what failed, Qt did not come up, \
                 not the thing under test"
            );

            // `Solium::reload`, from here down.
            crate::qml::clear_cache();
            let style = decorations.style().map(ToOwned::to_owned);

            assert!(
                decorations.set_style(&mut panes, None),
                "the first of the two"
            );
            assert!(
                panes.get(framed).and_then(Pane::decoration).is_some(),
                "and the first of the two must not be the call that drops it. \
                 `None` names the *default* decoration, not the absence of one, so \
                 this half is a rebuild as well -- if it cleared instead, the call \
                 below would walk the panes and find nothing left to rebuild, and \
                 every open window would be bare until it was reopened"
            );

            assert!(decorations.set_style(&mut panes, style), "and the second");

            assert!(
                panes.get(framed).and_then(Pane::decoration).is_some(),
                "a window that was framed before a reload is framed after it"
            );
            assert_eq!(
                panes
                    .get(framed)
                    .and_then(Pane::decoration)
                    .map(|frame| frame.shown.title.as_str()),
                Some(""),
                "and it is a different `Decoration`, built from the file as it now \
                 reads. Rebuilding rather than keeping is the whole point of the \
                 two calls: a reload that held on to the scenes it already had \
                 would not show an edited decoration until every window was \
                 reopened, which is the same defect as dropping them wearing \
                 better clothes"
            );

            // The other two panes are what pins the walk. `set_style` filters on
            // `Pane::decoration()`, and an inverted filter would hand a frame to a
            // pane that must never have one -- an override-redirect menu with a
            // titlebar on it -- while still passing every assertion above.
            assert!(
                matches!(panes.get(bare_pane).map(Pane::frame), Some(Frame::None)),
                "a pane that will never have a frame is not given one by a reload"
            );
            assert!(
                matches!(panes.get(waiting).map(Pane::frame), Some(Frame::Pending)),
                "and a client that has not arrived yet is still waiting, with its \
                 room still reserved"
            );

            // Deliberately not asserted: which pane is rebuilt first. That order
            // was `HashMap`'s and is now the stacking order, and nothing reads it.
        });
    }
}
