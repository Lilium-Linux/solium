//! The presentation transform, and the one animation clock.
//!
//! The idea the rest of the compositor rests on: a window is *drawn* wherever
//! its presentation says, not where the layout put it. Overview, the app
//! switcher, peek and the icon→window genie then stop being five features and
//! become one operation with five different targets — see
//! `docs/architecture.md`.
//!
//! Two rules that must not be relaxed:
//!
//! * **A transform never changes real geometry.** Leaving a mode restores the
//!   layout exactly, because the layout was never touched. Everything here
//!   takes real geometry as an argument and never writes it back.
//! * **There is one clock**, ticked once per frame by the render loop. Not one
//!   per window, per subsystem or per script — separate clocks are how modes
//!   end up animating at subtly different speeds.
//!
//! The *timing* lives in `solium-animation` and the *shapes* in
//! `solium-effects`, both crates with no compositor in them, so curves,
//! springs and vertex deformations can be tested and previewed without
//! launching this. What stays here is the part that needs a compositor: which
//! rectangle a window is travelling between, and which rectangle an effect is
//! aimed at this frame.

use std::{cell::RefCell, time::Duration};

use crate::mat4::Mat4;

use crate::pane::Pane;
use smithay::utils::{Logical, Point, Rectangle};
pub(crate) use solium_animation::Curve;
use solium_animation::{Animation, lerp};

/// A monotonic clock, sampled once per frame.
///
/// Every animation reads the same instant, so two windows started together
/// stay together even if a frame takes longer than expected.
#[derive(Debug)]
pub(crate) struct Clock {
    start: std::time::Instant,
}
impl Clock {
    pub(crate) fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }

    /// The time now.
    ///
    /// Read through, not sampled and cached. It used to be cached and refreshed
    /// once per frame, so that everything animating in one frame agreed about
    /// when "now" was — which is a real property, and it is kept where it
    /// matters by reading this once into a local at the top of a frame and
    /// passing that around.
    ///
    /// What a cached clock cannot survive is a compositor that only draws when
    /// something changed. On a still screen no frame is drawn, so the sample
    /// goes stale for exactly as long as the screen has been still, and a
    /// binding pressed then starts its animation at a time minutes in the past.
    /// The first frame that draws it evaluates it long past its end: no
    /// animation, just the result. It depended on how long you had left the
    /// screen alone, which is not a thing anyone would think to test.
    ///
    /// It could be fixed by sampling in every event source instead. That is the
    /// same discipline written down six times, and the seventh source added
    /// later would not know about it.
    pub(crate) fn now(&self) -> Duration {
        self.start.elapsed()
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

/// A logical rectangle from plain numbers.
///
/// `Rectangle::new` takes a typed `Point` and `Size`, which is the right
/// discipline for a compositor and pure noise at the call sites doing the
/// arithmetic. The conversion happens here and nowhere else.
pub(crate) fn logical(loc: (f64, f64), size: (f64, f64)) -> Rectangle<f64, Logical> {
    Rectangle::new(loc.into(), size.into())
}

/// A rectangle in the form `solium-effects` takes.
///
/// The one seam between the compositor's typed geometry and a crate that has
/// no dependencies and therefore no Smithay. It is here rather than in the
/// crate because the conversion is the compositor's problem in both
/// directions: nothing in `crates/effects` should be able to name `Logical`.
pub(crate) fn for_effects(rect: Rectangle<f64, Logical>) -> solium_effects::Rect {
    solium_effects::Rect::new(rect.loc.x, rect.loc.y, rect.size.w, rect.size.h)
}

/// What a deformation is aimed at.
///
/// **An identity, not a rectangle**, and that is the whole of it. A dock icon
/// moves: it slides as its neighbours open and close, and it is itself being
/// animated by the same clock. A rectangle read out of a Lua table when the
/// binding was pressed aims at where that icon was when the animation started,
/// so a 500 ms genie lands where the icon used to be — the same drift the
/// design rules already forbid for mirrors.
///
/// So the identity is carried and the compositor resolves it once per frame,
/// in `Solium::aimed_at`. What can be named is deliberately small: a fixed
/// place on screen, which does not move and is honest about it, and a pane,
/// which does. `sol.surface` is not here yet because a surface is not
/// addressable yet — that is the next stage of the plan, "Address: surfaces
/// and groups become transformable", and this enum is the seam it lands in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Anchor {
    /// A fixed rectangle in the global logical space. Snapshotted, because
    /// there is nothing to resolve: the caller is naming a *place* — the
    /// bottom edge of a monitor, a corner — rather than a thing.
    Rect(Rectangle<f64, Logical>),
    /// A pane, by the id scripts hold it as. Resolved to where it is being
    /// *drawn*, so a genie aimed at a window that is itself animating follows
    /// it rather than its layout slot.
    Pane(u64),
}

