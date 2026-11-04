//! The monitors, for a program that arranges them (#51): `kanshi`,
//! `wlr-randr`, a settings panel.
//!
//! `zwlr-output-management-v1`, written here since Smithay has none. Every
//! connected monitor is a *head* -- one the configuration switched off too,
//! so a program can turn it back on -- with the modes it offers and what it
//! is now: on or off, its mode, where it is, its rotation, its scale, and
//! whether it asked for variable refresh. A program proposes a whole
//! arrangement, every head on or off, and asks for it to be tested or
//! applied.
//!
//! **What a program proposes is an action, and the configuration answers
//! it.** A test is answered here: the arrangement is read whole, each mode
//! must be one its monitor offers, each scale one `sol.monitors` takes, and
//! at least one monitor stays on. An apply that passes the same reading
//! reaches Lua as the action `monitors.apply` from the surface named
//! [`SURFACE`], with the arrangement as `sol.monitors` rows in `monitors`;
//! the shipped `output_management.lua` answers it with `sol.monitors`, and
//! the program is told it succeeded only when an answer took an arrangement.
//! What is lit differently then -- a mode, a rotation, variable refresh -- is
//! lit again by the backend, the path a cable takes (`monitor::Arrangement::lit_as`).
//!
//! A proposal made from a picture of the monitors that has since changed is
//! cancelled, as the protocol says, so the program looks again: every pass
//! compares the heads with what was last told ([`Solium::publish_output_heads`]),
//! and a change is a new serial.
//!
//! `output_management = false` in `config.lua` takes it away from every
//! client. `state::tests::real_client::reflow_on_close::output_management`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use smithay::{
    reexports::{
        wayland_protocols_wlr::output_management::v1::server::{
            zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1},
            zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
            zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
            zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
            zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
            protocol::wl_output,
        },
    },
    utils::Transform,
};

use crate::json::Json;
use crate::state::Solium;

/// The surface name a program's proposal reaches Lua's `surface` listeners
/// with: `sol.on("surface", function(surface, action, data) end)` hears
/// `surface == "output-management"` and `action == "monitors.apply"`.
pub(crate) const SURFACE: &str = "output-management";

/// The version of `zwlr_output_manager_v1` offered: 4, with each head's
/// make, model and variable refresh.
const VERSION: u32 = 4;

/// One mode a monitor offers: its size in device pixels, its refresh in
/// mHz, and whether its EDID prefers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModeInfo {
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) refresh: i32,
    pub(crate) preferred: bool,
}

/// A connected monitor the configuration switched off: no `Output` to read
/// it from, so the backend says what it is. Still a head, off, so a program
/// can turn it on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Dark {
    pub(crate) name: String,
    pub(crate) make: String,
    pub(crate) model: String,
    /// In millimetres, as its EDID says; 0 for one that does not.
    pub(crate) physical: (i32, i32),
    pub(crate) modes: Vec<ModeInfo>,
}

/// What one head is, as a program is told it.
#[derive(Clone, Debug, PartialEq)]
struct Head {
    name: String,
    description: String,
    make: String,
    model: String,
    physical: (i32, i32),
    modes: Vec<ModeInfo>,
    enabled: bool,
    /// Which of `modes` it is driven at.
    current: Option<usize>,
    position: (i32, i32),
    transform: Transform,
    scale: f64,
    vrr: bool,
}

/// Whether the global is offered: read by its `can_view`.
#[derive(Debug, Clone)]
pub(crate) struct Offered(Arc<AtomicBool>);

/// One head as one client was told it: its object, its modes' objects, and
/// what it was told, to tell each change once.
#[derive(Debug)]
struct Told {
    head: ZwlrOutputHeadV1,
    modes: Vec<ZwlrOutputModeV1>,
    head_now: Head,
}

/// A bound manager and the heads its client holds.
#[derive(Debug)]
struct Bound {
    manager: ZwlrOutputManagerV1,
    heads: Vec<Told>,
}

/// The protocol's state.
#[derive(Debug)]
pub(crate) struct OutputManagement {
    offered: Offered,
    bound: Vec<Bound>,
    /// Moves each time the heads change: a proposal made from an older one
    /// is cancelled.
    serial: u32,
    /// The heads as last told.
    published: Vec<Head>,
    /// The connected monitors the configuration switched off, as the backend
    /// found them. Nested, there are none.
    pub(crate) dark: Vec<Dark>,
}

