#![expect(
    unsafe_code,
    reason = "the warp makes GL calls itself: Smithay's traits \
              cannot place four corners independently -- see \
              docs/spikes/2026-09-06-3d-presentation.md"
)]

//! Drawing a texture through arbitrary corners, with raw GL calls: the program
//! and its draw are in `warp/gl.rs`.
//!
//! Nothing in Smithay's renderer traits can place a texture's four corners
//! independently, and without that there is no perspective, no genie and no
//! fold — so this module draws with GL itself, through `with_context`. It is not
//! the only GLES in the renderer: the element set, the rounded-corner pass and
//! QML on the GPU are GLES too, and
//! `docs/spikes/2026-08-27-vulkan-on-smithay.md` lists what a Vulkan backend
//! would have to replace. See `docs/spikes/2026-09-06-3d-presentation.md` for
//! why this file exists.
//!
//! Deliberately knows nothing about windows. It takes a texture and four
//! corners, which is what a tilted window, a Stage Manager card, an overview
//! thumbnail and a folding panel all reduce to.
//!
//! The interpolation is projective, not affine. Two triangles with plain UVs
//! would sample correctly at the corners and wrongly everywhere else, with a
//! visible crease along the shared diagonal — the classic wrong-looking
//! perspective. Each corner carries `q = 1/w` and the fragment shader divides,
//! which is what makes a receding edge compress its texture the way it should.

use smithay::{
    backend::renderer::{
        Texture,
        element::{Element, Id, Kind, RenderElement, UnderlyingStorage},
        gles::{GlesError, GlesFrame, GlesRenderer, GlesTexture, ffi},
        utils::CommitCounter,
        utils::OpaqueRegions,
    },
    utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::mat4::Mat4;

mod gl;
pub(crate) use gl::Program;

/// One corner: where it lands, and its projective weight.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Corner {
    /// Position in physical output pixels.
    pub(crate) x: f32,
    pub(crate) y: f32,
    /// Texture coordinate, 0..1.
    pub(crate) u: f32,
    pub(crate) v: f32,
    /// `1/w` from the projection, for perspective-correct sampling.
    pub(crate) q: f32,
}

/// A texture drawn through four corners.
#[derive(Clone, Debug)]
pub(crate) struct Warp {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    mesh: Mesh,
    bounds: Rectangle<i32, Physical>,
    alpha: f32,
    program: Program,
}

impl Warp {
    /// Draw `texture` through `mesh`, with `program`.
    pub(crate) fn new(
        id: Id,
        commit: CommitCounter,
        texture: GlesTexture,
        mesh: Mesh,
        alpha: f32,
        program: Program,
    ) -> Self {
        // The bounding box is what the damage tracker reasons about: a warped
        // texture can land anywhere, and claiming a smaller area than it
        // covers leaves the difference undrawn until something else damages it.
        let mut left = f32::MAX;
        let mut right = f32::MIN;
        let mut top = f32::MAX;
        let mut bottom = f32::MIN;
        for corner in &mesh.vertices {
            left = left.min(corner.x);
            right = right.max(corner.x);
            top = top.min(corner.y);
            bottom = bottom.max(corner.y);
        }

        #[expect(
            clippy::cast_possible_truncation,
            reason = "output coordinates are small integers"
        )]
        let bounds = Rectangle::new(
            (left.floor() as i32, top.floor() as i32).into(),
            (
                ((right.ceil() - left.floor()) as i32).max(1),
                ((bottom.ceil() - top.floor()) as i32).max(1),
            )
                .into(),
        );

        Self {
            id,
            commit,
            texture,
            mesh,
            bounds,
            alpha,
            program,
        }
    }
}

/// A deformed window as triangles, in physical pixels.
///
/// Built per frame: the whole point is that it moves. A flat window never
/// reaches here -- it stays a rectangle and costs a rectangle.
#[derive(Clone, Debug)]
pub(crate) struct Mesh {
    /// A triangle list, three vertices per triangle.
    vertices: Vec<Corner>,
}

impl Mesh {
    /// Five floats a vertex, as the program reads them: x, y, u·q, v·q, q.
    pub(crate) fn interleaved(&self) -> Vec<f32> {
        let mut vertices = Vec::with_capacity(self.vertices.len() * 5);
        for corner in &self.vertices {
            vertices.extend_from_slice(&[
                corner.x,
                corner.y,
                corner.u * corner.q,
                corner.v * corner.q,
                corner.q,
            ]);
        }
        vertices
    }
}

