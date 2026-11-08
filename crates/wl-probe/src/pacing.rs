//! `wp_fifo_v1` and `wp_commit_timing_v1` (#76), checked by what reaches the
//! screen: presentation feedback for every frame.
//!
//! - **fifo**: frames committed back to back, each waiting on the barrier the
//!   one before it set, must each be presented, one a refresh. A compositor
//!   that advertises the global and ignores the barriers applies them all at
//!   once, presents the last and discards the rest.
//! - **commit timing**: a frame stamped for a quarter of a second from now
//!   must be presented no earlier than that, and not long after it.

use std::time::Duration;

use wayland_client::{Connection, QueueHandle};

use crate::{Probe, settle, solid_buffer};

/// How many frames the fifo check queues at once.
const QUEUED: usize = 12;

/// How far ahead the timed frame is stamped, and how late it may be shown:
/// a refresh of a 30 Hz screen, and the compositor's loop, with room.
const AHEAD: Duration = Duration::from_millis(250);
const LATE: Duration = Duration::from_millis(100);

/// Both checks, on the window `map_and_measure` mapped.
pub(crate) fn check(
    connection: &Connection,
    queue: &mut wayland_client::EventQueue<Probe>,
    probe: &mut Probe,
) -> Result<(), String> {
    let handle = queue.handle();
    let (Some(fifo), Some(timing), Some(surface), Some(shm), Some(presentation)) = (
        probe.fifo.clone(),
        probe.commit_timing.clone(),
        probe.surface.clone(),
        probe.shm.clone(),
        probe.presentation.clone(),
    ) else {
        return Err(
            "fifo and commit timing need the window the presentation check maps".to_owned(),
        );
    };
    let buffers = [
        solid_buffer(&shm, &handle, 320, 200),
        solid_buffer(&shm, &handle, 320, 200),
    ];
    let frame = |index: usize, handle: &QueueHandle<Probe>| {
        presentation.feedback(&surface, handle, ());
        surface.attach(Some(&buffers[index % 2]), 0, 0);
        surface.damage(0, 0, i32::MAX, i32::MAX);
    };

    println!();
    probe.presented.clear();
    probe.discarded = 0;
    let barriers = fifo.get_fifo(&surface, &handle, ());
    for index in 0..QUEUED {
        frame(index, &handle);
        barriers.wait_barrier();
        barriers.set_barrier();
        surface.commit();
    }
    wait_for(QUEUED, connection, queue, probe)?;
    println!(
        "fifo: {QUEUED} frames queued at once   presented: {}   discarded: {}",
        probe.presented.len(),
        probe.discarded
    );
    if probe.presented.len() != QUEUED {
        return Err(format!(
            "of {QUEUED} frames queued with fifo barriers, {} were presented and {} discarded: \
             the barriers did not hold them one a refresh",
            probe.presented.len(),
            probe.discarded
        ));
    }
    if !probe
        .presented
        .windows(2)
        .all(|pair| pair[1].when > pair[0].when)
    {
        return Err("two frames queued with fifo barriers were presented at once".to_owned());
    }
    barriers.destroy();

    probe.presented.clear();
    probe.discarded = 0;
    let timer = timing.get_timer(&surface, &handle, ());
    let due = monotonic_now() + AHEAD;
    frame(0, &handle);
    let seconds = due.as_secs();
    timer.set_timestamp((seconds >> 32) as u32, seconds as u32, due.subsec_nanos());
    surface.commit();
    wait_for(1, connection, queue, probe)?;
    let Some(shown) = probe.presented.first().map(|frame| frame.when) else {
        return Err("a frame stamped with a time was never presented".to_owned());
    };
    let off = if shown >= due {
        format!("{:?} after", shown - due)
    } else {
        format!("{:?} BEFORE", due - shown)
    };
    println!("commit timing: a frame stamped {AHEAD:?} ahead was shown {off} its time");
    timer.destroy();
    if shown < due {
        return Err(format!("a frame stamped with a time was shown {off} it"));
    }
    if shown - due > LATE {
        return Err(format!(
            "a frame stamped with a time was shown {off} it, more than {LATE:?} late"
        ));
    }
    Ok(())
}

/// Round trips until `count` frames have been presented or discarded, for at
/// most two seconds.
fn wait_for(
    count: usize,
    connection: &Connection,
    queue: &mut wayland_client::EventQueue<Probe>,
    probe: &mut Probe,
) -> Result<(), String> {
    for _ in 0..100 {
        settle(connection, queue, probe)?;
        if probe.presented.len() + probe.discarded >= count {
            break;
        }
    }
    Ok(())
}

/// CLOCK_MONOTONIC, the clock `wp_presentation` names and commit timing's
/// timestamps are on.
fn monotonic_now() -> Duration {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    Duration::new(
        u64::try_from(now.tv_sec).unwrap_or_default(),
        u32::try_from(now.tv_nsec).unwrap_or_default(),
    )
}