impl OutputManagement {
    /// Offered, as the default configuration does.
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let offered = Offered(Arc::new(AtomicBool::new(true)));
        display.create_global::<Solium, ZwlrOutputManagerV1, _>(VERSION, offered.clone());
        Self {
            offered,
            bound: Vec::new(),
            serial: 0,
            published: Vec::new(),
            dark: Vec::new(),
        }
    }

    pub(crate) fn offered(&self) -> bool {
        self.offered.0.load(Ordering::Relaxed)
    }

    /// Offer the global, or take it away: hidden from clients that have not
    /// bound it, and every manager bound is told it is finished, its heads
    /// with it, so no program keeps arranging monitors it was told it may
    /// not.
    pub(crate) fn offer(&mut self, on: bool) {
        self.offered.0.store(on, Ordering::Relaxed);
        if !on {
            for bound in self.bound.drain(..) {
                for told in bound.heads {
                    finish(&told);
                }
                bound.manager.finished();
            }
        }
    }
}

/// What a head's object carries: which monitor it is.
#[derive(Debug)]
pub(crate) struct HeadData(String);

/// What a mode's object carries: whose mode, and which.
#[derive(Debug)]
pub(crate) struct ModeData {
    head: String,
    mode: ModeInfo,
}

/// A proposal, kept in its object's own data: the serial it was made from,
/// and each head it says something of -- on with what its configuration
/// head says, or off.
#[derive(Debug)]
pub(crate) struct ConfigData {
    serial: u32,
    inner: Mutex<Proposal>,
}

#[derive(Debug, Default)]
struct Proposal {
    heads: BTreeMap<String, Option<ZwlrOutputConfigurationHeadV1>>,
    /// Applied or tested: it takes nothing more.
    used: bool,
}

/// What a proposal says of one head it turns on, in its object's own data:
/// which head, and each property set.
#[derive(Debug)]
pub(crate) struct ConfigHeadData {
    head: String,
    inner: Mutex<Asked>,
}

/// Each property a program set; the rest stay as they are.
#[derive(Clone, Debug, Default)]
struct Asked {
    mode: Option<ModeInfo>,
    custom: Option<(i32, i32, i32)>,
    position: Option<(i32, i32)>,
    transform: Option<Transform>,
    scale: Option<f64>,
    vrr: Option<bool>,
}

/// The heads now: every monitor lit, as its `Output` says, then every one
/// switched off.
fn heads_now(state: &Solium) -> Vec<Head> {
    let mut heads: Vec<Head> = state
        .space
        .outputs()
        .map(|output| {
            let name = output.name();
            let properties = output.physical_properties();
            let preferred = output.preferred_mode();
            let offered = output.modes();
            let modes = offered
                .iter()
                .map(|mode| ModeInfo {
                    width: mode.size.w,
                    height: mode.size.h,
                    refresh: mode.refresh,
                    preferred: preferred == Some(*mode),
                })
                .collect();
            let current = output
                .current_mode()
                .and_then(|now| offered.iter().position(|mode| *mode == now));
            let at = output.current_location();
            Head {
                description: output.description(),
                make: properties.make,
                model: properties.model,
                physical: (properties.size.w, properties.size.h),
                modes,
                enabled: true,
                current,
                position: (at.x, at.y),
                transform: output.current_transform(),
                scale: output.current_scale().fractional_scale(),
                vrr: state.arrangement.vrr(&name).unwrap_or(false),
                name,
            }
        })
        .collect();
    for dark in &state.output_management.dark {
        if heads.iter().any(|head| head.name == dark.name) {
            continue;
        }
        heads.push(Head {
            name: dark.name.clone(),
            description: format!("{} {} ({})", dark.make, dark.model, dark.name),
            make: dark.make.clone(),
            model: dark.model.clone(),
            physical: dark.physical,
            modes: dark.modes.clone(),
            enabled: false,
            current: None,
            position: (0, 0),
            transform: Transform::Normal,
            scale: 1.0,
            vrr: false,
        });
    }
    heads
}

