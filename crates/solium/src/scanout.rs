//! What a monitor really scans out, read back on the hardware (#436).
//!
//! A capture (`screencopy::fill`) draws a monitor's elements again, by hand,
//! into a buffer of its own. That is the picture the monitor *should* show,
//! and #436 is a dark edge on a blurred window that the monitor shows and no
//! capture does, with the damage ruled out (`SOLIUM_FULL_REDRAW`). Between
//! the two there are only the buffer the DRM compositor rendered and hands to
//! the display, and what the display does with it.
//!
//! `SOLIUM_SCANOUT_DUMP=<dir>` settles which half it is on: once `<dir>/now`
//! exists, the next frame drawn on each monitor is written twice, as PPM:
//!
//! * `<n>-<monitor>-scanout.ppm`: the buffer itself, blitted back from the
//!   swapchain slot queued for the display, with whatever went on the cursor
//!   or an overlay plane drawn over it;
//! * `<n>-<monitor>-capture.ppm`: the same elements drawn as a capture draws
//!   them, in the same pass.
//!
//! The log says how far apart the two are, where, how many pixels of the
//! scanout are not opaque, and what went on a plane of its own. `now` is taken
//! away once it is seen, so each `touch` is one shot. Read once; off, it costs
//! one `Option` test a frame.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use smithay::backend::{
    allocator::{Buffer, Fourcc, dmabuf::AsDmabuf},
    drm::{Framebuffer, compositor::RenderFrameResult},
    renderer::{
        Bind as _, Color32F, ExportMem as _, Frame as _, Offscreen as _, Renderer as _,
        element::{Element as _, Id, RenderElement},
        gles::{GlesRenderer, GlesTexture},
    },
};
use smithay::output::Output;
use smithay::utils::{Physical, Rectangle, Size};

/// How often `<dir>/now` is looked for: a stat a frame at 260 Hz is a
/// stat too many for a knob nobody is touching.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// How long a shot waits for a monitor to draw: one that is off or idle
/// is not waited for by the next `touch`.
const SHOT_FOR: Duration = Duration::from_secs(1);

/// The directory `SOLIUM_SCANOUT_DUMP` names, read once; `None` when unset
/// or empty.
pub(crate) fn dir() -> Option<&'static Path> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::var_os("SOLIUM_SCANOUT_DUMP")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)?;
        tracing::info!(
            dir = %dir.display(),
            "SOLIUM_SCANOUT_DUMP: `touch now` in it writes the next frame of each monitor as it is scanned out and as a capture draws it"
        );
        Some(dir)
    })
    .as_deref()
}

/// When a shot is taken: armed by `now` appearing, at most once every
/// [`LOOK_EVERY`], and then one frame of each monitor for [`SHOT_FOR`].
/// `tests::a_touch_is_one_frame_of_each_monitor`,
/// `tests::the_trigger_is_looked_for_at_most_every_hundred_ms`.
#[derive(Debug, Default)]
pub(crate) struct Trigger {
    looked: Option<Instant>,
    armed: Option<Instant>,
    shot: u32,
    done: Vec<String>,
}

impl Trigger {
    /// The shot's number if `monitor`'s frame drawn at `now` is to be
    /// written; `asked` looks for (and takes away) the trigger.
    pub(crate) fn due(
        &mut self,
        monitor: &str,
        now: Instant,
        asked: impl FnOnce() -> bool,
    ) -> Option<u32> {
        if self
            .armed
            .is_some_and(|armed| now.saturating_duration_since(armed) > SHOT_FOR)
        {
            self.armed = None;
        }
        if self.armed.is_none() {
            if self
                .looked
                .is_some_and(|looked| now.saturating_duration_since(looked) < LOOK_EVERY)
            {
                return None;
            }
            self.looked = Some(now);
            if !asked() {
                return None;
            }
            self.armed = Some(now);
            self.shot += 1;
            self.done.clear();
        }
        if self.done.iter().any(|done| done == monitor) {
            return None;
        }
        self.done.push(monitor.to_owned());
        Some(self.shot)
    }
}

/// The trigger the TTY backend keeps; on its one thread.
pub(crate) fn due(monitor: &str) -> Option<(PathBuf, u32)> {
    thread_local! {
        static TRIGGER: std::cell::RefCell<Trigger> = std::cell::RefCell::default();
    }
    let dir = dir()?;
    let shot = TRIGGER.with(|trigger| {
        trigger.borrow_mut().due(monitor, Instant::now(), || {
            std::fs::remove_file(dir.join("now")).is_ok()
        })
    })?;
    Some((dir.to_path_buf(), shot))
}

