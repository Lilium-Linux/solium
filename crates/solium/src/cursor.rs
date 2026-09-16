//! The pointer.
//!
//! # Three sources, and the precedence between them
//!
//! Something has to decide what the pointer looks like at any instant, and
//! three different things are entitled to say. Listed in the order they
//! arrive rather than in any order of rank:
//!
//! 1. **A client hands over pixels.** `wl_pointer.set_cursor` with a surface:
//!    the client rasterised a cursor itself and we draw its buffer, hotspot
//!    and all. `CursorImageStatus::Surface`, and `render.rs` draws it as a
//!    surface tree rather than through this module.
//! 2. **A client names a shape.** `wp_cursor_shape_device_v1.set_shape`: the
//!    client says "this is a text field" and leaves the picture to us. smithay
//!    hands it over as `CursorImageStatus::Named`, and what becomes of that
//!    name is the rest of this module — see [`shape`], which turns it into
//!    something a theme has a file under. **This is the arm that did not exist
//!    before #24**, and its absence was not a degraded cursor but *no cursor
//!    change at all*: a toolkit that only ever names shapes got the arrow over
//!    a text field, over a resize edge, over everything.
//! 3. **Nobody said, so the compositor's own.** Over the desktop, over a
//!    frame, as soon as the pointer leaves a client — `input/mod.rs` puts
//!    `Named(Default)` back, because a cursor belonging to a window the
//!    pointer has left is a cursor that vanishes the moment that window
//!    closes.
//!
//! **The precedence is last-writer-wins, and that is the protocol's rule
//! rather than a shortcut of ours.** Both client-side sources are gated by
//! smithay on the serial of the most recent `wl_pointer.enter` — `set_cursor`
//! in `wayland/seat/pointer.rs` and `set_shape` in `wayland/cursor_shape.rs`
//! call the same `allow_setting_cursor` — so a client can only speak while the
//! pointer is genuinely over its surface. There is no contest between two
//! clients to arbitrate, and ranking the two client mechanisms against each
//! other would only let us disagree with a toolkit about which of its own two
//! it meant. What *does* have to be guaranteed is that each write replaces the
//! last **whole**, so that a client alternating between a named shape and its
//! own buffer — ordinary enough: a toolkit naming `text` over a field and
//! attaching a bitmap for a drag — never leaves a fragment of the previous one
//! behind. That is [`Pointer::show`], the only writer, and
//! [`Pointer::showing`], the only reader, which is also where a surface that
//! has since died turns back into the compositor's arrow.
//!
//! # And underneath two of the three, two ways to draw a name
//!
//! Sources two and three both end in a *name*, and a name is drawn from the
//! configured XCursor theme where there is one, so that the pointer matches
//! what every other application on the machine draws; see [`theme`], which is
//! where that arrived and why. When there is no theme — none configured, none
//! in the environment, or a name nothing on disk answers to — a name is drawn
//! from *ours*, from QML, through the same design system as the window frames.
//!
//! **The QML pointer is not a fallback that was left lying around; it is the
//! floor.** It is deliberate, it is what a session with no theme configured
//! shows, and it is the only arm here that cannot fail for want of a file on
//! disk. Nested, none of this existed: the host compositor drew the cursor
//! over our window and we never had to think about it. On the hardware nothing
//! else will, and an invisible pointer is not a cosmetic problem — it is
//! indistinguishable from input being dead, which is exactly how it was
//! reported the first time this ran on a real screen. A machine with no cursor
//! themes installed must end up at the QML pointer, never at nothing — and a
//! *shape* the theme has never heard of must end up at the theme's own arrow
//! before it ever gets that far, which is [`shape::resolve`].

pub(crate) mod shape;
pub(crate) mod theme;

use std::path::PathBuf;

use anyhow::Result;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            Bind as _, ExportMem as _,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
            gles::GlesRenderer,
        },
    },
    input::pointer::{CursorIcon, CursorImageStatus},
    utils::{IsAlive as _, Logical, Point, Rectangle, Transform},
};

use crate::{
    qml::{
        self,
        paint::{Gpu, Kept, Said},
    },
    render::Element,
};

/// Where the point of the arrow is within the QML image.
///
/// The arrow is drawn with its tip in the top-left corner, so the buffer is
/// placed at the pointer position directly. Kept named rather than assumed,
/// because a cursor drawn a few pixels off its hotspot is maddening to use and
/// almost impossible to see in a screenshot.
const HOTSPOT: (i32, i32) = (0, 0);

/// How many device sizes of the pointer are kept at once.
///
/// One per monitor scale in use, and two is the realistic number. The cap is
/// here because the key is a pixel size rather than a monitor: a script that
/// animates an output's scale would otherwise mint a buffer per step and keep
/// every one of them.
///
/// **The same exposure exists twice and is now capped twice**, which is the
/// part of this worth saying: `theme::Theme`'s cache of rasterised cursors is
/// keyed on a device size too, is reached from the same per-output-per-frame
/// call, and would grow under the same animated scale — so it is bounded at
/// `theme::SIZES_KEPT`, the same four, by the same [`Kept`]. What differs is
/// what a size costs. Here it is one buffer: the pointer is the configured
/// size, which is 24 logical pixels by default and at most 256, so an entry is
/// 2.3 KB at the default and 1x, 9.2 KB at 2x, and 1 MB at the largest size a
/// setting may name on a 2x screen. There it is one buffer per cursor *name*,
/// thirty-six of them, each the theme's own nearest image rather than the size
/// asked for.
///
/// Four rather than the two `qml::paint`'s own [`Kept`] uses, and the reason is
/// the same arithmetic from the other end: a window frame is a whole window,
/// 3.9 MB of it on an ordinary one, and there is one per window — see `KEPT` in
/// `qml/paint.rs`. A generous guard costs nothing at pointer sizes.
const KEPT: usize = 4;