#[cfg(test)]
impl Mesh {
    /// Its vertices, for the tests of where a mesh lands.
    pub(crate) fn vertices(&self) -> &[Corner] {
        &self.vertices
    }
}

/// A part of a pane's unit square: (0,0)-(1,1) is the pane, and a popup may
/// reach past it. `tests::a_part_of_the_pane_lands_where_the_whole_pane_puts_those_points`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct UnitRect {
    pub(crate) u0: f64,
    pub(crate) v0: f64,
    pub(crate) u1: f64,
    pub(crate) v1: f64,
}

impl UnitRect {
    pub(crate) const WHOLE: Self = Self {
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
    };
}

/// A grid of points over `part` of a pane's unit square, in global logical
/// pixels: `(cols + 1) × (rows + 1)` of them, x and y each, row by row from
/// the top-left, as a geometry effect's `mesh` writes them
/// (`effect::geometry::mesh`). What [`mesh_grid`] projects.
/// `tests::a_lua_grid_lands_where_the_rust_genie_put_it`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Grid {
    pub(crate) cols: u32,
    pub(crate) rows: u32,
    pub(crate) part: UnitRect,
    pub(crate) points: Vec<f64>,
}

impl Grid {
    /// `part` of `rect` where it is: one cell, the rectangle's own corners,
    /// which is every flat and every tilted window's grid.
    /// `tests::the_whole_part_is_the_whole_mesh`.
    fn identity(rect: Rectangle<f64, smithay::utils::Logical>, part: UnitRect) -> Self {
        let from = crate::present::for_effects(rect);
        let mut points = Vec::with_capacity(8);
        for (u, v) in [
            (part.u0, part.v0),
            (part.u1, part.v0),
            (part.u0, part.v1),
            (part.u1, part.v1),
        ] {
            let (x, y) = from.at(u, v);
            points.extend_from_slice(&[x, y]);
        }
        Self {
            cols: 1,
            rows: 1,
            part,
            points,
        }
    }
}

/// [`mesh_part`] over the whole pane, which is the mesh as it was before parts:
/// the tests' way of saying "the whole window".
/// `tests::the_whole_part_is_the_whole_mesh`.
#[cfg(test)]
pub(crate) fn mesh(
    rect: Rectangle<f64, smithay::utils::Logical>,
    matrix: Mat4,
    pivot: (f32, f32),
    scale: f64,
) -> Option<Mesh> {
    mesh_part(rect, UnitRect::WHOLE, matrix, pivot, scale)
}

/// Cut `part` of a rectangle into a mesh and project it, about `pivot`: the
/// identity grid, the rectangle where it is, through [`mesh_grid`]. A
/// geometry effect's own grid goes to [`mesh_grid`] itself.
///
/// `pivot` is a fraction of `rect`, not pixels: `(0.5, 0.5)` is its centre and
/// `(0.0, 0.0)` its top-left corner. It is the one point the matrix leaves
/// alone, which is what separates a card flipping on its own spine from a card
/// flipping about the middle of itself.
///
/// `part` is a piece of the rectangle's unit square, [`UnitRect::WHOLE`] for
/// the whole window: each point lands where the whole window's mesh puts it --
/// the same rect, matrix and pivot -- and the texture's 0..1 runs across the
/// part, so a texture of the part alone is drawn through it. A part past the
/// window is extrapolated, not clamped.
/// `tests::a_part_of_the_pane_lands_where_the_whole_pane_puts_those_points`;
/// the whole part is the mesh before parts bit for bit,
/// `tests::the_whole_part_is_the_whole_mesh`.
///
/// Returns `None` when any vertex lands at or behind the viewer: a shape with
/// one vertex projected from behind is not that shape any more, and drawing it
/// anyway folds the texture across the screen.
pub(crate) fn mesh_part(
    rect: Rectangle<f64, smithay::utils::Logical>,
    part: UnitRect,
    matrix: Mat4,
    pivot: (f32, f32),
    scale: f64,
) -> Option<Mesh> {
    mesh_grid(
        &Grid::identity(rect, part),
        Point::from((0.0, 0.0)),
        rect,
        matrix,
        pivot,
        scale,
    )
}

