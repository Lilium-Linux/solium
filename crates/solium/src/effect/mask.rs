//! Each part's mask (\[16\] §2): what an effect's result is cut to in a
//! slot, unless the effect reads `shape` and owns its edges. Pure.

use smithay::utils::{Logical, Rectangle, Size};
use solium_effects::fragment::Corners;

use crate::decoration::Insets;

/// What a part's result is cut to (Ruling 15).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Mask {
    /// A rounded rectangle in the part's drawn space.
    Rect {
        rect: Rectangle<f64, Logical>,
        radii: Corners,
    },
    /// The self capture's own alpha (`mask = "alpha"`, T1 only).
    #[expect(
        dead_code,
        reason = "Task 21's self tier cuts a result by its own alpha"
    )]
    OwnAlpha,
}

/// A client's mask: its drawn rectangle and its own radii, square when it
/// is not rounded. `tests::a_clients_mask_is_its_drawn_rect_and_radii`.
pub(crate) fn client_mask(client: Rectangle<f64, Logical>, rounding: Option<Corners>) -> Mask {
    Mask::Rect {
        rect: client,
        radii: rounding.unwrap_or_else(|| Corners::all(0.0)),
    }
}

/// A pane's mask: its outer rectangle, every corner at the largest radius.
/// `tests::a_panes_mask_is_its_outer_rect_rounded_at_the_largest_radius`.
pub(crate) fn pane_mask(outer: Rectangle<f64, Logical>, radii: Option<Corners>) -> Mask {
    let largest = radii.map_or(0.0, |radii| radii.largest());
    Mask::Rect {
        rect: outer,
        radii: Corners::all(largest),
    }
}

/// The titlebar: the band of the strictly largest inset, its two outer
/// corners at the largest client radius. `None` with no strictly largest.
/// `tests::the_titlebar_is_the_band_of_the_strictly_largest_inset`,
/// `tests::a_ring_style_has_no_titlebar` and
/// `tests::the_titlebars_outer_corners_take_the_largest_client_radius`.
pub(crate) fn titlebar(
    insets: Insets,
    outer: Size<i32, Logical>,
    radii: Corners,
) -> Option<(Rectangle<i32, Logical>, Corners)> {
    let sides = [insets.top, insets.right, insets.bottom, insets.left];
    let largest = *sides.iter().max()?;
    if largest <= 0 || sides.iter().filter(|side| **side == largest).count() > 1 {
        return None;
    }
    let r = radii.largest();
    let (w, h) = (outer.w, outer.h);
    // A side bar runs between the top and bottom bands, as `Insets::bands`'
    // sides do (`tests::the_titlebar_is_the_band_of_the_strictly_largest_inset`).
    let middle = (h - insets.top - insets.bottom).max(0);
    let square = Corners::all(0.0);
    Some(if insets.top == largest {
        (
            Rectangle::new((0, 0).into(), (w, insets.top).into()),
            Corners {
                top_left: r,
                top_right: r,
                ..square
            },
        )
    } else if insets.bottom == largest {
        (
            Rectangle::new((0, h - insets.bottom).into(), (w, insets.bottom).into()),
            Corners {
                bottom_left: r,
                bottom_right: r,
                ..square
            },
        )
    } else if insets.left == largest {
        (
            Rectangle::new((0, insets.top).into(), (insets.left, middle).into()),
            square,
        )
    } else {
        (
            Rectangle::new(
                (w - insets.right, insets.top).into(),
                (insets.right, middle).into(),
            ),
            square,
        )
    })
}

/// Where a named region of a pane is. P15's seam: FX2 has the insets only,
/// and P15 adds a source of a scene's published regions that wins for a name
/// it publishes.
pub(crate) trait RegionSource {
    fn region(&self, name: &str) -> Option<(Rectangle<i32, Logical>, Corners)>;
}

/// The regions a pane style's insets give: the titlebar only.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FromInsets {
    pub(crate) insets: Insets,
    pub(crate) outer: Size<i32, Logical>,
    pub(crate) radii: Corners,
}

impl RegionSource for FromInsets {
    /// `tests::the_insets_name_the_titlebar_and_nothing_else`.
    fn region(&self, name: &str) -> Option<(Rectangle<i32, Logical>, Corners)> {
        (name == "titlebar")
            .then(|| titlebar(self.insets, self.outer, self.radii))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Rectangle, Size};
    use solium_effects::fragment::Corners;

    use super::{FromInsets, Mask, RegionSource, client_mask, pane_mask, titlebar};
    use crate::decoration::Insets;

    fn outer() -> Size<i32, smithay::utils::Logical> {
        (800, 600).into()
    }

    fn square() -> Corners {
        Corners::all(0.0)
    }

