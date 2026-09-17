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

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use crate::{
    pane::{Frame, Pane, PaneId, Panes},
    style::{Bleed, Depth, LayerSpec, Style},
};

use anyhow::{Context as _, Result, anyhow};
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

use solium_effects::fragment::Corners;

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

/// The style drawn when nothing has named one.
///
/// The same name `config.lua` carries, and it has to be a *name* rather than a
/// path: every style this build ships is a folder under `panes/`, so the
/// default has to be looked up where folders are. It is reached when no script
/// has run or one set the style back to nothing — a session whose scripts
/// failed to load still gets titlebars.
const DEFAULT_STYLE: &str = "top";

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
    /// The pane's outer size, in logical pixels.
    ///
    /// Here because [`Decoration::tell`] hands QML four numbers derived from it
    /// — `contentWidth`, `contentHeight`, `paneWidth`, `paneHeight` — and
    /// returns early when nothing it shows has changed. Without the size in
    /// that comparison a window resized without being retitled or refocused
    /// keeps the sizes it had when its title last changed, which
    /// `panes/reactive/Frame.qml` reads to size its own content.
    size: (i32, i32),
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

/// The rectangle a layer is rasterised into: the pane, grown by its bleed.
///
/// **This is the whole of the clip.** A layer is drawn into a buffer of exactly
/// this size, so content reaching past it is not clipped by a rule somebody has
/// to remember to apply — there is nowhere for it to go. Bleed being a promise
/// rather than a request is a property of the arrangement, which is why there is
/// no `clip: true` anywhere and no region intersected on the way out: one style
/// cannot force a full-screen repaint every frame because one style cannot
/// produce pixels outside its own canvas.
///
/// The pane's own corner moves within it — a canvas that bleeds upward starts
/// above the pane — which is why a layer is told `bleedLeft` and `bleedTop`:
/// `anchors.fill: parent` covers the canvas, and QML needs a known origin to
/// place the window's own corner against.
///
/// **Capped at [`qml::MAX_SIDE`] per side**, in logical pixels. `bleed` is
/// author-controlled and this is the first place it becomes a size, so it is
/// also the last place a typo can be turned into something drawable rather than
/// a 1.6 GB allocation. See [`fits`] for how much of an over-large bleed
/// survives, and `qml::MAX_SIDE` for why the cap is applied twice.
pub(crate) fn canvas(outer: Rectangle<i32, Logical>, bleed: Bleed) -> Rectangle<i32, Logical> {
    let (left, right) = fits(outer.size.w, bleed.left, bleed.right);
    let (top, bottom) = fits(outer.size.h, bleed.top, bleed.bottom);
    Rectangle::new(
        (outer.loc.x - left, outer.loc.y - top).into(),
        (outer.size.w + left + right, outer.size.h + top + bottom).into(),
    )
}

/// How much of the bleed asked for on one axis actually fits on the canvas.
///
/// Both sides are reduced in proportion to what they asked for, rather than one
/// of them being sacrificed: a layer that asked for a symmetric glow and is
/// given all of it on the left and none on the right is worse to look at than
/// one given half of each, and the arithmetic is the same length either way.
///
/// A pane already wider than [`qml::MAX_SIDE`] has `room` of zero, so it gets no
/// bleed at all and the canvas is the pane — today's behaviour, which is the
/// right thing for this to degrade to.
fn fits(pane: i32, before: i32, after: i32) -> (i32, i32) {
    let asked = before.saturating_add(after);
    let room = qml::MAX_SIDE.saturating_sub(pane);
    if asked <= room {
        return (before, after);
    }
    if room <= 0 || asked <= 0 {
        return (0, 0);
    }
    // Widened, because `before * room` reaches 2^31 * 2^13 and a style that
    // overflowed here would get a *negative* bleed — a canvas smaller than the
    // pane, which is the one thing `pixels` in `style.rs` refuses outright.
    // `before <= asked`, so the quotient is at most `room` and the narrowing
    // back cannot fail.
    let kept = i32::try_from(i64::from(before) * i64::from(room) / i64::from(asked)).unwrap_or(0);
    (kept, room - kept)
}

/// One layer's canvas, and where on screen it lands this frame.
///
/// Two rectangles because they are not the same rectangle and are not even in
/// the same units. [`Self::canvas`] is the pane's own space at the pane's own
/// size — what Qt lays the scene out in and what the buffer is — and
/// [`Self::drawn`] is where a presentation transform has put it, which in a
/// mode is somewhere else and a different size.
///
/// A pure function of a [`Drawing`] and a [`Bleed`], so the arithmetic that
/// decides both a layer's cost and its damage can be read out without a
/// renderer, a GPU or Qt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Spread {
    /// What the scene is rasterised into, relative to the pane's own top-left
    /// corner: `loc` is `(-left, -top)` and `size` is the pane plus the bleed.
    pub(crate) canvas: Rectangle<i32, Logical>,
    /// **The element's geometry, and therefore its damage.**
    ///
    /// Smithay tracks an element by its geometry and damages what it vacates,
    /// so a layer whose element is the *pane's* rect while its picture is the
    /// canvas would both squash the picture into the pane and leave the bleed
    /// undamaged when the window moves — an animating glow smeared across its
    /// neighbours, with the pane itself repainting perfectly. Stating the
    /// canvas here is what makes rule three hold, and it is the only thing
    /// that does.
    pub(crate) drawn: Rectangle<f64, Logical>,
}

