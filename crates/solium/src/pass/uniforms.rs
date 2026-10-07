//! A smithay registration read from the source it registers (#95).
//!
//! Smithay checks a uniform's value against the type it was registered with,
//! never against the type the shader declares, so a hand-written list can
//! disagree with its shader and draw every fragment wrong (#94: a `vec4`
//! registered as `_1f`). Here the list is read from the source, so nothing is
//! written by hand. Smithay and `solium_effects` only, so `dev/wirecheck`
//! includes this file by `#[path]` and registers exactly what the compositor
//! does. `pass::tests::the_registered_uniforms_are_the_declared_ones_but_smithays`.

use smithay::backend::renderer::gles::{UniformName, UniformType};
use solium_effects::glsl::{self, Glsl};

/// The uniforms smithay's custom programs bind themselves, read from
/// `compile_custom_texture_shader` and `compile_custom_pixel_shader` in
/// smithay 0.7's `gles/mod.rs`: `tex`, `alpha`, `matrix` and `tex_matrix` in a
/// texture program, `size`, `alpha`, `matrix` and `tex_matrix` in a pixel
/// program, and `tint` in both programs' debug variants. A texture program is
/// given no `size`, so a texture source measures itself with a uniform of its
/// own (`fragment::ROUNDED_CORNERS`' `tex_size`).
/// `pass::tests::the_registered_uniforms_are_the_declared_ones_but_smithays`.
pub(crate) const SMITHAYS: [&str; 6] = ["tex", "alpha", "tint", "matrix", "tex_matrix", "size"];

/// A smithay registration built from what `source` declares, smithay's own
/// left out, each with its declared type.
/// `pass::tests::the_registered_uniforms_are_the_declared_ones_but_smithays`.
pub(crate) fn registration(source: &str) -> Vec<UniformName<'static>> {
    glsl::uniforms(source)
        .into_iter()
        .filter(|uniform| !SMITHAYS.contains(&uniform.name.as_str()))
        .filter_map(|uniform| {
            let kind = match uniform.ty {
                Glsl::Float => UniformType::_1f,
                Glsl::Vec2 => UniformType::_2f,
                Glsl::Vec3 => UniformType::_3f,
                Glsl::Vec4 => UniformType::_4f,
                Glsl::Mat3 => UniformType::Matrix3x3,
                _ => return None,
            };
            Some(UniformName::new(uniform.name, kind))
        })
        .collect()
}
