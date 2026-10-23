//! A geometry or pixels effect's numeric params, packed: Smithay-free and
//! std only, so `dev/wirecheck` includes it beside `effect/gl.rs`, whose
//! `PixelUniforms` carries them (Task 29). `geometry` binds and names them.

/// The most numeric params a geometry or pixels effect may pack:
/// `effects.limits.params`' upper bound. A representation's size, not a
/// behaviour (Ruling 28): a geometry rides `present::Frame`, which is `Copy`,
/// so its params are an inline array, and the key picks how many of it may
/// be used. `settings::tests::the_engines_keys_read_with_their_defaults_and_bounds`.
pub(crate) const PARAMS_MAX: usize = 64;

/// A geometry or pixels effect's numeric params, in their sorted order: a
/// number, an integer as its number, a boolean as 0 or 1, a `vec4` as four.
/// An inline array, so a geometry can ride `present::Frame`, which is `Copy`.
/// `geometry::tests::params_pack_up_to_their_limit_and_lerp_only_like_with_like`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Params {
    len: u8,
    values: [f32; PARAMS_MAX],
}

/// Empty (an array past 32 has no derived `Default`).
impl Default for Params {
    fn default() -> Self {
        Self {
            len: 0,
            values: [0.0; PARAMS_MAX],
        }
    }
}

impl Params {
    /// The numbers, at most `limit` of them (`effects.limits.params`); more
    /// is `Err` with how many, for a problem naming the key.
    /// `geometry::tests::params_pack_up_to_their_limit_and_lerp_only_like_with_like`.
    pub(crate) fn pack(numbers: &[f32], limit: usize) -> Result<Self, usize> {
        if numbers.len() > limit.min(PARAMS_MAX) {
            return Err(numbers.len());
        }
        let mut params = Self::default();
        let len = u8::try_from(numbers.len()).map_err(|_| numbers.len())?;
        params
            .values
            .get_mut(..numbers.len())
            .ok_or(numbers.len())?
            .copy_from_slice(numbers);
        params.len = len;
        Ok(params)
    }

    pub(crate) fn as_slice(&self) -> &[f32] {
        self.values.get(..usize::from(self.len)).unwrap_or(&[])
    }

    /// Each lerped toward `other`'s when both hold as many; `other`'s
    /// otherwise. Whether both are one effect's, which a count cannot tell,
    /// is `present::Geometry::mix`'s to ask
    /// (`present::tests::another_effects_params_are_the_destinations_not_a_blend`).
    /// `geometry::tests::params_pack_up_to_their_limit_and_lerp_only_like_with_like`.
    pub(crate) fn lerp(self, other: Self, t: f64) -> Self {
        if self.len != other.len {
            return other;
        }
        let mut params = self;
        for (mine, theirs) in params
            .values
            .iter_mut()
            .zip(other.values)
            .take(usize::from(self.len))
        {
            #[expect(clippy::cast_possible_truncation, reason = "a param, as stored")]
            let blended = (f64::from(*mine) + (f64::from(theirs) - f64::from(*mine)) * t) as f32;
            *mine = blended;
        }
        params
    }
}