/// Where one layer's canvas is, and where it is drawn.
///
/// The bleed scales with the window exactly as the frame does: a thumbnail in
/// an overview carries its spikes at thumbnail size, because the buffer is
/// mapped onto this rectangle whole. Growing the drawn rect by the *unscaled*
/// bleed instead would map a shrunken pane's picture onto a full-size border,
/// which reads as the effect detaching from the window it belongs to.
///
/// Reachable from `render::elements` as well as from here, and for one reason:
/// the off-screen cull has to ask what rectangle a pane's widest layer will
/// occupy, and a second answer to that question is how a pane comes to be
/// culled off a screen its glow is still on.
pub(crate) fn spread(drawing: Drawing, bleed: Bleed) -> Spread {
    let canvas = canvas(Rectangle::from_size(drawing.outer), bleed);
    let across = crate::render::ratio(drawing.rect.size.w, drawing.outer.w);
    let down = crate::render::ratio(drawing.rect.size.h, drawing.outer.h);
    // From the canvas and not from the bleed as declared, so that the cap above
    // cannot leave the buffer and the rectangle it is drawn into disagreeing —
    // which would stretch the picture rather than clip it.
    let (left, top) = (-canvas.loc.x, -canvas.loc.y);
    let (wider, taller) = (
        canvas.size.w - drawing.outer.w,
        canvas.size.h - drawing.outer.h,
    );
    Spread {
        canvas,
        drawn: Rectangle::new(
            (
                drawing.rect.loc.x - f64::from(left) * across,
                drawing.rect.loc.y - f64::from(top) * down,
            )
                .into(),
            (
                drawing.rect.size.w + f64::from(wider) * across,
                drawing.rect.size.h + f64::from(taller) * down,
            )
                .into(),
        ),
    }
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
    /// How far past the pane this layer may paint, as it declared.
    ///
    /// Carried per layer and not per decoration because it *is* per layer:
    /// three layers of one style have three different canvases, and the cost of
    /// bleed is paid by the layer that asked for it. Read by [`spread`] for the
    /// canvas, by `overlay` below for the bands, and by
    /// [`Decoration::pointer`], which has to put the pointer back into a
    /// coordinate frame this moved.
    bleed: Bleed,
    /// Whether this layer paints outside the space the style reserved.
    ///
    /// A layer that stays inside the insets only has to have those bands
    /// copied and uploaded each time it changes; one that draws over the
    /// client -- a bar floating above the window, a glow across it -- has to
    /// have all of it copied, because anything in it may have moved. A layer
    /// at `behind` or `above` is one by definition.
    ///
    /// **And so is one with any bleed at all**, which is not an approximation:
    /// the bands are strips of the *buffer* measured from its corners, and a
    /// bleeding layer's buffer starts `bleed.left` to the left of the pane. Its
    /// top band would be a strip of empty canvas above the titlebar and the
    /// titlebar itself would never be copied, so a spike-throwing bar would
    /// upload its spikes and drop its own bar.
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
    /// What the style runs on the client's own pixels, read once when it was
    /// built.
    ///
    /// Here for the same reason `insets` is here and by the same route: the
    /// declaration lives on `PaneStyle`, and this is the pane's copy of it.
    /// **The renderer has no other way to reach a pane's `Style`** -- nothing
    /// keeps one after `from_style` has read it -- so a pane's decoration is
    /// what `render::prepare` asks whether this window needs a pass.
    ///
    /// Empty is the ordinary case and the whole cost model: a window whose
    /// style declares nothing here is drawn exactly as it was before any of
    /// this existed, and every window on an unstyled machine is that window.
    effects: Vec<solium_effects::fragment::Effect>,
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
        // paths — they are constants by contract.
        //
        // **What did change, and is worth knowing before someone relaxes that
        // contract.** Before this, both paths read the insets from a
        // client-sized scene, so a size-dependent inset gave the *same* wrong
        // answer on each. Now the GPU path reads them from a 1x1 scene and the
        // software path still reads them from a client-sized one, so it would
        // give two *different* wrong answers — a window whose frame reserved
        // one strip of space on a TTY and another under winit, from one QML
        // file. The day one binds an inset to `width` or `height`, this stops
        // being a shared inaccuracy and becomes a divergence between the two
        // paths, which is much harder to see: each path is self-consistent.
        //
        // It used to be held in check by the eight decorations that shipped,
        // every one of which declared `insetTop`/`Right`/`Bottom`/`Left` and
        // `overlay` as literals. Task 7 moved them into bundles, so the files
        // this reads are now only ever somebody else's and nothing in this tree
        // holds the line any more. A bundle has no such hazard — its insets are
        // read from a manifest built at 1x1 on both paths — which is one more
        // reason the conversion was worth doing.
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
                // A single QML file has no manifest to declare one in, so a
                // decoration's canvas is its pane and every arithmetic below
                // collapses to what it was before layers existed.
                bleed: Bleed::default(),
                overlay,
                buffer_size: (0, 0),
            }],
            insets,
            // A single QML file has no manifest either, and `client.radius` is
            // declared on `PaneStyle`. So a decoration that is one file runs
            // no effects and costs no pass, which is what it has always cost.
            effects: Vec::new(),
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
            // And the same, one line down, for the same reason: `client.radius`
            // is declared on `PaneStyle` once, and every layer is *told* it
            // rather than asked for it. See `LayerScene::build`.
            effects: style.effects.clone(),
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

    /// What this style runs on the client's own pixels.
    ///
    /// Asked once per pane per frame by `render::prepare`, and empty for every
    /// window on a machine nobody has styled -- so it is a slice and not an
    /// `Option<Effect>`, and answering costs a pointer and a length.
    pub(crate) fn effects(&self) -> &[solium_effects::fragment::Effect] {
        &self.effects
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
    /// **The size and the position are per layer**, which is what bleed is.
    /// Each layer is rasterised into its own canvas — the pane grown by the
    /// bleed *it* declared — and the element is placed at that canvas rather
    /// than at the pane, so the picture is not squashed back into the pane and
    /// the damage the compositor tracks is the canvas. The pane's position among
    /// the other panes is untouched: bleed changes a layer's canvas, not its
    /// depth, so a background window's glow is still covered by the window in
    /// front of it. See [`Spread`].
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

        // Before anything renders, on either path — and for **every** layer,
        // not the ones about to be drawn. This runs once per depth and the
        // write is guarded by `shown`, so telling only this depth's layers
        // would tell whichever depth came first and silently skip the rest.
        self.tell(look, width, height);
        // The **pane's** pixels and not any layer's canvas. `client_size` reads
        // this to size a client while a style is being swapped, and a bleeding
        // layer's canvas would tell it the window is larger than it is.
        self.buffer_size = (pixels(width), pixels(height));

        let insets = self.insets;
        let mut animating = false;
        for layer in at(self.layers.iter_mut(), depth) {
            // Per layer, because this is the whole of bleed: its own canvas,
            // its own buffer, and its own rectangle on screen.
            let spread = spread(drawing, layer.bleed);
            let size = (pixels(spread.canvas.size.w), pixels(spread.canvas.size.h));
            // The drawn size scales the layer with the window it belongs to.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a canvas is at most MAX_SIDE, which is an i32"
            )]
            let drawn_size: Size<i32, Logical> = (
                spread.drawn.size.w.round() as i32,
                spread.drawn.size.h.round().max(1.0) as i32,
            )
                .into();
            let placement = Placement {
                // Physical, which is what the parameter has always been: at 1x
                // a logical position was the same number and it did not matter.
                position: (
                    spread.drawn.loc.x * drawing.scale,
                    spread.drawn.loc.y * drawing.scale,
                ),
                size: drawn_size,
                alpha: drawing.alpha,
                kind: Kind::Unspecified,
            };
            let drawn = layer.element(renderer, insets, size, placement, drawing.scale);
            into.extend(drawn.element);
            animating |= drawn.animating;
        }
        animating
    }

    /// The widest any of this style's layers may paint past the pane.
    ///
    /// The union and not a sum: three layers each reaching 24px upward reach
    /// 24px upward. It is what bounds the whole decoration on screen, which is
    /// the one question asked of a pane rather than of a layer — see the
    /// off-screen cull in `render::elements`, where a pane whose own rect has
    /// left the monitor may still have a glow reaching back onto it.
    pub(crate) fn bleed(&self) -> Bleed {
        self.layers
            .iter()
            .fold(Bleed::default(), |widest, layer| Bleed {
                top: widest.top.max(layer.bleed.top),
                right: widest.right.max(layer.bleed.right),
                bottom: widest.bottom.max(layer.bleed.bottom),
                left: widest.left.max(layer.bleed.left),
            })
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
    /// tell whether the window is focused is a glow that is always on. The
    /// properties are declared on `PaneStyle` as well as on a delegated layer's
    /// own root, so an inline layer can bind to them too.
    ///
    /// The **size** is in the comparison as well as the look, and that is not
    /// tidiness. `paneWidth` and `paneHeight` are how a bleeding layer finds
    /// the window's own rectangle inside a canvas larger than it — with
    /// `bleedLeft` and `bleedTop`, which never change and are written once at
    /// build. Leaving the size out would mean a window that is resized without
    /// being retitled keeps the numbers it had at its last title change, which
    /// puts a spike-throwing bar's spikes at the width the window used to be.
    fn tell(&mut self, look: &Look<'_>, width: i32, height: i32) {
        let Look {
            title,
            focused,
            pointer_inside,
        } = *look;
        if self.shown.title == title
            && self.shown.focused == focused
            && self.shown.pointer_inside == pointer_inside
            && self.shown.size == (width, height)
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
            // The pane, which is not the canvas a bleeding layer is laid out
            // in and not the client either.
            layer.scene.set_int("paneWidth", width);
            layer.scene.set_int("paneHeight", height);
        }
        self.shown.title.clear();
        self.shown.title.push_str(title);
        self.shown.focused = focused;
        self.shown.pointer_inside = pointer_inside;
        self.shown.size = (width, height);
    }

    /// Tell the frame the pointer has left the window altogether.
    ///
    /// Sent as a position rather than a flag as well, because QML's hover
    /// handling is positional: a `MouseArea` that never sees the pointer leave
    /// stays hovered forever, and a border lit by proximity stays lit.
    ///
    /// Sent raw, **not** shifted into a bleeding layer's canvas the way
    /// [`Self::pointer`] shifts a real position. `(-1, -1)` means "off the end
    /// of everything", and a layer bleeding 24px to the left would receive
    /// `(23, 23)` — a point comfortably inside its canvas, so the leave event
    /// would arrive as a hover and the border lit by proximity would stay lit
    /// for the rest of the session.
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
    /// scoped* section is about and which nothing here can answer yet. What
    /// [`Self::take_action`] does in the meantime is decide which of the layers
    /// that answered is *believed*, topmost first — a strictly better guess
    /// than the one underneath, and one that needs nothing new from Qt.
    ///
    /// **Into each layer's own canvas**, which is not the pane's space once a
    /// layer bleeds. `x` and `y` are measured from the pane's top-left corner
    /// and a canvas starts `bleedLeft` to the left of it, so a layer with any
    /// bleed would otherwise find every button `bleedLeft` to the right of
    /// where the pointer really was — the close button lighting up while the
    /// pointer is over the maximise one. This is only the frame the point is
    /// expressed in; the *clip* is in the callers, both of which test the
    /// pane's outer rect before they reach a decoration at all — see
    /// `state::tests::a_point_in_the_bleed_is_not_in_the_pane`.
    pub(crate) fn pointer(&mut self, x: f64, y: f64, pressed: Option<bool>) {
        for layer in &mut self.layers {
            layer.scene.pointer(
                x + f64::from(layer.bleed.left),
                y + f64::from(layer.bleed.top),
                pressed,
            );
        }
    }

    /// Whether the pointer is over a button.
    ///
    /// Asked of QML rather than worked out from coordinates: QML owns the
    /// button layout, so a copy of it here would be a second authority that
    /// drifts the first time the frame is restyled.
    ///
    /// **Order-free, where [`Self::take_action`] is not**, and that is a claim
    /// about what this question is rather than an omission. It is not "which
    /// layer owns the press"; it is "is the pointer over a button at all", and
    /// the caller spends the answer on one decision — whether the press starts
    /// a window drag. A button on a lower layer is still a button the user can
    /// see and aim at, through a higher layer that painted nothing over it, so
    /// answering "no, the topmost layer has none" would drag the window out
    /// from under a close button. `any` over a set is the same answer in any
    /// order, so walking this one topmost-first would change how it reads and
    /// never what it answers — which is why the walk is left exactly as it was.
    pub(crate) fn on_button(&self) -> bool {
        self.layers
            .iter()
            .any(|layer| layer.scene.get_bool("onButton"))
    }

    /// What a button asked for since the last call, if anything.
    ///
    /// **Topmost first**, which is [`crate::render::PANE_ORDER`] read from the
    /// front — the order the compositor draws in — so that of the layers that
    /// *answered*, the one on top is believed. It arbitrates; it does not
    /// decide who is asked. [`Self::pointer`] still delivers to every layer, so
    /// an `above` layer painting an opaque panel over a close button it has no
    /// `MouseArea` of its own for leaves that button the only answerer, and the
    /// window closes under the panel. Fixing *that* needs the scene to say
    /// whether a `MouseArea` accepted, which is the spec's *Input, scoped*.
    /// This used to fold over
    /// declaration order, and declaration order is bottom-to-top (see [`at`]):
    /// two layers with a button in the same place handed the press to the one
    /// *underneath*, which is the opposite of what is on screen. A style with a
    /// single layer has both orders at once, which is why nothing has bitten.
    ///
    /// Taken from `PANE_ORDER` rather than restated as a list here, because a
    /// second ordering is one that can drift from the first and the drift would
    /// be silent: input precedence and stacking would disagree only for a style
    /// whose layers contest a point, and only in favour of the layer that
    /// cannot be seen.
    ///
    /// **Every layer is still asked**, even once one has answered, because
    /// `take_string` is what *clears* the property: a layer left unasked keeps
    /// its action and fires it on whichever later frame something else happens
    /// to ask. That is why this collects every answer and then picks, rather
    /// than stopping at the first — and why
    /// `a_layer_that_lost_the_press_still_has_its_action_taken` exists to fail
    /// if the collection is ever turned back into an early return.
    pub(crate) fn take_action(&mut self) -> Option<Action> {
        let layers = &mut self.layers;
        // Every answer, topmost first. A press that hit no button pushes
        // nothing, so the ordinary case allocates nothing either.
        let mut asked: Vec<Action> = Vec::new();
        crate::render::pane_pieces(&mut asked, |into, piece| match piece {
            crate::render::Piece::Layers(depth) => into.extend(
                at(layers.iter_mut(), depth)
                    // Driven to the end by `extend`, so the property is cleared
                    // on every layer of this depth and not only on the winner.
                    .filter_map(|layer| layer.scene.take_string("action"))
                    .filter_map(|name| Action::parse(&name)),
            ),
            // The client is in the order because the order is the frame's, and
            // a walk that left it out would be a second list. It has no
            // property to take.
            crate::render::Piece::Client => {}
        });
        asked.first().copied()
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

/// The four radii a layer of this style should hug, in logical pixels.
///
/// All zero when the style declares no effect that masks the client, which is
/// thirteen of the fifteen shipped bundles and every style nobody has
/// touched; `panes/rounded/` and `panes/flush/` are the two that declare a
/// radius -- and `flush/` is the one whose four are not all the same number --
/// while `panes/example/` writes `0` on purpose to show the key costs nothing. A
/// style wanting a rounded border around a *square* client declares no
/// `client.radius` and sets its own `radius` — and pays for no pass, which is
/// the point.
///
/// The *first* effect that is really an effect, which is the same choice
/// `pass::needs_pass` makes and is made here again rather than shared with it:
/// that one answers in `Effect`s for a shader, this one in numbers for QML,
/// and a `Style` holding two rounding effects at once is a thing to design
/// when something can declare one.
fn client_radii(style: &Style) -> Corners {
    style
        .effects
        .iter()
        .find(|effect| !effect.is_none_effect())
        .map_or(Corners::all(0.0), |effect| effect.radii())
}

/// A logical radius as the `i32` a layer is told.
///
/// Whole because `set_int` is; exact because the number came *from* an `i32`.
/// `style::load` reads every corner with `get_int` and widens it, so the round
/// trip back is the same value it started as — there is nothing here to round,
/// only a type to put back.
///
/// **Clamped at zero, and this is the one place the two sides of the seam
/// disagree on purpose.** A negative corner is reachable: `client.radius: -5`
/// with one corner declared positive gives three negative corners and one real
/// one, and `Corners::is_none` refuses only the case where *all* of them are.
/// On the compositor's side it means something — the shader's `p` is pushed
/// further negative on both axes, so a negative radius *inflates* the shape
/// rather than cutting it, and `pass::side_inset` keeps the sign and clamps
/// only the inset it derives from it.
///
/// QML has no such reading. `Rectangle.radius` is undefined for a negative,
/// and a layer doing arithmetic on one — `clientRadius + 2`, the outward hug —
/// would put its border *inside* the client instead of around it. So what
/// crosses the seam is the number a layer can use, and the sign stays on the
/// side that has a use for it.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the radius reached `Effect` from `get_int`, so it was an i32 \
              before it was an f64 and the round trip is exact"
)]
fn whole(radius: f64) -> i32 {
    (radius as i32).max(0)
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
        //
        // The **canvas** and not the pane, on the software path: this layer is
        // laid out at whatever it may paint on, which is the pane grown by the
        // bleed it declared. A scene built at the pane's size and only resized
        // later would lay every anchor out against the wrong rectangle for one
        // frame, which for a layer whose content is positioned against
        // `bleedTop` is the frame where the spikes are in the titlebar.
        let on_gpu = qml::on_gpu();
        let canvas = canvas(
            Rectangle::from_size(
                (
                    width + style.insets.horizontal(),
                    height + style.insets.vertical(),
                )
                    .into(),
            ),
            spec.bleed,
        );
        let built = if on_gpu {
            (1, 1)
        } else {
            (canvas.size.w.max(1), canvas.size.h.max(1))
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
        let mut scene = qml::Scene::for_host(path, built.0, built.1, initial)?;
        // Where the pane's own corner is inside the canvas. Written once
        // because a declared bleed never changes; the pane's *size* does, and
        // that goes through `tell` with everything else that can move.
        //
        // On the scene's root whichever way the layer was written: `PaneStyle`
        // declares all four for an inline layer, a delegated file declares the
        // ones it uses on its own root, and one that positions nothing against
        // the window declares neither and is handed a property it ignores.
        scene.set_int("bleedLeft", spec.bleed.left);
        scene.set_int("bleedTop", spec.bleed.top);
        // And what the style reserved, for the same reason and on the same
        // terms: written once, because a declared inset never changes either.
        //
        // **This is the property that makes a decoration convertible.** Seven
        // of the eight that shipped painted themselves from their own
        // `insetTop` — `height: frame.insetTop` — so a bundle whose `Pane.qml`
        // declared the number and whose `Frame.qml` had it deleted would have
        // drawn a bar of height zero. The alternative was the same literal
        // written into both files with a comment asking that they be kept in
        // step, which is two answers to one question and exactly what putting
        // `insets` on `PaneStyle` rather than on `Layer` was for.
        //
        // Flowing the *other* way from `Decoration::new`, which reads these
        // four off a single QML file because that file is the only place a
        // decoration's insets exist. A bundle names them once in its manifest
        // and every layer is told; the spelling is the same either way, so a
        // file converted into a bundle keeps the bindings it already had.
        scene.set_int("insetTop", style.insets.top);
        scene.set_int("insetRight", style.insets.right);
        scene.set_int("insetBottom", style.insets.bottom);
        scene.set_int("insetLeft", style.insets.left);
        // What the client is being masked to, so a layer can match it. A
        // rounded client inside a square border is the failure this prevents,
        // and it is declared once on the style — exactly why `client.radius`
        // lives on `PaneStyle` and not on `Layer`, and exactly how `insets`
        // above already work.
        //
        // **The split falls out of who drew the pixels.** The client's are the
        // application's, so the compositor masks them with a fragment program
        // — `pass.rs`, and the whole of this plan. A layer's are Qt's, and Qt
        // rounds a rectangle with one property; a GPU pass to do what
        // `Rectangle.radius` does for free would be absurd. So only the
        // *numbers* cross the seam, and this is the crossing.
        //
        // LOGICAL pixels, like every other value a layer is told. `pass.rs`
        // multiplies by the output scale for the shader; nothing QML sees is
        // ever in device pixels.
        //
        // All four, and not the one a titlebar happens to need. Same reason
        // all four insets are written: a layer that reads a property the
        // compositor decided not to send gets 0, and 0 is a legal radius —
        // so a corner left unwritten is indistinguishable from a corner the
        // style really squared, and a bar would hug a curve nobody cut.
        //
        // Written unconditionally for the same reason, so a style that
        // declares no radius tells its layers zero rather than leaving
        // whatever the file defaulted to.
        let radii = client_radii(style);
        scene.set_int("clientRadiusTopLeft", whole(radii.top_left));
        scene.set_int("clientRadiusTopRight", whole(radii.top_right));
        scene.set_int("clientRadiusBottomLeft", whole(radii.bottom_left));
        scene.set_int("clientRadiusBottomRight", whole(radii.bottom_right));
        // **And the singular survives.** It is not replaced by the four above:
        // `PaneStyle.qml` uses it itself, `docs/ricing.md` documents it, and a
        // bundle nobody in this tree wrote may read it — removing it would
        // break those silently, since an undeclared property reads back 0 and
        // 0 is a radius.
        //
        // The LARGEST of the four when they differ, because that field's only
        // in-tree use is an outward hug — `PaneStyle.qml`'s `clientRadius + 2`,
        // "hug it from outside" — and a hug has to clear the biggest cut or it
        // clips into it. The cost of being wrong this way is visible slack
        // around a corner that was cut less; the cost the other way is a border
        // crossing the curve. A layer that wants the real number for one corner
        // now has it by name.
        scene.set_int("clientRadius", whole(radii.largest()));
        // A layer at `behind` or `above` paints over the client by definition,
        // and so does any layer of a style that reserved nothing: there is
        // nowhere else for it to paint. Only a `frame` layer inside real insets
        // can be copied band by band, and only if it says it stays inside them
        // — and a layer with any bleed at all has already said it does not.
        let overlay = spec.depth != Depth::Frame
            || !style.insets.any()
            || spec.bleed.any()
            || scene.get_bool("overlay");
        Ok(Self {
            depth: spec.depth,
            name: spec.name.clone(),
            scene,
            backing: Backing::for_host(on_gpu),
            bleed: spec.bleed,
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
            // frame that is not coming. So `pane = "none"` only takes
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
/// the one file decorations have always been. That order is what made the
/// conversion of the shipped eight a move of files rather than a change of
/// setting: `pane = "top"` drew `decorations/top.qml` before Task 7 and draws
/// `panes/top/` after it, and nothing in `config.lua` changed on either side.
///
/// The two are the same thing at different sizes, which is why there is one
/// entry point and not two: a decoration is a style with one `frame` layer.
fn build(style: Option<&str>, width: i32, height: i32) -> Result<Decoration> {
    // Nothing named at all — no script has run, or one set the style back to
    // nothing — draws the default rather than failing. It is named here rather
    // than left to `qml_path`, which used to answer this with
    // `shipped_decoration("top")`: that file is a folder now, so the default has
    // to be a *name* looked up where bundles are, or a session whose scripts did
    // not load would put a missing-file error against every window it opened,
    // naming a path that has not existed since Task 7.
    let style = Some(style.unwrap_or(DEFAULT_STYLE));
    let Some(dir) = bundle(style) else {
        // Not a bundle, so a single QML file -- and if it is not one of those
        // either, say so **here**, naming the name that was typed and every
        // place both halves of the lookup went.
        //
        // `qml_path` used to answer this by fabricating
        // `<shipped>/decorations/<name>.qml` and letting Qt fail on it, which
        // after Task 7 names a file in a directory that is not in the tree and
        // says nothing about `panes/`. `style::resolve` answers the same
        // question the other way -- "a name in no place at all is `None`, which
        // names the name that was typed" -- and two answers to one question is
        // what the rest of this commit is organised against. This is the
        // `resolve` answer, because it is the one that can name the *name*: a
        // fabricated path names a guess, and the guess was wrong in both
        // directories at once.
        let Some(path) = qml_path(style) else {
            let name = chosen(style).unwrap_or_default();
            return Err(anyhow!(
                "no pane style called `{name}`. A style is a folder with a Pane.qml in it; \
                 this looked for one in [{}], and for a single-file decoration `{name}.qml` \
                 in [{}]",
                display_all(&crate::style::directories()),
                display_all(&decoration_directories()),
            ));
        };
        return Decoration::new(&path, width, height);
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

/// Say what a bundle built, in the order it will be drawn and how far past the
/// window it may draw.
///
/// At `debug`, once per window, and it earns the line because what this feature
/// adds is an *order* and a *reach*: "the spikes are behind the window" is
/// otherwise diagnosed by looking at the screen and guessing which of the depth
/// string, the declaration order and the element list was wrong.
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
    // The reach as well as the order, because a style that looks wrong raises
    // two questions and neither is answerable from the screen. "The spikes are
    // behind the window" is the order; "the spikes are inside the titlebar" is
    // a canvas that did not grow, and this is the number it grew by.
    tracing::debug!(
        bundle = %dir.display(),
        ?order,
        reach = ?decoration.bleed(),
        "built a pane style; topmost first, and the client is where it says"
    );
}

/// The style bundle this setting asks for, if it asks for one at all.
///
/// `SOLIUM_QML_TITLEBAR` and any name ending in `.qml` are a *file* and never a
/// bundle — `style::find` takes anything with a separator in it as a path, so
/// without this a `SOLIUM_PANE=~/mine.qml` would be handed back as a directory
/// and then reported as having no `Pane.qml` in it.
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

/// What a `decoration =` name can turn out to be.
///
/// The variant order is [`build`]'s order, and both the sort below and
/// [`catalogue`]'s shadowing rely on it: a bundle is tried first, so a bundle
/// wins. Pinned by `bundles_come_before_files_and_each_group_is_sorted`,
/// because a derived `Ord` over a reordered enum is a silent change.
///
/// `StyleKind` rather than `Kind` because this file already imports smithay's
/// `element::Kind` — the same reason the module header spells out which of the
/// three "Layer"s it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StyleKind {
    /// A folder under `panes/` with a `Pane.qml`: layers, and bleed.
    Bundle,
    /// One file under `decorations/`: a single layer at [`Depth::Frame`].
    ///
    /// Nothing ships as one any more — Task 7 made every shipped style a
    /// bundle — so this kind is now only ever the user's own files. The kind
    /// stays because those files do.
    File,
}

impl StyleKind {
    /// The word a script sees. Matched in `lua/tweaks.lua`.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Bundle => "bundle",
            Self::File => "file",
        }
    }
}

/// One thing this machine can be asked to draw.
///
/// Field order is the sort order: kind, then name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Offered {
    pub(crate) kind: StyleKind,
    pub(crate) name: String,
}

/// One directory to look in, and what a name found there means.
struct Place {
    dir: PathBuf,
    kind: StyleKind,
}

/// Everywhere a bare name is looked for, in the order [`build`] looks.
///
/// Both halves come from the function that does the looking —
/// [`crate::style::directories`] and [`decoration_directories`] — rather than
/// being spelled out again here, so there is no second copy of the order to
/// drift.
fn places() -> Vec<Place> {
    let bundles = crate::style::directories().into_iter().map(|dir| Place {
        dir,
        kind: StyleKind::Bundle,
    });
    let files = decoration_directories().into_iter().map(|dir| Place {
        dir,
        kind: StyleKind::File,
    });
    bundles.chain(files).collect()
}

/// Everything `places` can offer: one entry per name, nearest place winning.
///
/// A name found in an earlier place hides the same name in a later one, which
/// is [`build`]'s own order said once more — a bundle called `top` shadows
/// the user's `decorations/top.qml`, and the user's `top` shadows the shipped
/// one. So the
/// list cannot offer a name whose press lands on something else, which is the
/// single thing a discovered list can get wrong that a declared one could not.
///
/// Sorted for display, because `read_dir` order is whatever the filesystem
/// feels like: unsorted, the panel's buttons would move between runs.
fn catalogue(places: &[Place]) -> Vec<Offered> {
    let mut offered: Vec<Offered> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for place in places {
        // A directory that is not there is not an error and not a warning: most
        // people have no `panes/` of their own, and that is the normal case
        // rather than a misconfiguration.
        let Ok(entries) = std::fs::read_dir(&place.dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Some(name) = offered_by(&entry.path(), place.kind) else {
                continue;
            };
            if seen.insert(name.clone()) {
                offered.push(Offered {
                    kind: place.kind,
                    name,
                });
            }
        }
    }
    offered.sort();
    offered
}

/// The name `path` offers under a place of this kind, or `None` for one that
/// offers nothing.
fn offered_by(path: &Path, kind: StyleKind) -> Option<String> {
    match kind {
        // `Pane.qml` rather than merely a directory. `style::load` reads that
        // file first and fails with "has no Pane.qml" without it, so a folder
        // that has none is not a style and listing it would be listing a button
        // that cannot work. It also keeps shared pieces out of the panel: a
        // `panes/common/` of components several bundles import is a directory
        // and is not a style.
        StyleKind::Bundle => path
            .join("Pane.qml")
            .is_file()
            .then(|| path.file_name()?.to_str().map(ToOwned::to_owned))
            .flatten(),
        StyleKind::File => (path.is_file() && path.extension().is_some_and(|end| end == "qml"))
            .then(|| path.file_stem()?.to_str().map(ToOwned::to_owned))
            .flatten(),
    }
}

/// Everything this machine can be asked to draw, for a script to offer.
///
/// **Discovered, not declared.** `lua/tweaks.lua` kept a hand-written array of
/// the eight decorations that shipped, so a style the user wrote was never in
/// the Developer Tweaks panel however correct it was. The names now come from
/// the directories the compositor actually resolves against, the same way
/// `script::parse_easing` takes its curves from the animation engine rather
/// than a list beside it — a bundle dropped into
/// `~/.config/solium/qml/panes/` is in the panel after one reload, with
/// nothing to edit.
///
/// Walked on every call rather than cached: it is a directory listing, it
/// happens when a script asks, and the whole point is that it is current.
pub(crate) fn available() -> Vec<Offered> {
    catalogue(&places())
}

/// Everything *this build ships* can be asked to draw.
///
/// Shipped only, and the distinction is the whole point: this answers "does the
/// configuration in the repository name something the repository contains",
/// which [`available`] cannot, because it would resolve a name against
/// whatever happens to be in the developer's own `~/.config/solium` and pass
/// on their machine alone. Read by
/// `script::shipped::every_decoration_named_is_one_that_ships`.
#[cfg(test)]
pub(crate) fn ships() -> Vec<Offered> {
    catalogue(&[
        Place {
            dir: crate::style::shipped(),
            kind: StyleKind::Bundle,
        },
        Place {
            dir: shipped_decorations(),
            kind: StyleKind::File,
        },
    ])
}

/// What names the frame: the environment, and then the configuration.
///
/// The environment wins over the configuration, not the other way round. The
/// configuration always names something -- the shipped one says "top" -- so a
/// script's choice losing to nothing would be one thing, but a script's choice
/// *winning* leaves `SOLIUM_PANE` with no effect at all. It is set by whoever
/// started this particular run, to try one style for one session, and that is
/// the more specific intent.
///
/// `SOLIUM_DECORATION` is the old spelling of the same knob, kept for the same
/// reason `sol.decoration` is: a style used to be a single QML file and is now
/// a folder, the name changed with it, and no session started from a shell
/// history written before the change should silently draw something else. The
/// new name wins when both are set.
fn chosen(style: Option<&str>) -> Option<String> {
    named_by(
        std::env::var("SOLIUM_PANE").ok(),
        std::env::var("SOLIUM_DECORATION").ok(),
        style,
    )
}

/// The precedence, with the environment handed in rather than read.
///
/// Split out for the same reason `style::requirements_on` is: a test cannot
/// choose the other one. Tests share a process and its environment, so a test
/// that exported `SOLIUM_PANE` would decide the answer for every other test in
/// the binary -- which is why `the_environment_has_already_chosen` exists at
/// all. Here every combination is reachable and none of them touches anything
/// else.
///
/// What stays untested is the two `env::var` calls above, which is the honest
/// size of the gap and smaller than the alias going unexercised entirely.
fn named_by(
    pane: Option<String>,
    decoration: Option<String>,
    style: Option<&str>,
) -> Option<String> {
    pane.or(decoration).or_else(|| style.map(ToOwned::to_owned))
}

/// Whether this run's environment has already named a style.
///
/// One function rather than a pair of `var_os` calls at each site, because the
/// alias is the kind of thing that gets honoured in one place and forgotten in
/// another -- and every caller is a test standing down so it does not assert
/// about whatever the developer's shell had exported.
#[cfg(test)]
fn environment_names_a_style() -> bool {
    std::env::var_os("SOLIUM_PANE").is_some() || std::env::var_os("SOLIUM_DECORATION").is_some()
}

/// Where the frame's QML lives, when it is one file rather than a bundle.
///
/// Overridable so a frame can be restyled and reloaded without a rebuild,
/// which is most of the point of authoring it in QML.
///
/// **Nothing ships here any more.** Every style the compositor ships is a
/// bundle under `panes/`, so a bare name that reaches this function is one
/// [`crate::style::find`] did not answer, and the only names that resolve are
/// the user's own files under `~/.config/solium/qml/decorations/` and paths.
/// The path stays because those files exist on people's machines: one QML file
/// is still a decoration, and deleting the eight that shipped is not a reason
/// to stop reading anyone else's.
///
/// ```sh
/// SOLIUM_PANE=mine                # ~/.config/solium/qml/decorations/mine.qml
/// SOLIUM_PANE=~/mine.qml          # anywhere
/// ```
fn qml_path(style: Option<&str>) -> Option<PathBuf> {
    // The old name still works: it was a path to a titlebar, and it is a path
    // to a decoration now.
    if let Some(path) = std::env::var_os("SOLIUM_QML_TITLEBAR") {
        return Some(PathBuf::from(path));
    }
    let name = chosen(style)?;
    // A path is taken as given, existing or not, exactly as `style::resolve`
    // takes one: the caller typed one location, and an error about that
    // location is the most useful thing to say about it.
    if name.contains('/') || name.ends_with(".qml") {
        return Some(PathBuf::from(shellexpand(&name)));
    }
    // A bare name is a search, and a search that finds nothing answers `None`
    // rather than a path it made up. `build` is what turns that into a refusal
    // naming the name and both halves of the lookup.
    let file = format!("{name}.qml");
    decoration_directories()
        .into_iter()
        .map(|dir| dir.join(&file))
        .find(|candidate| candidate.is_file())
}

/// A list of directories, for an error that has to say where it went.
fn display_all(places: &[PathBuf]) -> String {
    places
        .iter()
        .map(|dir| dir.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The directories a single-file decoration is looked for in, nearest first.
///
/// A file of the same name in the user's own directory shadows the one that
/// ships, so `pane = "top"` can mean the user's idea of a top bar -- and since
/// Task 7 ships nothing here, that is the only way a name resolves to a file at
/// all. The same shape as [`crate::style::directories`] and for the same
/// reason: the lookup above and the listing in [`catalogue`] walk one list, so
/// what the panel offers is what a press resolves.
///
/// **[`shipped_decorations`] stays in the list although this build puts nothing
/// in it**, and is a directory that is not in the tree. It is kept so that the
/// *kinds* of place stay matched: [`places`] is built from this function, and
/// [`ships`] names [`shipped_decorations`] itself -- so dropping the shipped
/// half here would leave `ships` walking a kind of place `places` does not,
/// and the listing and the lookup would disagree about what a file style even
/// is. `catalogue` skips a directory it cannot read, so the cost is one path
/// in one error message saying where this looked -- which is true, and reads
/// better than an empty list.
///
/// Note what this does **not** claim: `ships` builds its own two-element list
/// rather than calling this, because it needs the shipped halves alone and
/// this function deliberately leads with the user's. So the two are matched by
/// review and by `what_the_panel_offers_is_what_a_press_resolves`, not by
/// construction -- weaker than [`crate::style::directories`]'s arrangement,
/// and said out loud rather than implied.
fn decoration_directories() -> Vec<PathBuf> {
    let mut places: Vec<PathBuf> = qml::user_qml_dir()
        .map(|dir| dir.join("decorations"))
        .into_iter()
        .collect();
    places.push(shipped_decorations());
    places
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

/// Where a single-file decoration would ship, if one did.
///
/// **Empty since Task 7, and not present in the tree.** The eight that lived
/// here are bundles under `panes/` now. Kept as the bottom of the file lookup
/// for the reason [`decoration_directories`] gives.
pub(crate) fn shipped_decorations() -> PathBuf {
    crate::assets::qml().join("decorations")
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

    use crate::style::Bleed;

    /// Everything Qt-touching in this file's tests goes through this. It moved
    /// to `qml` when `style` needed it too — the rule it encodes is the QML
    /// engine's thread affinity, which is not a fact about decorations.
    use crate::qml::qt_test::on_the_qt_thread;

    /// A bundle written into a directory of its own, as `style`'s tests do.
    ///
    /// Cleared first, because these are named after the test and the process
    /// and a second run of the same test in the same binary would otherwise
    /// find its own leftovers.
    fn fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("solium-layers-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        for (file, body) in files {
            std::fs::write(dir.join(file), body).expect("writing a bundle file");
        }
        dir
    }

    /// Three inline layers, one flat opaque colour each, at all three depths.
    ///
    /// Inline on purpose: a delegated layer is a file of its own and would have
    /// separate scenes whether or not anything hid anything, so it cannot tell
    /// the two failures apart. These three share one `Pane.qml`.
    const THREE_COLOURS: &str = r##"
        import QtQuick
        import Solium

        PaneStyle {
            Layer {
                depth: "behind"; name: "under"
                Rectangle { anchors.fill: parent; color: "#ff0000" }
            }
            Layer {
                depth: "frame"; name: "bar"
                Rectangle { anchors.fill: parent; color: "#00ff00" }
            }
            Layer {
                depth: "above"; name: "over"
                Rectangle { anchors.fill: parent; color: "#0000ff" }
            }
        }
    "##;

    /// The pixel in the middle of a rendered layer, as `(r, g, b, a)`.
    ///
    /// Premultiplied ARGB32 little-endian, which is `B G R A` in memory — the
    /// format `qml/host.cpp` fills its `QImage` with and the one the compositor
    /// blends, so this reads the same bytes the screen would get.
    fn middle(layer: &mut LayerScene, size: (i32, i32)) -> (u8, u8, u8, u8) {
        let rendered = layer.scene.render().expect("the layer renders");
        let x = usize::try_from(size.0 / 2).unwrap_or(0) * 4;
        let y = usize::try_from(size.1 / 2).unwrap_or(0);
        let at = y * rendered.stride + x;
        let pixel = rendered
            .pixels
            .get(at..at + 4)
            .expect("a pixel in the middle of the layer");
        (pixel[2], pixel[1], pixel[0], pixel[3])
    }

    /// **Each layer is its own picture, and it holds only its own layer.**
    ///
    /// The claim the whole feature rests on, taken as far as a test without a
    /// GPU can take it: three layers written into one `Pane.qml`, built through
    /// the real `style::load` and the real `Decoration::from_style`, rasterised
    /// by the real Qt, and read back as pixels.
    ///
    /// It separates the two ways `PaneStyle.showOneLayer` can be wrong, and both
    /// have been run as controls against this test:
    ///
    /// * **Hidden but never parented** — the shape the plan's `visible:` binding
    ///   would have left, since items in a `list<Item>` are not in the scene
    ///   graph at all. Measured: `[(0,0,0,0), (0,0,0,0), (0,0,0,0)]`, three
    ///   transparent pictures.
    /// * **Parented but never hidden** — every scene carries all three layers.
    ///   Measured: the readback above reads `[true, true, true]` for the scene
    ///   built for layer 0, which is a client sandwiched between two copies of
    ///   one picture.
    ///
    /// Red, green and blue, one per scene, is the only reading that is neither,
    /// which is why the colours are flat and opaque rather than anything
    /// prettier.
    #[test]
    fn every_layer_is_its_own_scene_and_draws_only_itself() {
        on_the_qt_thread(|| {
            let dir = fixture("colours", &[("Pane.qml", THREE_COLOURS)]);
            let style = crate::style::load(&dir).expect("the fixture loads");
            let size = (40, 30);
            let mut decoration =
                Decoration::from_style(&style, size.0, size.1).expect("three scenes");

            assert_eq!(decoration.layers.len(), 3, "one scene per declared layer");
            let depths: Vec<Depth> = decoration.layers.iter().map(|it| it.depth).collect();
            assert_eq!(
                depths,
                [Depth::Behind, Depth::Frame, Depth::Above],
                "in declaration order, which is not stacking order"
            );

            // Which layer each scene believes it is drawing, straight out of
            // the object tree. Three separate `PaneStyle` instances, each with
            // exactly one of its three children shown.
            for (index, layer) in decoration.layers.iter().enumerate() {
                let shown: Vec<Option<String>> = (0..3)
                    .map(|which| layer.scene.layer_field(which, "visible"))
                    .collect();
                let expected: Vec<Option<String>> = (0..3)
                    .map(|which| Some((which == index).to_string()))
                    .collect();
                assert_eq!(
                    shown, expected,
                    "the scene built for layer {index} shows the wrong children"
                );
            }

            if crate::qml::on_gpu() {
                // A GPU scene is built at 1x1 and rendered into a dmabuf; there
                // is no `QImage` to read a pixel out of and nothing here can
                // bind a texture. The readback above still holds on both paths,
                // and `dev/wirecheck` is where the GPU pictures are checked.
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }

            let colours: Vec<(u8, u8, u8, u8)> = decoration
                .layers
                .iter_mut()
                .map(|layer| middle(layer, size))
                .collect();
            assert_eq!(
                colours,
                [(255, 0, 0, 255), (0, 255, 0, 255), (0, 0, 255, 255)],
                "each layer's scene must hold that layer and nothing else -- all \
                 transparent means nothing was parented, and three blues mean \
                 every scene drew all three of them"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// A style whose `above` layer reaches 40px past the top of the pane.
    ///
    /// Three bands of flat opaque colour, and the green one is the control
    /// built into the fixture: it is drawn *above the canvas*, so a picture
    /// containing any of it is a layer that was given more room than it asked
    /// for. Inline rather than delegated, so this also exercises `bleedTop` and
    /// `paneHeight` reaching an inline layer through `PaneStyle`.
    const BLEEDING: &str = r##"
        import QtQuick
        import Solium

        PaneStyle {
            id: pane

            insets.top: 32

            Layer {
                depth: "above"; name: "spikes"; bleed: { "top": 40 }

                // The strip past the pane, which is the whole point.
                Rectangle {
                    x: 0; y: 0; width: parent ? parent.width : 0
                    height: pane.bleedTop
                    color: "#ff0000"
                }
                // The pane's own rectangle, found inside the canvas.
                Rectangle {
                    x: 0; y: pane.bleedTop; width: parent ? parent.width : 0
                    height: pane.paneHeight
                    color: "#0000ff"
                }
                // Above the canvas entirely, and **declared last** so it would
                // draw over both of the others: any green at all is a canvas
                // larger than this layer asked for.
                Rectangle {
                    x: 0; y: -60; width: parent ? parent.width : 0; height: 60
                    color: "#00ff00"
                }
            }
        }
    "##;

    /// One column of a rendered layer, top to bottom, as `(r, g, b, a)`.
    ///
    /// The length is the **buffer's own height**, worked out from the bytes
    /// rather than from anything that was asked for, so a test can measure how
    /// tall a canvas really came out. Premultiplied ARGB32 little-endian, as
    /// [`middle`] explains.
    fn column(layer: &mut LayerScene, x: i32) -> Vec<(u8, u8, u8, u8)> {
        let rendered = layer.scene.render().expect("the layer renders");
        let stride = rendered.stride.max(1);
        let at = usize::try_from(x.max(0)).unwrap_or(0) * 4;
        (0..rendered.pixels.len() / stride)
            .map(|row| {
                let from = row * stride + at;
                let pixel = rendered
                    .pixels
                    .get(from..from + 4)
                    .expect("a pixel in the layer");
                (pixel[2], pixel[1], pixel[0], pixel[3])
            })
            .collect()
    }

    /// How many rows of each colour a column has, in order.
    fn runs(column: &[(u8, u8, u8, u8)]) -> Vec<((u8, u8, u8, u8), usize)> {
        let mut runs: Vec<((u8, u8, u8, u8), usize)> = Vec::new();
        for pixel in column {
            match runs.last_mut() {
                Some((colour, count)) if colour == pixel => *count += 1,
                _ => runs.push((*pixel, 1)),
            }
        }
        runs
    }

    /// **A layer paints outside its pane, and stops exactly where it said.**
    ///
    /// The claim the whole project was started for, read as pixels. The
    /// compositor cannot be started here — there is no free VT — so the
    /// screenshot the brief asks for is not available; what stands in its place
    /// is the layer's own buffer, which is the thing a screenshot would be a
    /// photograph of. Everything up to the upload is real: the manifest, the
    /// bleed parsed out of it, `Decoration::from_style`, the canvas the scene
    /// was built at, `tell`, and Qt's own rasteriser.
    ///
    /// A 60x88 client under a 32px inset is a 60x120 pane. The `above` layer
    /// declares 40px of bleed at the top, so its canvas is 60x160 and reads,
    /// top to bottom: 40 rows of red that are **not over the window at all**,
    /// then 120 rows of blue that are. Nothing else fits in 160 rows, which is
    /// what makes the run lengths the assertion.
    ///
    /// **The clip is structural rather than checked**, and that is worth being
    /// exact about because it changes what a control can show. A layer is
    /// rasterised into a buffer that *is* its canvas, so content reaching past
    /// it is not rejected — there is nowhere for it to be drawn. The green band
    /// is 60px above the canvas and is absent from a picture holding every
    /// other band; it is declared last, so if the canvas ever were larger than
    /// the layer asked for it would paint over both of the others rather than
    /// hiding underneath them.
    ///
    /// Three controls, all run:
    ///
    /// | control | measured |
    /// |---|---|
    /// | `canvas` returning `outer` — the state before this task | 120 rows against 160: no strip at all |
    /// | the canvas grown by twice the declared bleed, with QML still told 40 | 200 rows against 160 |
    /// | the green band moved onto the canvas, at `y: 0` | 60 green rows then 100 blue: the run list does see green |
    ///
    /// The third is a control on the *reader* rather than on the code — it is
    /// what says the second assertion could fail at all.
    #[test]
    fn a_layer_paints_past_its_pane_and_stops_where_it_promised() {
        on_the_qt_thread(|| {
            let dir = fixture("bleeding", &[("Pane.qml", BLEEDING)]);
            let style = crate::style::load(&dir).expect("the fixture loads");
            assert_eq!(
                style.layers[0].bleed,
                Bleed {
                    top: 40,
                    right: 0,
                    bottom: 0,
                    left: 0
                },
                "per-side, so this layer pays for one side and not four"
            );

            let mut decoration = Decoration::from_style(&style, 60, 88).expect("one scene");
            // What `layer_elements` does before anything renders, which is
            // where `paneHeight` comes from. The pane, not the client and not
            // the canvas: 88 plus the 32 the style reserved.
            decoration.tell(
                &Look {
                    title: "",
                    focused: false,
                    pointer_inside: false,
                },
                60,
                120,
            );

            if crate::qml::on_gpu() {
                // A GPU scene is built at 1x1 into a dmabuf; there is no
                // `QImage` to read and nothing here can bind a texture. The
                // geometry above this is path-independent, and `dev/wirecheck`
                // is where GPU pictures are checked.
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }

            let layer = decoration.layers.first_mut().expect("the one layer");
            let column = column(layer, 2);

            assert_eq!(
                column.len(),
                160,
                "the canvas is the pane grown by the bleed: 120 rows of window and \
                 40 rows above it. 120 means nothing grew and the layer is still \
                 confined to its pane"
            );
            assert_eq!(
                runs(&column),
                vec![((255, 0, 0, 255), 40), ((0, 0, 255, 255), 120)],
                "40 rows of red *above the window* and then the window's own 120 -- \
                 the strip is the layer painting where the pane is not, and the \
                 boundary at exactly row 40 is it stopping where it said it would. \
                 Any green at all is the band drawn above the canvas having been \
                 given room it never asked for, which is bleed as a request rather \
                 than a promise"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **A bleeding layer has its whole canvas copied, not its bands.**
    ///
    /// The bands are strips of the *buffer* measured from its corners, and a
    /// bleeding layer's buffer no longer starts at the pane's corner. Copying
    /// them would upload a strip of empty canvas above the titlebar and never
    /// copy the titlebar itself — the one failure mode that shows a style's
    /// bleed correctly and drops the part of it that was there before.
    ///
    /// Two layers in one fixture rather than two fixtures, so the only
    /// difference between them is the property under test: same depth, same
    /// insets, same content.
    #[test]
    fn bleeding_is_enough_to_lose_the_band_optimisation() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "bands",
                &[(
                    "Pane.qml",
                    r#"
                    import QtQuick
                    import Solium

                    PaneStyle {
                        insets.top: 32
                        Layer { depth: "frame"; name: "plain" }
                        Layer { depth: "frame"; name: "reaching"; bleed: 12 }
                    }
                    "#,
                )],
            );
            let style = crate::style::load(&dir).expect("the fixture loads");
            let decoration = Decoration::from_style(&style, 60, 88).expect("two scenes");

            assert!(
                !decoration.layers[0].overlay,
                "a `frame` layer inside real insets that says nothing is still \
                 copied band by band -- if this is false the optimisation is gone \
                 for every window, not only the bleeding ones"
            );
            assert!(
                decoration.layers[1].overlay,
                "and the same layer with a bleed is not: its bands are measured \
                 from a corner 12px outside the pane, so the top band would be \
                 empty canvas and the titlebar would never be copied"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **The pointer arrives in each layer's own canvas, and a leave stays
    /// outside it.**
    ///
    /// `x` and `y` come from `Solium::decorated_under` and `chrome_under`, both
    /// of which measure from the pane's outer corner. A bleeding layer's scene
    /// no longer starts there, so without the shift every button in it would be
    /// `bleedLeft` to the right of where the pointer really was — the close
    /// button lighting up while the pointer is over the maximise one, which is
    /// a decoration that works and is one button out.
    ///
    /// The leave is the other half and points the other way, which is why both
    /// are in one test: `(-1, -1)` shifted by a 30x40 bleed is `(29, 39)`, a
    /// point comfortably inside a 90x160 canvas, so a border lit by proximity
    /// would light on the frame the pointer left the window and stay lit for
    /// the rest of the session.
    ///
    /// Both controls run:
    ///
    /// | control | measured |
    /// |---|---|
    /// | `pointer` not shifted | the scene reads the pointer at `(5, 7)`, 30 and 40 short |
    /// | `pointer_left` shifted the same way `pointer` is | still hovered after the pointer left |
    ///
    /// The *clip* is elsewhere and was already there: both callers gate on
    /// `drawn.rect.contains(location)`, the pane's own outer rect, so a spike
    /// over a neighbour never reaches this function at all. That is now stated
    /// as well as true — `state::tests::a_point_in_the_bleed_is_not_in_the_pane`.
    #[test]
    fn the_pointer_arrives_in_the_layers_own_canvas() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "pointer",
                &[(
                    "Pane.qml",
                    r#"
                    import QtQuick
                    import Solium

                    PaneStyle {
                        id: pane

                        insets.top: 32
                        // Where the scene thinks the pointer is, in its own
                        // coordinates -- which are the canvas's.
                        property int atX: Math.round(hit.mouseX)
                        property int atY: Math.round(hit.mouseY)
                        onButton: hit.containsMouse

                        Layer {
                            depth: "frame"; name: "buttons"
                            bleed: { "left": 30, "top": 40 }

                            MouseArea {
                                id: hit
                                anchors.fill: parent
                                hoverEnabled: true
                            }
                        }
                    }
                    "#,
                )],
            );
            let style = crate::style::load(&dir).expect("the fixture loads");
            let mut decoration = Decoration::from_style(&style, 60, 88).expect("one scene");

            // 5 across and 7 down from the **pane's** top-left corner, which is
            // what both input paths hand over.
            decoration.pointer(5.0, 7.0, None);
            let scene = &mut decoration.layers[0].scene;
            assert_eq!(
                (scene.get_int("atX"), scene.get_int("atY")),
                (35, 47),
                "the scene is laid out on a canvas that starts 30 left and 40 \
                 above the window, so a point 5 across and 7 down from the \
                 window is 35 and 47 in the picture -- (5, 7) is every button \
                 in this layer being 30 pixels to the right of where it is"
            );
            assert!(
                decoration.on_button(),
                "and it really is over the area, not merely reporting numbers"
            );

            decoration.pointer_left();
            assert!(
                !decoration.on_button(),
                "a leave is sent raw: (-1, -1) shifted by this layer's bleed is \
                 (29, 39), which is inside its canvas, so the window would stay \
                 hovered for the rest of its life"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// Two layers, each with a button under the same point, one action apiece.
    ///
    /// `Decoration::pointer` hands the press to **every** layer, so a style
    /// whose layers overlap produces more than one answer to "what did the user
    /// press"; this fixture is the smallest thing that does. The layers are
    /// declared bottom-first, which is the order a style is written in and the
    /// order the old fold read, so the two orderings disagree and the returned
    /// action says which one ran.
    ///
    /// `depth` is left to the caller: the same two layers at `behind`/`above`
    /// exercise [`crate::render::PANE_ORDER`], and both at `frame` exercise
    /// [`at`]'s within-depth reversal. Nothing else differs between the two
    /// cases, which is what makes each of them about one ordering.
    fn contested(lower: &str, upper: &str) -> String {
        format!(
            r#"
            import QtQuick
            import Solium

            PaneStyle {{
                id: pane

                // Declared first: the bottom of whichever pair this is, and
                // the layer a fold over declaration order would believe.
                Layer {{
                    depth: "{lower}"; name: "lower"
                    MouseArea {{ anchors.fill: parent; onPressed: pane.action = "close" }}
                }}
                // Declared second: drawn on top, so this is the one the user
                // is looking at and the one whose press must count.
                Layer {{
                    depth: "{upper}"; name: "upper"
                    MouseArea {{ anchors.fill: parent; onPressed: pane.action = "maximize" }}
                }}
            }}
            "#
        )
    }

    /// **A press two layers both claim belongs to the one on top.**
    ///
    /// Layers made this reachable and nothing enforced it: `take_action` folded
    /// `action.or(asked)` over `self.layers`, which is declaration order, and
    /// declaration order is bottom-to-top. So the *lowest* layer won a contested
    /// press — the opposite of what is on screen, and invisible until now
    /// because a style with one layer has both orders at once.
    ///
    /// Across depths here, so what is being read is `PANE_ORDER`: `above` comes
    /// before `behind` in the list the compositor draws from, and input
    /// precedence is that list read from the front.
    ///
    /// | control | measured |
    /// |---|---|
    /// | the old `action.or(asked)` fold over `self.layers` | `Some(Close)`: the `behind` layer wins |
    #[test]
    fn a_contested_press_belongs_to_the_topmost_layer() {
        on_the_qt_thread(|| {
            let dir = fixture("contested", &[("Pane.qml", &contested("behind", "above"))]);
            let style = crate::style::load(&dir).expect("the fixture loads");
            let mut decoration = Decoration::from_style(&style, 60, 88).expect("two scenes");

            decoration.pointer(20.0, 20.0, Some(true));
            assert_eq!(
                decoration.take_action(),
                Some(Action::ToggleMaximize),
                "both layers claimed the press, and the `above` one is the one \
                 drawn over the other -- `Close` here is the press being given \
                 to the layer underneath, which is `PANE_ORDER` read backwards"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **And within one depth, the later-declared layer is the one on top.**
    ///
    /// The same claim one level down. `PANE_ORDER` puts these two in the same
    /// piece, so it cannot separate them; [`at`] can, and already does for
    /// drawing — `one_depth_is_walked_backwards_and_the_others_are_left_alone`
    /// pins the same reversal for the element list. Reused rather than
    /// re-derived here, because a second reversal written beside the first is a
    /// second thing to keep in step.
    ///
    /// | control | measured |
    /// |---|---|
    /// | the old fold, which reads one depth forwards | `Some(Close)`: the earlier sibling wins |
    #[test]
    fn within_one_depth_the_press_belongs_to_the_later_layer() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "contested-depth",
                &[("Pane.qml", &contested("frame", "frame"))],
            );
            let style = crate::style::load(&dir).expect("the fixture loads");
            let mut decoration = Decoration::from_style(&style, 60, 88).expect("two scenes");

            decoration.pointer(20.0, 20.0, Some(true));
            assert_eq!(
                decoration.take_action(),
                Some(Action::ToggleMaximize),
                "QML's own rule for siblings is that the later one draws on \
                 top, so the later one is pressed -- `Close` is the within-depth \
                 walk having lost its reversal"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **The layer that lost still has its action taken.**
    ///
    /// The one thing an obvious tidy-up of `take_action` would break: stopping
    /// at the first layer that answered looks like the same function and is
    /// not. `take_string` is what *clears* `action`, so a layer left unasked
    /// keeps the press it lost — and fires it on whichever later frame anything
    /// else happens to ask. Pressing a button on the top layer of this style
    /// would close the window some seconds later, on an unrelated click.
    ///
    /// Measured as a **second** `take_action` with no press in between: nothing
    /// happened, so nothing may be reported.
    ///
    /// Both controls run, and they separate this test from the two above it —
    /// the tidy-up leaves *precedence* perfectly correct, so only this one
    /// catches it:
    ///
    /// | control | measured |
    /// |---|---|
    /// | `.take(1)` on the walk, and skip later depths once one answered | second call returns `Some(Close)`: the stale press |
    /// | the old `action.or(asked)` fold | first call returns `Some(Close)`: the fixture really is contested |
    #[test]
    fn a_layer_that_lost_the_press_still_has_its_action_taken() {
        on_the_qt_thread(|| {
            let dir = fixture("stale", &[("Pane.qml", &contested("behind", "above"))]);
            let style = crate::style::load(&dir).expect("the fixture loads");
            let mut decoration = Decoration::from_style(&style, 60, 88).expect("two scenes");

            decoration.pointer(20.0, 20.0, Some(true));
            assert_eq!(
                decoration.take_action(),
                Some(Action::ToggleMaximize),
                "the press itself, so this test fails loudly rather than \
                 vacuously if the fixture ever stops pressing anything"
            );
            assert_eq!(
                decoration.take_action(),
                None,
                "nothing has been pressed since, so nothing may be reported. \
                 `Close` here is the losing layer having kept its action -- a \
                 window that closes itself on some later, unrelated click"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// What bounds a whole pane on screen: the union, not the sum.
    ///
    /// Read by the off-screen cull in `render::elements`, which is the one
    /// place a pane's own rectangle stood between a layer and the monitor. Two
    /// layers reaching 24px upward reach 24px upward.
    #[test]
    fn a_decorations_reach_is_the_widest_of_its_layers_and_not_their_total() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "reach",
                &[(
                    "Pane.qml",
                    r#"
                    import QtQuick
                    import Solium

                    PaneStyle {
                        insets.top: 32
                        Layer { depth: "behind"; name: "glow";   bleed: 24 }
                        Layer { depth: "above";  name: "spikes"; bleed: { "top": 48, "left": 8 } }
                    }
                    "#,
                )],
            );
            let style = crate::style::load(&dir).expect("the fixture loads");
            let decoration = Decoration::from_style(&style, 60, 88).expect("two scenes");
            assert_eq!(
                decoration.bleed(),
                Bleed {
                    top: 48,
                    right: 24,
                    bottom: 24,
                    left: 24
                },
                "72 on top would be two layers' reach added together, which is \
                 not a distance anything paints at"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// The layers of one depth, newest on top, and the others left out.
    ///
    /// `layer_elements` and `layers_at` share one selection — [`at`] — so this
    /// pins the depth filter and the within-depth order for both. Later
    /// declared draws on top, which is QML's own rule for siblings, and the
    /// element list is topmost first.
    #[test]
    fn one_depth_is_walked_backwards_and_the_others_are_left_alone() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "two-at-one-depth",
                &[(
                    "Pane.qml",
                    r#"
                    import QtQuick
                    import Solium

                    PaneStyle {
                        Layer { depth: "frame";  name: "under-bar" }
                        Layer { depth: "above";  name: "over" }
                        Layer { depth: "frame";  name: "over-bar" }
                    }
                    "#,
                )],
            );
            let style = crate::style::load(&dir).expect("the fixture loads");
            let decoration = Decoration::from_style(&style, 40, 30).expect("three scenes");

            let names = |depth| decoration.layers_at(depth).collect::<Vec<_>>();
            assert_eq!(
                names(Depth::Frame),
                ["over-bar", "under-bar"],
                "declared second at this depth, so drawn on top, so first in a \
                 list that is topmost first"
            );
            assert_eq!(names(Depth::Above), ["over"]);
            assert!(names(Depth::Behind).is_empty());

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **The old spelling of the per-run override still names a style.**
    ///
    /// `SOLIUM_DECORATION` was the knob until Task 7 renamed it, and it is in
    /// people's shell history and in this repository's own older plan
    /// documents. An alias that silently stopped working would put the
    /// *configured* style on screen instead of the one that was asked for,
    /// which looks exactly like the variable being ignored -- and is.
    ///
    /// Through `named_by` rather than `chosen`, because a test that exported
    /// either name would decide the answer for every other test in this binary.
    /// All of the combinations, including the one that says which wins.
    #[test]
    fn the_old_environment_name_still_names_a_style() {
        let pane = || Some("bundled".to_owned());
        let old = || Some("legacy".to_owned());

        assert_eq!(named_by(pane(), None, None).as_deref(), Some("bundled"));
        assert_eq!(
            named_by(None, old(), None).as_deref(),
            Some("legacy"),
            "the old name on its own is honoured"
        );
        assert_eq!(
            named_by(pane(), old(), None).as_deref(),
            Some("bundled"),
            "and loses to the new one, which is the more deliberate spelling"
        );

        // Either of them still beats the configuration, which is the rule the
        // rename must not have changed: the variable is set by whoever started
        // this run, and that is the more specific intent.
        assert_eq!(
            named_by(None, old(), Some("configured")).as_deref(),
            Some("legacy")
        );
        assert_eq!(
            named_by(None, None, Some("configured")).as_deref(),
            Some("configured"),
            "and with neither set, the configuration decides"
        );
        assert_eq!(named_by(None, None, None), None);
    }

    /// **A delegated layer is told what the style reserved.**
    ///
    /// The property that made converting the shipped eight possible at all.
    /// Seven of them painted themselves from their own `insetTop` --
    /// `height: frame.insetTop` -- and a delegated layer is its own scene with
    /// no parent to read the manifest off, so without this the bundle version
    /// of `top` would declare 32 in `Pane.qml` and draw a bar of height zero.
    /// The alternative was the same literal in both files with a comment asking
    /// that they be kept in step, which is the two-answers-to-one-question that
    /// put `insets` on `PaneStyle` rather than on `Layer`.
    ///
    /// Read back through a *derived* property rather than through `insetTop`
    /// itself. `set_int` followed by `get_int` on the same name would pass
    /// against a QML root that never declared the property at all -- Qt stores
    /// it as a dynamic one and hands it straight back -- so what is asserted is
    /// that a **binding** saw the value, which only happens if it landed on a
    /// property the file declared.
    ///
    /// Four different numbers, so a pair read in the wrong order is not four
    /// correct answers. And `insetRight` is declared by this fixture and by
    /// none of the eight, which is the case that says the compositor writes all
    /// four rather than the ones something happened to need.
    #[test]
    fn a_delegated_layer_is_told_the_styles_insets() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "told-insets",
                &[
                    (
                        "Pane.qml",
                        r#"
                        import QtQuick
                        import Solium

                        PaneStyle {
                            insets.top: 7
                            insets.right: 11
                            insets.bottom: 13
                            insets.left: 17

                            Layer { depth: "frame"; name: "bar"; source: "Frame.qml" }
                        }
                        "#,
                    ),
                    (
                        "Frame.qml",
                        r#"
                        import QtQuick

                        Item {
                            property int insetTop: 0
                            property int insetRight: 0
                            property int insetBottom: 0
                            property int insetLeft: 0

                            // Bindings, so these stay zero unless the four above
                            // were really set on a property this file declared.
                            readonly property int sawTop: insetTop * 10
                            readonly property int sawRight: insetRight * 10
                            readonly property int sawBottom: insetBottom * 10
                            readonly property int sawLeft: insetLeft * 10
                        }
                        "#,
                    ),
                ],
            );
            let style = crate::style::load(&dir).expect("the fixture loads");
            let mut decoration = Decoration::from_style(&style, 60, 88).expect("one scene");
            let layer = decoration.layers.first_mut().expect("the one layer");

            assert_eq!(layer.scene.get_int("sawTop"), 70, "insets.top reached QML");
            assert_eq!(layer.scene.get_int("sawRight"), 110);
            assert_eq!(layer.scene.get_int("sawBottom"), 130);
            assert_eq!(layer.scene.get_int("sawLeft"), 170);

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// A bundle declaring a client radius and a delegated layer to read it.
    ///
    /// **Every default is non-zero, and that is the whole design of this
    /// fixture.** A `clientRadius` default of 0 makes "the compositor wrote 0"
    /// and "the compositor wrote nothing" the same reading, so the zero case
    /// below -- the one that pins the `map_or` -- could not fail. With 7 as the
    /// default, an unwritten property reads back 71 and a written zero reads
    /// back 1.
    ///
    /// The `+ 1` is there for the same reason one step further in: without it
    /// a written 0 and an unwritten 0 would both be 0 again.
    ///
    /// The four corners take that same shape with a **different default and a
    /// different constant each**, so the readbacks cannot be confused with one
    /// another: a pair of corners written in the wrong order, or one derived
    /// property bound to the wrong source, produces a number no correct run
    /// produces. Sharing one default across the four would make a transposition
    /// invisible in exactly the case that matters -- two corners a style
    /// declared the same.
    fn radius_fixture(name: &str, declared: &str) -> PathBuf {
        fixture(
            name,
            &[
                (
                    "Pane.qml",
                    &format!(
                        r#"
                        import QtQuick
                        import Solium

                        PaneStyle {{
                            {declared}
                            Layer {{ depth: "frame"; name: "bar"; source: "Frame.qml" }}
                        }}
                        "#
                    ),
                ),
                (
                    "Frame.qml",
                    r"
                        import QtQuick

                        Item {
                            property int clientRadius: 7
                            property int clientRadiusTopLeft: 2
                            property int clientRadiusTopRight: 3
                            property int clientRadiusBottomLeft: 4
                            property int clientRadiusBottomRight: 5

                            // Bindings, so these move only if the properties
                            // above were really set on ones this file declared.
                            readonly property int sawRadius: clientRadius * 10 + 1
                            readonly property int sawTopLeft: clientRadiusTopLeft * 100 + 1
                            readonly property int sawTopRight: clientRadiusTopRight * 100 + 2
                            readonly property int sawBottomLeft: clientRadiusBottomLeft * 100 + 3
                            readonly property int sawBottomRight: clientRadiusBottomRight * 100 + 4
                        }
                        ",
                ),
            ],
        )
    }

    /// **A layer is told what the client is being masked to**, so a border can
    /// match the curve instead of squaring it off around it.
    ///
    /// The other half of the seam this plan opens: the compositor rounds the
    /// client with a fragment program because those pixels are the
    /// application's, and QML rounds a layer with `Rectangle.radius` because
    /// those are Qt's. Only the number crosses, and it crosses exactly as
    /// `insets` do -- declared once on `PaneStyle`, written onto every layer.
    ///
    /// Read back through a *derived* property for the reason
    /// `a_delegated_layer_is_told_the_styles_insets` gives: `set_int` followed
    /// by `get_int` on one name is a round trip through the compositor's own
    /// map and passes against a scene that never loaded.
    #[test]
    fn a_layer_is_told_the_clients_radius() {
        on_the_qt_thread(|| {
            let dir = radius_fixture("told-radius", "client.radius: 12");
            let style = crate::style::load(&dir).expect("the fixture loads");
            let mut decoration = Decoration::from_style(&style, 60, 88).expect("one scene");
            let layer = decoration.layers.first_mut().expect("the one layer");

            assert_eq!(
                layer.scene.get_int("sawRadius"),
                121,
                "a declared `client.radius` has to reach a delegated layer, or \
                 a bundle's border squares off the corners the compositor cut"
            );
            // And the four siblings, each carrying the same 12: a bare
            // `client.radius` is every corner, so this is the inheritance half
            // of the seam rather than a second spelling of the line above.
            assert_eq!(layer.scene.get_int("sawTopLeft"), 1201);
            assert_eq!(layer.scene.get_int("sawTopRight"), 1202);
            assert_eq!(layer.scene.get_int("sawBottomLeft"), 1203);
            assert_eq!(layer.scene.get_int("sawBottomRight"), 1204);

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **Each corner reaches a layer by its own name, and `clientRadius` is the
    /// largest of them.**
    ///
    /// The test that can see a transposition: four declared values, all
    /// different, against four defaults that are also all different, so a pair
    /// written in the wrong order produces a number no correct run produces.
    /// `a_layer_is_told_the_clients_radius` cannot -- every corner there is 12,
    /// and so is every permutation of it.
    ///
    /// **`client.radius: 99` is declared and must not appear anywhere.** All
    /// four corners override it, so it is the fallback that is never taken; an
    /// implementation that wrote the style's own `radius` through to
    /// `clientRadius` instead of `largest()` reads back 991 here, and passes
    /// every other test in this file.
    ///
    /// The top-left is a written **zero**, which is the corner a square-topped
    /// style declares and the one a default of 0 could never have shown: it
    /// reads back 1, where an unwritten property reads 201.
    #[test]
    fn a_layer_is_told_each_corner_and_the_largest_of_them() {
        on_the_qt_thread(|| {
            let dir = radius_fixture(
                "told-corners",
                "client.radius: 99
                 client.radiusTopLeft: 0
                 client.radiusTopRight: 6
                 client.radiusBottomLeft: 14
                 client.radiusBottomRight: 8",
            );
            let style = crate::style::load(&dir).expect("the fixture loads");
            let mut decoration = Decoration::from_style(&style, 60, 88).expect("one scene");
            let layer = decoration.layers.first_mut().expect("the one layer");

            assert_eq!(
                layer.scene.get_int("sawTopLeft"),
                1,
                "a square corner has to be written as 0; 201 is the property \
                 left untouched, and a layer would hug a curve nobody cut"
            );
            assert_eq!(layer.scene.get_int("sawTopRight"), 602);
            assert_eq!(layer.scene.get_int("sawBottomLeft"), 1403);
            assert_eq!(layer.scene.get_int("sawBottomRight"), 804);
            assert_eq!(
                layer.scene.get_int("sawRadius"),
                141,
                "the singular is the LARGEST of the four -- its one in-tree use \
                 is `clientRadius + 2`, an outward hug, which has to clear the \
                 biggest cut. 991 means the style's own `client.radius` was \
                 written through, and it is the value no corner took"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// And a style that declares none tells its layers **zero**, rather than
    /// leaving whatever the file defaulted to.
    ///
    /// The control for the test above, and the one that can actually fail: it
    /// is the only assertion here that a hardcoded `set_int("clientRadius",
    /// 12)` would not satisfy, and the only one that separates "the compositor
    /// wrote 0" from "the compositor wrote nothing" -- see `radius_fixture`
    /// for how.
    #[test]
    fn a_layer_of_a_style_with_no_radius_is_told_zero() {
        on_the_qt_thread(|| {
            let dir = radius_fixture("told-no-radius", "");
            let style = crate::style::load(&dir).expect("the fixture loads");
            let mut decoration = Decoration::from_style(&style, 60, 88).expect("one scene");
            let layer = decoration.layers.first_mut().expect("the one layer");

            assert_eq!(
                layer.scene.get_int("sawRadius"),
                1,
                "a style declaring no radius must say so; 71 means the property \
                 was never written and the layer is hugging a curve nobody cut"
            );
            // And all four corners, on the same terms. Each has its own
            // non-zero default, so each of these says separately that the
            // compositor wrote a zero rather than that it wrote nothing --
            // which is the only way to catch three of four being written.
            assert_eq!(layer.scene.get_int("sawTopLeft"), 1, "201 is unwritten");
            assert_eq!(layer.scene.get_int("sawTopRight"), 2, "302 is unwritten");
            assert_eq!(layer.scene.get_int("sawBottomLeft"), 3, "403 is unwritten");
            assert_eq!(layer.scene.get_int("sawBottomRight"), 4, "504 is unwritten");

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **A negative corner reaches a layer as zero, where it means something
    /// on the compositor's side.**
    ///
    /// Reachable, and not by a contrived route: `client.radius: -5` is the
    /// fallback every corner that does not declare itself takes, so declaring
    /// one corner positive leaves the other three negative --
    /// `Corners::is_none` refuses only the case where *all* of them are, so the
    /// effect is real and the pass runs.
    ///
    /// `pass` keeps the sign, because a negative radius inflates the shape
    /// there rather than cutting it. QML has no reading for one:
    /// `Rectangle.radius` is undefined for a negative, and the `clientRadius +
    /// 2` hug would put a border *inside* the client. See `whole`, which is
    /// the one line where the two sides differ and says why.
    #[test]
    fn a_negative_corner_reaches_a_layer_as_zero() {
        on_the_qt_thread(|| {
            let dir = radius_fixture(
                "told-negative",
                "client.radius: -5
                 client.radiusTopRight: 6",
            );
            let style = crate::style::load(&dir).expect("the fixture loads");
            let mut decoration = Decoration::from_style(&style, 60, 88).expect("one scene");
            let layer = decoration.layers.first_mut().expect("the one layer");

            assert_eq!(
                layer.scene.get_int("sawTopLeft"),
                1,
                "a -5 corner is told as 0, not as -5 (which would read -499) \
                 and not left unwritten (which would read 201)"
            );
            assert_eq!(layer.scene.get_int("sawBottomLeft"), 3);
            assert_eq!(layer.scene.get_int("sawBottomRight"), 4);
            assert_eq!(
                layer.scene.get_int("sawTopRight"),
                602,
                "and the one corner that is a radius is untouched by the clamp"
            );
            assert_eq!(
                layer.scene.get_int("sawRadius"),
                61,
                "the singular is the largest, which here is the only positive one"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **A pane carries its style's client effects, because the renderer has
    /// no other way to reach them.**
    ///
    /// Nothing keeps a `Style` once `from_style` has read it, so the pane's
    /// decoration is where `render::prepare` asks whether this window needs a
    /// pass. Asserted through `pass::needs_pass` as well as against the list,
    /// because the list being right and the question being asked of it are two
    /// separate things and only the second one draws anything.
    #[test]
    fn a_decoration_carries_the_styles_client_effects() {
        on_the_qt_thread(|| {
            let dir = radius_fixture("carried-effects", "client.radius: 12");
            let style = crate::style::load(&dir).expect("the fixture loads");
            let decoration = Decoration::from_style(&style, 60, 88).expect("one scene");

            assert_eq!(
                decoration.effects(),
                [solium_effects::fragment::Effect::rounded(
                    solium_effects::fragment::Corners::all(12.0)
                )],
                "the declared radius has to survive the trip onto the pane"
            );
            assert_eq!(
                crate::pass::needs_pass(decoration.effects()),
                Some(solium_effects::fragment::Effect::rounded(
                    solium_effects::fragment::Corners::all(12.0)
                )),
                "and be recognised as wanting a pass, which is what runs one"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **And a style that declares nothing costs nothing**, which is the rule
    /// this whole plan is written around: every window on a machine nobody has
    /// styled is this window, and it must be drawn by exactly the path it was
    /// drawn by before any of this existed -- no capture, no bind, no program,
    /// no extra element.
    ///
    /// `needs_pass` answering `None` is the whole of what keeps it there, so
    /// that is what is asserted rather than the empty list alone: a list that
    /// is empty and a question that is never asked of it look identical from
    /// here and are not.
    #[test]
    fn a_decoration_with_no_declared_radius_runs_no_pass() {
        on_the_qt_thread(|| {
            let dir = radius_fixture("carried-nothing", "");
            let style = crate::style::load(&dir).expect("the fixture loads");
            let decoration = Decoration::from_style(&style, 60, 88).expect("one scene");

            assert!(decoration.effects().is_empty());
            assert_eq!(
                crate::pass::needs_pass(decoration.effects()),
                None,
                "an unstyled window must not buy an offscreen pass per frame"
            );

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// **A name that is nowhere names the name, not a path nobody asked for.**
    ///
    /// `qml_path` used to fabricate `<shipped>/decorations/<name>.qml` for a
    /// name it could not find and let Qt fail on opening it. After Task 7 that
    /// is a file in a directory that is not in the tree, so `pane = "topp"`
    /// reported a missing `topp.qml` under `qml/decorations` -- a guess, wrong
    /// in both directions, and with no mention of the `qml/panes` the answer is
    /// actually in.
    ///
    /// `style::resolve` had already answered the same question the other way in
    /// this same commit, and two answers to one question is what the rest of it
    /// is organised against. So the refusal now comes from `build`, and says
    /// the three things worth saying: the name, that a style is a folder with a
    /// `Pane.qml`, and every directory both halves of the lookup went to.
    #[test]
    fn a_name_that_is_nowhere_is_refused_by_name() {
        if the_environment_has_already_chosen() {
            // `build` reads `SOLIUM_PANE` before the argument.
            return;
        }
        on_the_qt_thread(|| {
            let err = build(Some("no-such-style-ships-here"), 300, 200)
                .expect_err("nothing of that name is anywhere");
            let said = err.to_string();

            // The name the user typed, as they typed it. The old message had
            // only a path built out of it.
            assert!(said.contains("`no-such-style-ships-here`"), "{said}");
            // **Both halves of the lookup.** This is the assertion the old
            // behaviour could not have passed: a failure raised by Qt opening a
            // fabricated `<shipped>/decorations/<name>.qml` names that one path
            // and nothing else, so `qml/panes` -- where the answer actually is
            // -- never appeared. Naming `<name>.qml` as a thing that was looked
            // *for* is fine and is kept; naming it as the thing that failed to
            // open was the defect.
            assert!(said.contains("qml/panes"), "{said}");
            assert!(said.contains("qml/decorations"), "{said}");
            // And what a style *is*, so the message is actionable on its own.
            assert!(said.contains("Pane.qml"), "{said}");
        });
    }

    /// **A name that is a folder under `panes/` reaches its layers.**
    ///
    /// The one link between the setting and everything above: `build` is what
    /// `insert` and `set_style` both call, and without this test `from_style`
    /// is reachable from tests and from nowhere else.
    ///
    /// Two shapes of bundle, because the useful claim is that one entry point
    /// reaches both. `example` declares three layers at three depths; `top`
    /// declares one at `frame`, which is what a decoration has always been and
    /// is the shape all eight shipped styles were converted into.
    ///
    /// `top` reserving `TITLEBAR_HEIGHT` is the assertion that survived the
    /// conversion unchanged while the thing behind it moved: the number used to
    /// be read out of `decorations/top.qml`'s own `insetTop` and now comes from
    /// `panes/top/Pane.qml`'s `insets.top`. Same 32, read from somewhere else,
    /// which is precisely what "a move of files rather than a change of
    /// setting" means and what the frame captures were compared against.
    #[test]
    fn a_name_that_is_a_bundle_builds_its_layers() {
        if the_environment_has_already_chosen() {
            // `build` reads `SOLIUM_PANE` before the argument, so a session
            // that set it decides this rather than the test does.
            return;
        }
        on_the_qt_thread(|| {
            let plain = build(Some("top"), 300, 200).expect("the shipped default builds");
            assert_eq!(plain.layers.len(), 1, "a titlebar is one layer");
            assert_eq!(plain.layers[0].depth, Depth::Frame);
            assert_eq!(
                plain.insets().top,
                TITLEBAR_HEIGHT,
                "and it reserves what it always reserved"
            );

            // Nothing named at all is the same style. Reached through `build`
            // rather than asserted about `DEFAULT_STYLE`, because what is worth
            // pinning is that the default resolves to something that *exists*:
            // it used to name `decorations/top.qml`, which this task deleted.
            let fallback = build(None, 300, 200).expect("the default builds");
            assert_eq!(fallback.insets().top, TITLEBAR_HEIGHT);

            let bundle = build(Some("example"), 300, 200).expect("the shipped example builds");
            // Guarded for the reason `style`'s `find_reaches_the_shipped_example`
            // is: a user bundle called `example` is entitled to win, that is the
            // feature, and then the shipped file's three layers are the wrong
            // assertion.
            let shipped = crate::style::find("example")
                == Some(PathBuf::from(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/qml/panes/example"
                )));
            if shipped {
                let depths: Vec<Depth> = bundle.layers.iter().map(|it| it.depth).collect();
                assert_eq!(depths, [Depth::Behind, Depth::Frame, Depth::Above]);
            } else {
                assert!(!bundle.layers.is_empty(), "a user bundle is still a bundle");
            }
        });
    }

    /// The canvas is the pane grown by the bleed, and the pane's own corner
    /// moves within it — which is why a layer is told `bleedLeft` and
    /// `bleedTop`: `anchors.fill: parent` covers the canvas, and QML needs a
    /// known origin to position the window's own corner against.
    #[test]
    fn a_canvas_is_the_pane_grown_by_its_bleed() {
        let outer = Rectangle::<i32, Logical>::new((100, 200).into(), (800, 600).into());
        let bleed = Bleed {
            top: 40,
            right: 10,
            bottom: 0,
            left: 20,
        };
        let canvas = super::canvas(outer, bleed);
        assert_eq!(canvas.loc.x, 80);
        assert_eq!(canvas.loc.y, 160);
        assert_eq!(canvas.size.w, 830);
        assert_eq!(canvas.size.h, 640);
    }

    #[test]
    fn no_bleed_means_the_canvas_is_the_pane() {
        let outer = Rectangle::<i32, Logical>::new((0, 0).into(), (400, 300).into());
        assert_eq!(super::canvas(outer, Bleed::default()), outer);
    }

    /// **A canvas is capped, because `bleed` is author-controlled.**
    ///
    /// `qml::MAX_SIDE` used to be reachable only by owning an implausible
    /// monitor; a layer's canvas is the first thing in this compositor whose
    /// size comes out of a style file, so `bleed: 20000` is now one typo away
    /// from a 1.6 GB allocation. The GPU path would refuse the buffer and
    /// freeze the scene with one warning; the software path would ask Qt for
    /// the `QImage` and `MemoryRenderBuffer::new` for the rest.
    ///
    /// Both sides are cut in proportion rather than one being sacrificed, so
    /// what a symmetric glow loses it loses symmetrically.
    #[test]
    fn a_canvas_larger_than_a_scene_may_be_is_cut_down_to_one() {
        let outer = Rectangle::<i32, Logical>::new((0, 0).into(), (1000, 800).into());
        let absurd = super::canvas(
            outer,
            Bleed {
                top: 20_000,
                right: 20_000,
                bottom: 20_000,
                left: 20_000,
            },
        );
        assert_eq!(
            (absurd.size.w, absurd.size.h),
            (crate::qml::MAX_SIDE, crate::qml::MAX_SIDE),
            "a canvas is never larger than a scene may be"
        );
        assert_eq!(
            (absurd.loc.x, absurd.loc.y),
            (
                -(crate::qml::MAX_SIDE - 1000) / 2,
                -(crate::qml::MAX_SIDE - 800) / 2
            ),
            "and what is left is split between the two sides that asked for it"
        );

        // The pane's own corner is still inside it, whatever was cut. A canvas
        // that had lost the whole of one side's bleed to rounding would put the
        // window outside the picture drawn around it.
        assert!(absurd.contains_rect(outer));

        // And a pane already too large for a scene keeps the behaviour it has
        // today rather than acquiring a negative canvas.
        let enormous =
            Rectangle::<i32, Logical>::new((0, 0).into(), (crate::qml::MAX_SIDE + 500, 200).into());
        let capped = super::canvas(
            enormous,
            Bleed {
                top: 0,
                right: 40,
                bottom: 0,
                left: 40,
            },
        );
        assert_eq!(capped.size.w, enormous.size.w, "no room, so no bleed");
        assert_eq!(capped.size.h, 200, "and the axis with room is untouched");
    }

    /// One drawing, used by the `spread` tests below.
    ///
    /// A window 800x600 at (100, 200), drawn exactly where it lives at 1x — so
    /// every number that comes out of `spread` is the bleed and nothing else.
    fn at_rest() -> Drawing {
        Drawing {
            rect: crate::present::logical((100.0, 200.0), (800.0, 600.0)),
            outer: (800, 600).into(),
            alpha: 1.0,
            scale: 1.0,
        }
    }

    /// **The element's rectangle is the canvas, which is what makes the damage
    /// the canvas.**
    ///
    /// Rule three. Smithay tracks an element by its geometry and damages what
    /// it vacates when that geometry moves, so a layer drawn at the *pane's*
    /// rectangle would leave the strip its bleed occupied undamaged — an
    /// animating glow smearing across its neighbours while the window it
    /// belongs to repaints perfectly, which is the hardest kind of rendering
    /// bug to attribute because the thing that looks broken is not the thing
    /// that is.
    ///
    /// The control is `spread` returning `drawing.rect` unchanged, which is
    /// what this code did before bleed existed: measured
    /// `(100, 200) 800x600` against the `(100, 152) 848x648` below, failing on
    /// the assertion that names the damage.
    #[test]
    fn a_layer_is_drawn_at_its_canvas_and_not_at_its_pane() {
        let bleed = Bleed {
            top: 48,
            right: 24,
            bottom: 0,
            left: 24,
        };
        let spread = super::spread(at_rest(), bleed);

        assert_eq!(
            spread.canvas,
            Rectangle::<i32, Logical>::new((-24, -48).into(), (848, 648).into()),
            "the canvas is the pane's own space, so its origin is the pane's corner"
        );
        assert!(
            (spread.drawn.loc.x - 76.0).abs() < 1e-9 && (spread.drawn.loc.y - 152.0).abs() < 1e-9,
            "the element starts where the canvas does -- 24 left and 48 above the \
             window -- and not where the window does: this is the rectangle the \
             damage tracker follows, and a bleed it does not cover is a bleed that \
             leaves trails when it animates. Measured {:?}",
            spread.drawn.loc
        );
        assert!(
            (spread.drawn.size.w - 848.0).abs() < 1e-9
                && (spread.drawn.size.h - 648.0).abs() < 1e-9,
            "and it is the whole canvas, so the picture maps onto it one to one \
             rather than being squashed back into the pane. Measured {:?}",
            spread.drawn.size
        );
    }

    /// A layer that asked for nothing costs nothing and moves nothing.
    ///
    /// The identity case, which every window on an ordinary desktop is: the
    /// canvas is the pane, the element is where it always was, and none of the
    /// arithmetic above is observable.
    #[test]
    fn a_layer_with_no_bleed_is_placed_exactly_where_it_was() {
        let drawing = at_rest();
        let spread = super::spread(drawing, Bleed::default());
        assert_eq!(spread.canvas, Rectangle::from_size(drawing.outer));
        assert_eq!(spread.drawn, drawing.rect);
    }

    /// **The bleed scales with the window, because the buffer is mapped onto
    /// the whole of the drawn rectangle.**
    ///
    /// A thumbnail in an overview carries its spikes at thumbnail size. Growing
    /// the drawn rect by the *unscaled* bleed instead would map a half-size
    /// pane's picture onto a full-size border, which reads as the effect coming
    /// unstuck from the window it belongs to — and, worse, would be wrong by a
    /// different amount at every step of a resize animation.
    #[test]
    fn a_halved_window_carries_a_halved_bleed() {
        let half = Drawing {
            rect: crate::present::logical((0.0, 0.0), (400.0, 300.0)),
            outer: (800, 600).into(),
            alpha: 1.0,
            scale: 1.0,
        };
        let spread = super::spread(
            half,
            Bleed {
                top: 48,
                right: 0,
                bottom: 0,
                left: 24,
            },
        );
        assert_eq!(
            spread.canvas.size,
            (824, 648).into(),
            "the scene is still rasterised at the window's own size: a thumbnail's \
             titlebar costs what a full-size one does"
        );
        assert!(
            (spread.drawn.loc.x + 12.0).abs() < 1e-9 && (spread.drawn.loc.y + 24.0).abs() < 1e-9,
            "but it lands half as far out. Measured {:?}",
            spread.drawn.loc
        );
        assert!(
            (spread.drawn.size.w - 412.0).abs() < 1e-9
                && (spread.drawn.size.h - 324.0).abs() < 1e-9,
            "and is half as large. Measured {:?}",
            spread.drawn.size
        );
    }

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
    /// `SOLIUM_QML_TITLEBAR` replaces the path outright and `SOLIUM_PANE` — or
    /// its old spelling `SOLIUM_DECORATION` — is read before the style, so any
    /// of the three turns a test that builds a frame into a test of whatever
    /// that variable points at: a folder that may not exist, or `none`, which
    /// builds nothing at all. Tests share a process and an environment, so the
    /// answer is to stand down rather than to unset it.
    fn the_environment_has_already_chosen() -> bool {
        std::env::var_os("SOLIUM_QML_TITLEBAR").is_some() || environment_names_a_style()
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
        // `pane = "none"` goes through `insert`, which marks it, while
        // one already framed when the style changed falls to `Pending` and
        // keeps reserving room. See `set_style`'s bare arm.
        if environment_names_a_style() {
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

    // ---- what the panel can offer -------------------------------------------
    //
    // These replace `script::shipped::the_tweaks_panel_lists_every_decoration_
    // that_ships`, which compared a hand-written Lua array against
    // `qml/decorations/`. That comparison is gone because both halves are:
    // the panel's list *is* the directory now, so "the panel offers something
    // that does not exist" and "a decoration nobody can reach" are no longer
    // failures that can happen.
    //
    // What a walk can get wrong instead is everything below. Its failures are
    // quiet in a way the old one's were not -- the panel renders a short list
    // exactly as happily as a complete one -- so the cheap instrument checks
    // (`is_empty`, "a bundle was found at all") are as load-bearing here as the
    // precedence cases.

    /// An empty `panes/` and `decorations/` pair of this test's own.
    ///
    /// Two directories rather than one, because everything here turns on which
    /// of the two a name was found in.
    fn shelf(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("solium-offered-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("panes")).expect("a panes directory");
        std::fs::create_dir_all(dir.join("decorations")).expect("a decorations directory");
        dir
    }

    /// A bundle called `name` on `shelf`, with the manifest that makes it one.
    fn bundle_on(shelf: &Path, name: &str) {
        let dir = shelf.join("panes").join(name);
        std::fs::create_dir_all(&dir).expect("a bundle directory");
        std::fs::write(dir.join("Pane.qml"), "import Solium\nPaneStyle {}\n")
            .expect("writing a manifest");
    }

    /// A single-file decoration called `name` on `shelf`.
    fn file_on(shelf: &Path, name: &str) {
        std::fs::write(
            shelf.join("decorations").join(format!("{name}.qml")),
            "import QtQuick\nItem {}\n",
        )
        .expect("writing a decoration");
    }

    /// The two places a shelf holds, in the order [`build`] tries them.
    fn places_on(shelf: &Path) -> Vec<Place> {
        vec![
            Place {
                dir: shelf.join("panes"),
                kind: StyleKind::Bundle,
            },
            Place {
                dir: shelf.join("decorations"),
                kind: StyleKind::File,
            },
        ]
    }

    /// **A bundle and a file of the same name are one entry, and it is the
    /// bundle.**
    ///
    /// Which is what `build` does with the name: `bundle()` is consulted first
    /// and only a `None` from it reaches `qml_path`. A panel showing both would
    /// be offering two buttons that do the same thing, and the one labelled as
    /// a file would be a lie about what the press draws.
    #[test]
    fn a_bundle_hides_a_single_file_of_the_same_name() {
        let shelf = shelf("shadow");
        bundle_on(&shelf, "top");
        file_on(&shelf, "top");

        assert_eq!(
            catalogue(&places_on(&shelf)),
            vec![Offered {
                kind: StyleKind::Bundle,
                name: "top".to_owned(),
            }],
            "one `top`, and it is the bundle"
        );

        // The control, and it is what makes the case above about *precedence*
        // rather than about bundles: take the bundle away and the same name is
        // still offered, now as the file it now is.
        std::fs::remove_dir_all(shelf.join("panes").join("top")).expect("removing the bundle");
        assert_eq!(
            catalogue(&places_on(&shelf)),
            vec![Offered {
                kind: StyleKind::File,
                name: "top".to_owned(),
            }],
            "with the bundle gone, the file is what `top` means"
        );
    }

    /// **A name in two places of the same kind is offered once.**
    ///
    /// The user's `panes/example/` shadows the shipped one, so the panel has
    /// one `example` button rather than two identical ones, only one of which
    /// could ever be reached.
    #[test]
    fn a_name_in_two_places_is_offered_once() {
        let mine = shelf("mine");
        let theirs = shelf("theirs");
        bundle_on(&mine, "twin");
        bundle_on(&theirs, "twin");
        let both = || {
            vec![
                Place {
                    dir: mine.join("panes"),
                    kind: StyleKind::Bundle,
                },
                Place {
                    dir: theirs.join("panes"),
                    kind: StyleKind::Bundle,
                },
            ]
        };

        assert_eq!(
            catalogue(&both()),
            vec![Offered {
                kind: StyleKind::Bundle,
                name: "twin".to_owned(),
            }],
            "the shadowed copy is not a second button"
        );

        // The control: a walk that simply stopped at the first directory would
        // give the same single entry above and be wrong. A name only the second
        // one has must still arrive.
        bundle_on(&theirs, "only-theirs");
        assert_eq!(
            catalogue(&both())
                .into_iter()
                .map(|offered| offered.name)
                .collect::<Vec<_>>(),
            vec!["only-theirs".to_owned(), "twin".to_owned()],
            "the second place is read; it is the repeated *name* that is dropped"
        );
    }

    /// **A directory under `panes/` with no `Pane.qml` is not a style.**
    ///
    /// `style::load` reads that file first and fails without it, so offering
    /// such a folder would be offering a button that cannot work. It is also
    /// how a `panes/common/` of shared components several bundles import stays
    /// out of the panel -- which is a thing somebody authoring a set of styles
    /// will make almost immediately.
    #[test]
    fn a_directory_with_no_manifest_is_not_offered() {
        let shelf = shelf("manifest");
        let common = shelf.join("panes").join("common");
        std::fs::create_dir_all(&common).expect("a directory");
        std::fs::write(common.join("Frame.qml"), "import QtQuick\nItem {}\n")
            .expect("writing a component");

        assert_eq!(
            catalogue(&places_on(&shelf)),
            Vec::<Offered>::new(),
            "a folder of components is not a style"
        );

        // The control: the only thing that was missing was the manifest.
        std::fs::write(common.join("Pane.qml"), "import Solium\nPaneStyle {}\n")
            .expect("writing a manifest");
        assert_eq!(
            catalogue(&places_on(&shelf)),
            vec![Offered {
                kind: StyleKind::Bundle,
                name: "common".to_owned(),
            }],
            "and with one, it is"
        );
    }

    /// **Bundles first, then files, each sorted by name.**
    ///
    /// Pinned rather than left to `StyleKind`'s declaration order, which is what
    /// actually decides it through the derived `Ord` -- reordering that enum
    /// would otherwise reorder the panel silently.
    ///
    /// It is a display requirement and not a preference. `qml/tweaks.qml` draws
    /// a group heading whenever the group differs from the previous entry's, so
    /// a list that interleaved the two kinds would print "Pane style" and
    /// "Decoration" alternately down the whole panel. And sorted at all because
    /// `read_dir` order is the filesystem's business: unsorted, the buttons
    /// would move between runs.
    #[test]
    fn bundles_come_before_files_and_each_group_is_sorted() {
        let shelf = shelf("order");
        bundle_on(&shelf, "zebra");
        bundle_on(&shelf, "apple");
        file_on(&shelf, "yak");
        file_on(&shelf, "bee");

        assert_eq!(
            catalogue(&places_on(&shelf))
                .into_iter()
                .map(|offered| (offered.kind, offered.name))
                .collect::<Vec<_>>(),
            vec![
                (StyleKind::Bundle, "apple".to_owned()),
                (StyleKind::Bundle, "zebra".to_owned()),
                (StyleKind::File, "bee".to_owned()),
                (StyleKind::File, "yak".to_owned()),
            ],
            "two contiguous runs, each in name order"
        );
    }

    /// **The eight this build ships are offered, once each, as bundles.**
    ///
    /// The instrument check, and the failure it is here for is silent: a
    /// `StyleKind::Bundle` walk that found nothing would leave the panel empty
    /// or short, looking exactly like a working panel, with no press to prove
    /// otherwise. Every other case in this section would still pass.
    ///
    /// It checked for a `StyleKind::File` too until Task 7, because the eight
    /// were single files and finding none meant the walk had missed a whole
    /// kind. They are folders now and **nothing ships as a file**, so that
    /// assertion could only ever pass by accident -- a stray `.qml` in the
    /// user's own directory, on the developer's machine alone. What replaces it
    /// is this: the eight by name, which is a claim about the thing that
    /// actually moved. The `File` kind is still exercised, on directories a
    /// test makes, by `bundles_come_before_files_and_each_group_is_sorted` and
    /// the shadowing cases above.
    #[test]
    fn the_eight_that_ship_are_offered_as_bundles() {
        let ships = ships();
        for name in [
            "top",
            "left",
            "bottom",
            "border",
            "reactive",
            "proximity",
            "reveal",
            "pulse",
        ] {
            assert!(
                ships.contains(&Offered {
                    kind: StyleKind::Bundle,
                    name: name.to_owned(),
                }),
                "qml/panes/{name}/ is a style this build ships and the walk did not \
                 offer it as a bundle. Offered: {ships:?}"
            );
            assert_eq!(
                ships.iter().filter(|it| it.name == name).count(),
                1,
                "{name} is offered more than once"
            );
        }
        assert!(
            ships.contains(&Offered {
                kind: StyleKind::Bundle,
                name: "example".to_owned(),
            }),
            "and the worked example, which is what the format is documented by"
        );
    }

    /// **Everything the panel offers is what pressing it would resolve to.**
    ///
    /// The load-bearing one, and the closest thing to a replacement for the
    /// pinning test that is gone: it ties the *listing* to the *resolver*
    /// rather than to a second copy of the directory layout. `style::find` is
    /// the call `bundle()` makes, so a `StyleKind::Bundle` it answers `None` for is
    /// a button labelled as a style with layers that would draw a single-file
    /// decoration, and a `StyleKind::File` it answers `Some` for is the reverse.
    ///
    /// Against this machine's real directories, the user's included -- which is
    /// the point, since shadowing is exactly what it is checking and only a
    /// real `~/.config/solium/qml/panes` can produce it. It asserts agreement
    /// rather than contents, so it says nothing about what is installed and
    /// passes on a machine with nothing of its own.
    ///
    /// Reads no environment variable, so it is safe beside the tests that set
    /// one: `available` and `style::find` are both pure directory walks, and it
    /// is `chosen()` -- deliberately not called here -- that reads
    /// `SOLIUM_PANE`.
    ///
    /// **The case it was run against, since a consistency check between two
    /// functions that share a directory list can easily be vacuous.** An empty
    /// `qml/panes/top/` used to make it fail with "`top` is offered as a single
    /// file, but `style::find` resolves it to ...", because `find` took any
    /// *directory* for a bare name while [`offered_by`] required a `Pane.qml`.
    /// Task 7 closed that: a bare name needs the manifest too, and skips a
    /// folder without one rather than answering with it. So the disagreement
    /// this test was written to catch can no longer be produced from an empty
    /// folder -- which is the outcome wanted, not a reason to stop checking.
    /// What it still holds is the same claim against every *other* way the two
    /// could drift: a directory list walked in one order here and another
    /// there, or a kind decided differently on each side. See
    /// `style::a_bare_name_needs_a_manifest_and_not_merely_a_folder`, which
    /// pins the rule itself.
    #[test]
    fn what_the_panel_offers_is_what_a_press_resolves() {
        let offered = available();
        assert!(
            !offered.is_empty(),
            "this machine offers no styles at all; the walk is broken, not the tree"
        );
        for entry in offered {
            let found = crate::style::find(&entry.name);
            match entry.kind {
                StyleKind::Bundle => assert!(
                    found.is_some(),
                    "{:?} is offered as a bundle, but `style::find` does not find it, so a \
                     press would fall through to a single file",
                    entry.name
                ),
                StyleKind::File => assert!(
                    found.is_none(),
                    "{:?} is offered as a single file, but `style::find` resolves it to {} -- \
                     a press would draw that bundle instead",
                    entry.name,
                    found.unwrap_or_default().display()
                ),
            }
        }
    }
}