/// Write `monitor`'s frame as it is scanned out and as a capture draws it,
/// and say how they differ. Waits on the GPU twice: a knob for one login.
pub(crate) fn write<B, F, E>(
    renderer: &mut GlesRenderer,
    result: &RenderFrameResult<'_, B, F, E>,
    elements: &[E],
    output: &Output,
    (dir, shot): (&Path, u32),
) -> Result<()>
where
    B: Buffer + AsDmabuf,
    <B as AsDmabuf>::Error: std::fmt::Debug,
    F: Framebuffer,
    E: RenderElement<GlesRenderer>,
{
    let monitor = output.name();
    let mode = output
        .current_mode()
        .context("a monitor with no mode")?
        .size;
    let transform = output.current_transform();
    let scale = output.current_scale().fractional_scale();
    let whole = Rectangle::from_size(mode);

    let scanout = {
        let mut texture: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, (mode.w, mode.h).into())
            .context("no buffer to blit the scanout into")?;
        let pixels = {
            let mut framebuffer = renderer.bind(&mut texture).context("binding the blit")?;
            result
                .blit_frame_result(
                    mode,
                    transform,
                    scale,
                    renderer,
                    &mut framebuffer,
                    [whole],
                    std::iter::empty::<Id>(),
                )
                .map_err(|err| anyhow!("blitting the scanout back: {err:?}"))?
                .wait()
                .map_err(|_| anyhow!("waiting for the blit"))?;
            read(renderer, &framebuffer, mode)?
        };
        crate::warp::release_framebuffer(renderer);
        pixels
    };

    let capture = {
        let mut texture: GlesTexture = renderer
            .create_buffer(Fourcc::Abgr8888, (mode.w, mode.h).into())
            .context("no buffer to draw the capture into")?;
        let pixels = {
            let mut framebuffer = renderer.bind(&mut texture).context("binding the capture")?;
            {
                let _frame = crate::qml::frame_in_flight();
                let mut frame = renderer
                    .render(&mut framebuffer, mode, transform.invert())
                    .context("rendering the capture")?;
                if let Err(err) = crate::pool::paint_over(
                    &mut frame,
                    transform.transform_size(mode),
                    Color32F::new(0.05, 0.05, 0.06, 1.0),
                    elements,
                    scale,
                ) {
                    tracing::warn!(?err, "an element did not render into the dump's capture");
                }
                frame
                    .finish()
                    .map_err(|_| anyhow!("the capture did not finish"))?
                    .wait()
                    .map_err(|_| anyhow!("waiting for the capture failed"))?;
            }
            read(renderer, &framebuffer, mode)?
        };
        crate::warp::release_framebuffer(renderer);
        pixels
    };

    let stem = dir.join(format!("{shot:03}-{monitor}"));
    write_ppm(&path_with(&stem, "scanout"), mode, &scanout)?;
    write_ppm(&path_with(&stem, "capture"), mode, &capture)?;
    let apart = compare(mode, &scanout, &capture);
    tracing::info!(
        monitor,
        shot,
        differing = apart.differing,
        largest = apart.largest,
        within = ?apart.within,
        not_opaque = apart.not_opaque,
        overlays = result.overlay_elements.len(),
        overlay_ids = ?result.overlay_elements.iter().map(|e| e.id().clone()).collect::<Vec<_>>(),
        cursor_plane = result.cursor_element.is_some(),
        needs_sync = result.needs_sync(),
        "SOLIUM_SCANOUT_DUMP: the frame as scanned out against the same frame drawn as a capture"
    );
    Ok(())
}

/// `<stem>-<what>.ppm`.
fn path_with(stem: &Path, what: &str) -> PathBuf {
    let mut name = stem.as_os_str().to_owned();
    name.push(format!("-{what}.ppm"));
    PathBuf::from(name)
}

/// A bound target's pixels, BGRA, top row first: smithay draws every target
/// but the nested window upright, so rows are not turned here as
/// `capture::take_frame` turns the nested window's.
fn read(
    renderer: &mut GlesRenderer,
    framebuffer: &smithay::backend::renderer::gles::GlesTarget<'_>,
    size: Size<i32, Physical>,
) -> Result<Vec<u8>> {
    let mapping = renderer
        .copy_framebuffer(
            framebuffer,
            Rectangle::from_size((size.w, size.h).into()),
            Fourcc::Argb8888,
        )
        .map_err(|err| anyhow!("copying back: {err}"))?;
    Ok(renderer
        .map_texture(&mapping)
        .map_err(|err| anyhow!("mapping: {err}"))?
        .to_vec())
}