/// A head's mode and the head itself, finished: the monitor is gone, or
/// its client may no longer hold it.
fn finish(told: &Told) {
    for mode in &told.modes {
        if mode.is_alive() {
            mode.finished();
        }
    }
    if told.head.is_alive() {
        told.head.finished();
    }
}

/// New mode objects for `head`, each told what it is.
fn make_modes(
    handle: &DisplayHandle,
    client: &Client,
    head: &ZwlrOutputHeadV1,
    now: &Head,
) -> Vec<ZwlrOutputModeV1> {
    now.modes
        .iter()
        .filter_map(|mode| {
            let object = client
                .create_resource::<ZwlrOutputModeV1, _, Solium>(
                    handle,
                    head.version(),
                    ModeData {
                        head: now.name.clone(),
                        mode: *mode,
                    },
                )
                .ok()?;
            head.mode(&object);
            object.size(mode.width, mode.height);
            if mode.refresh > 0 {
                object.refresh(mode.refresh);
            }
            if mode.preferred {
                object.preferred();
            }
            Some(object)
        })
        .collect()
}

/// Tell `head` what changed from `was` to `now` -- everything, for a head
/// told nothing. What only an enabled head has is sent while it is on.
fn tell(head: &ZwlrOutputHeadV1, modes: &[ZwlrOutputModeV1], was: Option<&Head>, now: &Head) {
    let changed = |same: fn(&Head, &Head) -> bool| was.is_none_or(|was| !same(was, now));
    if was.is_none() {
        head.name(now.name.clone());
        head.description(now.description.clone());
        if now.physical.0 > 0 && now.physical.1 > 0 {
            head.physical_size(now.physical.0, now.physical.1);
        }
        if head.version() >= 2 {
            head.make(now.make.clone());
            head.model(now.model.clone());
        }
    }
    let turned = changed(|was, now| was.enabled == now.enabled);
    if turned {
        head.enabled(i32::from(now.enabled));
    }
    if !now.enabled {
        return;
    }
    // A head just turned on is told all of it; the modes were told anew when
    // they changed, so the current one is named again then too.
    let all = turned || changed(|was, now| was.modes == now.modes);
    if (all || changed(|was, now| was.current == now.current))
        && let Some(mode) = now.current.and_then(|index| modes.get(index))
    {
        head.current_mode(mode);
    }
    if all || changed(|was, now| was.position == now.position) {
        head.position(now.position.0, now.position.1);
    }
    if all || changed(|was, now| was.transform == now.transform) {
        head.transform(wl_output::Transform::from(now.transform));
    }
    #[expect(clippy::float_cmp, reason = "a change, not a closeness")]
    if all || changed(|was, now| was.scale == now.scale) {
        head.scale(now.scale);
    }
    if head.version() >= 4 && (all || changed(|was, now| was.vrr == now.vrr)) {
        head.adaptive_sync(if now.vrr {
            zwlr_output_head_v1::AdaptiveSyncState::Enabled
        } else {
            zwlr_output_head_v1::AdaptiveSyncState::Disabled
        });
    }
}

/// Bring one client's heads to `now`, then `done` with `serial`.
fn bring(handle: &DisplayHandle, bound: &mut Bound, now: &[Head], serial: u32) {
    let Some(client) = bound.manager.client() else {
        return;
    };
    // Gone first, then each one there now in order.
    bound.heads.retain(|told| {
        let here = now.iter().any(|head| head.name == told.head_now.name);
        if !here {
            finish(told);
        }
        here
    });
    for head in now {
        if let Some(told) = bound
            .heads
            .iter_mut()
            .find(|told| told.head_now.name == head.name)
        {
            if !told.head.is_alive() {
                continue;
            }
            if told.head_now.modes != head.modes {
                for mode in told.modes.drain(..) {
                    if mode.is_alive() {
                        mode.finished();
                    }
                }
                told.modes = make_modes(handle, &client, &told.head, head);
            }
            tell(&told.head, &told.modes, Some(&told.head_now), head);
            told.head_now = head.clone();
            continue;
        }
        let Ok(object) = client.create_resource::<ZwlrOutputHeadV1, _, Solium>(
            handle,
            bound.manager.version(),
            HeadData(head.name.clone()),
        ) else {
            continue;
        };
        bound.manager.head(&object);
        let modes = make_modes(handle, &client, &object, head);
        tell(&object, &modes, None, head);
        bound.heads.push(Told {
            head: object,
            modes,
            head_now: head.clone(),
        });
    }
    bound.manager.done(serial);
}