/// A deformation a rectangle cannot express, and what it is aimed at.
///
/// The shape itself lives in `solium-effects`, which is a crate precisely so
/// that adding *fold*, *curl* or *page-turn* is a file with unit tests and a
/// preview slider rather than another arm of an enum in here. What stays on
/// this side is the half that needs a compositor: the anchor, and resolving it
/// every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Deform {
    /// The named vertex function and its parameters.
    pub(crate) effect: solium_effects::Deform,
    /// Where it is pulling the window to, or out of.
    pub(crate) anchor: Anchor,
}

/// A deform with its anchor resolved: what the renderer can actually draw.
///
/// Separate from [`Deform`] so the resolution cannot be forgotten — there is
/// no way to hand `warp::mesh` an unresolved anchor, because it does not take
/// one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Aimed {
    pub(crate) effect: solium_effects::Deform,
    /// The far end of the morph, in logical coordinates, this frame.
    pub(crate) to: Rectangle<f64, Logical>,
}

impl Deform {
    /// The same deform, doing nothing.
    fn at_rest(self) -> Self {
        Self {
            effect: self.effect.at_rest(),
            anchor: self.anchor,
        }
    }

    /// Blend two deforms, either of which may be absent.
    ///
    /// Absent means "not deformed", which for a genie is progress 0 -- so a
    /// script animating into one does not have to name the starting state, and
    /// clearing one animates back out of it.
    fn blend(from: Option<Self>, to: Option<Self>, progress: f64) -> Option<Self> {
        match (from, to) {
            (None, None) => None,
            (Some(one), None) => Some(one.mix(one.at_rest(), progress)),
            (None, Some(other)) => Some(other.at_rest().mix(other, progress)),
            (Some(one), Some(other)) => Some(one.mix(other, progress)),
        }
    }

    /// Blend towards another deform.
    ///
    /// The parameters blend; the anchor does not. There is no identity half
    /// way between one dock icon and another, and a rectangle interpolated
    /// between two of them is a place neither of them is — which is the
    /// snapshot problem back again, wearing a blend. The destination's anchor
    /// is what the whole animation aims at, which is what a script asking for
    /// one means.
    fn mix(self, other: Self, progress: f64) -> Self {
        Self {
            effect: self.effect.mix(other.effect, progress),
            anchor: other.anchor,
        }
    }
}

/// How a window is drawn for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub(crate) rect: Rectangle<f64, Logical>,
    pub(crate) opacity: f32,
    /// A 4x4 applied about the rect's centre. Identity means flat, and flat
    /// stays on the cheap path — see
    /// `docs/spikes/2026-09-06-3d-presentation.md`.
    pub(crate) matrix: Mat4,
    /// A deformation that no rectangle and no matrix can express, such as a
    /// genie. `None` is the ordinary case and stays on the cheap path.
    pub(crate) deform: Option<Deform>,
}

impl Frame {
    /// The identity frame: drawn at real geometry, fully opaque.
    pub(crate) fn real(geometry: Rectangle<i32, Logical>) -> Self {
        Self {
            rect: geometry.to_f64(),
            opacity: 1.0,
            matrix: Mat4::IDENTITY,
            deform: None,
        }
    }