/// Project a grid of points, each moved by `shift` (global to the screen's
/// logical space), through `matrix` about `pivot` of `rect` (the window's
/// rectangle on that screen), and cut it into triangles: what a geometry
/// effect's `mesh` placed (`effect::geometry::mesh`), drawn.
///
/// The grid places points around inside the window's own space; the matrix
/// then places that in 3D. They compose, which is what lets a genie happen
/// to a window that is also tilted. Each point's texture coordinate is where
/// it is in the grid, `(c / cols, r / rows)`, across the part's capture.
/// `tests::a_lua_grid_lands_where_the_rust_genie_put_it`.
///
/// Returns `None` when the grid has not as many points as its size says, or
/// any vertex lands at or behind the viewer, as [`mesh_part`] does.
pub(crate) fn mesh_grid(
    grid: &Grid,
    shift: Point<f64, smithay::utils::Logical>,
    rect: Rectangle<f64, smithay::utils::Logical>,
    matrix: Mat4,
    pivot: (f32, f32),
    scale: f64,
) -> Option<Mesh> {
    let (columns, rows) = (grid.cols.max(1), grid.rows.max(1));
    if grid.points.len() != crate::effect::sandbox::numbers(columns, rows) {
        return None;
    }
    // The point the matrix turns about, and the point the projection is
    // measured from. `(0.5, 0.5)` is the rect's centre, which is what this
    // computed before `pivot` existed and is what every flat window still
    // passes -- so the default is not a special case, it is the same two
    // multiplications with a 0.5 that used to be spelled `/ 2.0`.
    let (centre_x, centre_y) = (
        rect.loc.x + rect.size.w * f64::from(pivot.0),
        rect.loc.y + rect.size.h * f64::from(pivot.1),
    );
    #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
    let scale32 = scale as f32;

    let mut corners = Vec::with_capacity(grid.points.len() / 2);
    for row in 0..=rows {
        let along_v = f64::from(row) / f64::from(rows);
        for column in 0..=columns {
            let along_u = f64::from(column) / f64::from(columns);
            let at = 2 * (row as usize * (columns as usize + 1) + column as usize);
            let (x, y) = (
                grid.points.get(at)? + shift.x,
                grid.points.get(at + 1)? + shift.y,
            );
            #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
            let offset = (((x - centre_x) as f32), ((y - centre_y) as f32));
            let (projected_x, projected_y, w) = matrix.project_with_w(offset.0, offset.1, 0.0)?;
            #[expect(clippy::cast_possible_truncation, reason = "screen-sized floats")]
            let (origin_x, origin_y) = ((centre_x * scale) as f32, (centre_y * scale) as f32);
            #[expect(clippy::cast_possible_truncation, reason = "unit square floats")]
            corners.push(Corner {
                x: origin_x + projected_x * scale32,
                y: origin_y + projected_y * scale32,
                u: along_u as f32,
                v: along_v as f32,
                q: 1.0 / w,
            });
        }
    }

    // Two triangles per cell. Indices would save a third of the upload, but
    // this is a few thousand floats a frame at most and a flat list is one
    // less thing to get wrong.
    let at = |column: u32, row: u32| {
        corners
            .get((row * (columns + 1) + column) as usize)
            .copied()
    };
    let mut vertices = Vec::with_capacity((columns * rows * 6) as usize);
    for row in 0..rows {
        for column in 0..columns {
            let top_left = at(column, row)?;
            let top_right = at(column + 1, row)?;
            let bottom_right = at(column + 1, row + 1)?;
            let bottom_left = at(column, row + 1)?;
            vertices.extend_from_slice(&[
                top_left,
                top_right,
                bottom_right,
                top_left,
                bottom_right,
                bottom_left,
            ]);
        }
    }
    Some(Mesh { vertices })
}

