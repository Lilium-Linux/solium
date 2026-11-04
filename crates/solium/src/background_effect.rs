//! What is behind a surface, blurred at its client's asking (#78):
//! `ext-background-effect-v1`.
//!
//! A client cannot blur what is behind it -- it cannot see it -- so it names
//! the part of its surface to blur, with `set_blur_region`, and the
//! compositor draws it. A panel, a dock, a launcher, a dialog.
//!
//! **The region is data, and what is drawn there is configuration.** This
//! keeps each surface's region, double-buffered as the protocol says (it
//! takes effect at the surface's next commit), as disjoint rectangles in the
//! surface's own space ([`region_of`]). An effect rule draws there: its
//! `match` names a surface that asked with `blur_region = true`, and its
//! `mask = "region"` cuts its result to the region rather than to the part's
//! shape. `effects.client_blur` in `config.lua` is the rule the configuration
//! ships, expanded ahead of yours, so a rule of yours for the same part and
//! slot wins, and `false` there leaves the region to your own rules.
//!
//! `config.effects.client_blur = false` also stops the global being offered:
//! a client asking for a blur nobody draws would be told it has one.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use smithay::{
    reexports::{
        wayland_protocols::ext::background_effect::v1::server::{
            ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1},
            ext_background_effect_surface_v1::{self, ExtBackgroundEffectSurfaceV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, Weak,
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Rectangle},
    wayland::compositor::{
        Cacheable, RectangleKind, RegionAttributes, get_region_attributes, with_states,
    },
};

use crate::state::Solium;

/// Whether the global is offered: read by its `can_view`.
#[derive(Debug, Clone)]
pub(crate) struct Offered(Arc<AtomicBool>);

impl Offered {
    fn on(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// The global, offered from the start: the shipped `effects.client_blur`
/// draws a blur, and a configuration turning it off takes it away again.
#[derive(Debug)]
pub(crate) struct BackgroundEffect {
    offered: Offered,
}

impl BackgroundEffect {
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let offered = Offered(Arc::new(AtomicBool::new(true)));
        display.create_global::<Solium, ExtBackgroundEffectManagerV1, _>(1, offered.clone());
        Self { offered }
    }

    /// Offer the global, or hide it from clients that have not bound it. A
    /// client that has keeps its objects; nothing draws their regions once
    /// the rule that did is gone.
    pub(crate) fn offer(&self, on: bool) {
        self.offered.0.store(on, Ordering::Relaxed);
    }
}

/// A surface's blur region, double-buffered: disjoint rectangles in the
/// surface's own logical space, or none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BlurRegion(pub(crate) Option<Vec<Rectangle<i32, Logical>>>);

impl Cacheable for BlurRegion {
    fn commit(&mut self, _dh: &DisplayHandle) -> Self {
        self.clone()
    }

    fn merge_into(self, into: &mut Self, _dh: &DisplayHandle) {
        *into = self;
    }
}

/// The `ext_background_effect_surface_v1` a surface has, so a second one is
/// the protocol error it is.
#[derive(Debug, Default)]
struct Taken(Mutex<Option<Weak<ExtBackgroundEffectSurfaceV1>>>);

/// A region as disjoint rectangles: each added rectangle less what is
/// already covered, and each subtracted one taken out of all of them, in the
/// order the client gave them. Disjoint, so a result drawn once per
/// rectangle covers each pixel once.
/// `tests::a_region_is_its_rectangles_added_and_subtracted_in_order_and_disjoint`.
pub(crate) fn disjoint(region: &RegionAttributes) -> Vec<Rectangle<i32, Logical>> {
    let mut rects: Vec<Rectangle<i32, Logical>> = Vec::new();
    for (kind, rect) in &region.rects {
        if rect.size.w <= 0 || rect.size.h <= 0 {
            continue;
        }
        match kind {
            RectangleKind::Add => {
                let fresh = rect.subtract_rects(rects.iter().copied());
                rects.extend(fresh);
            }
            RectangleKind::Subtract => {
                rects = rects
                    .into_iter()
                    .flat_map(|each| each.subtract_rect(*rect))
                    .collect();
            }
        }
    }
    rects
}

/// The blur region a surface's client asked for, as it was last committed:
/// disjoint rectangles in the surface's own space, or `None` when it asked
/// for none.
pub(crate) fn region_of(surface: &WlSurface) -> Option<Vec<Rectangle<i32, Logical>>> {
    with_states(surface, |states| {
        states.cached_state.get::<BlurRegion>().current().0.clone()
    })
    .filter(|rects| !rects.is_empty())
}

impl GlobalDispatch<ExtBackgroundEffectManagerV1, Offered> for Solium {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ExtBackgroundEffectManagerV1>,
        _offered: &Offered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        manager.capabilities(ext_background_effect_manager_v1::Capability::Blur);
    }