    /// Scaled about its own centre — what "smaller, in place" means, and the
    /// shape both the open animation and peek want.
    pub(crate) fn scaled(self, factor: f64) -> Self {
        let size = (self.rect.size.w * factor, self.rect.size.h * factor);
        let loc = (
            self.rect.loc.x + (self.rect.size.w - size.0) / 2.0,
            self.rect.loc.y + (self.rect.size.h - size.1) / 2.0,
        );
        Self {
            rect: logical(loc, size),
            opacity: self.opacity,
            matrix: self.matrix,
            deform: self.deform,
        }
    }

    /// The same frame, drawn through `matrix` about its own centre.
    ///
    #[expect(
        dead_code,
        reason = "scripts set a transform through sol.present; this is the \
                  builder for whatever sets one in Rust"
    )]
    pub(crate) fn with_matrix(mut self, matrix: Mat4) -> Self {
        self.matrix = matrix;
        self
    }

    pub(crate) fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    fn blend(self, other: Self, progress: f64) -> Self {
        let mix = |a: f64, b: f64| lerp(a, b, progress);
        Self {
            rect: logical(
                (
                    mix(self.rect.loc.x, other.rect.loc.x),
                    mix(self.rect.loc.y, other.rect.loc.y),
                ),
                (
                    mix(self.rect.size.w, other.rect.size.w),
                    mix(self.rect.size.h, other.rect.size.h),
                ),
            ),
            #[expect(
                clippy::cast_possible_truncation,
                reason = "opacity is a small float either way"
            )]
            opacity: lerp(f64::from(self.opacity), f64::from(other.opacity), progress) as f32,
            // Element-wise, which is not how one rotates halfway between two
            // rotations — that needs the rotation pulled apart and blended as
            // an angle. It is right for the cases the compositor animates:
            // from identity into a transform, or between two of the same kind.
            // A script animating between two unrelated rotations gets
            // something that passes through a squashed middle, and should
            // animate the angle instead and rebuild the matrix per frame.
            matrix: self.matrix.blend(other.matrix, progress),
            deform: Deform::blend(self.deform, other.deform, progress),
        }
    }
}

/// An in-flight transform: where the window was, where it is going, and when.
///
/// The when is `solium-animation`'s problem; this only knows the two ends.
#[derive(Clone, Copy, Debug)]
struct Transform {
    from: Frame,
    to: Frame,
    animation: Animation,
    /// Drop the transform when it lands, so the window goes back to being drawn
    /// at real geometry with no per-frame cost. Set when leaving a mode.
    release: bool,
}

impl Transform {
    fn frame(&self, now: Duration) -> Frame {
        self.from.blend(self.to, self.animation.progress(now))
    }

    fn finished(&self, now: Duration) -> bool {
        self.animation.done(now)
    }
}

/// A pane's in-flight transform, and whether it has ever been on screen.
///
/// This lives on the pane rather than on the client's window for two reasons.
/// A pane can be on screen before it has a window at all, so there would be
/// nowhere to keep it; and it has to survive adoption untouched, or a window
/// would snap the instant its application arrived — which is the seam this
/// whole refactor exists to remove.
///
/// Interior mutability throughout, because every path that draws holds its
/// pane by shared reference. Opaque: the functions below are the only way in.
#[derive(Debug, Default)]
pub(crate) struct Slot {
    transform: RefCell<Option<Transform>>,
    shown: std::cell::Cell<bool>,
}

/// Run `f` against a pane's transform, if nothing is already inside it.
///
/// `try_borrow_mut` rather than `borrow_mut`: a re-entrant borrow would panic,
/// and a compositor panic takes the session with it. Losing one frame of an
/// animation is the better failure.
fn with_slot<T>(pane: &Pane, f: impl FnOnce(&mut Option<Transform>) -> T) -> Option<T> {
    match pane.drawn().transform.try_borrow_mut() {
        Ok(mut current) => Some(f(&mut current)),
        Err(_) => {
            tracing::warn!("transform slot was already borrowed, skipping");
            None
        }
    }
}

