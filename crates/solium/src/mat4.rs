//! 4x4 transforms, for drawing a window somewhere a rectangle cannot reach.
//!
//! Column-major, like GL, so a matrix built here can be handed to a shader
//! without transposing it — and so that anyone reading this beside the GL code
//! is reading the same layout in both places.
//!
//! Only what the presentation layer needs: build one, compose them, and
//! project a corner through the result. No inverse, because hit-testing
//! deliberately stays on the undeformed rectangle — see the spike in
//! `docs/spikes/2026-09-06-3d-presentation.md`.

/// A 4x4 transform, column-major.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mat4(pub(crate) [f32; 16]);

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat4 {
    pub(crate) const IDENTITY: Self = Self([
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]);

    /// Whether this is the identity, and so whether the cheap path applies.
    ///
    /// Compared with a tolerance rather than exactly: a matrix composed from
    /// a rotation of zero degrees is identity in intent and not always in the
    /// last bit, and a window should not fall onto the expensive path because
    /// a cosine returned 0.9999999.
    pub(crate) fn is_identity(&self) -> bool {
        self.0
            .iter()
            .zip(Self::IDENTITY.0.iter())
            .all(|(a, b)| (a - b).abs() < 1e-6)
    }

    pub(crate) fn translate(x: f32, y: f32, z: f32) -> Self {
        let mut m = Self::IDENTITY;
        m.0[12] = x;
        m.0[13] = y;
        m.0[14] = z;
        m
    }

    pub(crate) fn scale(x: f32, y: f32, z: f32) -> Self {
        let mut m = Self::IDENTITY;
        m.0[0] = x;
        m.0[5] = y;
        m.0[10] = z;
        m
    }

    pub(crate) fn rotate_x(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        let mut m = Self::IDENTITY;
        m.0[5] = c;
        m.0[6] = s;
        m.0[9] = -s;
        m.0[10] = c;
        m
    }

    pub(crate) fn rotate_y(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        let mut m = Self::IDENTITY;
        m.0[0] = c;
        m.0[2] = -s;
        m.0[8] = s;
        m.0[10] = c;
        m
    }

    pub(crate) fn rotate_z(radians: f32) -> Self {
        let (s, c) = radians.sin_cos();
        let mut m = Self::IDENTITY;
        m.0[0] = c;
        m.0[1] = s;
        m.0[4] = -s;
        m.0[5] = c;
        m
    }

    /// A perspective divide at `distance` from the viewer.
    ///
    /// Not a full projection matrix: the compositor already works in output
    /// pixels, so all this adds is the `w` that makes far edges converge.
    /// `distance` is in the same pixels — a smaller number is a wider lens and
    /// a more violent foreshortening.
    pub(crate) fn perspective(distance: f32) -> Self {
        let mut m = Self::IDENTITY;
        if distance.abs() > f32::EPSILON {
            m.0[11] = -1.0 / distance;
        }
        m
    }

    /// `self` then `other`, as transforms are read left to right.
    pub(crate) fn then(self, other: Self) -> Self {
        let a = &other.0;
        let b = &self.0;
        let mut out = [0.0_f32; 16];
        for column in 0..4 {
            for row in 0..4 {
                let mut sum = 0.0;
                for step in 0..4 {
                    sum += a[step * 4 + row] * b[column * 4 + step];
                }
                out[column * 4 + row] = sum;
            }
        }
        Self(out)
    }

    /// Project a point, dividing through by `w`.
    ///
    /// Returns `None` when the point lands at or behind the viewer, where the
    /// divide is meaningless — a caller that drew it anyway would get a corner
    /// flipped to the far side of the screen.
    pub(crate) fn project(&self, x: f32, y: f32, z: f32) -> Option<(f32, f32)> {
        let m = &self.0;
        let out_x = m[0] * x + m[4] * y + m[8] * z + m[12];
        let out_y = m[1] * x + m[5] * y + m[9] * z + m[13];
        let out_w = m[3] * x + m[7] * y + m[11] * z + m[15];
        if out_w <= 1e-6 {
            return None;
        }
        Some((out_x / out_w, out_y / out_w))
    }
}

#[cfg(test)]
mod tests {
    use super::Mat4;

    #[test]
    fn identity_leaves_a_point_alone() {
        assert_eq!(Mat4::IDENTITY.project(3.0, 4.0, 0.0), Some((3.0, 4.0)));
        assert!(Mat4::IDENTITY.is_identity());
    }

    #[test]
    fn translation_moves_it() {
        let m = Mat4::translate(10.0, -5.0, 0.0);
        assert_eq!(m.project(1.0, 1.0, 0.0), Some((11.0, -4.0)));
        assert!(!m.is_identity());
    }

    #[test]
    fn scale_multiplies_it() {
        assert_eq!(
            Mat4::scale(2.0, 3.0, 1.0).project(4.0, 5.0, 0.0),
            Some((8.0, 15.0))
        );
    }

    /// A quarter turn about z sends +x to +y.
    #[test]
    fn rotation_turns_the_plane() {
        let (x, y) = Mat4::rotate_z(std::f32::consts::FRAC_PI_2)
            .project(1.0, 0.0, 0.0)
            .expect("in front of the viewer");
        assert!(x.abs() < 1e-6, "x went to {x}");
        assert!((y - 1.0).abs() < 1e-6, "y went to {y}");
    }

    /// Composition reads left to right: move, *then* double.
    #[test]
    fn composition_applies_in_reading_order() {
        let m = Mat4::translate(1.0, 0.0, 0.0).then(Mat4::scale(2.0, 2.0, 1.0));
        assert_eq!(m.project(0.0, 0.0, 0.0), Some((2.0, 0.0)));
    }

    /// The property that makes it 3D rather than 2D: with perspective, a
    /// rotation about y makes one edge shorter than the other. An affine
    /// transform cannot do that, however it is composed.
    #[test]
    fn perspective_makes_parallel_edges_converge() {
        let m = Mat4::rotate_y(0.6).then(Mat4::perspective(800.0));
        let top = m.project(200.0, -100.0, 0.0).expect("visible");
        let bottom = m.project(200.0, 100.0, 0.0).expect("visible");
        // The far edge is pushed toward the centre, so the two corners of the
        // receding side no longer sit at the same distance from the axis.
        let near = m.project(-200.0, -100.0, 0.0).expect("visible");
        let far_width = (top.0 - 0.0).abs();
        let near_width = (near.0 - 0.0).abs();
        assert!(
            (far_width - near_width).abs() > 1.0,
            "no foreshortening: {far_width} vs {near_width}"
        );
        assert!((top.1 - bottom.1).abs() > 1.0, "the edge kept some height");
    }

    /// A rotation of nothing is still the cheap path.
    #[test]
    fn a_zero_rotation_stays_on_the_fast_path() {
        assert!(Mat4::rotate_y(0.0).is_identity());
        assert!(
            Mat4::rotate_y(0.0)
                .then(Mat4::perspective(0.0))
                .is_identity()
        );
    }

    /// Behind the viewer there is no answer, and inventing one flips a corner
    /// to the far side of the screen.
    #[test]
    fn a_point_behind_the_viewer_has_no_projection() {
        let m = Mat4::perspective(100.0);
        assert_eq!(m.project(0.0, 0.0, 200.0), None);
    }
}
