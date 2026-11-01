//! A window's own edge, stretched over the part of its rectangle its picture
//! does not reach (`resize.background = "edge"`, the default).
//!
//! While a window changes size its application is a frame or more behind the
//! rectangle on screen: a tiled seam dragged faster than either side draws, a
//! layout's glide, a window opened beside another. What the picture does not
//! cover used to be a solid colour, and that was a grey band down every fast
//! seam and the only visible thing on a transparent terminal (2026-10-16).
//! Hyprland stretches the picture's own edge out over the gap instead (its
//! `render:expand_undersized_textures`), so a window that is behind reads as
//! one with more of its own background, and a transparent one stays
//! transparent.
//!
//! **Where each piece samples.** [`crate::render::uncovered`]'s strips are cut
//! wherever the picture ends ([`pieces`]), and each piece samples the
//! picture's texels nearest to it: a piece beside the picture its last column,
//! one under it its last row, a corner its corner texel, each stretched over
//! the piece. Along the edge the samples line up with the picture, so a
//! window's edge carries on outward exactly as it is drawn.
//!
//! **The picture is the window's geometry, not its surface.** A client that
//! draws its own shadow (GTK, Firefox) has a surface wider than its window, and
//! the surface's last column is shadow; the window's last column is what
//! belongs next to it.
//!
//! **And only the root surface's**, the picture the window is: a subsurface is
//! drawn as it is. One that is a buffer this cannot follow -- a transform, no
//! texture yet -- is drawn as `crate::resizing::EDGE_FALLBACK` instead, by the
//! caller.

use smithay::{
    backend::renderer::{
        Renderer as _,
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture},
        utils::{CommitCounter, OpaqueRegions, with_renderer_surface_state},
    },
    desktop::Window,
    utils::{Buffer as BufferCoords, Physical, Rectangle, Scale, Transform},
    wayland::seat::WaylandFocus as _,
};

/// How many pieces a pane's uncovered strips are drawn in at most: four
/// strips, the two bands of which are each cut in three where the picture
/// ends. What [`crate::pane::Pane::edges`] holds an identity for.
pub(crate) const PIECES: usize = 8;

/// How wide a sample of one texel is taken, in texels, about its centre:
/// smithay draws nothing from a source of no size, and this is narrow enough
/// that linear filtering reads that one texel and nothing beside it.
const SLIVER: f64 = 1.0 / 64.0;

/// The parts of `strips` to draw, each with where it samples the picture, in
/// the picture's own pixels from its corner.
///
/// `strips` are [`crate::render::uncovered`]'s, `picture` where the picture is
/// drawn, both in the output's pixels. Each strip is cut where the picture
/// starts and ends on each axis, and a cut that lies inside the picture on both
/// is dropped: `uncovered` ends a strip under the picture's edge, by up to a
/// pixel, and drawing the picture's own pixels again behind themselves would
/// darken a translucent one there. Along an axis on which a piece lies within
/// the picture, it samples the same pixels; past the picture, the one pixel at
/// that edge, as a sliver about its centre.
/// `tests::a_strip_beside_the_picture_samples_its_last_column`,
/// `tests::a_band_under_the_picture_is_cut_where_the_picture_ends`.
pub(crate) fn pieces(
    strips: &[Rectangle<i32, Physical>; 4],
    picture: Rectangle<i32, Physical>,
) -> Vec<(Rectangle<i32, Physical>, Rectangle<f64, Physical>)> {
    if picture.size.w <= 0 || picture.size.h <= 0 {
        return Vec::new();
    }
    let across = (picture.loc.x, picture.loc.x + picture.size.w);
    let down = (picture.loc.y, picture.loc.y + picture.size.h);
    let mut out = Vec::new();
    for strip in strips {
        if strip.size.w <= 0 || strip.size.h <= 0 {
            continue;
        }
        let xs = cut(strip.loc.x, strip.loc.x + strip.size.w, across);
        let ys = cut(strip.loc.y, strip.loc.y + strip.size.h, down);
        for &(x0, x1) in &xs {
            for &(y0, y1) in &ys {
                let inside = |(a, b): (i32, i32), (c, d): (i32, i32)| a >= c && b <= d;
                if inside((x0, x1), across) && inside((y0, y1), down) {
                    continue;
                }
                let (u0, u1) = sample(x0, x1, across);
                let (v0, v1) = sample(y0, y1, down);
                let from = (f64::from(across.0), f64::from(down.0));
                out.push((
                    Rectangle::from_extremities((x0, y0), (x1, y1)),
                    Rectangle::from_extremities(
                        (u0 - from.0, v0 - from.1),
                        (u1 - from.0, v1 - from.1),
                    ),
                ));
            }
        }
    }
    out
}