    fn can_view(_client: Client, offered: &Offered) -> bool {
        offered.on()
    }
}

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        manager: &ExtBackgroundEffectManagerV1,
        request: ext_background_effect_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let ext_background_effect_manager_v1::Request::GetBackgroundEffect { id, surface } =
            request
        else {
            return;
        };
        let taken = with_states(&surface, |states| {
            states.data_map.insert_if_missing_threadsafe(Taken::default);
            states
                .data_map
                .get::<Taken>()
                .and_then(|taken| taken.0.lock().ok().map(|held| held.clone()))
                .flatten()
                .is_some_and(|held| held.upgrade().is_ok())
        });
        // Every new object is made, a refused one too, before the error.
        let object = data_init.init(id, surface.downgrade());
        if taken {
            manager.post_error(
                ext_background_effect_manager_v1::Error::BackgroundEffectExists,
                "this surface already has a background effect object",
            );
            return;
        }
        with_states(&surface, |states| {
            if let Some(taken) = states.data_map.get::<Taken>()
                && let Ok(mut held) = taken.0.lock()
            {
                *held = Some(object.downgrade());
            }
        });
    }
}

impl Dispatch<ExtBackgroundEffectSurfaceV1, Weak<WlSurface>> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        object: &ExtBackgroundEffectSurfaceV1,
        request: ext_background_effect_surface_v1::Request,
        surface: &Weak<WlSurface>,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let Ok(surface) = surface.upgrade() else {
            object.post_error(
                ext_background_effect_surface_v1::Error::SurfaceDestroyed,
                "the surface this background effect was for is gone",
            );
            return;
        };
        match request {
            ext_background_effect_surface_v1::Request::SetBlurRegion { region } => {
                let rects = region
                    .as_ref()
                    .map(|region| disjoint(&get_region_attributes(region)));
                with_states(&surface, |states| {
                    *states.cached_state.get::<BlurRegion>().pending() = BlurRegion(rects);
                });
            }
            ext_background_effect_surface_v1::Request::Destroy => {
                // The protocol: the effect is gone at the surface's next
                // commit, as a null region would be.
                with_states(&surface, |states| {
                    *states.cached_state.get::<BlurRegion>().pending() = BlurRegion(None);
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    fn area(rects: &[Rectangle<i32, Logical>]) -> i32 {
        rects.iter().map(|each| each.size.w * each.size.h).sum()
    }

    #[test]
    fn a_region_is_its_rectangles_added_and_subtracted_in_order_and_disjoint() {
        let region = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, rect(0, 0, 100, 40)),
                // Overlapping the first: only what it adds is new.
                (RectangleKind::Add, rect(50, 0, 100, 40)),
                // A hole in the middle of both.
                (RectangleKind::Subtract, rect(40, 10, 20, 20)),
                // Nothing at all.
                (RectangleKind::Add, rect(0, 0, 0, 10)),
            ],
        };
        let rects = disjoint(&region);
        assert_eq!(area(&rects), 150 * 40 - 20 * 20, "{rects:?}");
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!a.overlaps(*b), "{a:?} overlaps {b:?}");
            }
        }
        assert!(
            rects
                .iter()
                .all(|each| !each.overlaps(rect(40, 10, 20, 20))),
            "the hole was filled: {rects:?}"
        );
    }

    #[test]
    fn a_region_added_after_a_subtraction_fills_it_again() {
        let region = RegionAttributes {
            rects: vec![
                (RectangleKind::Add, rect(0, 0, 10, 10)),
                (RectangleKind::Subtract, rect(0, 0, 10, 10)),
                (RectangleKind::Add, rect(2, 2, 4, 4)),
            ],
        };
        assert_eq!(disjoint(&region), [rect(2, 2, 4, 4)]);
    }
}
