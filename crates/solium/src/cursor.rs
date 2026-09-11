//! The pointer.
//!
//! Two cases, and both have to work. A client that sets its own cursor gets it
//! drawn — an I-beam over text, a resize arrow on an edge — and everything else
//! gets ours, drawn from QML through the same design system as the window
//! frames, so the pointer belongs to the same look as the rest.
//!
//! Nested, none of this existed: the host compositor drew the cursor over our
//! window and we never had to think about it. On the hardware nothing else
//! will, and an invisible pointer is not a cosmetic problem — it is
//! indistinguishable from input being dead, which is exactly how it was
//! reported the first time this ran on a real screen.

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
    input::pointer::CursorImageStatus,
    utils::{Logical, Point, Rectangle, Transform},
};

use crate::{
    qml::{
        self,
        paint::{Gpu, Kept, Said},
    },
    render::Element,
};

/// How big the cursor image is, in logical pixels.
const SIZE: i32 = 24;

/// Where the point of the arrow is within that image.
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
/// Four rather than the two `qml::paint`'s own [`Kept`] uses, and the reason is
/// the buffer. A pointer is 24 logical pixels square, so an entry is 2.3 KB at
/// 1x and 9.2 KB at 2x — a generous guard costs nothing here. A window frame is
/// a whole window, 3.9 MB of it on an ordinary one, and there is one per
/// window; see `KEPT` in `qml/paint.rs` for that arithmetic.
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
    pub(crate) fn new() -> Result<Self> {
        qml::start()?;
        // `SIZE` square to begin with, which at 1x is also the size it stays.
        // It is *not* a scene that is never resized, whatever its buffer being
        // one picture might suggest: 24 is 24 *logical* pixels, so the pointer
        // crossing onto a 2x monitor needs a 48-pixel one and the GPU path
        // rebinds onto a new buffer to get it. See `Cursor::element`.
        let scene = qml::Scene::for_host(&qml_path(), SIZE, SIZE, None)?;
        Ok(Self {
            scene,
            backing: if qml::on_gpu() {
                // The size the scene really is, unlike the shell surfaces:
                // there is nothing to discover about a pointer's size, so it is
                // allocated right the first time and only a new scale moves it.
                Backing::Gpu(Gpu::new((SIZE, SIZE)))
            } else {
                Backing::Memory
            },
            buffers: Kept::keeping(KEPT),
            drawing: Said::default(),
            uploading: Said::default(),
        })
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
        // 24 logical pixels, whatever the monitor is. On a 2x display that is
        // a 48-pixel image, and drawing the 24-pixel one there would leave a
        // pointer a quarter of the size it should be -- which on a HiDPI panel
        // is a pointer you cannot find, and this module exists because an
        // invisible pointer reads as input being dead.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a cursor is 24 logical pixels"
        )]
        let edge = ((f64::from(SIZE) * scale).round() as i32).max(1);

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

        // The whole buffer in its own pixels, mapped down to 24 logical
        // pixels, which the output scale takes back up to `held`. When `held`
        // is `edge` — every case but a frozen scene — those two are the same
        // number, which is also what `copy_element_to_cursor_bo` requires
        // before it will use the plane's fast path: it refuses any element
        // whose src and drawn size disagree.
        let source = Rectangle::from_size((f64::from(held), f64::from(held)).into());
        let uploaded = MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            position,
            buffer,
            None,
            Some(source),
            Some((SIZE, SIZE).into()),
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
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/cursor.qml"))
}

/// The pointer as the compositor holds it: what to show, and what to draw it
/// with.
///
/// One place, because the two are only meaningful together — a status with
/// nothing able to render it is an invisible pointer, which is the bug this
/// module exists to have fixed.
#[derive(Debug)]
pub(crate) struct Pointer {
    /// What the pointer should look like, as clients and Smithay set it.
    pub(crate) status: CursorImageStatus,
    art: Option<Cursor>,
    /// Set once QML has failed, so a broken scene costs one error and not one
    /// per frame for the life of the session.
    unavailable: bool,
}

impl Default for Pointer {
    fn default() -> Self {
        Self {
            status: CursorImageStatus::default_named(),
            art: None,
            unavailable: false,
        }
    }
}

impl Pointer {
    /// Our own arrow, built the first time it is needed.
    ///
    /// Built lazily because a compositor that cannot start QML should still
    /// run — badly, with no pointer, but still be escapable — rather than fail
    /// to launch on a machine where the display is the only way to see why.
    pub(crate) fn art(&mut self) -> Option<&mut Cursor> {
        if self.art.is_none() && !self.unavailable {
            match Cursor::new() {
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
    use super::{KEPT, Kept};

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