/// How a rasterised pointer reaches the screen.
///
/// Not a preference, and not this module's decision: Qt fixes its scene graph
/// for the life of the process and a host that came up on one backend refuses
/// scenes of the other kind, so this follows `qml::on_gpu` — see
/// [`qml::Scene::for_host`], which is where it is actually decided.
///
/// **Both arms end in a `MemoryRenderBuffer`, and that is the point.** The
/// pointer is the one element in the compositor that can occupy a dedicated
/// hardware plane, and smithay reaches a cursor plane only through
/// `RenderElement::underlying_storage` — which has exactly two variants,
/// `Wayland` and `Memory` (`renderer/element/mod.rs:103-109`). A
/// `TextureRenderElement` implements none, inherits the default `None`, and
/// `copy_element_to_cursor_bo` gives up on its first line
/// (`drm/compositor/mod.rs:4190-4193`). There is no `UnderlyingStorage` a
/// dmabuf-backed texture can satisfy, so a GPU cursor drawn as a texture turns
/// every pointer motion on a TTY into a full composite and page flip of the
/// whole output, logged nowhere above `trace!`.
///
/// **And there is no fallback underneath it on this build**, which is the half
/// of that worth stating rather than the half that reassures. smithay does have
/// a pixman path that renders the element into the cursor buffer when the copy
/// fails, at `mod.rs:3257-3269` — but it is behind `#[cfg(feature =
/// "renderer_pixman")]` and `renderer_pixman` is not in our feature list
/// (`Cargo.toml:24-43`). What compiles here is the `#[cfg(not(...))]` arm at
/// `mod.rs:3244`, whose failure branch is a `trace!` and a plain `return None`.
/// So `copy_element_to_cursor_bo` is the only route to the plane in this
/// binary, and losing it loses the plane outright. (Reading the pixman arm is
/// still instructive: it asks for `underlying_storage` too, at `mod.rs:3269`,
/// so it would refuse a texture as well — but a fallback that is not compiled
/// cannot be the reason anything works.)
///
/// So on the GPU path Qt still draws the pointer — it has to, a GPU host
/// refuses software scenes — and the result is read straight back out of the
/// dmabuf into a `MemoryRenderBuffer`. That is a readback the other scenes do
/// not pay, and it is affordable here for the reason the whole trade is
/// lopsided: the pointer is 24 logical pixels, so there is almost no
/// rasterisation to save by putting it on the GPU, and it changes on a scale or
/// a theme change rather than per frame.
///
/// Measured rather than assumed — `dev/wirecheck` runs the round trip and
/// prints what it costs. The whole import, bind, `copy_framebuffer` and map, on
/// this machine, is **~65 µs of fixed cost plus 3.5–4 ns per pixel**. Fitted
/// over a five-point sweep rather than read off one pair:
///
/// ```text
/// 24x24       576 px     66.9 µs
/// 48x48      2304 px     69.1 µs
/// 96x96      9216 px    110.8 µs
/// 192x192   36864 px    192.3 µs
/// 384x384  147456 px    605.8 µs
/// ```
///
/// **The pixel term is negligible at cursor sizes and only at cursor sizes**,
/// and that scope is the load-bearing part. At 24x24 it is ~2 µs of ~67 — three
/// per cent — so what is being paid really is the round trip, and the cache
/// below turns even that into once per size per change rather than per frame.
/// The claim expires almost immediately above the pointer: at 384x384 the pixel
/// term alone is ~540 µs and the whole call is 606, nine times what the cursor
/// pays for its entire readback. So do not carry "it is the round trip, not the
/// pixels" out of this paragraph and use it to justify dropping the cache — it
/// is a statement about 24- and 48-pixel squares, not about readbacks.
///
/// The pair this used to cite — 24x24 at 63.5 µs against 48x48 at 59.8, the
/// *smaller* size measuring slower — was a single-run artefact and was never
/// evidence for anything. Seven runs at each size give medians of 68.0 and
/// 67.1, and the spread within one size (65.6–80.2) is wider than the gap
/// between the two sizes.
#[derive(Debug)]
enum Backing {
    /// Qt rasterises into a `QImage` and we copy out of it.
    Memory,
    /// Qt draws into a dmabuf we allocated; we sample it and read it back.
    Gpu(Gpu),
}

/// Our own pointer, rasterised once per size and reused.
#[derive(Debug)]
pub(crate) struct Cursor {
    scene: qml::Scene,
    backing: Backing,
    /// How big it is, in **logical** pixels — the resolved setting, not a
    /// constant, since `config.lua` and `XCURSOR_SIZE` can both name it. See
    /// [`theme::Settings`].
    ///
    /// Logical, and multiplied by each output's scale in [`Cursor::element`]
    /// through [`theme::pixels`]; the same function the themed path uses, so
    /// the two pointers cannot disagree about how big a pointer is.
    size: i32,
    /// One uploadable buffer per device size the pointer has been asked for.
    ///
    /// Keyed on one edge because a pointer is square. The window frames use the
    /// same container keyed on a size *pair*, which is why [`Kept`] is generic
    /// over its key — see `qml/paint.rs`.
    buffers: Kept<i32, MemoryRenderBuffer>,
    /// Whether producing a pointer image has already failed and said so.
    ///
    /// One latch for the whole of [`Cursor::fill`] rather than one per `warn!`,
    /// because there is one failure here and the several messages are its
    /// stages: whichever of them is reached first is the one worth reading, and
    /// the ones after it did not run. See [`Said`].
    ///
    /// The shape that needs it: when a GPU rebind persistently fails the scene
    /// freezes on a smaller buffer, [`Cursor::fill`] pushes under that frozen
    /// size, and so `buffers.current(edge, ..)` misses for that output on every
    /// frame for the rest of the session. The retry is deliberate and right, but
    /// it means each frame pays a failed allocation and a full readback *and*,
    /// without this, said so four times.
    drawing: Said,
    /// And whether turning a finished image into an element has.
    ///
    /// Separate from `drawing`, because it is a different failure at a
    /// different stage — one of them silencing the other would hide the case
    /// where both are happening.
    uploading: Said,
}

impl Cursor {
    /// `size` is logical pixels; see [`Cursor::size`].
    pub(crate) fn new(size: i32) -> Result<Self> {
        qml::start()?;
        // `size` square to begin with, which at 1x is also the size it stays.
        // It is *not* a scene that is never resized, whatever its buffer being
        // one picture might suggest: 24 is 24 *logical* pixels, so the pointer
        // crossing onto a 2x monitor needs a 48-pixel one and the GPU path
        // rebinds onto a new buffer to get it. See `Cursor::element`.
        let scene = qml::Scene::for_host(&qml_path(), size, size, None)?;
        Ok(Self {
            scene,
            backing: if qml::on_gpu() {
                // The size the scene really is, unlike the shell surfaces:
                // there is nothing to discover about a pointer's size, so it is
                // allocated right the first time and only a new scale moves it.
                Backing::Gpu(Gpu::new((size, size)))
            } else {
                Backing::Memory
            },
            size,
            buffers: Kept::keeping(KEPT),
            drawing: Said::default(),
            uploading: Said::default(),
        })
    }

    /// Draw at a different logical size from now on, after a reload.
    ///
    /// Nothing is thrown away and nothing needs to be. `buffers` is keyed on
    /// the *device* edge, and the pixels for a 48-pixel pointer are the same
    /// 48-pixel pointer whether they came from 24 logical at 2x or 48 logical
    /// at 1x — `cursor.qml` has no size-dependent content. What changes is the
    /// logical size those pixels are drawn at, and that is read from here
    /// fresh on every call rather than cached anywhere.
    fn set_size(&mut self, size: i32) {
        self.size = size;
    }

