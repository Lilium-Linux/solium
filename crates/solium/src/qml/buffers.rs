//! How big the buffer a GPU scene draws into is, and when it is replaced:
//! `qml.buffers` (2026-10-16).
//!
//! A dmabuf cannot change size, so a scene whose buffer is exactly its own
//! size needs a new one at every size it is drawn at: a GBM allocation, an
//! `eglCreateImageKHR`, a Qt render-target swap and a full render. A window
//! frame is sized from its window, so that was every frame of a drag on the
//! window's edge and of a layout's sweep: 2.5 ms at p50 and 3.7 ms at p95 per
//! frame on the 260 Hz TTY trace of 2026-10-15, against a 3.85 ms deadline.
//!
//! So a buffer is allocated with room to grow, in steps, and the scene is
//! drawn at its real size into the top-left of it (`qml/host.cpp`'s
//! `place_in_buffer`; `dev/wirecheck`'s "a scene in a buffer larger than
//! itself" proves Qt draws it there, one to one at the scale), and only that
//! rectangle is sampled (`paint::Gpu::element`). A new buffer is made only
//! when the scene outgrows the one it has, when that one still holds another
//! monitor's picture, or once the size has held when the buffer has become
//! far larger than the scene.
//!
//! **The policy, and why it is this one.** Each side is rounded up to a whole
//! number of `step` device pixels (128 unless configured): a drag growing a
//! window is then given a new buffer once per 128 pixels it grows rather than
//! once per frame, about a dozen a second at the fastest drag, and a buffer
//! wastes less than one step a side. A scene that shrinks keeps drawing into
//! the buffer it has; once its size holds for a frame, a buffer `shrink`
//! steps or more larger than it on a side (2 unless configured) is replaced
//! by a snug one. Two and not one, so a size wavering across a step's edge
//! (a hand on a mouse, held still) is not given a new buffer every time it
//! crosses back: a snug buffer is less than one step larger than its scene,
//! and only a scene two steps smaller than its buffer is moved.
//!
//! Pure arithmetic, so the whole policy is tested without Qt or a GPU.

use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicI32, Ordering};

/// `qml.buffers.step` unless configured, in device pixels.
/// `tests::a_scene_growing_a_pixel_a_frame_is_rebound_once_a_step`.
pub(crate) const STEP: i32 = 128;

/// `qml.buffers.shrink` unless configured, in steps.
/// `tests::a_size_wavering_across_a_steps_edge_is_not_rebound_each_time`.
pub(crate) const SHRINK: i32 = 2;

/// What `qml.buffers.step` may be: 0, a buffer exactly the scene's size and
/// a new one at every size (as before 2026-10-16), or up to 1024 pixels of
/// room a side. `tests::a_step_of_zero_rebinds_at_every_size`.
pub(crate) const STEPS: RangeInclusive<i32> = 0..=1024;

/// What `qml.buffers.shrink` may be: at least 2, or a size wavering across a
/// step's edge is rebound at every crossing
/// (`tests::a_size_wavering_across_a_steps_edge_is_not_rebound_each_time`),
/// and at most 64.
pub(crate) const SHRINKS: RangeInclusive<i32> = 2..=64;

/// `qml.buffers`: the step a buffer's sides are whole numbers of, and how
/// many steps larger than its scene a buffer may stay once the size holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Buffers {
    step: i32,
    shrink: i32,
}

impl Default for Buffers {
    fn default() -> Self {
        Self {
            step: STEP,
            shrink: SHRINK,
        }
    }
}

impl Buffers {
    /// A policy, if both numbers are within their bounds ([`STEPS`],
    /// [`SHRINKS`]).
    pub(crate) fn new(step: i32, shrink: i32) -> Option<Self> {
        (STEPS.contains(&step) && SHRINKS.contains(&shrink)).then_some(Self { step, shrink })
    }

    /// The buffer a scene of `size` device pixels is given: each side
    /// rounded up to a whole number of steps, and no larger than
    /// [`super::target::MAX_SIDE`] unless the scene itself is.
    pub(crate) fn snug(self, size: (i32, i32)) -> (i32, i32) {
        let side = |pixels: i32| {
            if self.step <= 0 {
                return pixels;
            }
            let steps = pixels.saturating_add(self.step - 1) / self.step;
            steps
                .saturating_mul(self.step)
                .min(super::target::MAX_SIDE)
                .max(pixels)
        };
        (side(size.0), side(size.1))
    }

    /// Whether `buffer` is far larger than a scene of `size` needs: on
    /// either side, `shrink` steps or more past it.
    pub(crate) fn roomy(self, buffer: (i32, i32), size: (i32, i32)) -> bool {
        let far = self.step.saturating_mul(self.shrink);
        self.step > 0 && (buffer.0 - size.0 >= far || buffer.1 - size.1 >= far)
    }
}

/// What a frame that draws a scene does with the buffer under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Draw into the buffer the scene has, at the size asked for.
    InPlace,
    /// Move the scene onto a new buffer of this size first.
    Rebind((i32, i32)),
}