/// Draw `pane` at `to`, animating from wherever it is being drawn right now.
///
/// Animating from the *current* frame rather than from real geometry is what
/// makes re-entering a mode mid-animation continuous instead of a snap.
pub(crate) fn present(
    pane: &Pane,
    real: Rectangle<i32, Logical>,
    to: Frame,
    now: Duration,
    duration: Duration,
    easing: Curve,
) {
    let from = frame(pane, real, now);
    with_slot(pane, |slot| {
        *slot = Some(Transform {
            from,
            to,
            animation: Animation::new(now, duration, easing),
            release: false,
        });
    });
}

/// Animate back to real geometry, then stop transforming this pane.
pub(crate) fn clear(
    pane: &Pane,
    real: Rectangle<i32, Logical>,
    now: Duration,
    duration: Duration,
    easing: Curve,
) {
    let from = frame(pane, real, now);
    with_slot(pane, |slot| {
        *slot = Some(Transform {
            from,
            to: Frame::real(real),
            animation: Animation::new(now, duration, easing),
            release: true,
        });
    });
}

/// The frame to draw this window in. Real geometry when nothing is animating.
pub(crate) fn frame(pane: &Pane, real: Rectangle<i32, Logical>, now: Duration) -> Frame {
    with_slot(pane, |slot| {
        slot.map_or_else(|| Frame::real(real), |transform| transform.frame(now))
    })
    .unwrap_or_else(|| Frame::real(real))
}

/// Retire finished transforms. Returns whether this window is still animating.
pub(crate) fn settle(pane: &Pane, now: Duration) -> bool {
    with_slot(pane, |slot| {
        let Some(transform) = *slot else {
            return false;
        };
        if !transform.finished(now) {
            return true;
        }
        if transform.release {
            *slot = None;
        }
        false
    })
    .unwrap_or(false)
}

/// Claim the first-show moment, returning whether this call won it.
///
/// The moment is the first commit that carries a buffer, not the map request:
/// before that commit the client has not been told its size, so there is no
/// geometry either to place it at or to animate to. Both happen here, once.
pub(crate) fn mark_shown(pane: &Pane) -> bool {
    !pane.drawn().shown.replace(true)
}

/// Put a pane at `start` and animate it to where it actually lives.
///
/// The primitive behind every "appears from somewhere" animation. With a dock
/// icon's rectangle it is the icon-grows-into-a-window genie; with the window's
/// own rectangle shrunk a little it is an ordinary open; with a rectangle off
/// the edge of the screen it is something nobody has asked for yet. The
/// compositor does not decide which — see `docs/shell-boundary.md`.
pub(crate) fn from(
    pane: &Pane,
    real: Rectangle<i32, Logical>,
    start: Frame,
    now: Duration,
    duration: Duration,
    easing: Curve,
) {
    with_slot(pane, |slot| {
        *slot = Some(Transform {
            from: start,
            to: Frame::real(real),
            animation: Animation::new(now, duration, easing),
            // Released on arrival: an opened window is an ordinary window.
            release: true,
        });
    });
}

/// The animation a window plays when it first has something to show.
///
/// The fallback for when no script has an opinion. `lua/open.lua` normally
/// does.
pub(crate) fn open(pane: &Pane, real: Rectangle<i32, Logical>, now: Duration) {
    let target = Frame::real(real);
    with_slot(pane, |slot| {
        *slot = Some(Transform {
            from: target.scaled(0.88).with_opacity(0.0),
            to: target,
            animation: Animation::new(now, Duration::from_millis(220), Curve::OutBack),
            release: true,
        });
    });
}

/// How long a window takes to leave. The compositor waits this out before
/// telling the client to close, so the animation runs on a window that is
/// still alive and still able to paint.
pub(crate) const CLOSING: Duration = Duration::from_millis(190);

/// Animate a window out: away from the viewer, and gone.
///
/// The reverse of `open`, and deliberately not released when it lands -- the
/// window has to stay invisible for the moment between the animation ending
/// and the client acting on the close it is about to be sent, or it would
/// reappear at full size for a frame or two.
pub(crate) fn close(pane: &Pane, real: Rectangle<i32, Logical>, now: Duration) {
    let from = frame(pane, real, now);
    with_slot(pane, |slot| {
        *slot = Some(Transform {
            from,
            to: from.scaled(0.86).with_opacity(0.0),
            animation: Animation::new(now, CLOSING, Curve::InOutQuad),
            release: false,
        });
    });
}