    /// **The titlebar is the band of the strictly largest inset**: top for
    /// `top`, bottom for `bottom`, left for `left`, between the top and
    /// bottom bands on a side.
    #[test]
    fn the_titlebar_is_the_band_of_the_strictly_largest_inset() {
        let top = titlebar(
            Insets {
                top: 32,
                ..Insets::NONE
            },
            outer(),
            square(),
        )
        .expect("a bar");
        assert_eq!(top.0, Rectangle::new((0, 0).into(), (800, 32).into()));
        let bottom = titlebar(
            Insets {
                bottom: 30,
                ..Insets::NONE
            },
            outer(),
            square(),
        )
        .expect("a bar");
        assert_eq!(bottom.0, Rectangle::new((0, 570).into(), (800, 30).into()));
        let left = titlebar(
            Insets {
                left: 34,
                top: 4,
                bottom: 4,
                right: 4,
            },
            outer(),
            square(),
        )
        .expect("a bar");
        assert_eq!(left.0, Rectangle::new((0, 4).into(), (34, 592).into()));
        let right = titlebar(
            Insets {
                right: 30,
                top: 4,
                bottom: 4,
                left: 4,
            },
            outer(),
            square(),
        )
        .expect("a bar");
        assert_eq!(right.0, Rectangle::new((770, 4).into(), (30, 592).into()));
        // `reactive`'s: a top bar over a ring.
        let reactive = titlebar(
            Insets {
                top: 30,
                right: 4,
                bottom: 4,
                left: 4,
            },
            outer(),
            square(),
        )
        .expect("a bar");
        assert_eq!(reactive.0, Rectangle::new((0, 0).into(), (800, 30).into()));
    }

    /// `border` and `proximity` are rings: no side is the largest, so no
    /// band is the bar. Two sides tied are no titlebar either.
    #[test]
    fn a_ring_style_has_no_titlebar() {
        let ring = Insets {
            top: 4,
            right: 4,
            bottom: 4,
            left: 4,
        };
        assert!(titlebar(ring, outer(), square()).is_none());
        let tied = Insets {
            top: 32,
            bottom: 32,
            ..Insets::NONE
        };
        assert!(titlebar(tied, outer(), square()).is_none());
    }

    #[test]
    fn a_style_with_no_insets_has_no_titlebar() {
        assert!(titlebar(Insets::NONE, outer(), square()).is_none());
        assert!(
            FromInsets {
                insets: Insets::NONE,
                outer: outer(),
                radii: square(),
            }
            .region("titlebar")
            .is_none()
        );
    }

    /// The bar's outer corners take the largest client radius (the
    /// `clientRadius` every layer is told); the corners against the client
    /// are square, and a side bar's are all square.
    #[test]
    fn the_titlebars_outer_corners_take_the_largest_client_radius() {
        let radii = Corners {
            top_left: 6.0,
            top_right: 12.0,
            bottom_left: 0.0,
            bottom_right: 0.0,
        };
        let corners = |insets| {
            let (_, c) = titlebar(insets, outer(), radii).expect("a bar");
            (c.top_left, c.top_right, c.bottom_left, c.bottom_right)
        };
        assert_eq!(
            corners(Insets {
                top: 32,
                ..Insets::NONE
            }),
            (12.0, 12.0, 0.0, 0.0)
        );
        assert_eq!(
            corners(Insets {
                bottom: 30,
                ..Insets::NONE
            }),
            (0.0, 0.0, 12.0, 12.0)
        );
        assert_eq!(
            corners(Insets {
                left: 34,
                ..Insets::NONE
            }),
            (0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(
            corners(Insets {
                right: 34,
                ..Insets::NONE
            }),
            (0.0, 0.0, 0.0, 0.0)
        );
    }

    /// The region source FX2 has names the titlebar and nothing else: every
    /// other region is P15's.
    #[test]
    fn the_insets_name_the_titlebar_and_nothing_else() {
        let radii = Corners::all(8.0);
        let insets = Insets {
            top: 32,
            ..Insets::NONE
        };
        let source = FromInsets {
            insets,
            outer: outer(),
            radii,
        };
        assert_eq!(source.region("titlebar"), titlebar(insets, outer(), radii));
        assert!(source.region("titlebar").is_some());
        assert!(source.region("bar").is_none());
        assert!(source.region("").is_none());
    }

    /// **A client's mask is its drawn rectangle and its own radii**; an
    /// unrounded client's is square.
    #[test]
    fn a_clients_mask_is_its_drawn_rect_and_radii() {
        let rect = Rectangle::new((10.0, 42.0).into(), (780.0, 548.0).into());
        let radii = Corners {
            top_left: 0.0,
            top_right: 0.0,
            bottom_left: 12.0,
            bottom_right: 12.0,
        };
        assert_eq!(client_mask(rect, Some(radii)), Mask::Rect { rect, radii });
        assert_eq!(
            client_mask(rect, None),
            Mask::Rect {
                rect,
                radii: Corners::all(0.0)
            }
        );
    }

    /// **A pane's mask is its outer rectangle, rounded at the largest radius**
    /// on all four corners; an unrounded pane's is square.
    #[test]
    fn a_panes_mask_is_its_outer_rect_rounded_at_the_largest_radius() {
        let rect = Rectangle::new((0.0, 0.0).into(), (800.0, 600.0).into());
        let radii = Corners {
            top_left: 0.0,
            top_right: 0.0,
            bottom_left: 12.0,
            bottom_right: 6.0,
        };
        assert_eq!(
            pane_mask(rect, Some(radii)),
            Mask::Rect {
                rect,
                radii: Corners::all(12.0)
            }
        );
        assert_eq!(
            pane_mask(rect, None),
            Mask::Rect {
                rect,
                radii: Corners::all(0.0)
            }
        );
    }
}