/// A transform by its protocol value.
fn transform_of(value: wl_output::Transform) -> Option<Transform> {
    Some(match value {
        wl_output::Transform::Normal => Transform::Normal,
        wl_output::Transform::_90 => Transform::_90,
        wl_output::Transform::_180 => Transform::_180,
        wl_output::Transform::_270 => Transform::_270,
        wl_output::Transform::Flipped => Transform::Flipped,
        wl_output::Transform::Flipped90 => Transform::Flipped90,
        wl_output::Transform::Flipped180 => Transform::Flipped180,
        wl_output::Transform::Flipped270 => Transform::Flipped270,
        _ => return None,
    })
}

/// A proposal read whole against the heads: one `sol.monitors` row per
/// head, what it did not set kept as it is, or why it cannot be.
/// `tests::a_proposal_is_read_whole_into_rows`.
fn rows_of(
    heads: &[Head],
    proposed: &BTreeMap<String, Option<Asked>>,
    vrr_now: impl Fn(&str) -> Option<bool>,
) -> Result<Vec<Json>, String> {
    let mut rows = Vec::new();
    let mut lit = 0;
    for head in heads {
        let mut row = BTreeMap::new();
        row.insert("name".to_owned(), Json::Text(head.name.clone()));
        let Some(Some(asked)) = proposed.get(&head.name) else {
            row.insert("enabled".to_owned(), Json::Bool(false));
            rows.push(Json::Object(row));
            continue;
        };
        lit += 1;
        let mode = match (asked.mode, asked.custom) {
            (Some(mode), _) => mode,
            (None, Some((width, height, refresh))) => *head
                .modes
                .iter()
                .find(|mode| {
                    mode.width == width
                        && mode.height == height
                        && (refresh == 0 || (mode.refresh - refresh).abs() <= 1000)
                })
                .ok_or_else(|| {
                    format!(
                        "{}: {width}x{height} at {refresh} mHz is not a mode it offers",
                        head.name
                    )
                })?,
            (None, None) => *head
                .current
                .and_then(|index| head.modes.get(index))
                .or_else(|| head.modes.iter().find(|mode| mode.preferred))
                .or_else(|| head.modes.first())
                .ok_or_else(|| format!("{}: offers no mode to drive it at", head.name))?,
        };
        let scale = asked.scale.unwrap_or(head.scale);
        if crate::monitor::scaling(&scale).is_none() {
            return Err(format!(
                "{}: a scale of {scale} is outside 0.5 to 8",
                head.name
            ));
        }
        let (x, y) = asked.position.unwrap_or(head.position);
        let mut wanted = BTreeMap::new();
        wanted.insert("w".to_owned(), Json::Number(f64::from(mode.width)));
        wanted.insert("h".to_owned(), Json::Number(f64::from(mode.height)));
        // In hertz, as a configuration writes it.
        wanted.insert(
            "refresh".to_owned(),
            Json::Number(f64::from((mode.refresh + 500) / 1000)),
        );
        row.insert("enabled".to_owned(), Json::Bool(true));
        row.insert("x".to_owned(), Json::Number(f64::from(x)));
        row.insert("y".to_owned(), Json::Number(f64::from(y)));
        row.insert("mode".to_owned(), Json::Object(wanted));
        row.insert("scale".to_owned(), Json::Number(scale));
        row.insert(
            "transform".to_owned(),
            Json::Text(
                crate::monitor::transform_name(asked.transform.unwrap_or(head.transform))
                    .to_owned(),
            ),
        );
        if let Some(vrr) = asked.vrr.or_else(|| vrr_now(&head.name)) {
            row.insert("vrr".to_owned(), Json::Bool(vrr));
        }
        rows.push(Json::Object(row));
    }
    if lit == 0 {
        return Err("no monitor would be on".to_owned());
    }
    Ok(rows)
}