    /// The cursor as something to draw, at `location`.
    ///
    /// `Kind::Cursor` is not decoration: it is what lets the DRM backend put
    /// this on the hardware cursor plane, which moves the pointer without
    /// redrawing the screen behind it. See [`Backing`] for why that survives on
    /// the GPU path only because this still ends in a `MemoryRenderBuffer`.
    ///
    /// Concrete on `GlesRenderer` rather than generic since the GPU path
    /// arrived, for the reason `ShellSurface::element` gives: taking the
    /// thread's EGL context back off Qt is `EGLContext::make_current`, and
    /// nothing on the `Renderer` traits says where the context is.
    pub(crate) fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        location: Point<f64, Logical>,
        scale: f64,
    ) -> Option<Element> {
        // `self.size` logical pixels, whatever the monitor is. On a 2x display
        // that is twice as many device pixels, and drawing the 1x image there
        // would leave a pointer a quarter of the size it should be -- which on
        // a HiDPI panel is a pointer you cannot find, and this module exists
        // because an invisible pointer reads as input being dead.
        //
        // Through `theme::pixels` rather than multiplied here, so that the QML
        // pointer and a themed one cannot end up disagreeing about how big a
        // pointer is at a given scale. That function is where the reasoning
        // for keeping the setting logical lives.
        let edge = theme::pixels(self.size, scale);

        // Anything kept is the *previous* picture when Qt has something new,
        // and the render below is about to spend the flag that says so. The
        // invalidation is inside `Kept::current` rather than a `clear` written
        // out here, so that it cannot be forgotten and so that the window
        // frames invalidate the same way — see `qml/paint.rs`. It happens
        // *before* the render for the same reason it always did: the size this
        // frame does not want has to be re-made when it is next asked for
        // instead of being served stale for ever.
        //
        // In practice it fires once, on the first frame: `cursor.qml` has no
        // animation and nothing writes its properties, so after the pointer has
        // been drawn once it never asks to be drawn again.
        //
        // Which is also why the pointer does **not** need
        // `Scene::animation_in_flight` beside this, and why it is not a
        // `Painted`. Two separate reasons, and both have to hold: `cursor.qml`
        // has no animation to run, and the pointer is drawn from pointer
        // motion, which damages the screen by itself and brings its own frames.
        // Put an animation in `cursor.qml` and neither reason survives -- it
        // would advance while the mouse moves and freeze the instant it stopped,
        // the exact shape `render::Drawn` exists for -- so that change is also a
        // change here. `needs_render` is the right question in *this* line
        // regardless: what it guards is a cache of pixels, not a frame.
        //
        // **And "theme change" has no trigger at all today — it is the shape
        // this is built for, not something that happens.** `Solium/Theme.qml`
        // is a `pragma Singleton` whose twenty-one colours are every one of
        // them a `readonly property` bound to a literal, and nothing in the
        // compositor writes a property on the cursor scene — there is no
        // `set_int`/`set_bool` on it anywhere, unlike a decoration or a pane.
        // So `needs_render` can return true here on frame 1 and never again.
        // The wiring is right and cheap and should stay; what it is not is
        // exercised. The unit tests below cover the invalidation honestly and
        // say so, but nothing anywhere drives a theme change end to end, so do
        // not read a green test suite as evidence that a live re-theme repaints
        // the pointer. It has never been done once.
        //
        // The size actually in hand, which is `edge` unless a GPU rebind failed
        // and the scene is frozen on a smaller buffer.
        let fresh = self.scene.needs_render();
        let held = if self.buffers.current(edge, fresh) {
            edge
        } else {
            self.fill(renderer, edge, scale)?
        };
        let buffer = self.buffers.get_mut(held)?;

        // Physical, and the hotspot is logical, so both go through the scale.
        let position = (
            (location.x - f64::from(HOTSPOT.0)) * scale,
            (location.y - f64::from(HOTSPOT.1)) * scale,
        );

        // The whole buffer in its own pixels, mapped down to `self.size`
        // logical pixels, which the output scale takes back up to `held`. When
        // `held` is `edge` — every case but a frozen scene — those two are the
        // same number, which is also what `copy_element_to_cursor_bo` requires
        // before it will use the plane's fast path: it refuses any element
        // whose src and drawn size disagree.
        let source = Rectangle::from_size((f64::from(held), f64::from(held)).into());
        let uploaded = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            position,
            buffer,
            None,
            Some(source),
            Some((self.size, self.size).into()),
            Kind::Cursor,
        );
        match uploaded {
            Ok(element) => {
                self.uploading.worked();
                Some(Element::Chrome(element))
            }
            Err(err) => {
                self.uploading.once(|| {
                    tracing::warn!(
                        ?err,
                        "could not upload the cursor; the pointer will be invisible. Said once \
                         until it uploads again"
                    );
                });
                None
            }
        }
    }

    /// Draw the pointer at `edge` pixels and keep the result. Returns the size
    /// that was actually produced.
    ///
    /// **Every way out of here that is not `Some` runs again next frame**, for
    /// this output and every other one: nothing was pushed, so `buffers.current`
    /// is still false. That is deliberate — a GBM allocation that failed because
    /// the GPU was momentarily full heals itself with nobody having to notice —
    /// and it is why each complaint on the way out goes through `self.drawing`
    /// rather than straight to `warn!`. See [`Said`].
    fn fill(&mut self, renderer: &mut GlesRenderer, edge: i32, scale: f64) -> Option<i32> {
        // Several fields of one struct, borrowed at once.
        let Self {
            scene,
            backing,
            buffers,
            drawing,
            ..
        } = self;
        let (pixels, stride, held) = match backing {
            Backing::Memory => {
                scene.resize(edge, edge, scale);
                match scene.render() {
                    Ok(rendered) => (Pixels::Borrowed(rendered.pixels), rendered.stride, edge),
                    Err(err) => {
                        drawing.once(|| {
                            tracing::warn!(
                                ?err,
                                "the cursor did not render. Said once until it renders again"
                            );
                        });
                        return None;
                    }
                }
            }
            Backing::Gpu(gpu) => {
                // No complaint of ours on this `?`: `Gpu::sample` has its own
                // latches — the same `Said` this module uses — and has already
                // said whatever there was to say.
                let (texture, size) = {
                    let shown = gpu.sample(scene, renderer, (edge, edge), scale)?;
                    (shown.texture.clone(), shown.size)
                };
                let read = read_back(renderer, texture, size, drawing)?;
                let stride = usize::try_from(size.0.max(0)).unwrap_or_default() * 4;
                (Pixels::Owned(read), stride, size.0)
            }
        };

        let row_bytes = usize::try_from(held.max(0)).unwrap_or_default() * 4;
        let mut buffer =
            MemoryRenderBuffer::new(Fourcc::Argb8888, (held, held), 1, Transform::Normal, None);
        let mut context = buffer.render();
        let copy = context.draw(|target| {
            for (row, destination) in target.chunks_exact_mut(row_bytes).enumerate() {
                let start = row * stride;
                let Some(source) = pixels.as_ref().get(start..start + row_bytes) else {
                    return Err(());
                };
                destination.copy_from_slice(source);
            }
            Ok(vec![Rectangle::from_size((held, held).into())])
        });
        if copy.is_err() {
            drawing.once(|| {
                tracing::warn!(
                    "the cursor image was smaller than its buffer. Said once until one copies \
                     again"
                );
            });
            return None;
        }
        drop(context);

        buffers.push(held, buffer);
        drawing.worked();
        Some(held)
    }
}

