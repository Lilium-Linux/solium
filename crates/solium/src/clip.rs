//! One client surface drawn through its client's rounded rectangle, in its
//! own place: rounding with no capture (\[16\] 0.5).
//!
//! The element is the surface's own, wrapped: same id, same commit, same
//! damage, so a still client is not redrawn. Its program maps the surface's
//! texture coordinate into the client's own pixels (`input_to_geo`), computed
//! at draw time from exactly what smithay uses (`texture_mat`), which covers
//! buffer transforms, viewporter crops, buffer scale, `y_inverted` dmabufs and
//! the rescale a zoomed window is drawn through, all at once
//! (`tests::input_to_geo_maps_each_corner_of_a_surface_onto_the_client`).

use smithay::{
    backend::renderer::{
        Color32F, Renderer, Texture as _,
        element::{
            Element, Id, Kind, RenderElement, UnderlyingStorage,
            surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture},
        },
        gles::{GlesError, GlesFrame, GlesRenderer, Uniform, UniformValue},
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size, Transform},
};
use solium_effects::fragment::{
    COLOUR_UNIFORM, Corners, GEO_PX_UNIFORM, GEO_SIZE_UNIFORM, INPUT_TO_GEO_UNIFORM, RADIUS_UNIFORM,
};

/// The client's rounded rectangle, in the inner element's own physical space
/// (before any rescale), and how the rescale maps it onto the output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Clip {
    pub(crate) rect: Rectangle<f64, Physical>,
    /// Physical, `Corners`' field order: `pass::physical_radii`.
    pub(crate) radii: Corners,
    /// What the rescale scales about.
    pub(crate) origin: Point<i32, Physical>,
    pub(crate) factor: Scale<f64>,
}

/// How a surface is drawn: its texture, or a single-pixel buffer's colour.
/// `tests::a_single_pixel_surface_is_drawn_through_the_solid_program`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ClipDraw {
    Texture,
    Solid(Color32F),
}

/// Which program a surface is drawn through.
/// `tests::a_single_pixel_surface_is_drawn_through_the_solid_program`.
pub(crate) fn route<R: Renderer>(texture: &WaylandSurfaceTexture<R>) -> ClipDraw {
    match texture {
        WaylandSurfaceTexture::Texture(_) => ClipDraw::Texture,
        WaylandSurfaceTexture::SolidColor(colour) => ClipDraw::Solid(*colour),
    }
}

/// A 3x3 matrix, column-major, as GL takes it.
type Mat3 = [f32; 9];

fn product(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [0.0; 9];
    for column in 0..3 {
        for row in 0..3 {
            out[column * 3 + row] = (0..3).map(|k| a[k * 3 + row] * b[column * 3 + k]).sum();
        }
    }
    out
}

const fn scaling(x: f32, y: f32) -> Mat3 {
    [x, 0.0, 0.0, 0.0, y, 0.0, 0.0, 0.0, 1.0]
}

const fn moving(x: f32, y: f32) -> Mat3 {
    [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, x, y, 1.0]
}

/// The inverse of an affine 3x3 (last row 0, 0, 1), or `None` when singular.
/// Singular is a determinant of zero, found by what dividing by it gives, not
/// by comparing it with a small number: a texture matrix's entries are one
/// over the buffer's size, so a large buffer's determinant is small and sound.
/// `tests::a_large_buffers_matrix_is_inverted`, `tests::a_degenerate_matrix_is_refused`.
fn inverse(m: &Mat3) -> Option<Mat3> {
    let det = m[0] * m[4] - m[3] * m[1];
    let (a, b, c, d) = (m[4] / det, -m[1] / det, -m[3] / det, m[0] / det);
    let out = [
        a,
        b,
        0.0,
        c,
        d,
        0.0,
        -(a * m[6] + c * m[7]),
        -(b * m[6] + d * m[7]),
        1.0,
    ];
    out.iter().all(|value| value.is_finite()).then_some(out)
}

// `texture_mat` is ported from smithay 0.7.0, whose notice follows.
//
// MIT License
//
// Copyright (c) 2017 Victor Berger and Victoria Brekenfeld
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