impl Solium {
    /// Tell every program what changed among the monitors since it was
    /// last told, each change once and then `done` with a new serial. Every
    /// pass, beside the window lists, and as cheap with nobody bound:
    /// nothing at all.
    pub(crate) fn publish_output_heads(&mut self) {
        self.output_management
            .bound
            .retain(|bound| bound.manager.is_alive());
        if self.output_management.bound.is_empty() {
            return;
        }
        let now = heads_now(self);
        if now == self.output_management.published {
            return;
        }
        let state = &mut self.output_management;
        state.serial = state.serial.wrapping_add(1);
        state.published = now;
        for bound in &mut state.bound {
            bring(&self.display_handle, bound, &state.published, state.serial);
        }
    }

    /// Test or apply `config`: see the module's documentation.
    fn settle_proposal(
        &mut self,
        config: &ZwlrOutputConfigurationV1,
        data: &ConfigData,
        test: bool,
    ) {
        let proposed = {
            let Ok(mut held) = data.inner.lock() else {
                config.failed();
                return;
            };
            if held.used {
                config.post_error(
                    zwlr_output_configuration_v1::Error::AlreadyUsed,
                    "this configuration was already applied or tested",
                );
                return;
            }
            held.used = true;
            held.heads
                .iter()
                .map(|(name, head)| {
                    let asked = head.as_ref().map(|head| {
                        head.data::<ConfigHeadData>()
                            .and_then(|data| data.inner.lock().ok().map(|asked| asked.clone()))
                            .unwrap_or_default()
                    });
                    (name.clone(), asked)
                })
                .collect::<BTreeMap<_, _>>()
        };
        // What is true now, told first, so a proposal from a picture that
        // has since changed is the stale one it is.
        self.publish_output_heads();
        let heads = self.output_management.published.clone();
        if let Some(missing) = heads.iter().find(|head| !proposed.contains_key(&head.name)) {
            config.post_error(
                zwlr_output_configuration_v1::Error::UnconfiguredHead,
                format!("{} is neither enabled nor disabled", missing.name),
            );
            return;
        }
        if data.serial != self.output_management.serial {
            config.cancelled();
            return;
        }
        let rows = match rows_of(&heads, &proposed, |name| self.arrangement.vrr(name)) {
            Ok(rows) => rows,
            Err(why) => {
                tracing::info!(why, test, "a program's monitor arrangement was refused");
                config.failed();
                return;
            }
        };
        if test {
            config.succeeded();
            return;
        }
        let mut asked = BTreeMap::new();
        asked.insert("monitors".to_owned(), Json::List(rows));
        let snapshot = self.snapshot();
        let Some(mut scripts) = self.scripts.take() else {
            config.failed();
            return;
        };
        let outcome =
            scripts.surface_action(SURFACE, "monitors.apply", &Json::Object(asked), snapshot);
        self.scripts = Some(scripts);
        let took = outcome
            .commands
            .iter()
            .any(|command| matches!(command, crate::script::Command::Monitors(_)));
        self.apply(outcome);
        if took {
            config.succeeded();
            // Told now rather than at the next pass, so the program reads
            // back what it asked for straight after `succeeded`.
            self.publish_output_heads();
        } else {
            tracing::info!("nothing in the configuration answered monitors.apply");
            config.failed();
        }
    }
}

impl GlobalDispatch<ZwlrOutputManagerV1, Offered> for Solium {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputManagerV1>,
        _offered: &Offered,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        // Told what there is at once, as the protocol says, from the same
        // picture every other client was last told, and its serial.
        if state.output_management.bound.is_empty() {
            state.output_management.published = heads_now(state);
            state.output_management.serial = state.output_management.serial.wrapping_add(1);
        }
        let mut bound = Bound {
            manager,
            heads: Vec::new(),
        };
        let output_management = &state.output_management;
        bring(
            handle,
            &mut bound,
            &output_management.published,
            output_management.serial,
        );
        state.output_management.bound.push(bound);
    }

    fn can_view(_client: Client, offered: &Offered) -> bool {
        offered.0.load(Ordering::Relaxed)
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &ZwlrOutputManagerV1,
        request: zwlr_output_manager_v1::Request,
        (): &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_output_manager_v1::Request::CreateConfiguration { id, serial } => {
                data_init.init(
                    id,
                    ConfigData {
                        serial,
                        inner: Mutex::default(),
                    },
                );
            }
            zwlr_output_manager_v1::Request::Stop => {
                if let Some(index) = state
                    .output_management
                    .bound
                    .iter()
                    .position(|bound| bound.manager == *manager)
                {
                    let bound = state.output_management.bound.remove(index);
                    for told in &bound.heads {
                        finish(told);
                    }
                }
                manager.finished();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputHeadV1, HeadData> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _head: &ZwlrOutputHeadV1,
        _request: zwlr_output_head_v1::Request,
        _data: &HeadData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        // `release` only: nothing more is sent to an object that is gone.
    }
}