/// `[from, to)` cut wherever the picture's `[start, end)` begins or ends
/// inside it.
fn cut(from: i32, to: i32, (start, end): (i32, i32)) -> Vec<(i32, i32)> {
    let mut at = vec![from];
    at.extend(
        [start, end]
            .into_iter()
            .filter(|each| *each > from && *each < to),
    );
    at.push(to);
    at.sort_unstable();
    at.dedup();
    at.windows(2)
        .filter_map(|pair| match pair {
            [a, b] => Some((*a, *b)),
            _ => None,
        })
        .collect()
}

/// What `[from, to)` samples of the picture's `[start, end)`: itself where it
/// lies within, else a sliver about the centre of the picture's nearest pixel.
fn sample(from: i32, to: i32, (start, end): (i32, i32)) -> (f64, f64) {
    let centre = |pixel: i32| {
        let at = f64::from(pixel) + 0.5;
        (at - SLIVER / 2.0, at + SLIVER / 2.0)
    };
    if to <= start {
        centre(start)
    } else if from >= end {
        centre(end - 1)
    } else {
        (f64::from(from.max(start)), f64::from(to.min(end)))
    }
}

/// One piece: a part of the picture's edge texels, stretched over `dst`.
#[derive(Debug)]
pub(crate) struct Edge {
    id: Id,
    /// The root surface's, so a new picture redraws the piece.
    commit: CommitCounter,
    texture: GlesTexture,
    /// What it samples, in the buffer's pixels.
    src: Rectangle<f64, BufferCoords>,
    dst: Rectangle<i32, Physical>,
    alpha: f32,
    /// The client's rounded rectangle, and the programs to draw through it,
    /// when the client is drawn through one: the pieces stop where its corners
    /// do, as the client and a solid strip do.
    clip: Option<(crate::clip::Clip, crate::pass::ClipPrograms)>,
}

/// `strips`' pieces for `window`, drawn from its root surface's texture, or
/// `None` when there is no edge this can stretch: no texture yet, a buffer
/// transform, a surface with no size. The caller draws the solid fallback then.
///
/// `picture` is where the window's geometry is drawn, in the output's pixels,
/// and `ids` the pane's (`Pane::edges`).
/// `tests::a_windows_edge_is_sampled_inside_its_geometry_not_its_shadow`.
pub(crate) fn elements(
    renderer: &GlesRenderer,
    window: &Window,
    strips: &crate::render::Strips,
    picture: Rectangle<i32, Physical>,
    ids: &[Id; PIECES],
    alpha: f32,
    programs: Option<&crate::pass::ClipPrograms>,
) -> Option<Vec<Edge>> {
    let surface = window.wl_surface()?;
    let (texture, view, scale, transform, commit) =
        with_renderer_surface_state(&surface, |state| {
            Some((
                state.texture::<GlesTexture>(renderer.context_id())?.clone(),
                state.view()?,
                state.buffer_scale(),
                state.buffer_transform(),
                state.current_commit(),
            ))
        })??;
    if transform != Transform::Normal {
        return None;
    }
    let geometry = window.geometry();
    let window_in_buffer = in_buffer(
        geometry.to_f64(),
        view.src,
        view.dst.to_f64(),
        f64::from(scale),
    )?;
    let clip = strips
        .clip
        .and_then(|clip| programs.map(|programs| (clip, programs.clone())));
    let pieces = pieces(&strips.rects, picture);
    Some(
        pieces
            .into_iter()
            .zip(ids)
            .map(|((dst, sample), id)| Edge {
                id: id.clone(),
                commit,
                texture: texture.clone(),
                src: to_buffer(sample, picture, window_in_buffer),
                dst,
                alpha,
                clip: clip.clone(),
            })
            .collect(),
    )
}

/// The window's geometry, in the surface's own logical pixels, as a rectangle
/// of its buffer's pixels: through the surface's viewport (`src` in the
/// buffer's logical pixels, `dst` the surface's size), then the buffer's scale.
/// `None` for a surface of no size.
fn in_buffer(
    geometry: Rectangle<f64, smithay::utils::Logical>,
    src: Rectangle<f64, smithay::utils::Logical>,
    dst: smithay::utils::Size<f64, smithay::utils::Logical>,
    scale: f64,
) -> Option<Rectangle<f64, BufferCoords>> {
    if dst.w <= 0.0 || dst.h <= 0.0 {
        return None;
    }
    let (kx, ky) = (src.size.w / dst.w, src.size.h / dst.h);
    Some(Rectangle::new(
        (
            (src.loc.x + geometry.loc.x * kx) * scale,
            (src.loc.y + geometry.loc.y * ky) * scale,
        )
            .into(),
        (geometry.size.w * kx * scale, geometry.size.h * ky * scale).into(),
    ))
}

/// A sample in the picture's pixels as a rectangle of the buffer's: the
/// picture is the window's geometry drawn at `picture`'s size.
fn to_buffer(
    sample: Rectangle<f64, Physical>,
    picture: Rectangle<i32, Physical>,
    window: Rectangle<f64, BufferCoords>,
) -> Rectangle<f64, BufferCoords> {
    let (kx, ky) = (
        window.size.w / f64::from(picture.size.w.max(1)),
        window.size.h / f64::from(picture.size.h.max(1)),
    );
    Rectangle::new(
        (
            window.loc.x + sample.loc.x * kx,
            window.loc.y + sample.loc.y * ky,
        )
            .into(),
        (sample.size.w * kx, sample.size.h * ky).into(),
    )
}

