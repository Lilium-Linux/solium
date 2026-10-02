//! The compositor's side of hosted scenes: reading what they report, and what
//! that changes.

use std::collections::BTreeMap;

use smithay::output::Output;

use crate::scripted::Edges;
use crate::state::Solium;

/// What hosted surfaces reserve, by monitor, leaving out every monitor on
/// which they reserve nothing.
/// `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`.
pub(crate) type Reserves = BTreeMap<String, Edges>;

/// How many times one settle reads the scenes again for what its own layout
/// pass and action handlers declared, before it leaves the rest to the next
/// settle, so handlers that keep changing a reserve cannot hold a dispatch.
/// `state::tests::real_client::a_reserve_an_action_handler_changes_reflows_the_windows_in_the_clicks_dispatch`,
/// `state::tests::real_client::handlers_that_keep_changing_a_reserve_cannot_hold_the_clicks_dispatch`.
const SETTLE_ROUNDS: usize = 4;

impl Solium {
    /// Read what every hosted scene reports, reserves first, then (later
    /// tasks) grabs and keyboard wants, and then the actions they asked for.
    /// Called after anything that can run QML code in a dispatch -- the end
    /// of an input dispatch and a frame's settle among them -- once a
    /// declaration has been applied, and once the surfaces are placed on
    /// the monitors (`state::tests::real_client::a_scene_built_on_a_monitor_that_arrives_reserves_in_the_hotplugs_dispatch`),
    /// and never from inside itself (Ruling 11).
    ///
    /// When what hosted surfaces reserve is no longer what the last layout
    /// pass was laid out against, the layout runs once, here, in the
    /// dispatch that read it, so the windows glide into the new work area
    /// from this instant on the compositor's clock.
    /// `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_reserve_the_scene_changes_at_a_press_reflows_the_tiled_windows_once_from_that_instant`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_reserve_declared_again_or_taken_away_reflows_the_windows_once_each`.
    ///
    /// What the layout pass and the actions' handlers run here declare is
    /// read here too, in another round, and not at the next settle: a bar
    /// button whose handler hides the bar re-flows the windows at the
    /// release that pressed it.
    /// `state::tests::real_client::a_reserve_an_action_handler_changes_reflows_the_windows_in_the_clicks_dispatch`.
    pub(crate) fn settle_scenes(&mut self) {
        if self.settling_scenes {
            return;
        }
        self.settling_scenes = true;
        for _ in 0..SETTLE_ROUNDS {
            self.scenes_to_settle = false;
            for surface in self.surfaces.iter_mut() {
                surface.take_reserves();
            }
            if self.reserves() != self.laid_out_reserves {
                self.redraw = true;
                self.trigger_relayout();
            }
            self.settle_surfaces();
            if !self.scenes_to_settle {
                break;
            }
        }
        self.settling_scenes = false;
    }

    /// Settle the scenes after a declaration: now, outside any dispatch, and
    /// otherwise once the outermost dispatch is applied whole. Not part way
    /// through it, where a `layout` handler that wrote a property moving the
    /// reserve would have the windows re-flowed and then put back by the
    /// places the same pass asked for after it.
    /// `state::tests::real_client::reflow_on_close::hosted::a_property_a_layout_handler_writes_reflows_the_windows_against_the_reserve_it_moved`,
    /// `state::tests::real_client::reflow_on_close::hosted::a_reserve_declared_again_or_taken_away_reflows_the_windows_once_each`.
    /// While a settle is running, in that settle's next round instead,
    /// dispatch or none: the surfaces placed once a handler's dispatch is
    /// applied inside the settle are read there too.
    /// `state::tests::real_client::a_reserve_an_action_handler_changes_reflows_the_windows_in_the_clicks_dispatch`,
    /// `state::tests::real_client::a_scene_an_action_moves_to_the_new_primary_reserves_in_the_clicks_dispatch`.
    pub(crate) fn settle_scenes_once_dispatched(&mut self) {
        if self.dispatching == 0 && !self.settling_scenes {
            self.settle_scenes();
        } else {
            self.scenes_to_settle = true;
        }
    }

    /// What hosted surfaces reserve on one monitor, every edge summed.
    /// `state::tests::real_client::reflow_on_close::hosted::a_declared_reserve_takes_its_edge_out_of_the_work_area`,
    /// `state::tests::real_client::reflow_on_close::hosted::two_surfaces_reserving_one_edge_take_both`.
    pub(crate) fn reserved_on(&self, output: &Output) -> Edges {
        let Some(geometry) = self.space.output_geometry(output) else {
            return Edges::default();
        };
        let primary = self.primary_output();
        self.surfaces
            .iter()
            .filter(|surface| {
                surface
                    .area_on(output, geometry, primary.as_ref())
                    .is_some()
            })
            .fold(Edges::default(), |all, surface| {
                all.add(surface.reserve_on(output))
            })
    }

    /// What hosted surfaces reserve on every monitor now.
    /// `state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`.
    pub(crate) fn reserves(&self) -> Reserves {
        self.space
            .outputs()
            .map(|output| (output.name(), self.reserved_on(output)))
            .filter(|(_, reserved)| *reserved != Edges::default())
            .collect()
    }
}