/// BGRA rows as a binary PPM.
fn write_ppm(path: &Path, size: Size<i32, Physical>, bgra: &[u8]) -> Result<()> {
    let (w, h) = (
        usize::try_from(size.w).unwrap_or(0),
        usize::try_from(size.h).unwrap_or(0),
    );
    let mut out = Vec::with_capacity(w * h * 3 + 32);
    out.extend_from_slice(format!("P6\n{w} {h}\n255\n").as_bytes());
    for [blue, green, red, _alpha] in bgra.as_chunks::<4>().0.iter().take(w * h) {
        out.extend_from_slice(&[*red, *green, *blue]);
    }
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

/// How two BGRA pictures of `size` differ: the pixels whose colour differs,
/// the largest difference in one channel, the rectangle holding every such
/// pixel, and the pixels of the first that are not opaque.
/// `tests::two_pictures_are_compared_pixel_by_pixel`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Apart {
    pub(crate) differing: usize,
    pub(crate) largest: u8,
    pub(crate) within: Option<Rectangle<i32, Physical>>,
    pub(crate) not_opaque: usize,
}

/// [`Apart`] of `first` and `second`, BGRA rows of `size`.
pub(crate) fn compare(size: Size<i32, Physical>, first: &[u8], second: &[u8]) -> Apart {
    let width = usize::try_from(size.w).unwrap_or(0).max(1);
    let mut apart = Apart::default();
    let (mut low, mut high): (Option<(usize, usize)>, (usize, usize)) = (None, (0, 0));
    for (index, (a, b)) in first
        .as_chunks::<4>()
        .0
        .iter()
        .zip(second.as_chunks::<4>().0)
        .enumerate()
    {
        if a[3] != u8::MAX {
            apart.not_opaque += 1;
        }
        let largest = a
            .iter()
            .zip(b)
            .take(3)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        if largest == 0 {
            continue;
        }
        apart.differing += 1;
        apart.largest = apart.largest.max(largest);
        let (x, y) = (index % width, index / width);
        low = Some(low.map_or((x, y), |(lx, ly)| (lx.min(x), ly.min(y))));
        high = (high.0.max(x), high.1.max(y));
    }
    apart.within = low.map(|(x, y)| {
        let at = |n: usize| i32::try_from(n).unwrap_or(i32::MAX);
        Rectangle::new(
            (at(x), at(y)).into(),
            (at(high.0 - x + 1), at(high.1 - y + 1)).into(),
        )
    });
    apart
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use smithay::utils::Rectangle;

    use super::{Apart, Trigger, compare};

    #[test]
    fn a_touch_is_one_frame_of_each_monitor() {
        let start = Instant::now();
        let mut trigger = Trigger::default();
        assert_eq!(trigger.due("DP-1", start, || false), None, "nothing asked");
        let later = start + Duration::from_millis(150);
        assert_eq!(trigger.due("DP-1", later, || true), Some(1));
        assert_eq!(
            trigger.due("DP-2", later, || panic!("looked again while armed")),
            Some(1)
        );
        assert_eq!(
            trigger.due("DP-1", later, || false),
            None,
            "DP-1 had its frame"
        );
        let after = later + Duration::from_millis(1100);
        assert_eq!(
            trigger.due("DP-1", after, || false),
            None,
            "the shot is over"
        );
        assert_eq!(
            trigger.due("DP-1", after + Duration::from_millis(200), || true),
            Some(2),
            "a second touch is a second shot"
        );
    }

    #[test]
    fn the_trigger_is_looked_for_at_most_every_hundred_ms() {
        let start = Instant::now();
        let mut trigger = Trigger::default();
        let mut looks = 0;
        for ms in 0..100 {
            let _ = trigger.due("DP-1", start + Duration::from_millis(ms), || {
                looks += 1;
                false
            });
        }
        assert_eq!(looks, 1);
    }

    #[test]
    fn two_pictures_are_compared_pixel_by_pixel() {
        let size = (3, 2).into();
        let first = [
            0, 0, 0, 255, 9, 9, 9, 255, 0, 0, 0, 255, //
            0, 0, 0, 255, 0, 0, 0, 200, 0, 0, 0, 255,
        ];
        let mut second = first;
        second[4] = 1; // (1, 0), blue by 8
        second[16] = 3; // (1, 1), blue by 3
        assert_eq!(
            compare(size, &first, &second),
            Apart {
                differing: 2,
                largest: 8,
                within: Some(Rectangle::new((1, 0).into(), (1, 2).into())),
                not_opaque: 1,
            }
        );
        assert_eq!(compare(size, &first, &first).within, None);
    }
}