/// Whichever of the two paths produced the pixels, as one slice.
///
/// The software path borrows them from the scene and the GPU path owns them, so
/// the copy below cannot name a single type without this.
enum Pixels<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl AsRef<[u8]> for Pixels<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Borrowed(pixels) => pixels,
            Self::Owned(pixels) => pixels,
        }
    }
}

/// Read a scene's dmabuf back into ordinary memory.
///
/// The pointer's whole reason for being on this path — see [`Backing`]. Top-down
/// premultiplied ARGB8888, tightly packed, which is exactly what
/// `MemoryRenderBuffer` wants and what `import_memory` would have been handed on
/// the software path; `dev/wirecheck` asserts that byte for byte rather than
/// leaving it to the orientation conventions of three different APIs.
///
/// Binds a framebuffer, so it must not run underneath another bind — every
/// backend builds its elements before it binds anything, which is the same
/// convention `offscreen::capture` relies on.
///
/// `said` is the caller's latch and not this function's own, because the three
/// failures below are three stages of one operation: the first one reached is
/// the one worth reading and the rest never ran. Without it this is three
/// `warn!` per output per frame for the life of the session — see [`Said`].
fn read_back(
    renderer: &mut GlesRenderer,
    texture: smithay::backend::renderer::gles::GlesTexture,
    size: (i32, i32),
    said: &mut Said,
) -> Option<Vec<u8>> {
    let mut texture = texture;
    let framebuffer = renderer
        .bind(&mut texture)
        .inspect_err(|err| {
            said.once(|| {
                tracing::warn!(
                    ?err,
                    "could not bind the cursor's buffer to read it. Said once until it reads again"
                );
            });
        })
        .ok()?;
    let mapping = renderer
        .copy_framebuffer(
            &framebuffer,
            Rectangle::from_size(size.into()),
            Fourcc::Argb8888,
        )
        .inspect_err(|err| {
            said.once(|| {
                tracing::warn!(
                    ?err,
                    "could not copy the cursor's buffer. Said once until it reads again"
                );
            });
        })
        .ok();
    drop(framebuffer);
    // Whatever happened, leave nothing of ours bound: the next thing to bind is
    // an output's own buffer, and a framebuffer left over here would take the
    // whole frame with it.
    crate::warp::release_framebuffer(renderer);
    let pixels = renderer
        .map_texture(&mapping?)
        .inspect_err(|err| {
            said.once(|| {
                tracing::warn!(
                    ?err,
                    "could not map the cursor's buffer. Said once until it reads again"
                );
            });
        })
        .ok()?
        .to_vec();
    // Not cleared here: `fill` does that, once it has a buffer in hand. A
    // readback that succeeds and is then rejected by the row copy in `fill` has
    // still left the pointer undrawn, and is not a success to reset on.
    Some(pixels)
}

fn qml_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SOLIUM_QML_CURSOR") {
        return PathBuf::from(path);
    }
    crate::assets::qml().join("cursor.qml")
}

/// Whether the configured theme has been looked for yet, and what came back.
///
/// `Missing` is a settled answer and not a retry. A theme that is not
/// installed will not appear part-way through a session, and walking the whole
/// icon search path — every inherited theme, every directory in
/// `XCURSOR_PATH` — to find that out again on every frame is how a pointer
/// turns into a syscall storm. `configure` is the only thing that puts this
/// back to `Unasked`, and only when the theme's *name* changed.
#[derive(Debug, Default)]
enum Loaded {
    #[default]
    Unasked,
    /// No theme configured, or the configured one is not installed. Either
    /// way, the QML pointer is what gets drawn.
    Missing,
    /// Boxed because it carries a cache of rasterised cursors and the other
    /// two arms are empty; an enum is as big as its widest one.
    Theme(Box<theme::Theme>),
}

/// The pointer as the compositor holds it: what to show, and what to draw it
/// with.
///
/// One place, because the two are only meaningful together — a status with
/// nothing able to render it is an invisible pointer, which is the bug this
/// module exists to have fixed.
#[derive(Debug)]
pub(crate) struct Pointer {
    /// What the pointer should look like, as of whichever of the three sources
    /// spoke most recently. See the module header for what those are.
    ///
    /// **Private, and that is the point of this field rather than an
    /// afterthought.** It is written only through [`Pointer::show`] and read
    /// only through [`Pointer::showing`], so that "each source replaces the
    /// last whole" and "a dead surface becomes the arrow" are two lines that
    /// exist once instead of invariants every caller has to remember. It was
    /// `pub(crate)` and assigned from four places before #24; a third source
    /// writing to it is exactly the change that makes that untenable.
    status: CursorImageStatus,
    /// The theme and size, after `config.lua`, the environment and the
    /// built-in default have been consulted in that order. See
    /// [`theme::Settings::resolve`], which is where that order is argued.
    settings: theme::Settings,
    /// The theme's cursors, looked for once. See [`Loaded`].
    loaded: Loaded,
    art: Option<Cursor>,
    /// Set once QML has failed, so a broken scene costs one error and not one
    /// per frame for the life of the session.
    unavailable: bool,
    /// Whether uploading a *themed* cursor has failed and said so.
    ///
    /// Separate from `Cursor::uploading` next door for the reason that one is
    /// separate from `Cursor::drawing`: these are two different failures with
    /// two different causes, and a shared latch means whichever happens second
    /// is the one nobody ever hears about.
    theming: Said,
}

impl Default for Pointer {
    fn default() -> Self {
        Self {
            status: CursorImageStatus::default_named(),
            // Resolved from the environment straight away, rather than waiting
            // for `sol.cursor`. `lua/init.lua` does call it, but a user who
            // copied `init.lua` into ~/.config/solium before this setting
            // existed has one that does not — and `XCURSOR_THEME` being
            // honoured must not depend on a line in a file they wrote last
            // year. Two `env::var` calls; the theme itself is still lazy.
            settings: theme::Settings::resolve(
                &theme::Configured::default(),
                &theme::Environment::read(),
            ),
            loaded: Loaded::Unasked,
            art: None,
            unavailable: false,
            theming: Said::default(),
        }
    }
}

impl Pointer {
    /// Take what one of the three sources has just said the pointer is.
    ///
    /// **The only writer of `status`, and the whole of the precedence between
    /// the three.** The precedence is that the most recent caller wins
    /// outright, and the module header argues why that is the protocol's
    /// answer rather than a convenient one; the *mechanism* is that this
    /// assigns rather than merges, so nothing of the previous source survives
    /// the call. A client that names `text` over its entry field and then
    /// attaches a bitmap for a drag gets the bitmap and no trace of the
    /// I-beam, and the same in reverse — which is the stale pointer this
    /// replaces a scattering of direct assignments to prevent.
    ///
    /// Callers, one per source: `SeatHandler::cursor_image` in `state.rs`
    /// carries both client-side ones (`Named` is `wp_cursor_shape_v1`, since
    /// `wl_pointer.set_cursor` can only produce `Surface` or `Hidden`), and
    /// the two motion handlers in `input/mod.rs` carry the compositor's own.
    pub(crate) fn show(&mut self, image: CursorImageStatus) {
        self.status = image;
    }

