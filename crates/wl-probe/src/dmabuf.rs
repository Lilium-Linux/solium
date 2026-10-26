//! Exercise the device feedback used by real recording and portal clients.

use std::{
    fs::File,
    os::unix::fs::{FileExt as _, MetadataExt as _},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_compositor::WlCompositor, wl_registry, wl_surface::WlSurface},
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_dmabuf_feedback_v1::{self, ZwpLinuxDmabufFeedbackV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};

#[derive(Default)]
struct Feedback {
    device: Option<u64>,
    formats: usize,
    target: Option<u64>,
    indices: Vec<u16>,
    main_tranche: bool,
    done: bool,
}

#[derive(Default)]
struct Check {
    feedback: [Feedback; 2],
}

pub(super) fn check() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<Check>(&connection)?;
    let handle = queue.handle();
    let present = globals
        .contents()
        .with_list(|list| list.iter().any(|g| g.interface == "zwp_linux_dmabuf_v1"));
    if std::env::var("WL_PROBE_DMABUF").as_deref() == Ok("absent") {
        assert!(
            !present,
            "DMA-BUF is still advertised with the fallback enabled"
        );
        println!("PASS DMA-BUF absent: shared-memory fallback");
        return Ok(());
    }
    // Bind exactly the version requested by wf-recorder, not the advertised maximum.
    let dmabuf: ZwpLinuxDmabufV1 = globals.bind(&handle, 4..=4, ())?;
    let compositor: WlCompositor = globals.bind(&handle, 1..=5, ())?;
    let surface = compositor.create_surface(&handle, ());
    let default = dmabuf.get_default_feedback(&handle, 0);
    let per_surface = dmabuf.get_surface_feedback(&surface, &handle, 1);
    let mut state = Check::default();
    queue.roundtrip(&mut state)?;
    for (name, feedback) in ["default", "surface"].into_iter().zip(&state.feedback) {
        assert!(feedback.done, "{name}: feedback never completed");
        assert!(feedback.formats > 0, "{name}: no usable format table");
        assert!(
            feedback.main_tranche,
            "{name}: no formats for the main device"
        );
        let device = feedback.device.ok_or("no main device")?;
        let local = std::fs::read_dir("/dev/dri")?
            .filter_map(Result::ok)
            .any(|entry| entry.metadata().is_ok_and(|m| m.rdev() == device));
        assert!(
            local,
            "{name}: main device {device} is not a local DRM node"
        );
        println!(
            "PASS DMA-BUF {name}: {} format/modifier pairs, main device {device}",
            feedback.formats
        );
    }
    default.destroy();
    per_surface.destroy();
    surface.destroy();
    dmabuf.destroy();
    connection.flush()?;
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Check {
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
wayland_client::delegate_noop!(Check: ignore WlCompositor);
wayland_client::delegate_noop!(Check: ignore WlSurface);
wayland_client::delegate_noop!(Check: ignore ZwpLinuxDmabufV1);

impl Dispatch<ZwpLinuxDmabufFeedbackV1, usize> for Check {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let feedback = &mut state.feedback[*index];
        match event {
            zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => {
                feedback.device = Some(u64::from_ne_bytes(
                    device.try_into().expect("native dev_t must fit u64"),
                ));
            }
            zwp_linux_dmabuf_feedback_v1::Event::FormatTable { fd, size } => {
                assert!(size > 0 && size % 16 == 0, "invalid format table size");
                let mut bytes = vec![0; size as usize];
                File::from(fd)
                    .read_exact_at(&mut bytes, 0)
                    .expect("read complete format table");
                feedback.formats = bytes.len() / 16;
            }
            zwp_linux_dmabuf_feedback_v1::Event::TrancheTargetDevice { device } => {
                feedback.target = Some(u64::from_ne_bytes(
                    device.try_into().expect("native dev_t must fit u64"),
                ));
            }
            zwp_linux_dmabuf_feedback_v1::Event::TrancheFormats { indices } => {
                assert_eq!(indices.len() % 2, 0, "unaligned format indices");
                feedback.indices.extend(
                    indices
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|v| u16::from_ne_bytes([v[0], v[1]])),
                );
            }
            zwp_linux_dmabuf_feedback_v1::Event::TrancheDone => {
                assert!(!feedback.indices.is_empty(), "empty feedback tranche");
                assert!(
                    feedback
                        .indices
                        .iter()
                        .all(|i| usize::from(*i) < feedback.formats),
                    "format index outside table"
                );
                feedback.main_tranche |=
                    feedback.device.is_some() && feedback.target == feedback.device;
                feedback.target = None;
                feedback.indices.clear();
            }
            zwp_linux_dmabuf_feedback_v1::Event::Done => feedback.done = true,
            _ => {}
        }
    }
}