/// The plan for drawing a scene at `size` device pixels.
///
/// `buffer` is the size of the buffer it is on, if it has one. `resting` is
/// whether the size is the one the picture already in that buffer was drawn
/// at, so the size has held for a frame. `occupied` is whether that buffer
/// holds a picture another monitor is still shown from (a window across two
/// monitors at two scales), which drawing into it would draw over.
pub(crate) fn plan(
    policy: Buffers,
    buffer: Option<(i32, i32)>,
    size: (i32, i32),
    resting: bool,
    occupied: bool,
) -> Plan {
    let fresh = Plan::Rebind(policy.snug(size));
    let Some(buffer) = buffer else {
        return fresh;
    };
    // A step of 0 is a buffer exactly the scene's size, so any other size is
    // a new one: the rule before 2026-10-16.
    let fits = if policy.step <= 0 {
        buffer == size
    } else {
        size.0 <= buffer.0 && size.1 <= buffer.1
    };
    if !fits || occupied || (resting && policy.roomy(buffer, size)) {
        return fresh;
    }
    Plan::InPlace
}

/// The policy this process draws with: `qml.buffers`, set once as the
/// configuration is read, before any scene is built (`tty.rs`, `winit.rs`).
static STEP_SET: AtomicI32 = AtomicI32::new(STEP);
static SHRINK_SET: AtomicI32 = AtomicI32::new(SHRINK);

/// Draw with `policy` from now on.
pub(crate) fn configure(policy: Buffers) {
    STEP_SET.store(policy.step, Ordering::Relaxed);
    SHRINK_SET.store(policy.shrink, Ordering::Relaxed);
}