    /// What to draw this frame.
    ///
    /// **The only reader, because it is not a pure getter**: a client's cursor
    /// surface can be destroyed while the pointer is still over that client,
    /// and `render.rs` drawing a dead surface's tree produces no elements and
    /// therefore no pointer at all. `input/mod.rs` already puts the arrow back
    /// when the pointer *moves off* a client, which covers the ordinary case
    /// and not this one: a window that dies under a stationary pointer leaves
    /// nothing to draw and no motion to notice it with, and the session looks
    /// like input has stopped. Downgrading here rather than watching for
    /// surface destruction keeps it to one check on a path that already runs
    /// every frame.
    ///
    /// Not unit-tested, and said plainly rather than covered over: a
    /// `WlSurface` cannot be conjured without a display, and standing one up
    /// inside the test binary is the process-global-state trap that has
    /// aborted this suite before. What is testable is that the three sources
    /// replace each other cleanly, and the tests below do that.
    pub(crate) fn showing(&mut self) -> CursorImageStatus {
        if let CursorImageStatus::Surface(surface) = &self.status
            && !surface.alive()
        {
            self.status = CursorImageStatus::default_named();
        }
        self.status.clone()
    }

    /// Apply what the configuration said, over what the environment says.
    ///
    /// Called from `Command::Cursor`, so it runs again on every
    /// `super+shift+r`. Doing nothing when nothing changed matters: this is
    /// reached on every reload, and re-loading the theme would throw away
    /// every rasterised cursor to arrive back at the same ones.
    ///
    /// **Returns whether the pointer's picture may now be different**, which
    /// the caller turns into damage. A reload that changed only the cursor
    /// theme or size is a change nothing else on screen notices: the backends
    /// draw on damage, and a stationary pointer produces none, so without the
    /// answer the new pointer appears whenever something unrelated next
    /// happens to redraw. `false` is the overwhelmingly common case — a reload
    /// that changed a keybinding — and costs the caller nothing.
    pub(crate) fn configure(
        &mut self,
        configured: &theme::Configured,
        environment: &theme::Environment,
    ) -> bool {
        let settings = theme::Settings::resolve(configured, environment);
        // **A reload is also a second chance to find a theme that was not
        // installed last time**, and that is why an unchanged setting is not
        // by itself a reason to stop. `Loaded::Missing` is a settled answer
        // deliberately — see [`Loaded`] — but what settles it is a frame, not a
        // reload: installing the theme `config.lua` already names and pressing
        // `super+shift+r` did nothing at all, which contradicts `config.lua`'s
        // own "trying a theme out is `super+shift+r`". Asking again here costs
        // one directory walk per keypress, not one per frame.
        //
        // Only when a theme is actually named: with none, `load` reaches
        // `Missing` without touching the disk and there is nothing to retry.
        let retry = settings.theme.is_some() && matches!(self.loaded, Loaded::Missing);
        if settings == self.settings && !retry {
            return false;
        }
        tracing::debug!(
            theme = settings.theme.as_deref().unwrap_or("<none: Solium's own>"),
            size = settings.size,
            "pointer"
        );
        // Only a different *name* is worth looking for again — or a name whose
        // last answer was "not installed". A size change does not invalidate a
        // theme: `Theme` keys its cache on the device size too, so the new size
        // simply misses and the old entries stay useful for whichever monitor
        // is still asking for them.
        if retry || settings.theme != self.settings.theme {
            self.loaded = Loaded::Unasked;
            // A new theme is new news: whatever the old one's cursors failed
            // to do, the next failure is worth hearing about again.
            self.theming.worked();
        }
        if let Some(art) = self.art.as_mut() {
            art.set_size(settings.size);
        }
        self.settings = settings;
        true
    }

    /// The pointer as something to draw, at `location` on an output at
    /// `scale`.
    ///
    /// `icon` reaches here from either of the two sources that end in a name —
    /// a client's `wp_cursor_shape_v1.set_shape`, or the compositor's own
    /// arrow — and nothing below distinguishes them, deliberately: a shape a
    /// client named and a shape we named are the same request for the same
    /// picture, and a session where the two were themed differently would be
    /// the split-personality pointer this whole area exists to have closed.
    ///
    /// **Two arms, and the order between them is the whole feature.** A themed
    /// cursor first, because that is the one that matches the rest of the
    /// machine. Solium's own QML pointer second, and it is reached by every
    /// route the first arm can fail by: no theme configured, a theme that is
    /// not installed, a theme that has neither this cursor nor an arrow under
    /// any name it knows, a cursor file that is malformed, an upload that was
    /// refused. None of those is allowed to end in nothing being drawn.
    ///
    /// Note where the *shape* fallback sits relative to these two: it is
    /// inside the first arm, not beside it. A shape the theme has not got
    /// becomes the theme's own arrow in [`shape::resolve`] and never reaches
    /// the second arm at all, so a themed session stays wholly themed.
    pub(crate) fn element(
        &mut self,
        renderer: &mut GlesRenderer,
        icon: CursorIcon,
        location: Point<f64, Logical>,
        scale: f64,
    ) -> Option<Element> {
        // Logical size times this output's scale, at the point of use. See
        // `theme::pixels`.
        let pixels = theme::pixels(self.settings.size, scale);
        if let Some(element) = self.themed(renderer, icon, pixels, location, scale) {
            return Some(element);
        }
        self.art()
            .and_then(|cursor| cursor.element(renderer, location, scale))
    }

    /// The themed cursor for `icon` at `pixels` device pixels, if there is
    /// one.
    ///
    /// Returns `None` rather than complaining for every ordinary way of not
    /// having one — those are diagnosed once each, where they happen, in
    /// [`theme::Theme`]. The `warn!` here is for the one case that is a real
    /// fault: a cursor that was found and rasterised and then would not
    /// upload.
    fn themed(
        &mut self,
        renderer: &mut GlesRenderer,
        icon: CursorIcon,
        pixels: i32,
        location: Point<f64, Logical>,
        scale: f64,
    ) -> Option<Element> {
        // Taken out by value rather than held as a borrow, so that the `Said`
        // below is still reachable. The buffer is an `Arc` and an id behind
        // that derive, so the clone is a refcount and shares the imported
        // texture with the copy the theme keeps — not a second image.
        let (size, hotspot, buffer) = {
            let ready = self.ready(icon, pixels)?;
            (ready.size, ready.hotspot, ready.buffer.clone())
        };

        // `scale` is divided by below, and a division is not a place to take an
        // output's word for anything: a zero or a NaN would make the quotient
        // infinite and the cast saturate to a two-billion-pixel logical size.
        // `theme::pixels` guards its own multiplication for the same reason.
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };

        // Both in device pixels, and the hotspot is already in the image's own
        // pixels, so it is subtracted *after* the scale rather than through
        // it — unlike the QML pointer's, which is logical because it is a
        // constant we wrote. Getting this backwards puts every I-beam and
        // every resize arrow a few pixels off what it points at, which is
        // maddening to use and almost impossible to see in a screenshot.
        let position = (
            location.x * scale - f64::from(hotspot.0),
            location.y * scale - f64::from(hotspot.1),
        );

        // Drawn one device pixel per image pixel, which is why the logical
        // size below is the image's size divided back out by the scale rather
        // than `self.settings.size`. A theme has the sizes its author drew: ask
        // for 48 from a theme that only has 32 and this draws the 32 at its own
        // size instead of stretching it, which is what libxcursor, wlroots and
        // every other consumer of these files do.
        //
        // It is also what keeps the hardware cursor plane reachable *when the
        // arithmetic comes out even*, which is the honest version of that
        // claim: `copy_element_to_cursor_bo` refuses any element whose source
        // and drawn sizes disagree, and `round(edge / scale) * scale` is `edge`
        // exactly at integer scales and at any fractional scale the theme has a
        // matching size for. A 32-pixel image on a 1.5x output is the case
        // where it does not come out even, and there the pointer composites
        // rather than flying on the plane — a cost, not a fault, and visible
        // only as a slightly busier frame.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "an image edge is at most SIZES.end() * MAX_SCALE; see theme::pixels"
        )]
        let logical = |edge: i32| ((f64::from(edge) / scale).round() as i32).max(1);
        let source = Rectangle::from_size((f64::from(size.0), f64::from(size.1)).into());
        let uploaded = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            position,
            &buffer,
            None,
            Some(source),
            Some((logical(size.0), logical(size.1)).into()),
            Kind::Cursor,
        );
        match uploaded {
            Ok(element) => {
                self.theming.worked();
                Some(Element::Chrome(element))
            }
            Err(err) => {
                self.theming.once(|| {
                    tracing::warn!(
                        ?err,
                        cursor = icon.name(),
                        "could not upload a themed cursor; falling back to Solium's own pointer. \
                         Said once until one uploads again"
                    );
                });
                None
            }
        }
    }

    /// The themed cursor for `icon` at `pixels`, or nothing.
    ///
    /// **This is the whole of the decision between the two pointers**, and it
    /// is its own function because it needs no renderer: loading a theme and
    /// turning an xcursor file into a `MemoryRenderBuffer` is a directory walk,
    /// a file read and a memcpy. So the case that matters most here — a
    /// machine with no cursor themes installed ending at Solium's own pointer
    /// rather than at nothing — is assertable in a unit test with no GPU, no
    /// Qt and no theme on disk, and the tests below assert it through this
    /// exact call rather than through a re-implementation of it.
    ///
    /// Which name to ask the theme for is [`shape`]'s answer and not this
    /// function's: a shape names a *meaning*, an xcursor theme is a directory
    /// of files named in X11's vocabulary, and the two do not line up
    /// one-for-one. So this walks the chain `shape::resolve` produces — the
    /// w3c name the client used, then every legacy spelling — and only then
    /// looks the winner up. A theme with `hand2` and no `pointer` is ordinary
    /// rather than broken, and asking for one name would make most themes on
    /// disk look as though they had almost no cursors at all.
    fn ready(&mut self, icon: CursorIcon, pixels: i32) -> Option<&theme::Ready> {
        self.load();
        let Loaded::Theme(found) = &mut self.loaded else {
            return None;
        };
        // Resolved to a `&'static str` first, so the borrow the predicate
        // holds on `found` is over before the lookup needs its own.
        let name = shape::resolve(icon, |candidate| found.has(candidate, pixels))?;
        found.ready(name, pixels)
    }

    /// Look for the configured theme, once. See [`Loaded`].
    fn load(&mut self) {
        if !matches!(self.loaded, Loaded::Unasked) {
            return;
        }
        let Some(name) = self.settings.theme.clone() else {
            // Not a failure and not worth a line: no theme configured and none
            // in the environment is the default, and the QML pointer is what
            // it means.
            self.loaded = Loaded::Missing;
            return;
        };
        match theme::Theme::load(&name) {
            Some(found) => {
                tracing::info!(theme = %name, size = self.settings.size, "cursor theme");
                self.loaded = Loaded::Theme(Box::new(found));
            }
            None => {
                // Loud, once, because this is a *named* theme that is not
                // there — someone asked for it and is about to wonder why the
                // pointer does not match the rest of their desktop. Loud and
                // not fatal: the line below says what is drawn instead, and
                // what is drawn instead is a visible pointer.
                tracing::warn!(
                    theme = %name,
                    "no cursor theme by that name is installed; drawing Solium's own pointer"
                );
                self.loaded = Loaded::Missing;
            }
        }
    }

    /// Our own arrow, built the first time it is needed.
    ///
    /// Built lazily because a compositor that cannot start QML should still
    /// run — badly, with no pointer, but still be escapable — rather than fail
    /// to launch on a machine where the display is the only way to see why.
    pub(crate) fn art(&mut self) -> Option<&mut Cursor> {
        if self.art.is_none() && !self.unavailable {
            match Cursor::new(self.settings.size) {
                Ok(cursor) => self.art = Some(cursor),
                Err(err) => {
                    tracing::error!(?err, "no cursor: the pointer will be invisible");
                    self.unavailable = true;
                }
            }
        }
        self.art.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use smithay::input::pointer::{CursorIcon, CursorImageStatus};

    use super::{KEPT, Kept, Loaded, Pointer, theme};

    /// Each source replaces the last whole, so nothing of the previous one
    /// survives.
    ///
    /// **The switch the issue is explicit about**: a toolkit that names `text`
    /// over its entry field and then hands over its own bitmap for a drag —
    /// and back again — must show exactly what it last asked for. The
    /// mechanism is that [`Pointer::show`] assigns rather than merges, and the
    /// value of pinning it is that it is the kind of invariant that survives
    /// right up until somebody adds a "keep the old one if the new one is
    /// unavailable" branch, which is how a stale pointer gets written.
    ///
    /// `Hidden` stands in for the client-pixels source here: it is the arm of
    /// `wl_pointer.set_cursor` that carries no surface, and a `WlSurface`
    /// cannot be conjured without a display — standing one up inside the test
    /// binary is the process-global-state trap that has aborted this suite
    /// before. What the substitution costs is real and small: it exercises
    /// every transition except the one whose payload is a surface.
    #[test]
    fn each_source_replaces_the_last_whole() {
        let mut pointer = Pointer::default();
        assert!(
            matches!(
                pointer.showing(),
                CursorImageStatus::Named(CursorIcon::Default)
            ),
            "a fresh pointer is the compositor's own arrow"
        );

        // A client names a shape.
        pointer.show(CursorImageStatus::Named(CursorIcon::Text));
        assert!(matches!(
            pointer.showing(),
            CursorImageStatus::Named(CursorIcon::Text)
        ));

        // The same client names a different one; the I-beam is gone.
        pointer.show(CursorImageStatus::Named(CursorIcon::Pointer));
        assert!(matches!(
            pointer.showing(),
            CursorImageStatus::Named(CursorIcon::Pointer)
        ));

        // It sets its own cursor instead -- here, to nothing at all.
        pointer.show(CursorImageStatus::Hidden);
        assert!(
            matches!(pointer.showing(), CursorImageStatus::Hidden),
            "a named shape outlived the client setting its own cursor"
        );

        // And back to naming a shape, which is the direction a stale pointer
        // would show up in.
        pointer.show(CursorImageStatus::Named(CursorIcon::Grabbing));
        assert!(matches!(
            pointer.showing(),
            CursorImageStatus::Named(CursorIcon::Grabbing)
        ));

        // The compositor takes it back when the pointer leaves the client.
        pointer.show(CursorImageStatus::default_named());
        assert!(matches!(
            pointer.showing(),
            CursorImageStatus::Named(CursorIcon::Default)
        ));
    }

    /// Reading the pointer twice without a write in between is the same
    /// answer.
    ///
    /// [`Pointer::showing`] takes `&mut self` because it can downgrade a dead
    /// surface, which makes "is it a getter" a fair question to have an answer
    /// to: for every status that is not a surface it must be one, or the
    /// pointer would flicker between frames with nothing having changed.
    #[test]
    fn reading_the_pointer_does_not_change_it() {
        let mut pointer = Pointer::default();
        pointer.show(CursorImageStatus::Named(CursorIcon::NwseResize));
        let first = pointer.showing();
        let second = pointer.showing();
        assert!(matches!(
            (first, second),
            (
                CursorImageStatus::Named(CursorIcon::NwseResize),
                CursorImageStatus::Named(CursorIcon::NwseResize)
            )
        ));
    }

    /// A shape no theme can supply still ends at a *drawable* pointer, with no
    /// theme installed at all.
    ///
    /// The end of the chain the issue asks for, asserted at the end rather
    /// than in the middle: `shape::resolve` turns an unavailable shape into
    /// the theme's arrow, and when there is no theme even that fails — at
    /// which point `Pointer::element` falls through to the QML pointer.
    /// `ready` returning `None` is precisely what sends it there, and what
    /// must never happen is `ready` returning `None` *and* the caller having
    /// nowhere to go.
    ///
    /// `ZoomIn` because hardly any theme on disk has it, so it is the
    /// realistic unavailable shape rather than a contrived one.
    #[test]
    fn an_unavailable_shape_still_reaches_a_drawable_pointer() {
        let mut pointer = Pointer::default();
        pointer.configure(
            &theme::Configured {
                theme: Some(theme::NOT_INSTALLED.to_owned()),
                size: Some(24),
            },
            &theme::Environment::default(),
        );
        for icon in [
            CursorIcon::ZoomIn,
            CursorIcon::ContextMenu,
            CursorIcon::Text,
        ] {
            assert!(
                pointer.ready(icon, 24).is_none(),
                "{} produced a themed cursor with no theme installed",
                icon.name()
            );
        }
        assert!(
            matches!(pointer.loaded, Loaded::Missing),
            "and the missing theme was not left to be looked for again per shape"
        );
    }

    /// A configured theme that is not installed must end at Solium's own
    /// pointer, and not at nothing.
    ///
    /// **This is the one that would have been a black screen.** The QML
    /// pointer is the floor of this module — the header says why, and it is
    /// the same reason the module exists at all — so the failure to guard
    /// against is not "the theme did not load" but "the theme did not load and
    /// nothing was drawn instead". `Pointer::element` has exactly two arms and
    /// `ready` returning `None` is what sends it to the second; asserting on
    /// that call is asserting on the real branch rather than on a copy of it.
    ///
    /// No GPU, no Qt and no theme on disk are needed to say this, which is the
    /// point: a machine with no cursor themes installed at all is precisely
    /// the machine this has to be true on.
    #[test]
    fn a_theme_that_will_not_load_falls_back_to_our_own_pointer() {
        let mut pointer = Pointer::default();
        pointer.configure(
            &theme::Configured {
                theme: Some(theme::NOT_INSTALLED.to_owned()),
                size: Some(32),
            },
            &theme::Environment::default(),
        );
        assert_eq!(pointer.settings.size, 32, "the size is still honoured");
        assert!(
            pointer.ready(CursorIcon::Default, 32).is_none(),
            "a theme that is not installed produced a themed cursor"
        );
        assert!(
            matches!(pointer.loaded, Loaded::Missing),
            "and it was not left to be looked for again on the next frame"
        );
    }

    /// Configuring no theme at all is the default, and it is also the QML
    /// pointer — not an error, and not a reason to go looking on disk.
    #[test]
    fn no_theme_configured_is_our_own_pointer() {
        let mut pointer = Pointer::default();
        pointer.configure(
            &theme::Configured::default(),
            &theme::Environment::default(),
        );
        assert_eq!(pointer.settings.theme, None);
        assert!(pointer.ready(CursorIcon::Default, 24).is_none());
    }

    /// And the size that reaches the QML pointer is the configured one times
    /// the output scale.
    ///
    /// `Cursor::element` cannot be called without Qt and a renderer, so what
    /// is pinned is the arithmetic it does on its first line — through the
    /// same function, so a change to one is a change to both. At 1x the bug
    /// this guards is invisible, which is why the assertion that matters is
    /// the 2x one.
    ///
    /// **Read the name for exactly what it says, which is less than it
    /// sounds.** This asserts that the right *number* is computed. It says
    /// nothing about whether `cursor.qml` then draws anything that size, and
    /// for the whole of #81 it did not: the arrow was in a hard-coded 24-unit
    /// item, so a configured 48 padded the buffer and a configured 16 clipped
    /// the arrow's tail, and this test passed through both. What it takes to
    /// ask the other question is a real Qt and a real buffer — see "the QML
    /// pointer is drawn at the size it is given" in `dev/wirecheck`, which
    /// measures the drawn extent and is where that claim now lives.
    #[test]
    fn the_configured_size_reaches_the_qml_pointer_scaled() {
        let mut pointer = Pointer::default();
        pointer.configure(
            &theme::Configured {
                theme: None,
                size: Some(32),
            },
            &theme::Environment::default(),
        );
        assert_eq!(theme::pixels(pointer.settings.size, 1.0), 32);
        assert_eq!(theme::pixels(pointer.settings.size, 2.0), 64);
    }

    /// A reload that changed nothing asks for no frame, and one that changed
    /// the pointer asks for one.
    ///
    /// The answer is what `Command::Cursor` turns into damage. Both backends
    /// draw on damage and a stationary pointer produces none, so a
    /// `super+shift+r` that changed only the cursor size would otherwise show
    /// the new pointer at the next unrelated redraw — while trying a size out,
    /// that is when the mouse is next jiggled. The other half matters as much:
    /// a reload that changed a keybinding must not schedule a frame, or every
    /// reload costs a full composite of every output for nothing.
    ///
    /// Settled with a first call rather than asserted against
    /// `Pointer::default`, which resolves from this machine's real environment
    /// and so is not a known value in a test.
    #[test]
    fn only_a_reload_that_changed_the_pointer_asks_for_a_frame() {
        let mut pointer = Pointer::default();
        let environment = theme::Environment::default();
        let configured = theme::Configured {
            theme: None,
            size: Some(32),
        };
        pointer.configure(&configured, &environment);
        assert!(
            !pointer.configure(&configured, &environment),
            "a reload that changed nothing at all asked for a frame"
        );
        assert!(
            pointer.configure(
                &theme::Configured {
                    theme: None,
                    size: Some(48),
                },
                &environment
            ),
            "a reload that changed the pointer's size did not ask for a frame"
        );
    }

    /// A theme that was not installed last time is looked for again on a
    /// reload, with the configuration unchanged.
    ///
    /// **`config.lua`'s own words are "trying a theme out is
    /// `super+shift+r`"**, and the obvious way to try one out is to name it,
    /// find the pointer unchanged, install it, and press the key again. That
    /// did nothing: [`Loaded::Missing`] is a settled answer — rightly, because
    /// re-walking the icon search path per *frame* is a syscall storm — but
    /// `configure` returned early on unchanged settings, so nothing ever put it
    /// back. A reload is a keypress, not a frame.
    #[test]
    fn a_reload_looks_again_for_a_theme_that_was_not_installed() {
        let mut pointer = Pointer::default();
        let environment = theme::Environment::default();
        let configured = theme::Configured {
            theme: Some(theme::NOT_INSTALLED.to_owned()),
            size: Some(24),
        };
        pointer.configure(&configured, &environment);
        assert!(pointer.ready(CursorIcon::Default, 24).is_none());
        assert!(
            matches!(pointer.loaded, Loaded::Missing),
            "the missing theme was not settled, so there is nothing to retry"
        );

        assert!(
            pointer.configure(&configured, &environment),
            "the same config.lua, pressed again after installing the theme, was a no-op"
        );
        assert!(
            matches!(pointer.loaded, Loaded::Unasked),
            "and the theme was not looked for again"
        );
    }

    /// But a session with *no* theme named has nothing to look for, and must
    /// not be put back to `Unasked` on every reload.
    ///
    /// The negative half of the test above. `load` reaches `Missing` without
    /// touching the disk when there is no name, so retrying it is pure churn —
    /// and, through the answer, a frame on every `super+shift+r` for ever.
    #[test]
    fn a_reload_with_no_theme_named_looks_for_nothing() {
        let mut pointer = Pointer::default();
        let environment = theme::Environment::default();
        let configured = theme::Configured::default();
        pointer.configure(&configured, &environment);
        assert!(pointer.ready(CursorIcon::Default, 24).is_none());
        assert!(matches!(pointer.loaded, Loaded::Missing));
        assert!(
            !pointer.configure(&configured, &environment),
            "a reload with no cursor theme configured asked for a frame"
        );
        assert!(matches!(pointer.loaded, Loaded::Missing));
    }

    /// The pointer's own use of the shared cache, at the pointer's own cap.
    ///
    /// These four are unchanged in what they assert. [`Kept`] moved to
    /// `qml/paint.rs` when the window frames became its second caller, and
    /// generalising it over the key and the cap changed how it is named and
    /// constructed here and nothing else: `Kept<u32>` became
    /// `Kept<i32, u32>`, `Kept::default()` became `Kept::keeping(KEPT)`, the
    /// `clear`-then-`has` pair became the one `current` call the compositor now
    /// makes, and `held.len()` became `count()`.
    ///
    /// The pointer is built for **every** output on every frame -- `render.rs`
    /// says so in as many words and argues for it -- so a desktop with a 1x and
    /// a 2x monitor asks for 24 and then 48, alternating, for as long as the
    /// session lasts. Before this cache each swing re-rasterised the scene; on
    /// the GPU path it was a GBM allocation, an EGLImage, a full Qt re-render
    /// and an import, twice a frame, for ever.
    ///
    /// The claim is that it costs two misses in total rather than two a frame,
    /// and that is the thing worth a test: it is arithmetic, and it does not
    /// need Qt.
    #[test]
    fn two_monitors_at_two_scales_cost_two_fills_and_then_nothing() {
        let mut kept: Kept<i32, u32> = Kept::keeping(KEPT);
        let mut fills = 0;
        for frame in 0..100 {
            for edge in [24, 48] {
                if !kept.current(edge, false) {
                    fills += 1;
                    kept.push(edge, frame);
                }
            }
        }
        assert_eq!(fills, 2, "the pointer was re-drawn after the first frame");
        assert!(kept.get(24).is_some() && kept.get(48).is_some());
    }

    /// And whatever invalidates the pointer has to invalidate *every* size at
    /// once, or the monitor that is not being looked at keeps the old pointer
    /// for ever.
    ///
    /// Read this for exactly what it is: a `current` call told the scene is
    /// fresh drops every entry. It is **not** end-to-end coverage of a theme
    /// change, and there is no such coverage anywhere — nothing in the
    /// compositor can currently trigger one at all, for the reasons set out at
    /// the `needs_render` call in `Cursor::element`. So this passing says the
    /// container forgets when it is told to. It says nothing about whether
    /// anything ever tells it.
    #[test]
    fn clearing_invalidates_every_size() {
        let mut kept: Kept<i32, u32> = Kept::keeping(KEPT);
        kept.push(24, 1);
        kept.push(48, 1);
        assert!(!kept.current(24, true), "the size asked for survived");
        assert!(
            !kept.current(48, false),
            "the other monitor's size survived"
        );
    }

    /// The cap is a guard against a script animating an output's scale, which
    /// would otherwise mint a buffer per step and keep every one.
    #[test]
    fn the_cap_evicts_the_oldest() {
        let mut kept: Kept<i32, u32> = Kept::keeping(KEPT);
        for edge in 1..=(KEPT as i32 + 2) {
            kept.push(edge, 0);
        }
        assert_eq!(kept.count(), KEPT);
        assert!(!kept.current(1, false), "the oldest size was kept");
        assert!(
            kept.current(KEPT as i32 + 2, false),
            "the newest size was dropped"
        );
    }

    /// Asking for a size already held must not grow the list, or a pointer that
    /// never changes size still evicts everything else eventually.
    #[test]
    fn re_pushing_a_size_replaces_it() {
        let mut kept: Kept<i32, u32> = Kept::keeping(KEPT);
        kept.push(24, 1);
        kept.push(24, 2);
        assert_eq!(kept.count(), 1);
        assert_eq!(kept.get(24).copied(), Some(2));
    }
}