impl Dispatch<ZwlrOutputModeV1, ModeData> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _mode: &ZwlrOutputModeV1,
        _request: zwlr_output_mode_v1::Request,
        _data: &ModeData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ConfigData> for Solium {
    fn request(
        state: &mut Self,
        _client: &Client,
        config: &ZwlrOutputConfigurationV1,
        request: zwlr_output_configuration_v1::Request,
        data: &ConfigData,
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        // Which head a request names, and whether the proposal may still
        // take it; a configuration head is made either way, as the protocol
        // has every new object made before its error.
        let name_of = |head: &ZwlrOutputHeadV1| {
            head.data::<HeadData>()
                .map(|data| data.0.clone())
                .unwrap_or_default()
        };
        let take = |name: String, head: Option<ZwlrOutputConfigurationHeadV1>| {
            let Ok(mut held) = data.inner.lock() else {
                return;
            };
            if held.used {
                config.post_error(
                    zwlr_output_configuration_v1::Error::AlreadyUsed,
                    "this configuration was already applied or tested",
                );
            } else {
                match held.heads.entry(name) {
                    std::collections::btree_map::Entry::Occupied(taken) => config.post_error(
                        zwlr_output_configuration_v1::Error::AlreadyConfiguredHead,
                        format!("{} was already enabled or disabled", taken.key()),
                    ),
                    std::collections::btree_map::Entry::Vacant(free) => {
                        free.insert(head);
                    }
                }
            }
        };
        match request {
            zwlr_output_configuration_v1::Request::EnableHead { id, head } => {
                let name = name_of(&head);
                let object = data_init.init(
                    id,
                    ConfigHeadData {
                        head: name.clone(),
                        inner: Mutex::default(),
                    },
                );
                take(name, Some(object));
            }
            zwlr_output_configuration_v1::Request::DisableHead { head } => {
                take(name_of(&head), None);
            }
            zwlr_output_configuration_v1::Request::Apply => {
                state.settle_proposal(config, data, false);
            }
            zwlr_output_configuration_v1::Request::Test => {
                state.settle_proposal(config, data, true);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ConfigHeadData> for Solium {
    fn request(
        _state: &mut Self,
        _client: &Client,
        object: &ZwlrOutputConfigurationHeadV1,
        request: zwlr_output_configuration_head_v1::Request,
        data: &ConfigHeadData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_output_configuration_head_v1::{Error, Request};
        let Ok(mut asked) = data.inner.lock() else {
            return;
        };
        let twice = |object: &ZwlrOutputConfigurationHeadV1| {
            object.post_error(Error::AlreadySet, "this property was already set");
        };
        match request {
            Request::SetMode { mode } => {
                if asked.mode.is_some() || asked.custom.is_some() {
                    return twice(object);
                }
                match mode.data::<ModeData>() {
                    Some(mode) if mode.head == data.head => asked.mode = Some(mode.mode),
                    _ => object.post_error(Error::InvalidMode, "not a mode of this head"),
                }
            }
            Request::SetCustomMode {
                width,
                height,
                refresh,
            } => {
                if asked.mode.is_some() || asked.custom.is_some() {
                    return twice(object);
                }
                if width <= 0 || height <= 0 || refresh < 0 {
                    object.post_error(Error::InvalidCustomMode, "a mode needs a size");
                    return;
                }
                asked.custom = Some((width, height, refresh));
            }
            Request::SetPosition { x, y } => {
                if asked.position.replace((x, y)).is_some() {
                    twice(object);
                }
            }
            Request::SetTransform { transform } => {
                let Some(transform) = (match transform {
                    WEnum::Value(value) => transform_of(value),
                    WEnum::Unknown(_) => None,
                }) else {
                    object.post_error(Error::InvalidTransform, "not a wl_output transform");
                    return;
                };
                if asked.transform.replace(transform).is_some() {
                    twice(object);
                }
            }
            Request::SetScale { scale } => {
                if scale <= 0.0 {
                    object.post_error(Error::InvalidScale, "a scale must be above zero");
                    return;
                }
                if asked.scale.replace(scale).is_some() {
                    twice(object);
                }
            }
            Request::SetAdaptiveSync { state } => {
                let on = match state {
                    WEnum::Value(zwlr_output_head_v1::AdaptiveSyncState::Enabled) => true,
                    WEnum::Value(zwlr_output_head_v1::AdaptiveSyncState::Disabled) => false,
                    _ => {
                        object.post_error(
                            Error::InvalidAdaptiveSyncState,
                            "adaptive sync is enabled or disabled",
                        );
                        return;
                    }
                };
                if asked.vrr.replace(on).is_some() {
                    twice(object);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: i32, height: i32, refresh: i32, preferred: bool) -> ModeInfo {
        ModeInfo {
            width,
            height,
            refresh,
            preferred,
        }
    }

    fn head(name: &str) -> Head {
        Head {
            name: name.to_owned(),
            description: String::new(),
            make: String::new(),
            model: String::new(),
            physical: (0, 0),
            modes: vec![
                mode(2560, 1440, 165_000, false),
                mode(1920, 1080, 60_000, true),
            ],
            enabled: true,
            current: Some(0),
            position: (0, 0),
            transform: Transform::Normal,
            scale: 1.0,
            vrr: false,
        }
    }

    fn field<'a>(row: &'a Json, key: &str) -> Option<&'a Json> {
        match row {
            Json::Object(row) => row.get(key),
            _ => None,
        }
    }

    /// **A proposal is read whole into rows**: what one head did not set is
    /// kept, a custom mode is one the head offers, a head left off is off,
    /// and an arrangement with nothing on, a mode no head offers, or a
    /// scale `sol.monitors` refuses is no arrangement at all.
    #[test]
    fn a_proposal_is_read_whole_into_rows() {
        let heads = [head("DP-1"), head("DP-2")];
        let mut proposed = BTreeMap::new();
        proposed.insert(
            "DP-1".to_owned(),
            Some(Asked {
                position: Some((2560, 0)),
                custom: Some((1920, 1080, 59_940)),
                transform: Some(Transform::_90),
                ..Asked::default()
            }),
        );
        proposed.insert("DP-2".to_owned(), None);
        let rows = rows_of(&heads, &proposed, |_| Some(true)).expect("an arrangement");
        let first = &rows[0];
        assert_eq!(field(first, "x"), Some(&Json::Number(2560.0)));
        assert_eq!(
            field(first, "transform"),
            Some(&Json::Text("90".to_owned()))
        );
        assert_eq!(field(first, "scale"), Some(&Json::Number(1.0)), "kept");
        assert_eq!(field(first, "vrr"), Some(&Json::Bool(true)), "kept");
        let mode = field(first, "mode").expect("a mode");
        assert_eq!(
            (field(mode, "w"), field(mode, "refresh")),
            (Some(&Json::Number(1920.0)), Some(&Json::Number(60.0))),
            "the offered mode nearest, in hertz"
        );
        assert_eq!(field(&rows[1], "enabled"), Some(&Json::Bool(false)));

        let mut dark = proposed.clone();
        dark.insert("DP-1".to_owned(), None);
        assert!(rows_of(&heads, &dark, |_| None).is_err(), "nothing on");
        let mut odd = proposed.clone();
        odd.insert(
            "DP-1".to_owned(),
            Some(Asked {
                custom: Some((1234, 567, 0)),
                ..Asked::default()
            }),
        );
        assert!(
            rows_of(&heads, &odd, |_| None).is_err(),
            "a mode it does not offer"
        );
        let mut tiny = proposed;
        tiny.insert(
            "DP-1".to_owned(),
            Some(Asked {
                scale: Some(0.25),
                ..Asked::default()
            }),
        );
        assert!(
            rows_of(&heads, &tiny, |_| None).is_err(),
            "a scale it refuses"
        );
    }
}
