//! The lock screen's rule, which both peers hold to: a window manager, so a
//! script can be told at once; the compositor, whatever a window manager
//! says.

use crate::Command;

/// Whether a binding at the lock screen may not ask for this: act on a
/// window or a workspace, or end the session, which is how someone would
/// get to the desktop underneath a lock (the compositor refuses its own
/// Ctrl+Alt+Backspace there for the same reason). A binding allowed at the
/// lock may not ask for any of it (#224); everything else it may, which is
/// what a volume, brightness or media key needs.
///
/// Every command named, with no catch-all, so that one added to the
/// protocol is decided here rather than allowed by default.
pub fn reaches_past_the_lock(command: &Command) -> bool {
    use Command as C;
    match command {
        C::Present(_)
        | C::PresentFrom(_)
        | C::Group(_)
        | C::PresentGroup(_)
        | C::ClearGroup(_)
        | C::Clear(_)
        | C::Touch(_)
        | C::Click(_)
        | C::Focus(_)
        | C::Warp(_)
        | C::ToggleFullscreen(_)
        | C::ToggleMaximize(_)
        | C::DragHold(_)
        | C::DragGrip(_)
        | C::ResizeHold(_)
        | C::Minimize(_)
        | C::Unminimize(_)
        | C::ShowMinimized(_)
        | C::KeepAbove(_)
        | C::Press(_)
        | C::Place(_)
        | C::Unplace(_)
        | C::Close(_)
        | C::Quit => true,
        // Declaring the workspaces moves windows between them.
        C::Configure(configure) => configure.section == "workspaces",
        // `sol.act`'s verbs on windows and workspaces; `apps.launch` is a
        // spawn, and `folder` opens one. `notifications.invoke` brings the
        // sender's window forward, and a tray item's actions reach into its
        // application.
        C::Act(act) => {
            let action = &act.action;
            action.starts_with("windows.")
                || action.starts_with("workspaces.")
                || action.starts_with("tray.")
                || action.as_str() == "notifications.invoke"
        }
        C::Keep(_)
        | C::Listen(_)
        | C::Declare(_)
        | C::Grab(_)
        | C::Status(_)
        | C::Decoration(_)
        | C::PaneValues(_)
        // What a frame shows, which is nothing over a lock.
        | C::FrameValues(_)
        | C::Surface(_)
        | C::SurfaceGone(_)
        | C::Power(_)
        | C::Spawn(_)
        | C::RunDetached(_)
        | C::Run(_)
        | C::Timer(_)
        | C::TimerCancel(_)
        | C::FileRead(_)
        | C::FileWrite(_)
        | C::Watch(_)
        | C::WatchCancel(_)
        | C::Store(_)
        | C::Notify(_)
        // Over a lock, it changes nothing.
        | C::LockNow
        // Nor does a peek, which is nothing over a lock.
        | C::LockPeek(_)
        // No window holds the shortcuts at the lock, and taking them back
        // gives a window nothing.
        | C::TakeShortcutsBack
        // Nor the pointer: nothing holds it at the lock.
        | C::ReleasePointer
        | C::Reload => false,
    }
}