/// smithay's `build_texture_mat` (`backend/renderer/gles/mod.rs:2979-3022` in
/// smithay 0.7.0, MIT, its notice above) with the `y_inverted` flip from
/// `:2512-2515`, ported to plain column-major arrays: the matrix taking a
/// point of `dst`, relative to its corner, to the texture coordinate the
/// surface's own shader samples. Private in smithay, so ported.
/// `tests::the_texture_matrix_is_smithays`.
pub(crate) fn texture_mat(
    src: Rectangle<f64, BufferCoords>,
    dst: Rectangle<i32, Physical>,
    texture: Size<i32, BufferCoords>,
    transform: Transform,
    y_inverted: bool,
) -> Mat3 {
    let fitted = transform.transform_size(src.size);
    #[expect(clippy::cast_possible_truncation, reason = "texture-sized floats")]
    let (sx, sy, w, h) = (
        (fitted.w / f64::from(dst.size.w)) as f32,
        (fitted.h / f64::from(dst.size.h)) as f32,
        fitted.w as f32,
        fitted.h as f32,
    );
    let turn: Mat3 = *AsRef::<[f32; 9]>::as_ref(&transform.matrix());
    let shift = match transform {
        Transform::Normal | Transform::Flipped90 => (0.0, 0.0),
        Transform::_90 => (0.0, w),
        Transform::_180 => (w, h),
        Transform::_270 => (h, 0.0),
        Transform::Flipped => (w, 0.0),
        Transform::Flipped180 => (0.0, h),
        Transform::Flipped270 => (h, w),
    };
    let mut m = scaling(sx, sy);
    m = product(&turn, &m);
    m = product(&moving(shift.0, shift.1), &m);
    #[expect(clippy::cast_possible_truncation, reason = "texture-sized floats")]
    let crop = moving(src.loc.x as f32, src.loc.y as f32);
    m = product(&crop, &m);
    #[expect(clippy::cast_possible_truncation, reason = "texture-sized floats")]
    let normalise = scaling(
        (1.0 / f64::from(texture.w)) as f32,
        (1.0 / f64::from(texture.h)) as f32,
    );
    m = product(&normalise, &m);
    if y_inverted {
        m = product(&scaling(1.0, -1.0), &m);
    }
    m
}

/// The matrix from a surface's texture coordinate to its client's own pixels:
/// back through the texture matrix to `dst`, which is the final, rescaled
/// rectangle, then `geo = (final - origin) / factor + origin - rect.loc`.
/// `None` when the texture matrix cannot be inverted.
/// `tests::input_to_geo_maps_each_corner_of_a_surface_onto_the_client`,
/// `tests::a_large_buffers_matrix_is_inverted`,
/// `tests::a_degenerate_matrix_is_refused`.
pub(crate) fn input_to_geo(
    texture_mat: Mat3,
    dst: Rectangle<i32, Physical>,
    clip: &Clip,
) -> Option<Mat3> {
    let back = inverse(&texture_mat)?;
    #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
    let (fx, fy) = (clip.factor.x as f32, clip.factor.y as f32);
    #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
    let (tx, ty) = (
        (f64::from(dst.loc.x - clip.origin.x) / clip.factor.x + f64::from(clip.origin.x)
            - clip.rect.loc.x) as f32,
        (f64::from(dst.loc.y - clip.origin.y) / clip.factor.y + f64::from(clip.origin.y)
            - clip.rect.loc.y) as f32,
    );
    let to_client = [1.0 / fx, 0.0, 0.0, 0.0, 1.0 / fy, 0.0, tx, ty, 1.0];
    Some(product(&to_client, &back))
}

