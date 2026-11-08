//! What the compositor tells a window manager, as the protocol's rows:
//! plain data, with no Smithay or renderer type in them, made in this one
//! place so the wire never sees an internal type.

use solium_wm_protocol as wire;

use crate::script::{MonitorInfo, Parentage, ProblemRow, Rect, WindowInfo};

pub(crate) fn rect(rect: Rect) -> wire::Rect {
    wire::Rect {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
    }
}

pub(crate) fn window(window: &WindowInfo) -> wire::Window {
    let size = |size: smithay::utils::Size<i32, smithay::utils::Logical>| wire::Size {
        w: size.w,
        h: size.h,
    };
    wire::Window {
        id: window.id,
        rect: rect(window.rect),
        insets: wire::Insets {
            top: window.insets.top,
            right: window.insets.right,
            bottom: window.insets.bottom,
            left: window.insets.left,
        },
        title: window.title.clone(),
        focused: window.focused,
        monitor: window.monitor.clone(),
        modal: window.modal,
        parent: match window.parent {
            Parentage::None => wire::Parentage::None,
            Parentage::Unknown => wire::Parentage::Unknown,
            Parentage::Window(id) => wire::Parentage::Window(id),
        },
        leaving: window.leaving,
        minimized: window.minimized,
        above: window.above,
        icon: window.icon.map(rect),
        app_id: window.app_id.clone(),
        min: window.min.map(size),
        max: window.max.map(size),
        cramped: window.cramped,
        shown: window.shown,
        fullscreen: window.fullscreen,
        maximized: window.maximized,
        style: window.style.clone(),
        client_side: window.client_side,
        blur_region: window.blur_region,
        x11_type: window.x11_type.clone(),
        accepts_input: window.accepts_input,
        class: window.class.clone(),
        instance: window.instance.clone(),
        content_type: window.content_type.to_owned(),
        pid: window.pid,
    }
}

pub(crate) fn monitor(monitor: &MonitorInfo) -> wire::Monitor {
    wire::Monitor {
        name: monitor.name.clone(),
        area: rect(monitor.area),
        whole: rect(monitor.whole),
        scale: monitor.scale,
        focused: monitor.focused,
        primary: monitor.primary,
        transform: monitor.transform.clone(),
        off: monitor.off,
    }
}

pub(crate) fn keyboard(keyboard: &crate::keymap::State) -> wire::Keyboard {
    wire::Keyboard {
        layouts: keyboard.layouts.clone(),
        short_names: keyboard.short.clone(),
        active: u32::try_from(keyboard.active).unwrap_or(u32::MAX),
        caps: keyboard.caps,
        num: keyboard.num,
        repeat_rate: keyboard.repeat_rate,
        repeat_delay: keyboard.repeat_delay,
    }
}

pub(crate) fn text_field(field: &crate::text_input::Field) -> wire::TextField {
    wire::TextField {
        window: field.window,
        caret: field.caret.map(|caret| wire::Rect {
            x: caret.loc.x,
            y: caret.loc.y,
            w: caret.size.w,
            h: caret.size.h,
        }),
        framed: field.framed,
    }
}

pub(crate) fn problem(problem: &ProblemRow) -> wire::Problem {
    wire::Problem {
        effect: problem.effect.clone(),
        file: problem.file.clone(),
        line: problem.line,
        column: problem.column,
        message: problem.message.clone(),
        severity: problem.severity.to_owned(),
    }
}
