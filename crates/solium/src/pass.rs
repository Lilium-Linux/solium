//! Passes: what an effect reading its own node costs, and where it is paid.
//!
//! An effect that declares `Inputs::Nothing` never reaches this file. One that
//! declares `Inputs::SelfTexture` cannot be a single element in a flat list,
//! because it needs the node's pixels before it can draw: the client's
//! surfaces are rendered into a texture of their own, and *that* is drawn,
//! through a fragment program, in the client's place.
//!
//! The capture is [`crate::offscreen::capture`], which already exists for the
//! genie and already keeps its texture on the pane rather than allocating one
//! a frame. That was made a prerequisite of this work rather than a follow-up
//! for exactly this reason: an effect system multiplies a per-frame allocation
//! by the number of animating windows.

use smithay::backend::renderer::gles::{GlesRenderer, GlesTexProgram, UniformName, UniformType};
use solium_effects::fragment::{Effect, Inputs, RADIUS_UNIFORM, ROUNDED_CORNERS, SIZE_UNIFORM};

/// Whether this node's effects need the node rendered to a texture first, and
/// which effect wants it.
///
/// `None` is the answer for every window on a machine nobody has styled, and
/// it is asked once per pane per frame, so it is a slice walk and nothing
/// more. The first effect wins: one pass, one program, and a node wanting two
/// fragment effects at once is a thing to design when something wants it.
///
/// [`Inputs::Backdrop`] answers `None` here, which is correct and is *not* the
/// whole answer: nothing constructs one yet, and the day something does, an
/// effect that reads what is beneath the node must be refused out loud rather
/// than quietly skipped. That refusal belongs to whoever plans the pass, not
/// to this filter -- see the note on [`Inputs::Backdrop`] itself.
// Read by the pass that runs one, which is a later task in the same plan; the
// tests below are the only caller today. `expect` and not `allow`, as `style`
// and `mat4` do, so the marker cannot outlive the reason for it.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the question is asked by the renderer, which is a later task \
                  in the same plan"
    )
)]
pub(crate) fn needs_pass(effects: &[Effect]) -> Option<Effect> {
    effects
        .iter()
        .copied()
        .find(|effect| !effect.is_none_effect() && effect.inputs() == Inputs::SelfTexture)
}

/// The compiled fragment programs, one of each, for the life of the renderer.
///
/// Compiling a shader is not a per-frame cost anybody should pay, and
/// `compile_custom_texture_shader` calls `make_current` -- which is not free
/// and, worse, is exactly the kind of thing that has broken Qt's stale
/// thread-local `currentContext` five separate times in this codebase.
///
/// **That hazard does not reach this call, and the reason is worth writing
/// down because it is not obvious from the call site.** Two things hold it:
///
/// * It cannot run inside a live `GlesFrame`. `GlesFrame` holds
///   `&'frame mut GlesRenderer` (smithay `gles/mod.rs:328`), so `&mut
///   GlesRenderer` in this signature is a borrow the frame already has. The
///   invariant `qml::no_frame_in_flight` states by convention is, for this one
///   call, a compile error -- which is why there is no runtime assertion here.
/// * Between frames it is one more ordinary `GlesRenderer` entry point.
///   `import_dmabuf`, `bind`, `render` and `wait` all `make_current` the same
///   way, on every frame, and Qt's belief is corrected on the way *in* to Qt
///   rather than on the way out: `solium_qml_scene_render_gpu` and
///   `solium_qml_scene_free` open with `clear_stale_current_context`, and
///   `solium_qml_scene_rebind` with `take_the_thread`. Compiling on first use
///   adds no new kind of interleaving, only one more instance of one that is
///   already handled.
#[derive(Debug, Default)]
#[expect(
    dead_code,
    reason = "held by the renderer and asked for a program there, which is a \
              later task in the same plan"
)]
pub(crate) struct Programs {
    rounded: Option<GlesTexProgram>,
    /// Set once a compile has been tried and failed, so the warning is logged
    /// once rather than at sixty or two hundred and sixty hertz.
    rounded_failed: bool,
}

#[expect(
    dead_code,
    reason = "asked for a program by the renderer, which is a later task in \
              the same plan"
)]
impl Programs {
    /// The rounded-corner program, compiling it on first use.
    ///
    /// `None` means the shader did not compile, and the caller draws the
    /// window square rather than not at all: a driver that cannot build this
    /// program should cost someone their rounded corners, not their desktop.
    pub(crate) fn rounded(&mut self, renderer: &mut GlesRenderer) -> Option<&GlesTexProgram> {
        if self.rounded.is_none() && !self.rounded_failed {
            match renderer.compile_custom_texture_shader(
                ROUNDED_CORNERS,
                &[
                    UniformName::new(RADIUS_UNIFORM, UniformType::_1f),
                    // Ours because smithay gives a texture program no `size`.
                    UniformName::new(SIZE_UNIFORM, UniformType::_2f),
                ],
            ) {
                Ok(program) => self.rounded = Some(program),
                Err(err) => {
                    // Latched before the warning and never cleared, because the
                    // thing that failed is a string constant against a driver:
                    // it will fail identically on the next frame and the one
                    // after, and a warning per frame per window is how a log
                    // stops being readable at the moment somebody needs it.
                    self.rounded_failed = true;
                    tracing::warn!(
                        ?err,
                        "the rounded-corner shader did not compile; windows will be drawn square"
                    );
                }
            }
        }
        self.rounded.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The question the renderer asks every pane, every frame. It has to be
    /// cheap and it has to answer `None` for the overwhelmingly common case,
    /// or the ordinary window stops being ordinary.
    #[test]
    fn a_pane_with_no_effects_needs_no_pass() {
        assert_eq!(needs_pass(&[]), None);
    }

    #[test]
    fn an_effect_reading_self_needs_a_pass() {
        let rounded = Effect::rounded(10.0);
        assert_eq!(needs_pass(&[rounded]), Some(rounded));
    }

    /// A zero radius reaches here only if `style::load` let it through, and
    /// `needs_pass` refusing it too is deliberate belt and braces: the cost of
    /// being wrong is every window on the machine rendering offscreen.
    #[test]
    fn a_none_effect_needs_no_pass() {
        assert_eq!(needs_pass(&[Effect::rounded(0.0)]), None);
    }

    /// The first effect wins, and it is the first one that *needs a pass* --
    /// not the first one in the list.
    ///
    /// Without this, `a_none_effect_needs_no_pass` is satisfied by a
    /// `needs_pass` that stops at the first element and reports whatever it
    /// finds there, because a one-element list cannot tell "skipped it" from
    /// "stopped at it". A style that declared a none-effect and a real one
    /// would then round nothing.
    #[test]
    fn a_none_effect_does_not_hide_the_one_behind_it() {
        let rounded = Effect::rounded(8.0);
        assert_eq!(needs_pass(&[Effect::rounded(0.0), rounded]), Some(rounded));
    }
}