/// `regions`, inside the clip's rectangle and less its four corner squares
/// (each `ceil(radius)` on a side, at the clip's corners, in the same space).
/// Nothing at all for a radius that is not a number.
/// `tests::a_rounded_surface_claims_all_but_its_four_corner_squares`,
/// `tests::a_fractional_radius_cuts_the_whole_pixel_it_touches`,
/// `tests::an_absurd_radius_claims_nothing_wrongly`,
/// `tests::a_surface_past_the_clients_rectangle_claims_only_inside_it`.
pub(crate) fn cut_corners(
    regions: &[Rectangle<i32, Physical>],
    clip: &Clip,
) -> Vec<Rectangle<i32, Physical>> {
    let radii = [
        clip.radii.top_left,
        clip.radii.top_right,
        clip.radii.bottom_left,
        clip.radii.bottom_right,
    ];
    if radii.iter().any(|radius| !radius.is_finite()) {
        return Vec::new();
    }
    #[expect(clippy::cast_possible_truncation, reason = "clamped to the rectangle")]
    let side = |radius: f64| {
        radius
            .max(0.0)
            .min(clip.rect.size.w.max(clip.rect.size.h))
            .ceil() as i32
    };
    #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
    let (left, top, right, bottom) = (
        clip.rect.loc.x.floor() as i32,
        clip.rect.loc.y.floor() as i32,
        (clip.rect.loc.x + clip.rect.size.w).ceil() as i32,
        (clip.rect.loc.y + clip.rect.size.h).ceil() as i32,
    );
    let squares = [
        Rectangle::new((left, top).into(), (side(radii[0]), side(radii[0])).into()),
        Rectangle::new(
            (right - side(radii[1]), top).into(),
            (side(radii[1]), side(radii[1])).into(),
        ),
        Rectangle::new(
            (left, bottom - side(radii[2])).into(),
            (side(radii[2]), side(radii[2])).into(),
        ),
        Rectangle::new(
            (right - side(radii[3]), bottom - side(radii[3])).into(),
            (side(radii[3]), side(radii[3])).into(),
        ),
    ];
    #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
    let inside = Rectangle::from_extremities(
        (clip.rect.loc.x.ceil() as i32, clip.rect.loc.y.ceil() as i32),
        (
            (clip.rect.loc.x + clip.rect.size.w).floor() as i32,
            (clip.rect.loc.y + clip.rect.size.h).floor() as i32,
        ),
    );
    regions
        .iter()
        .filter_map(|region| region.intersection(inside))
        .flat_map(|region| region.subtract_rects(squares.iter().copied()))
        .filter(|rect| !rect.is_empty())
        .collect()
}

/// What a surface at `at` claims as opaque once clipped: its own claim
/// (relative to itself, as smithay's are) cut by the client's rectangle moved
/// to it, and nothing while it fades, since the damage tracker never
/// multiplies a claim by the alpha.
/// `tests::a_subsurface_is_cut_only_where_its_corner_is_the_clients`,
/// `tests::a_fading_surface_claims_nothing`.
pub(crate) fn claim(
    regions: &[Rectangle<i32, Physical>],
    at: Point<i32, Physical>,
    alpha: f32,
    clip: &Clip,
) -> Vec<Rectangle<i32, Physical>> {
    if alpha < 1.0 {
        return Vec::new();
    }
    let local = Clip {
        rect: Rectangle::new(clip.rect.loc - at.to_f64(), clip.rect.size),
        ..*clip
    };
    cut_corners(regions, &local)
}

/// One client surface, drawn through the clipped programs in its own place.
#[derive(Debug)]
pub(crate) struct Clipped {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    clip: Clip,
    programs: crate::pass::ClipPrograms,
}

impl Clipped {
    pub(crate) fn new(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        clip: Clip,
        programs: crate::pass::ClipPrograms,
    ) -> Self {
        Self {
            inner,
            clip,
            programs,
        }
    }

    /// The four the clipped programs share; the solid one adds its colour.
    fn uniforms(&self, to_geo: Mat3) -> Vec<Uniform<'static>> {
        let radii = &self.clip.radii;
        #[expect(clippy::cast_possible_truncation, reason = "pixel-sized floats")]
        let packed = (
            radii.top_left as f32,
            radii.top_right as f32,
            radii.bottom_left as f32,
            radii.bottom_right as f32,
        );
        #[expect(clippy::cast_possible_truncation, reason = "pixel-sized floats")]
        let (size, px) = (
            (self.clip.rect.size.w as f32, self.clip.rect.size.h as f32),
            self.clip.factor.x.min(self.clip.factor.y) as f32,
        );
        vec![
            Uniform::new(
                INPUT_TO_GEO_UNIFORM,
                UniformValue::Matrix3x3 {
                    matrices: vec![to_geo],
                    transpose: false,
                },
            ),
            Uniform::new(GEO_SIZE_UNIFORM, size),
            Uniform::new(RADIUS_UNIFORM, packed),
            Uniform::new(GEO_PX_UNIFORM, px),
        ]
    }
}