/// The Rust genie's grid: `deform` placed at every point of its own grid
/// (`Deform::segments`) over `part`, from `from` toward `to`, global
/// logical. The oracle the `genie` folder is held to, in the tests and,
/// through `SOLIUM_GEOMETRY_ORACLE`, in a debug build only (Rulings 19, 23):
/// a release build has no code that draws an effect from Rust.
/// `tests::a_lua_grid_lands_where_the_rust_genie_put_it`.
#[cfg(any(test, debug_assertions))]
pub(crate) fn oracle_grid(
    from: Rectangle<f64, smithay::utils::Logical>,
    to: Rectangle<f64, smithay::utils::Logical>,
    deform: solium_effects::Deform,
    part: UnitRect,
) -> Grid {
    let (from, to) = (
        crate::present::for_effects(from),
        crate::present::for_effects(to),
    );
    let (cols, rows) = deform.segments();
    let mut points = Vec::with_capacity(crate::effect::sandbox::numbers(cols, rows));
    for row in 0..=rows {
        let v = part.v0 + (part.v1 - part.v0) * (f64::from(row) / f64::from(rows));
        for column in 0..=cols {
            let u = part.u0 + (part.u1 - part.u0) * (f64::from(column) / f64::from(cols));
            let (x, y) = deform.place(from, to, u, v);
            points.extend_from_slice(&[x, y]);
        }
    }
    Grid {
        cols,
        rows,
        part,
        points,
    }
}

impl Element for Warp {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        Rectangle::from_size(self.texture.size().to_f64())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.bounds
    }

    /// None. A warped texture's edges are antialiased against whatever is
    /// behind them, and claiming any of it opaque would leave the background
    /// undrawn under a rotated edge.
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

impl RenderElement<GlesRenderer> for Warp {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if damage.is_empty() {
            return Ok(());
        }
        let projection = *frame.projection();
        let texture = self.texture.tex_id();
        let vertices = self.mesh.interleaved();
        let (program, alpha) = (self.program, self.alpha);
        // Only the damage, each rectangle under its own scissor: the damage is
        // relative to `dst`, the mesh is in the frame's pixels. A warp kept
        // across passes (its id is its capture's) is handed partial damage,
        // and the whole mesh drawn under it would blend its translucent edge
        // twice: wirecheck case 11g,
        // `tests::a_damage_rectangle_scissors_where_the_projection_puts_it`.
        let rects: Vec<Rectangle<i32, Physical>> = damage
            .iter()
            .map(|rect| Rectangle::new(dst.loc + rect.loc, rect.size))
            .collect();
        // A program compiled between frames and carried here, so a warp cannot
        // fail to draw for want of one: `pass::tests::a_program_that_will_not_compile_is_tried_once`.
        frame.with_context(|gl| {
            let mut viewport = [0_i32; 4];
            // SAFETY: a context is current inside `with_context`; the program's
            // names were made against this renderer's context.
            unsafe {
                gl.GetIntegerv(ffi::VIEWPORT, viewport.as_mut_ptr());
                let scissors: Vec<[i32; 4]> = rects
                    .iter()
                    .map(|rect| self::gl::scissor_box(&projection, viewport, *rect))
                    .collect();
                program.draw(gl, &projection, texture, &vertices, alpha, &scissors);
            }
        })
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        // Never a scanout candidate: the point of this element is that it is
        // not a rectangle, and a plane can only show a rectangle.
        None
    }
}

/// Unused, but part of the element contract for a texture-backed element.
#[expect(dead_code, reason = "kept beside src() for whoever needs the size")]
pub(crate) fn texture_size(texture: &GlesTexture) -> Size<i32, BufferCoords> {
    let _ = Transform::Normal;
    texture.size()
}

