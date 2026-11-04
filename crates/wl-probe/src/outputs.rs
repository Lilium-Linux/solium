//! The monitors as `zwlr-output-management-v1` tells them, and an
//! arrangement proposed through it (#51): what `wlr-randr` does, from a
//! client written here, since none is installed.
//!
//!     WL_PROBE_OUTPUTS=1
//!         one line a head: name, on or off, mode, position, scale,
//!         transform, and how many modes it offers; then a line of its
//!         modes, `modes<TAB>WxH@R ...`.
//!     WL_PROBE_ARRANGE="scale=1.5 transform=90 x=0 y=0 mode=1280x720@60 vrr=on"
//!         the first head on with what is named, every other head as it is,
//!         applied; WL_PROBE_ARRANGE_TEST=1 tests it instead. Prints the
//!         answer, `succeeded`, `failed` or `cancelled`, and the heads after.

use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_output, wl_registry},
};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

#[derive(Default, Clone)]
struct Head {
    name: String,
    enabled: bool,
    current: Option<ZwlrOutputModeV1>,
    modes: Vec<ZwlrOutputModeV1>,
    position: (i32, i32),
    scale: f64,
    transform: u32,
    vrr: bool,
}

#[derive(Default)]
struct Outputs {
    heads: Vec<(ZwlrOutputHeadV1, Head)>,
    modes: Vec<(ZwlrOutputModeV1, (i32, i32, i32))>,
    serial: Option<u32>,
    answer: Option<&'static str>,
}

impl Outputs {
    fn mode(&self, mode: &ZwlrOutputModeV1) -> Option<(i32, i32, i32)> {
        self.modes
            .iter()
            .find(|(each, _)| each == mode)
            .map(|(_, size)| *size)
    }

    fn print(&self) {
        for (_, head) in &self.heads {
            let mode = head
                .current
                .as_ref()
                .and_then(|mode| self.mode(mode))
                .map_or_else(
                    || "-".to_owned(),
                    |(w, h, r)| format!("{w}x{h}@{:.2}", f64::from(r) / 1000.0),
                );
            println!(
                "{}\t{}\t{mode}\t{},{}\tscale {}\ttransform {}\tvrr {}\t{} modes",
                head.name,
                if head.enabled { "on" } else { "off" },
                head.position.0,
                head.position.1,
                head.scale,
                head.transform,
                head.vrr,
                head.modes.len()
            );
            let modes: Vec<String> = head
                .modes
                .iter()
                .filter_map(|mode| self.mode(mode))
                .map(|(w, h, r)| format!("{w}x{h}@{:.2}", f64::from(r) / 1000.0))
                .collect();
            println!("modes\t{}", modes.join(" "));
        }
    }
}

pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<Outputs>(&connection)?;
    let handle = queue.handle();
    let manager: ZwlrOutputManagerV1 = globals.bind(&handle, 4..=4, ())?;
    let mut state = Outputs::default();
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    let Ok(arrange) = std::env::var("WL_PROBE_ARRANGE") else {
        state.print();
        return Ok(());
    };
    let serial = state.serial.ok_or("no done from the manager")?;
    let configuration = manager.create_configuration(serial, &handle, ());
    for (index, (head, told)) in state.heads.iter().enumerate() {
        if index > 0 && !told.enabled {
            configuration.disable_head(head);
            continue;
        }
        let on: ZwlrOutputConfigurationHeadV1 = configuration.enable_head(head, &handle, ());
        if index > 0 {
            continue;
        }
        let (mut x, mut y) = (None, None);
        for word in arrange.split_whitespace() {
            let Some((key, value)) = word.split_once('=') else {
                continue;
            };
            match key {
                "scale" => on.set_scale(value.parse()?),
                "transform" => {
                    let transform = match value {
                        "normal" => wl_output::Transform::Normal,
                        "90" => wl_output::Transform::_90,
                        "180" => wl_output::Transform::_180,
                        "270" => wl_output::Transform::_270,
                        other => return Err(format!("not a transform: {other}").into()),
                    };
                    on.set_transform(transform);
                }
                "vrr" => on.set_adaptive_sync(if value == "on" {
                    zwlr_output_head_v1::AdaptiveSyncState::Enabled
                } else {
                    zwlr_output_head_v1::AdaptiveSyncState::Disabled
                }),
                "x" => x = Some(value.parse()?),
                "y" => y = Some(value.parse()?),
                "mode" => {
                    let (size, refresh) = value.split_once('@').unwrap_or((value, "0"));
                    let (w, h) = size.split_once('x').ok_or("a mode is WxH@R")?;
                    let refresh: f64 = refresh.parse()?;
                    #[expect(clippy::cast_possible_truncation, reason = "a refresh in mHz")]
                    on.set_custom_mode(w.parse()?, h.parse()?, (refresh * 1000.0) as i32);
                }
                _ => {}
            }
        }
        if x.is_some() || y.is_some() {
            on.set_position(x.unwrap_or(told.position.0), y.unwrap_or(told.position.1));
        }
    }
    if std::env::var_os("WL_PROBE_ARRANGE_TEST").is_some() {
        configuration.test();
    } else {
        configuration.apply();
    }
    while state.answer.is_none() {
        queue.blocking_dispatch(&mut state)?;
    }
    println!("{}", state.answer.unwrap_or("?"));
    queue.roundtrip(&mut state)?;
    state.print();
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Outputs {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Outputs {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.heads.push((head, Head::default()));
            }
            zwlr_output_manager_v1::Event::Done { serial } => state.serial = Some(serial),
            _ => {}
        }
    }

    event_created_child!(Outputs, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for Outputs {
    fn event(
        state: &mut Self,
        head: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_output_head_v1::Event::Mode { mode } = &event {
            state.modes.push((mode.clone(), (0, 0, 0)));
        }
        let Some((_, told)) = state.heads.iter_mut().find(|(each, _)| each == head) else {
            return;
        };
        match event {
            zwlr_output_head_v1::Event::Name { name } => told.name = name,
            zwlr_output_head_v1::Event::Mode { mode } => told.modes.push(mode),
            zwlr_output_head_v1::Event::Enabled { enabled } => told.enabled = enabled != 0,
            zwlr_output_head_v1::Event::CurrentMode { mode } => told.current = Some(mode),
            zwlr_output_head_v1::Event::Position { x, y } => told.position = (x, y),
            zwlr_output_head_v1::Event::Scale { scale } => told.scale = scale,
            zwlr_output_head_v1::Event::Transform { transform } => {
                told.transform = match transform {
                    WEnum::Value(value) => u32::from(value),
                    WEnum::Unknown(value) => value,
                };
            }
            zwlr_output_head_v1::Event::AdaptiveSync { state } => {
                told.vrr = matches!(
                    state,
                    WEnum::Value(zwlr_output_head_v1::AdaptiveSyncState::Enabled)
                );
            }
            _ => {}
        }
    }

    event_created_child!(Outputs, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for Outputs {
    fn event(
        state: &mut Self,
        mode: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some((_, told)) = state.modes.iter_mut().find(|(each, _)| each == mode) else {
            return;
        };
        match event {
            zwlr_output_mode_v1::Event::Size { width, height } => {
                told.0 = width;
                told.1 = height;
            }
            zwlr_output_mode_v1::Event::Refresh { refresh } => told.2 = refresh,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for Outputs {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.answer = Some(match event {
            zwlr_output_configuration_v1::Event::Succeeded => "succeeded",
            zwlr_output_configuration_v1::Event::Failed => "failed",
            zwlr_output_configuration_v1::Event::Cancelled => "cancelled",
            _ => "?",
        });
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ()> for Outputs {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputConfigurationHeadV1,
        _: <ZwlrOutputConfigurationHeadV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