impl Element for Edge {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.src
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.dst
    }

    /// None: a window's edge can be translucent, and claiming less is only
    /// drawing more.
    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for Edge {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if let Some((clip, programs)) = &self.clip {
            return crate::clip::draw_texture(
                frame,
                &self.texture,
                src,
                dst,
                damage,
                self.alpha,
                clip,
                programs,
            );
        }
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            &[],
            Transform::Normal,
            self.alpha,
            None,
            &[],
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{SLIVER, in_buffer, pieces, to_buffer};
    use smithay::utils::{Logical, Physical, Rectangle};

    fn at(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// **A strip beside the picture samples its last column**, row for row,
    /// so the window's right edge carries on outward as it is drawn.
    #[test]
    fn a_strip_beside_the_picture_samples_its_last_column() {
        let picture = at(100, 50, 400, 300);
        let nothing = at(500, 50, 0, 0);
        let beside = at(500, 50, 60, 300);
        let got = pieces(&[nothing, nothing, nothing, beside], picture);
        assert_eq!(got.len(), 1, "{got:?}");
        let (dst, sample) = got[0];
        assert_eq!(dst, beside);
        assert!(
            (sample.loc.x + sample.size.w / 2.0 - 399.5).abs() < 1e-9 && sample.size.w <= SLIVER,
            "the centre of the picture's last column: {sample:?}"
        );
        assert_eq!((sample.loc.y, sample.size.h), (0.0, 300.0), "row for row");
    }

    /// **A band under the picture is cut where the picture ends**: the part
    /// under it samples its last row, column for column, and the part past its
    /// corner samples the corner pixel.
    #[test]
    fn a_band_under_the_picture_is_cut_where_the_picture_ends() {
        let picture = at(100, 50, 400, 300);
        let nothing = at(100, 350, 0, 0);
        let below = at(100, 350, 460, 40);
        let got = pieces(&[nothing, below, nothing, nothing], picture);
        assert_eq!(got.len(), 2, "{got:?}");
        let (under, under_sample) = got[0];
        assert_eq!(under, at(100, 350, 400, 40));
        assert_eq!((under_sample.loc.x, under_sample.size.w), (0.0, 400.0));
        assert!((under_sample.loc.y + under_sample.size.h / 2.0 - 299.5).abs() < 1e-9);
        let (corner, corner_sample) = got[1];
        assert_eq!(corner, at(500, 350, 60, 40));
        assert!((corner_sample.loc.x + corner_sample.size.w / 2.0 - 399.5).abs() < 1e-9);
        assert!((corner_sample.loc.y + corner_sample.size.h / 2.0 - 299.5).abs() < 1e-9);
    }

    /// **A strip's last pixel under the picture is not drawn again**:
    /// `uncovered` ends a strip under the picture's edge by up to a pixel, and
    /// the picture's own pixels drawn behind themselves would darken a
    /// translucent window there.
    #[test]
    fn the_pixel_a_strip_reaches_under_the_picture_is_left_alone() {
        let picture = at(100, 50, 400, 300);
        let nothing = at(500, 50, 0, 0);
        let beside = at(499, 50, 61, 300);
        let got = pieces(&[nothing, nothing, nothing, beside], picture);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, at(500, 50, 60, 300));
    }

    /// **A window's edge is sampled inside its geometry, not its shadow**: a
    /// client 26 px of shadow wide on the left and 23 on top, at buffer scale
    /// 2, has its window's last column at buffer pixel `2 * (26 + w) - 1`.
    #[test]
    fn a_windows_edge_is_sampled_inside_its_geometry_not_its_shadow() {
        let geometry: Rectangle<f64, Logical> =
            Rectangle::new((26.0, 23.0).into(), (400.0, 300.0).into());
        let src: Rectangle<f64, Logical> = Rectangle::new((0.0, 0.0).into(), (452.0, 346.0).into());
        let window =
            in_buffer(geometry, src, (452.0, 346.0).into(), 2.0).expect("a surface with a size");
        assert_eq!(
            (window.loc.x, window.loc.y, window.size.w, window.size.h),
            (52.0, 46.0, 800.0, 600.0)
        );
        let picture = at(0, 0, 400, 300);
        let last: Rectangle<f64, Physical> =
            Rectangle::new((399.5 - SLIVER / 2.0, 0.0).into(), (SLIVER, 300.0).into());
        let buffer = to_buffer(last, picture, window);
        assert!(
            (buffer.loc.x + buffer.size.w / 2.0 - (52.0 + 799.0)).abs() < 1e-6,
            "{buffer:?}"
        );
    }
}