/// Map a point in drawn space into a window's own coordinates.
///
/// The inverse of the transform, and the reason hit-testing keeps working in a
/// mode: a thumbnail must be clickable where it is drawn, and the client must
/// still receive coordinates in its own scale.
pub(crate) fn to_window_space(
    frame: Frame,
    real: Rectangle<i32, Logical>,
    point: Point<f64, Logical>,
) -> Point<f64, Logical> {
    let scale_x = if frame.rect.size.w == 0.0 {
        1.0
    } else {
        f64::from(real.size.w) / frame.rect.size.w
    };
    let scale_y = if frame.rect.size.h == 0.0 {
        1.0
    } else {
        f64::from(real.size.h) / frame.rect.size.h
    };
    Point::from((
        (point.x - frame.rect.loc.x) * scale_x + f64::from(real.loc.x),
        (point.y - frame.rect.loc.y) * scale_y + f64::from(real.loc.y),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_advances_without_being_asked() {
        // It was a sample, refreshed once per *drawn* frame. Drawing waits for
        // something to have changed, so on a still screen no frame is drawn and
        // the sample freezes for as long as the screen is left alone. An
        // animation started against a frozen clock begins minutes in the past
        // and is already finished the first time anything evaluates it: the
        // window arrives at its destination with no animation at all.
        //
        // Which made it look intermittent -- it worked if you had been doing
        // something, and did nothing if you had not.
        let clock = Clock::new();
        let first = clock.now();
        std::thread::sleep(Duration::from_millis(2));
        assert!(
            clock.now() > first,
            "the clock must advance on its own, or an animation started \
             between two frames begins at the wrong instant"
        );
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn a_transform_starts_where_it_was_and_lands_on_its_target() {
        let from = Frame::real(rect(0, 0, 100, 100));
        let to = Frame::real(rect(500, 400, 50, 50));
        let transform = Transform {
            from,
            to,
            animation: Animation::new(
                Duration::from_millis(1000),
                Duration::from_millis(200),
                Curve::OutCubic,
            ),
            release: false,
        };

        assert_eq!(transform.frame(Duration::from_millis(1000)), from);
        assert_eq!(transform.frame(Duration::from_millis(1200)), to);
        // Past the end it stays landed rather than overshooting off-screen.
        assert_eq!(transform.frame(Duration::from_millis(5000)), to);
        assert!(transform.finished(Duration::from_millis(1200)));
        assert!(!transform.finished(Duration::from_millis(1100)));
    }

    #[test]
    fn a_zero_length_transform_is_immediately_at_its_target() {
        let to = Frame::real(rect(10, 10, 20, 20));
        let transform = Transform {
            from: Frame::real(rect(0, 0, 100, 100)),
            to,
            animation: Animation::new(Duration::from_millis(500), Duration::ZERO, Curve::OutCubic),
            release: false,
        };
        assert_eq!(transform.frame(Duration::from_millis(500)), to);
    }

    #[test]
    fn scaling_is_about_the_centre() {
        let frame = Frame::real(rect(0, 0, 100, 100)).scaled(0.5);
        assert_eq!(frame.rect.loc.x, 25.0);
        assert_eq!(frame.rect.loc.y, 25.0);
        assert_eq!(frame.rect.size.w, 50.0);
    }

    #[test]
    fn hit_testing_inverts_the_transform() {
        let real = rect(0, 0, 400, 300);
        // Drawn at half size, offset to the right.
        let drawn = Frame {
            rect: logical((500.0, 100.0), (200.0, 150.0)),
            opacity: 1.0,
            matrix: Mat4::IDENTITY,
            deform: None,
        };
        // The centre of the thumbnail is the centre of the window.
        let mapped = to_window_space(drawn, real, Point::from((600.0, 175.0)));
        assert!((mapped.x - 200.0).abs() < 1e-6, "got {}", mapped.x);
        assert!((mapped.y - 150.0).abs() < 1e-6, "got {}", mapped.y);
    }
}