/// Point GL back at the window system's framebuffer.
///
/// An offscreen pass binds a framebuffer object of its own. On a backend that
/// renders into an EGL surface rather than an FBO -- winit is one -- nothing
/// ever binds FBO 0 again, so that object stays current and every later frame
/// is drawn into a texture nobody shows. The compositor looks frozen while
/// happily reporting successful frames. So whoever binds an FBO puts this
/// back.
pub(crate) fn release_framebuffer(renderer: &mut GlesRenderer) {
    // SAFETY: `with_context` makes the renderer's context current for the
    // call, and 0 is always a valid framebuffer name.
    let restored = unsafe { renderer.with_context(|gl| gl.BindFramebuffer(ffi::FRAMEBUFFER, 0)) };
    if let Err(err) = restored {
        tracing::warn!(?err, "could not release the offscreen framebuffer");
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Logical, Rectangle};

    use super::mesh;
    use crate::mat4::Mat4;

    /// smithay's projection for an output of `w`x`h` under `transform`
    /// (`gles/mod.rs:2065-2090`), column-major as `frame.projection()` gives it.
    fn projection(w: i32, h: i32, transform: smithay::utils::Transform) -> [f32; 9] {
        let (mut w, mut h) = (w as f32, h as f32);
        if matches!(
            transform,
            smithay::utils::Transform::_90
                | smithay::utils::Transform::_270
                | smithay::utils::Transform::Flipped90
                | smithay::utils::Transform::Flipped270
        ) {
            std::mem::swap(&mut w, &mut h);
        }
        let ortho = [2.0 / w, 0.0, 0.0, 0.0, -2.0 / h, 0.0, -1.0, 1.0, 1.0];
        let turn: [f32; 9] = *AsRef::<[f32; 9]>::as_ref(&transform.matrix());
        let flip = [1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0];
        let times = |a: &[f32; 9], b: &[f32; 9]| {
            let mut out = [0.0; 9];
            for column in 0..3 {
                for row in 0..3 {
                    out[column * 3 + row] =
                        (0..3).map(|k| a[k * 3 + row] * b[column * 3 + k]).sum();
                }
            }
            out
        };
        times(&times(&flip, &turn), &ortho)
    }

    /// **A damage rectangle scissors where the projection puts it.** Under no
    /// transform the box is the rectangle; under a quarter turn or a flip it
    /// keeps its area.
    #[test]
    fn a_damage_rectangle_scissors_where_the_projection_puts_it() {
        use smithay::utils::{Rectangle, Transform};
        let rect = Rectangle::new((100, 50).into(), (200, 100).into());
        let normal = super::gl::scissor_box(
            &projection(1920, 1080, Transform::Normal),
            [0, 0, 1920, 1080],
            rect,
        );
        assert_eq!(normal, [100, 50, 200, 100]);
        for transform in [Transform::_90, Transform::Flipped180] {
            let [_, _, w, h] = super::gl::scissor_box(
                &projection(1920, 1080, transform),
                [0, 0, 1920, 1080],
                rect,
            );
            assert_eq!(w * h, 200 * 100, "{transform:?} changed the area");
        }
    }

    /// A quarter turn about the centre moves every corner. The same turn about
    /// the top-left corner leaves that corner exactly where it was -- which is
    /// the whole difference, and the reason a card stack needs this.
    ///
    /// **Size, location and pivot each carry two different numbers**, because a
    /// pair that reads the same both ways round cannot see the components of
    /// that pair being swapped. A square rect hides a transposed size; an
    /// origin at `(100, 100)` hides a transposed location; and `(0.5, 0.5)` and
    /// `(0.0, 0.0)` are their own transpositions, so `(1.0, 0.0)` is here to be
    /// the pivot that is not. Three symmetries, three swaps, one rule.
    #[test]
    fn a_pivot_is_the_point_the_matrix_leaves_alone() {
        let rect = Rectangle::<f64, Logical>::new((40.0, 90.0).into(), (200.0, 100.0).into());
        let turn = Mat4::rotate_z(std::f32::consts::FRAC_PI_2);

        let centred = mesh(rect, turn, (0.5, 0.5), 1.0).expect("a mesh");
        let cornered = mesh(rect, turn, (0.0, 0.0), 1.0).expect("a mesh");

        // Vertex 0 is (u, v) = (0, 0): the rect's top-left, at (40, 90).
        let (cx, cy) = (cornered.vertices[0].x, cornered.vertices[0].y);
        assert!(
            (cx - 40.0).abs() < 0.01 && (cy - 90.0).abs() < 0.01,
            "a turn about the top-left leaves the top-left alone, got ({cx}, {cy})"
        );
        // A negative control, and only that: about the centre the top-left
        // moves. It is satisfied by almost any wrong pivot, so it catches
        // nothing on its own -- it is here so the assertion above cannot be
        // passed by a `mesh` that simply never moves anything.
        let (mx, my) = (centred.vertices[0].x, centred.vertices[0].y);
        assert!(
            (mx - 40.0).abs() > 1.0 || (my - 90.0).abs() > 1.0,
            "a turn about the centre moves the top-left, got ({mx}, {my})"
        );

        // `(1.0, 0.0)` is the top-right corner, at (240, 90). Read the pair the
        // other way round and the pivot is (40, 190) instead -- the mistake a
        // square window would hide.
        let top_right = mesh(rect, turn, (1.0, 0.0), 1.0).expect("a mesh");
        // Vertex 1 is (u, v) = (1, 0): the rect's top-right.
        let (tx, ty) = (top_right.vertices[1].x, top_right.vertices[1].y);
        assert!(
            (tx - 240.0).abs() < 0.01 && (ty - 90.0).abs() < 0.01,
            "a turn about the top-right leaves the top-right alone, got ({tx}, {ty})"
        );
    }

    /// And the default is the centre it replaced. Every unanimated window on
    /// the machine takes this path.
    ///
    /// Checked as a property, not against a second copy of the old arithmetic.
    /// A point reflection -- `scale(-1, -1, 1)` -- sends every point to its
    /// opposite through the pivot, so it maps the rect onto itself exactly when
    /// the pivot is the rect's centre. That is **one** equation, `2P = TL + BR`,
    /// asserted from both ends because the pair reads better than the half; it
    /// is not two independent checks, and dropping either loses nothing.
    ///
    /// There is no trigonometry in a point reflection and `scale` leaves the
    /// bottom row of the matrix at `0, 0, 0, 1`, so `w` is exactly 1, the
    /// perspective divide is exact, and every value involved is a small
    /// integer. Hence `assert_eq!` on f32 rather than an epsilon.
    ///
    /// The rect's origin is asymmetric for the reason the test above gives.
    #[test]
    fn the_default_pivot_is_the_centre_it_replaced() {
        let rect = Rectangle::<f64, Logical>::new((40.0, 70.0).into(), (300.0, 200.0).into());
        let through = mesh(rect, Mat4::scale(-1.0, -1.0, 1.0), (0.5, 0.5), 1.0).expect("a mesh");

        // Vertices 0 and 2 are (u, v) = (0, 0) and (1, 1): the rect's top-left
        // at (40, 70) and its bottom-right at (340, 270), which it exchanges.
        assert_eq!(
            (through.vertices[0].x, through.vertices[0].y),
            (340.0, 270.0),
            "the top-left reflects onto the bottom-right"
        );
        assert_eq!(
            (through.vertices[2].x, through.vertices[2].y),
            (40.0, 70.0),
            "and the bottom-right back onto the top-left: the same equation"
        );

        // A second pin on the pivot, through a matrix with trigonometry in it.
        //
        // **This says nothing about the rotation** -- it would pass identically
        // with `Mat4::IDENTITY`, and that is the point rather than a weakness.
        // The six offsets of the triangle list `[TL, TR, BR, TL, BR, BL]` sum
        // to exactly zero about the pivot, so for *any* linear map the mesh's
        // mean lands back on the pivot itself. Asserting it lands on the rect's
        // centre therefore pins the pivot there whatever matrix a window
        // carries -- which is the one thing this test is about. The finiteness
        // check is along for the ride.
        let turned = mesh(rect, Mat4::rotate_y(0.3), (0.5, 0.5), 1.0).expect("a mesh");
        let (mut sum_x, mut sum_y) = (0.0_f32, 0.0_f32);
        for corner in &turned.vertices {
            assert!(corner.x.is_finite() && corner.y.is_finite());
            sum_x += corner.x;
            sum_y += corner.y;
        }
        #[expect(clippy::cast_precision_loss, reason = "six vertices")]
        let count = turned.vertices.len() as f32;
        let (average_x, average_y) = (sum_x / count, sum_y / count);
        assert!(
            (average_x - 190.0).abs() < 0.001 && (average_y - 170.0).abs() < 0.001,
            "the mesh's mean is the pivot, which is the centre, got ({average_x}, {average_y})"
        );
    }

    /// The whole pane, as a part, is exactly the old mesh, bit for bit.
    #[test]
    fn the_whole_part_is_the_whole_mesh() {
        use super::{UnitRect, mesh_part};
        let rect = Rectangle::<f64, Logical>::new((40.0, 90.0).into(), (200.0, 100.0).into());
        let whole = mesh(rect, Mat4::rotate_y(0.3), (0.5, 0.5), 1.0).expect("a mesh");
        let part =
            mesh_part(rect, UnitRect::WHOLE, Mat4::rotate_y(0.3), (0.5, 0.5), 1.0).expect("a mesh");
        assert!(
            whole
                .vertices
                .iter()
                .zip(&part.vertices)
                .all(|(a, b)| a.x == b.x && a.y == b.y && a.u == b.u && a.v == b.v && a.q == b.q)
        );
    }

    /// **A Lua grid lands where the Rust genie put it**: the folder's grid
    /// through `mesh_grid`, against the Rust genie's grid through the same,
    /// through `rotate_y(0.3)` about a pivot of (1, 0), on a screen at
    /// x = 1920, which the shift moves both onto: every vertex within 1e-3
    /// physical px.
    #[test]
    fn a_lua_grid_lands_where_the_rust_genie_put_it() {
        let rect = Rectangle::new((2020.0, 100.0).into(), (800.0, 600.0).into());
        let to = Rectangle::new((2520.0, 1040.0).into(), (64.0, 32.0).into());
        let shift = (-1920.0, 0.0).into();
        let on_screen = Rectangle::new((100.0, 100.0).into(), (800.0, 600.0).into());
        let rust = solium_effects::Deform::Genie {
            progress: 0.37,
            spread: 1.4,
            axis: solium_effects::Axis::Down,
        };
        let oracle = super::oracle_grid(rect, to, rust, super::UnitRect::WHOLE);
        let expected = super::mesh_grid(
            &oracle,
            shift,
            on_screen,
            Mat4::rotate_y(0.3),
            (1.0, 0.0),
            1.0,
        )
        .expect("a mesh");
        let grid = crate::effect::geometry::tests::genie_grid(rect, to, 0.37, 1.4);
        assert_eq!((grid.cols, grid.rows), (oracle.cols, oracle.rows));
        let got = super::mesh_grid(
            &grid,
            shift,
            on_screen,
            Mat4::rotate_y(0.3),
            (1.0, 0.0),
            1.0,
        )
        .expect("a mesh");
        assert_eq!(expected.vertices().len(), got.vertices().len());
        for (a, b) in expected.vertices().iter().zip(got.vertices()) {
            assert!(
                (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3,
                "{a:?} {b:?}"
            );
        }
        // Moved onto the screen: the first point is the window's top-left
        // at progress 0.37 (the top row has not started), 100 px in.
        let flat = super::mesh_grid(&grid, shift, on_screen, Mat4::IDENTITY, (0.5, 0.5), 1.0)
            .expect("a mesh");
        assert!(
            (flat.vertices()[0].x - 100.0).abs() < 1e-3,
            "{:?}",
            flat.vertices()[0]
        );
    }

    /// **A grid short of its points draws nothing**, rather than a mesh
    /// that reads past what was written.
    #[test]
    fn a_grid_short_of_its_points_draws_nothing() {
        let rect = Rectangle::<f64, Logical>::new((0.0, 0.0).into(), (100.0, 100.0).into());
        let short = super::Grid {
            cols: 2,
            rows: 2,
            part: super::UnitRect::WHOLE,
            points: vec![0.0; 16],
        };
        assert!(
            super::mesh_grid(
                &short,
                (0.0, 0.0).into(),
                rect,
                Mat4::IDENTITY,
                (0.5, 0.5),
                1.0
            )
            .is_none()
        );
    }

    /// **A part of the pane lands where the whole pane puts those points**: a
    /// menu turns and folds with its window, about the window's own pivot.
    #[test]
    fn a_part_of_the_pane_lands_where_the_whole_pane_puts_those_points() {
        use super::{UnitRect, mesh_part};
        let rect = Rectangle::<f64, Logical>::new((40.0, 90.0).into(), (200.0, 100.0).into());
        let turn = Mat4::rotate_y(0.3);
        let whole = mesh(rect, turn, (0.5, 0.5), 1.0).expect("a mesh");
        let right = mesh_part(
            rect,
            UnitRect {
                u0: 0.5,
                v0: 0.0,
                u1: 1.0,
                v1: 1.0,
            },
            turn,
            (0.5, 0.5),
            1.0,
        )
        .expect("a mesh");
        // One cell each: whole is (0,0) (1,0) (1,1) …; the right half's (1,0)
        // corner is the whole's (1,0) corner.
        let (a, b) = (whole.vertices[1], right.vertices[1]);
        assert!((a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3);
        assert!(
            (right.vertices[1].u - 1.0).abs() < 1e-6,
            "texture coordinates run 0..1 across the part"
        );
        // A part reaching past the pane is extrapolated, not clamped.
        let past = mesh_part(
            rect,
            UnitRect {
                u0: 0.8,
                v0: 0.8,
                u1: 1.3,
                v1: 1.4,
            },
            Mat4::IDENTITY,
            (0.5, 0.5),
            1.0,
        )
        .expect("a mesh");
        assert!((past.vertices[2].x - (40.0 + 200.0 * 1.3)).abs() < 1e-3);
    }
}