/// The policy this process draws with.
pub(crate) fn configured() -> Buffers {
    Buffers {
        step: STEP_SET.load(Ordering::Relaxed),
        shrink: SHRINK_SET.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::{Buffers, Plan, plan};

    /// A window frame 850 pixels tall dragged a pixel wider (or narrower) on
    /// every frame from `from` to `to`, starting on `buffer`: the new
    /// buffers it was given, and the one it ends on.
    fn drag(
        policy: Buffers,
        from: i32,
        to: i32,
        mut buffer: Option<(i32, i32)>,
    ) -> (u32, Option<(i32, i32)>) {
        let mut rebinds = 0;
        let widths: Vec<i32> = if from <= to {
            (from..=to).collect()
        } else {
            (to..=from).rev().collect()
        };
        for width in widths {
            if let Plan::Rebind(next) = plan(policy, buffer, (width, 850), false, false) {
                rebinds += 1;
                buffer = Some(next);
            }
        }
        (rebinds, buffer)
    }

    /// **A scene growing a pixel a frame is given a new buffer once a step,
    /// not once a frame**: 501 frames of a drag from 1150 to 1650 wide make
    /// 5 buffers at the default step (the first, then at 1153, 1281, 1409
    /// and 1537), where a buffer exactly the scene's size made 501.
    #[test]
    fn a_scene_growing_a_pixel_a_frame_is_rebound_once_a_step() {
        let (rebinds, buffer) = drag(Buffers::default(), 1150, 1650, None);
        assert_eq!(rebinds, 5, "one buffer a step, and the first");
        assert_eq!(buffer, Some((1664, 896)));
        let exact = Buffers::new(0, 2).expect("a step of 0 is allowed");
        assert_eq!(
            drag(exact, 1150, 1650, None).0,
            501,
            "the old rule: one a frame"
        );
    }

    /// The step is the knob: at 256 the same drag makes 3 buffers, and
    /// every buffer is a whole number of steps at least as large as its scene.
    #[test]
    fn the_step_is_configurable() {
        let wide = Buffers::new(256, 2).expect("256 is a step");
        let (rebinds, buffer) = drag(wide, 1150, 1650, None);
        assert_eq!((rebinds, buffer), (3, Some((1792, 1024))));
        for size in [(1, 1), (127, 129), (128, 128), (2560, 1440)] {
            let snug = Buffers::default().snug(size);
            assert!(snug.0 >= size.0 && snug.1 >= size.1, "{size:?} in {snug:?}");
            assert_eq!((snug.0 % 128, snug.1 % 128), (0, 0), "{size:?} in {snug:?}");
            assert!(
                snug.0 - size.0 < 128 && snug.1 - size.1 < 128,
                "{size:?} in {snug:?}"
            );
        }
    }

    /// No buffer past the allocator's cap: a scene within it is given a
    /// buffer within it, however the step rounds.
    #[test]
    fn a_buffer_is_never_past_the_cap_for_a_scene_within_it() {
        let max = super::super::target::MAX_SIDE;
        let wide = Buffers::new(1000, 2).expect("1000 is a step");
        assert_eq!(wide.snug((max - 3, 10)), (max, 1000));
        assert_eq!(Buffers::default().snug((max, max)), (max, max));
    }

    /// **A step of 0 is the old rule**, a buffer exactly the scene's size:
    /// every new size is a new buffer, and the same size draws in place.
    #[test]
    fn a_step_of_zero_rebinds_at_every_size() {
        let exact = Buffers::new(0, 2).expect("a step of 0 is allowed");
        assert_eq!(exact.snug((1150, 850)), (1150, 850));
        assert_eq!(
            plan(exact, Some((1151, 850)), (1150, 850), false, false),
            Plan::Rebind((1150, 850))
        );
        assert_eq!(
            plan(exact, Some((1150, 850)), (1150, 850), true, false),
            Plan::InPlace
        );
    }

    /// **A shrinking scene draws into the buffer it has**, and is given a
    /// snug one once, when its size holds: 501 frames of a drag from 1650
    /// to 1150 wide make none, and the first frame at rest at 1150 makes
    /// one of 1152x896; the next frame at rest makes none.
    #[test]
    fn a_shrinking_scene_draws_in_place_and_is_given_a_snug_buffer_once_at_rest() {
        let policy = Buffers::default();
        let (rebinds, buffer) = drag(policy, 1650, 1150, Some((1664, 896)));
        assert_eq!((rebinds, buffer), (0, Some((1664, 896))));
        assert_eq!(
            plan(policy, buffer, (1150, 850), true, false),
            Plan::Rebind((1152, 896))
        );
        assert_eq!(
            plan(policy, Some((1152, 896)), (1150, 850), true, false),
            Plan::InPlace
        );
    }

    /// **A size wavering across a step's edge is not rebound each time it
    /// crosses**: a width going 1280, 1281, 1280, … for 100 frames, each
    /// frame at rest (as a hand held on a mouse makes it), gives the scene
    /// one new buffer, the 1408 it grows into at 1281, and no more: 1408 is
    /// one step past 1280, not two. With `shrink = 1` it would be rebound on
    /// every crossing, which is why 2 is the least `SHRINKS` allows.
    #[test]
    fn a_size_wavering_across_a_steps_edge_is_not_rebound_each_time() {
        let policy = Buffers::default();
        let mut buffer = Some((1280, 896));
        let mut rebinds = 0;
        for frame in 0..100 {
            let width = 1280 + frame % 2;
            if let Plan::Rebind(next) = plan(policy, buffer, (width, 850), true, false) {
                rebinds += 1;
                buffer = Some(next);
            }
        }
        assert_eq!((rebinds, buffer), (1, Some((1408, 896))));
        assert_eq!(Buffers::new(128, 1), None, "a shrink of 1 is refused");
    }

    /// The shrink is the knob: at 4 steps, a buffer 3 steps larger than its
    /// scene at rest is kept, and one 4 steps larger is replaced.
    #[test]
    fn the_shrink_is_configurable() {
        let lax = Buffers::new(128, 4).expect("4 is a shrink");
        assert_eq!(
            plan(lax, Some((1152 + 3 * 128, 896)), (1150, 850), true, false),
            Plan::InPlace
        );
        assert_eq!(
            plan(lax, Some((1152 + 4 * 128, 896)), (1150, 850), true, false),
            Plan::Rebind((1152, 896))
        );
        assert_eq!(
            plan(
                Buffers::default(),
                Some((1152 + 2 * 128, 896)),
                (1150, 850),
                true,
                false
            ),
            Plan::Rebind((1152, 896))
        );
    }

    /// **A buffer another monitor's picture is still shown from is not drawn
    /// over**: a window across a 1x and a 2x monitor is drawn at two sizes,
    /// and the 2x picture lives in the buffer the scene is on, so the 1x
    /// picture is drawn into a buffer of its own however well it would fit.
    /// Not at rest either, and the buffer's room does not matter.
    #[test]
    fn a_buffer_another_monitor_still_shows_is_not_drawn_over() {
        let policy = Buffers::default();
        assert_eq!(
            plan(policy, Some((2304, 1792)), (1150, 850), false, true),
            Plan::Rebind((1152, 896))
        );
        assert_eq!(
            plan(policy, Some((2304, 1792)), (1150, 850), false, false),
            Plan::InPlace
        );
    }

    /// Bounds: a negative step, a step past 1024 and a shrink past 64 are
    /// refused, as a shrink of 1 is.
    #[test]
    fn the_numbers_are_held_to_their_bounds() {
        assert_eq!(Buffers::new(-1, 2), None);
        assert_eq!(Buffers::new(1025, 2), None);
        assert_eq!(Buffers::new(128, 65), None);
        assert_eq!(Buffers::new(128, 2), Some(Buffers::default()));
    }

    /// What `configure` sets is what `configured` reads.
    #[test]
    fn the_configured_policy_is_the_one_drawn_with() {
        let before = super::configured();
        let wide = Buffers::new(512, 3).expect("a policy");
        super::configure(wide);
        assert_eq!(super::configured(), wide);
        super::configure(before);
    }
}
