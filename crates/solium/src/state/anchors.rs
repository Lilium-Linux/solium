//! The compositor's half of named regions: draining what the hosted scenes
//! published into the registry once a pass, answering where things are for
//! `anchor::resolve`, and listing the regions for `sol.anchors`.

use smithay::output::Output;
use smithay::utils::{Logical, Point};

use super::Solium;
use crate::anchor::{Context, Owner, Places, Rect, Row, Same, Settings};

impl Solium {
    /// Drain what the hosted scenes' regions did since the last pass into
    /// the registry. Called once a pass from `render::prepare`, after the QML
    /// tick, so a flight aimed at a region this pass aims at where the scene
    /// is laid out for this pass. A scene new since its last take replaces
    /// what was held of its owner, and an owner with no instance any more
    /// (its monitor gone, its surface removed) takes its regions with it.
    /// `tests::real_client::a_region_resolves_at_its_instances_place_on_its_monitor`,
    /// `tests::real_client::a_torn_down_scene_takes_its_regions_with_it`,
    /// `tests::a_rebuilt_scene_replaces_its_regions`.
    pub(crate) fn read_regions(&mut self) {
        crate::qml::hosted::publish_regions();
        for surface in self.surfaces.iter_mut() {
            let name = surface.name().to_owned();
            for (owner, fresh, changes) in surface.take_regions() {
                if fresh {
                    self.anchors.drop_owner(&owner);
                }
                self.anchors.apply(&mut self.names, &owner, &name, &changes);
            }
        }
        if !self.anchors.is_empty() {
            let surfaces = &self.surfaces;
            self.anchors.retain_owners(|owner| {
                surfaces
                    .get(owner.surface)
                    .is_some_and(|surface| surface.hosts_named(&owner.output))
            });
        }
    }

    /// Take `config.anchors`, or say why it was refused and keep what was
    /// set. A new `shown_opacity` reaches the QML host, which reads every
    /// scene's regions again.
    /// `tests::real_client::anchors_same_and_edge_reach_resolution`.
    pub(crate) fn apply_anchoring(&mut self, settings: Result<Settings, String>) {
        self.effects.clear_problems_of("anchors");
        match settings {
            Ok(settings) => {
                crate::qml::hosted::set_region_threshold(settings.shown_opacity);
                self.anchor_settings = settings;
            }
            Err(error) => self
                .effects
                .push_problem(crate::effect::host::Problem::error(
                    "anchors",
                    &crate::script::Scripts::config_path(),
                    None,
                    error,
                )),
        }
    }

    /// Every published region whose name starts with `prefix`, where it is
    /// on the desktop: `sol.anchors`. Nothing at all when nothing is
    /// published, which is every snapshot of a desktop with no regions.
    /// `script::tests::sol_anchors_lists_the_regions_under_a_prefix`.
    pub(crate) fn anchor_rows(&self, prefix: &str) -> Vec<Row> {
        if self.anchors.is_empty() {
            return Vec::new();
        }
        self.anchors
            .under(&self.names, prefix)
            .filter_map(|(name, owner, published)| {
                let origin = self.scene_origin(owner)?;
                Some(Row {
                    name: name.to_owned(),
                    monitor: owner.output.clone(),
                    rect: Rect::new(
                        (
                            origin.x + published.local.loc.x,
                            origin.y + published.local.loc.y,
                        )
                            .into(),
                        published.local.size,
                    ),
                    radius: published.radius,
                    shown: published.shown,
                })
            })
            .collect()
    }

    fn output_named(&self, name: &str) -> Option<Output> {
        self.space
            .outputs()
            .find(|output| output.name() == name)
            .cloned()
    }
}

/// Where things are, for `anchor::resolve`: a scene by its instance's place,
/// carried by a selection as `Anchor::Surface` is; monitors by name or role;
/// a pane where it is drawn; `anchors.same` as the configuration set it.
/// `tests::real_client::solium_answers_each_same_rule`.
impl Places for Solium {
    fn scene_origin(&self, owner: &Owner) -> Option<Point<f64, Logical>> {
        let output = self.output_named(&owner.output)?;
        Some(self.surface_drawn_on(owner.surface, &output)?.to_f64().loc)
    }

    fn monitor(&self, which: &str, _ctx: &Context<'_>) -> Option<(String, Rect)> {
        let output = match which {
            "active" => self.active_output()?,
            "primary" => self.primary_output()?,
            name => self.output_named(name)?,
        };
        let geometry = self.space.output_geometry(&output)?;
        Some((output.name(), geometry.to_f64()))
    }

    fn monitor_by(&self, how: Same, ctx: &Context<'_>) -> Option<String> {
        match how {
            Same::Pane => {
                let pane = self.panes.by_script_id(ctx.id?)?;
                self.output_of(self.pane_outer(pane))
                    .map(|output| output.name())
            }
            Same::Sender => ctx
                .output
                .and_then(|name| self.output_named(name))
                .map(|output| output.name()),
            Same::Pointer => {
                let at = self.seat.get_pointer()?.current_location();
                crate::monitor::at(&self.space, at).map(|output| output.name())
            }
            Same::Active => self.active_output().map(|output| output.name()),
            Same::Primary => self.primary_output().map(|output| output.name()),
        }
    }

    fn settings(&self) -> &Settings {
        &self.anchor_settings
    }

    fn pane(&self, id: u64) -> Option<Rect> {
        let pane = self.panes.by_script_id(id)?;
        Some(self.drawn(pane.id(), self.pane_outer(pane)).rect)
    }

    fn surface(&self, name: &str, ctx: &Context<'_>) -> Option<Rect> {
        let id = self.surfaces.named(name)?;
        let output = self.output_named(&crate::anchor::same_monitor(ctx, self)?)?;
        Some(self.surface_drawn_on(id, &output)?.to_f64())
    }
}