impl Element for Clipped {
    fn id(&self) -> &Id {
        self.inner.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }
    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.inner.transform()
    }
    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }
    /// [`claim`], of the surface's own claim.
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        claim(
            &self.inner.opaque_regions(scale),
            self.inner.geometry(scale).loc,
            self.inner.alpha(),
            &self.clip,
        )
        .into_iter()
        .collect()
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for Clipped {
    /// The program is passed to each call and never set on the frame, so
    /// there is nothing to clear afterwards (wirecheck's case 11h draws
    /// through the same two calls). A matrix that cannot be inverted draws the
    /// surface as smithay would, square, rather than not at all.
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        match route(self.inner.texture()) {
            ClipDraw::Texture => {
                let WaylandSurfaceTexture::Texture(texture) = self.inner.texture() else {
                    return self.inner.draw(frame, src, dst, damage, opaque_regions);
                };
                let tex = texture_mat(
                    src,
                    dst,
                    texture.size(),
                    self.inner.transform(),
                    texture.is_y_inverted(),
                );
                let Some(to_geo) = input_to_geo(tex, dst, &self.clip) else {
                    return self.inner.draw(frame, src, dst, damage, opaque_regions);
                };
                frame.render_texture_from_to(
                    texture,
                    src,
                    dst,
                    damage,
                    opaque_regions,
                    self.inner.transform(),
                    self.inner.alpha(),
                    Some(&self.programs.texture),
                    &self.uniforms(to_geo),
                )
            }
            ClipDraw::Solid(colour) => {
                // The same `src` and `size` smithay builds the pixel
                // program's texture matrix from, so `v_coords` runs 0..1
                // across `dst` and this matrix takes it back.
                let size = Size::<i32, BufferCoords>::from((dst.size.w, dst.size.h));
                let whole = Rectangle::from_size(size.to_f64());
                let tex = texture_mat(whole, dst, size, Transform::Normal, false);
                let Some(to_geo) = input_to_geo(tex, dst, &self.clip) else {
                    return self.inner.draw(frame, src, dst, damage, opaque_regions);
                };
                let mut uniforms = self.uniforms(to_geo);
                uniforms.push(Uniform::new(
                    COLOUR_UNIFORM,
                    (colour.r(), colour.g(), colour.b(), colour.a()),
                ));
                frame.render_pixel_shader_to(
                    &self.programs.solid,
                    whole,
                    dst,
                    size,
                    Some(damage),
                    self.inner.alpha(),
                    &uniforms,
                )
            }
        }
    }

    /// Never a scanout candidate: a plane would show the buffer uncut.
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Physical, Rectangle, Scale, Transform};
    use solium_effects::fragment::Corners;

    use super::{Clip, claim, cut_corners, input_to_geo, texture_mat};

    fn clip(radii: Corners) -> Clip {
        Clip {
            rect: Rectangle::new((0.0, 0.0).into(), (300.0, 200.0).into()),
            radii,
            origin: (0, 0).into(),
            factor: Scale::from(1.0),
        }
    }

    fn apply(m: &[f32; 9], x: f32, y: f32) -> (f32, f32) {
        (m[0] * x + m[3] * y + m[6], m[1] * x + m[4] * y + m[7])
    }

    /// **A rounded surface claims all but its four corner squares**, and a
    /// square corner keeps its pixel, which the old single inset rectangle gave
    /// up along with its rows.
    #[test]
    fn a_rounded_surface_claims_all_but_its_four_corner_squares() {
        let whole = [Rectangle::<i32, Physical>::from_size((300, 200).into())];
        let claimed = cut_corners(
            &whole,
            &clip(Corners {
                top_left: 0.0,
                top_right: 12.0,
                bottom_left: 12.0,
                bottom_right: 12.0,
            }),
        );
        let covers = |x: i32, y: i32| claimed.iter().any(|rect| rect.contains((x, y)));
        assert!(covers(0, 0), "the square corner gave up its pixel");
        assert!(
            !covers(299, 0) && !covers(0, 199) && !covers(299, 199),
            "a cut corner is claimed"
        );
        assert!(covers(12, 0) && covers(287, 12) && covers(150, 100));
        let covered = (0..300)
            .flat_map(|x| (0..200).map(move |y| (x, y)))
            .filter(|(x, y)| covers(*x, *y))
            .count();
        assert_eq!(
            covered,
            300 * 200 - 3 * 12 * 12,
            "the claim is the window less exactly three squares"
        );
    }

    /// A fractional radius cuts the whole pixel it touches.
    #[test]
    fn a_fractional_radius_cuts_the_whole_pixel_it_touches() {
        let whole = [Rectangle::<i32, Physical>::from_size((300, 200).into())];
        let claimed = cut_corners(&whole, &clip(Corners::all(11.2)));
        assert!(!claimed.iter().any(|rect| rect.contains((11, 11))));
    }

    /// A radius past the window, or one that is not a number, claims nothing
    /// rather than overflowing or claiming a corner it cuts. One broken corner
    /// is enough, as the deleted client pass's own test had it: `f64::max`
    /// ignores a NaN, so a guard that asked the largest radius would let one
    /// through.
    #[test]
    fn an_absurd_radius_claims_nothing_wrongly() {
        let whole = [Rectangle::<i32, Physical>::from_size((300, 200).into())];
        assert!(
            cut_corners(&whole, &clip(Corners::all(1e12)))
                .iter()
                .all(|rect| !rect.contains((0, 0)))
        );
        for broken in [f64::NAN, f64::INFINITY] {
            for radii in [
                Corners::all(broken),
                Corners {
                    top_left: broken,
                    ..Corners::all(12.0)
                },
                Corners {
                    bottom_right: broken,
                    ..Corners::all(12.0)
                },
            ] {
                assert!(
                    cut_corners(&whole, &clip(radii)).is_empty(),
                    "{radii:?} claimed a corner the program cuts"
                );
            }
        }
    }

    /// A surface that reaches past its client's rectangle claims only what is
    /// inside it: the program cuts everything outside, so a claim there would
    /// leave whatever the last frame drew.
    #[test]
    fn a_surface_past_the_clients_rectangle_claims_only_inside_it() {
        let wider = [Rectangle::<i32, Physical>::new(
            (-20, -20).into(),
            (340, 240).into(),
        )];
        let claimed = cut_corners(&wider, &clip(Corners::all(12.0)));
        let covers = |x: i32, y: i32| claimed.iter().any(|rect| rect.contains((x, y)));
        assert!(!covers(-1, 50) && !covers(300, 50) && !covers(150, -1) && !covers(150, 200));
        assert!(covers(0, 50) && covers(299, 50) && covers(150, 0) && covers(150, 199));
    }

    /// **A subsurface is cut only where its corner is the client's**: its
    /// claim is relative to itself, so the client's rectangle is moved to it
    /// first. A 150x100 subsurface at (160,120) in a 300x200 client at (10,20)
    /// shares only the client's bottom-right corner.
    #[test]
    fn a_subsurface_is_cut_only_where_its_corner_is_the_clients() {
        let client = Clip {
            rect: Rectangle::new((10.0, 20.0).into(), (300.0, 200.0).into()),
            radii: Corners::all(12.0),
            origin: (10, 20).into(),
            factor: Scale::from(1.0),
        };
        let own = [Rectangle::<i32, Physical>::from_size((150, 100).into())];
        let claimed = claim(&own, (160, 120).into(), 1.0, &client);
        let covers = |x: i32, y: i32| claimed.iter().any(|rect| rect.contains((x, y)));
        assert!(covers(0, 0), "its own corner, not the client's, was cut");
        assert!(!covers(149, 99), "the client's corner is claimed");
        let covered = (0..150)
            .flat_map(|x| (0..100).map(move |y| (x, y)))
            .filter(|(x, y)| covers(*x, *y))
            .count();
        assert_eq!(covered, 150 * 100 - 12 * 12);
    }

    /// A fading surface claims nothing: the damage tracker reads the claim and
    /// the alpha apart and never multiplies them. 0.999 as well as 0.5, so
    /// neither `alpha <= 0.5` nor `alpha == 0.0` passes.
    #[test]
    fn a_fading_surface_claims_nothing() {
        let whole = [Rectangle::<i32, Physical>::from_size((300, 200).into())];
        let rounded = clip(Corners::all(12.0));
        assert!(claim(&whole, (0, 0).into(), 0.5, &rounded).is_empty());
        assert!(claim(&whole, (0, 0).into(), 0.999, &rounded).is_empty());
        assert!(
            !claim(&whole, (0, 0).into(), 1.0, &rounded).is_empty(),
            "and a surface that is not fading still claims its middle"
        );
    }

    /// smithay's texture matrix, ported: a viewporter crop of a 100x50 buffer
    /// to its (10,10) 50x25 corner, drawn at 100x50, samples 0.1..0.6 by
    /// 0.2..0.7. Worked by hand from `gles/mod.rs:2979-3022`.
    #[test]
    fn the_texture_matrix_is_smithays() {
        let m = texture_mat(
            Rectangle::new((10.0, 10.0).into(), (50.0, 25.0).into()),
            Rectangle::from_size((100, 50).into()),
            (100, 50).into(),
            Transform::Normal,
            false,
        );
        let (a, b) = (apply(&m, 0.0, 0.0), apply(&m, 100.0, 50.0));
        assert!(
            (a.0 - 0.1).abs() < 1e-6 && (a.1 - 0.2).abs() < 1e-6,
            "{a:?}"
        );
        assert!(
            (b.0 - 0.6).abs() < 1e-6 && (b.1 - 0.7).abs() < 1e-6,
            "{b:?}"
        );
    }

    /// **The surface-to-client matrix puts each corner of a surface on the
    /// client**: a subsurface at (60,70) in a client at (10,20), for every
    /// transform, under a crop, at buffer scale 2 and at a rescale of one half.
    #[test]
    fn input_to_geo_maps_each_corner_of_a_surface_onto_the_client() {
        let client = Clip {
            rect: Rectangle::new((10.0, 20.0).into(), (300.0, 200.0).into()),
            radii: Corners::all(12.0),
            origin: (10, 20).into(),
            factor: Scale::from(1.0),
        };
        for transform in [Transform::Normal, Transform::_90, Transform::Flipped180] {
            for (dst, factor) in [
                (Rectangle::new((60, 70).into(), (100, 50).into()), 1.0),
                (Rectangle::new((35, 45).into(), (50, 25).into()), 0.5),
            ] {
                let client = Clip {
                    factor: Scale::from(factor),
                    ..client
                };
                let buffer = transform
                    .transform_size(smithay::utils::Size::<i32, smithay::utils::Buffer>::from((
                        200, 100,
                    )));
                let tex = texture_mat(
                    Rectangle::from_size(buffer.to_f64()),
                    dst,
                    buffer,
                    transform,
                    false,
                );
                let to_geo = input_to_geo(tex, dst, &client).expect("an invertible matrix");
                for (x, y) in [(0.0, 0.0), (dst.size.w as f32, dst.size.h as f32)] {
                    let (u, v) = apply(&tex, x, y);
                    let (gx, gy) = apply(&to_geo, u, v);
                    let final_x = dst.loc.x as f32 + x;
                    let final_y = dst.loc.y as f32 + y;
                    let want_x = (final_x - 10.0) / factor as f32 + 10.0 - 10.0;
                    let want_y = (final_y - 20.0) / factor as f32 + 20.0 - 20.0;
                    assert!(
                        (gx - want_x).abs() < 1e-3 && (gy - want_y).abs() < 1e-3,
                        "{transform:?} at {factor}: ({gx}, {gy}) not ({want_x}, {want_y})"
                    );
                }
            }
        }
    }

    /// **A large buffer's matrix is still inverted.** Its entries are one over
    /// the buffer's size, so a 5120x2880 buffer drawn 1:1 has a determinant
    /// near 7e-8, under `f32::EPSILON`: an absolute test for a singular matrix
    /// would leave a maximised window on a 5K monitor square.
    #[test]
    fn a_large_buffers_matrix_is_inverted() {
        let whole = Rectangle::<i32, Physical>::from_size((5120, 2880).into());
        let client = Clip {
            rect: Rectangle::from_size((5120.0, 2880.0).into()),
            radii: Corners::all(12.0),
            origin: (0, 0).into(),
            factor: Scale::from(1.0),
        };
        let tex = texture_mat(
            Rectangle::from_size((5120.0, 2880.0).into()),
            whole,
            (5120, 2880).into(),
            Transform::Normal,
            false,
        );
        let to_geo = input_to_geo(tex, whole, &client).expect("an invertible matrix");
        let (gx, gy) = apply(&to_geo, 1.0, 1.0);
        assert!(
            (gx - 5120.0).abs() < 1e-2 && (gy - 2880.0).abs() < 1e-2,
            "({gx}, {gy})"
        );
    }

    /// A matrix that cannot be inverted, from a crop with no width, is refused
    /// rather than handed to the shader as infinities.
    #[test]
    fn a_degenerate_matrix_is_refused() {
        let dst = Rectangle::<i32, Physical>::from_size((100, 50).into());
        let tex = texture_mat(
            Rectangle::from_size((0.0, 50.0).into()),
            dst,
            (100, 50).into(),
            Transform::Normal,
            false,
        );
        assert!(input_to_geo(tex, dst, &clip(Corners::all(12.0))).is_none());
    }

    /// A single-pixel buffer goes through the solid program, with its colour.
    #[test]
    fn a_single_pixel_surface_is_drawn_through_the_solid_program() {
        use smithay::backend::renderer::{
            Color32F, element::surface::WaylandSurfaceTexture, test::DummyRenderer,
        };
        let colour = Color32F::new(1.0, 0.0, 0.0, 1.0);
        let solid: WaylandSurfaceTexture<DummyRenderer> = WaylandSurfaceTexture::SolidColor(colour);
        assert_eq!(super::route(&solid), super::ClipDraw::Solid(colour));
    }
}
