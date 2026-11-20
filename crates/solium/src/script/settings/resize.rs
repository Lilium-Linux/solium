//! `resize`: how a window's picture is fitted while it changes size, what
//! fills the rest, and how long it waits for its application.

use crate::script::doc::{Doc, describe};

/// Read the section. Each key left out keeps its default, and one of the
/// wrong kind is named in the log and keeps its default too.
pub(crate) fn parse(options: &Doc) -> crate::resizing::Settings {
    let mut resize = crate::resizing::Settings::default();
    // A name that is not one of the four is named in the log and the
    // default kept, rather than guessed at. Someone who wrote `fill =
    // "stretched"` wants to be told, and a compositor that silently
    // picks for them is one they cannot debug.
    if let Doc::Text(name) = options.get("fill") {
        match crate::resizing::Fill::named(name) {
            Some(fill) => {
                // Said out loud rather than left to be discovered.
                // `scene` is a real setting and its render path is
                // real, but the only pane that has a scene to draw is
                // one whose application has not painted yet -- so on
                // an ordinary window it is `anchor` today, and someone
                // who chose it and saw no difference deserves to be
                // told why rather than left doubting their config.
                // `crate::resizing::Fill::Scene` carries the whole of
                // it, including what arming one would cost.
                if fill == crate::resizing::Fill::Scene {
                    tracing::warn!(
                        "resize.fill = \"scene\" only draws a scene a window already \
                         has, which today means one resized before its application \
                         painted; anywhere else it behaves as \"anchor\""
                    );
                }
                resize.fill = fill;
            }
            None => tracing::warn!(
                fill = %name,
                "resize.fill is one of anchor, crop, stretch or scene; keeping the \
                 default"
            ),
        }
    }
    // The same for the colour of what a picture does not cover, for
    // the same reasons. `the_shipped_resize_setting_reaches_the_compositor`.
    if let Doc::Text(name) = options.get("background") {
        match crate::resizing::background_named(name) {
            Some(background) => resize.background = background,
            None => tracing::warn!(
                background = %name,
                "resize.background is \"edge\", \"#rrggbb\", \"#rrggbbaa\" or \"none\"; \
                 keeping the default"
            ),
        }
    }
    // And how long a picture waits for its client once a change is
    // over (#233): milliseconds, 0 to `resizing::PATIENCE_MAX`, under
    // the same contract. `the_shipped_resize_setting_reaches_the_compositor`.
    match options.get("patience") {
        Doc::Nil => {}
        value => {
            let most = crate::resizing::PATIENCE_MAX.as_secs_f64() * 1000.0;
            match value
                .number()
                .filter(|millis| (0.0..=most).contains(millis))
                .and_then(|millis| std::time::Duration::try_from_secs_f64(millis / 1000.0).ok())
            {
                Some(patience) => resize.patience = patience,
                None => tracing::warn!(
                    value = describe(value),
                    most_ms = most,
                    "resize.patience is a number of milliseconds from 0 to most_ms; \
                     keeping the default"
                ),
            }
        }
    }
    // And how long the windows a layout's drag moves wait for one
    // another's applications: milliseconds, 0 to `resizing::SYNC_MAX`,
    // under the same contract. `the_shipped_resize_setting_reaches_the_compositor`.
    match options.get("sync") {
        Doc::Nil => {}
        value => {
            let most = crate::resizing::SYNC_MAX.as_secs_f64() * 1000.0;
            match value
                .number()
                .filter(|millis| (0.0..=most).contains(millis))
                .and_then(|millis| std::time::Duration::try_from_secs_f64(millis / 1000.0).ok())
            {
                Some(sync) => resize.sync = sync,
                None => tracing::warn!(
                    value = describe(value),
                    most_ms = most,
                    "resize.sync is a number of milliseconds from 0 to most_ms; \
                     keeping the default"
                ),
            }
        }
    }
    // And whether a window being resized is told to draw its next
    // size while its last is still being drawn: true or false, under
    // the same contract. `the_shipped_resize_setting_reaches_the_compositor`.
    match options.get("overlap") {
        Doc::Nil => {}
        Doc::Bool(overlap) => resize.overlap = *overlap,
        value => tracing::warn!(
            value = describe(value),
            "resize.overlap is true or false; keeping the default"
        ),
    }
    // What a window being dragged is drawn at: the size its
    // application has drawn, or the pointer's rectangle. Under the same
    // contract. `the_shipped_resize_setting_reaches_the_compositor`.
    if let Doc::Text(name) = options.get("follow") {
        match crate::resizing::Follow::named(name) {
            Some(follow) => resize.follow = follow,
            None => tracing::warn!(
                follow = %name,
                "resize.follow is \"app\" or \"pointer\"; keeping the default"
            ),
        }
    }
    // And how often a window being dragged is told its new size: 0 on
    // every frame, or at most that many times a second. Under the
    // same contract. `the_shipped_resize_setting_reaches_the_compositor`,
    // `resize_tell_hz_is_every_frame_or_a_rate`.
    match options.get("tell_hz") {
        Doc::Nil => {}
        value => match value.number().and_then(crate::resizing::tell_every) {
            Some(tell) => resize.tell = tell,
            None => tracing::warn!(
                value = describe(value),
                most = crate::resizing::TELL_HZ_MAX,
                "resize.tell_hz is 0 (every frame) or a number of times a second \
                 from 1 to most; keeping the default"
            ),
        },
    }
    resize
}
