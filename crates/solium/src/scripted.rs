//! Surfaces a script declares, drawn by the compositor in QML.
//!
//! One primitive for everything the compositor draws that is not a window and
//! not a client: a wallpaper, a bar, a dock, a heads-up display, a debug
//! overlay. A script names a QML file, says which layer and which monitors,
//! and hands over some properties; the compositor rasterises it and puts it in
//! the frame.
//!
//! ## Why this exists rather than a wallpaper
//!
//! The wallpaper was written first, the narrow way: a `Command::Wallpaper`, an
//! accessor on `Solium`, a special case in `render.rs`, a hundred lines of
//! Rust for a picture behind the windows. It worked, and it was the wrong
//! shape by this project's own test — `docs/modes.md` says that if a new thing
//! needs new Rust, the layer underneath is missing something.
//!
//! It also demonstrated the wrong claim. A built-in wallpaper says the
//! compositor has a wallpaper. This says anything QML can draw can be part of
//! the desktop without touching Rust, which is the claim the project actually
//! makes, and the wallpaper is now its first and smallest proof: `lua/wallpaper.lua`
//! is nine lines and there is no wallpaper code in the compositor at all.
//!
//! ## What is deliberately not here
//!
//! Input. These surfaces are drawn and not clicked. The shell and the tweaks
//! panel each have their own pointer routing today, and giving every scripted
//! surface a share of the pointer is a bigger question than this — it needs
//! the scoped grab in #85. A surface that wants clicks is still a layer-shell
//! client, which is the supported route and always was.

use std::{collections::HashMap, path::PathBuf};

use smithay::{
    output::Output,
    utils::{Logical, Rectangle},
};

use crate::surface::ShellSurface;

/// Where a surface sits in the frame.
///
/// Named after the wlr-layer-shell layers on purpose, and each one sits
/// *under* the matching layer of real client surfaces — so a `swaybg` on the
/// background layer covers a scripted background, and a real bar covers a
/// scripted one. A compositor's own furniture yielding to a client's is the
/// right way round: the client was installed on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Layer {
    /// Under everything, including client background surfaces.
    #[default]
    Background,
    /// Above the background, below windows.
    Bottom,
    /// Above windows, below client top and overlay surfaces.
    Top,
    /// Above client surfaces. Below the pointer, which is above everything.
    Overlay,
}

impl Layer {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "background" => Some(Self::Background),
            "bottom" => Some(Self::Bottom),
            "top" => Some(Self::Top),
            "overlay" => Some(Self::Overlay),
            _ => None,
        }
    }
}

/// Which monitors a surface is drawn on, and how big it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum On {
    /// One instance per monitor, filling it.
    EveryMonitor,
    /// One instance, filling the primary monitor.
    Primary,
    /// One instance, filling the monitor with this connector name.
    Monitor(String),
    /// Exactly this rectangle in the global space, on whichever monitors it
    /// overlaps.
    Rect(Rectangle<i32, Logical>),
}

/// What a script asked for. Carried from Lua to the compositor, so it holds no
/// rasterisations and can be cloned like every other command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Declaration {
    pub(crate) name: String,
    pub(crate) scene: PathBuf,
    pub(crate) layer: Layer,
    pub(crate) on: On,
    /// The property bag, already JSON.
    pub(crate) properties: String,
    /// Whether the pointer reaches it.
    ///
    /// Off by default, and that is the safe default rather than the tidy one:
    /// a full-screen background surface that took the pointer would swallow
    /// every click on the desktop, and the symptom would be "windows stopped
    /// responding" rather than anything mentioning wallpapers.
    pub(crate) interactive: bool,
}

/// A declaration, plus what has been rasterised for it.
#[derive(Debug)]
pub(crate) struct Surface {
    pub(crate) declared: Declaration,
    /// One rasterisation per monitor it is drawn on, keyed by connector name.
    ///
    /// Per monitor because a `ShellSurface` caches one rasterisation at one
    /// size: two screens of different sizes sharing one would re-rasterise a
    /// full-screen scene twice a frame, for ever.
    instances: HashMap<String, ShellSurface>,
}

impl Surface {
    pub(crate) fn new(declared: Declaration) -> Self {
        Self {
            declared,
            instances: HashMap::new(),
        }
    }

    pub(crate) fn name(&self) -> &str {
        &self.declared.name
    }

    pub(crate) fn layer(&self) -> Layer {
        self.declared.layer
    }

    pub(crate) fn interactive(&self) -> bool {
        self.declared.interactive
    }

    /// Offer the pointer to this surface's instance on one monitor.
    ///
    /// Returns whether it was inside. A surface that takes the pointer stops
    /// it reaching anything underneath, which is what makes a button a button.
    pub(crate) fn pointer(
        &mut self,
        output: &Output,
        area: Rectangle<i32, Logical>,
        x: f64,
        y: f64,
        pressed: Option<bool>,
    ) -> bool {
        self.instance(output)
            .is_some_and(|instance| instance.pointer(area, x, y, pressed))
    }

    /// Whatever the scene asked for since it was last looked at.
    ///
    /// The same one-way channel the window frames and the tweaks panel use:
    /// QML sets `action`, the compositor takes it and clears it, so a press is
    /// acted on once. Asked of every monitor's instance because the press
    /// landed on exactly one of them and this does not know which.
    pub(crate) fn taken_action(&mut self) -> Option<String> {
        self.instances
            .values_mut()
            .find_map(crate::surface::ShellSurface::taken_action)
    }

    /// Where this surface goes on one monitor, if it goes there at all.
    pub(crate) fn area_on(
        &self,
        output: &Output,
        geometry: Rectangle<i32, Logical>,
        primary: Option<&Output>,
    ) -> Option<Rectangle<i32, Logical>> {
        match &self.declared.on {
            On::EveryMonitor => Some(geometry),
            On::Primary => (Some(output) == primary).then_some(geometry),
            On::Monitor(name) => (&output.name() == name).then_some(geometry),
            // Clipped to the monitor, so a rect spanning two screens is drawn
            // on both and each gets its own share rather than the whole thing
            // twice.
            On::Rect(rect) => geometry.intersection(*rect),
        }
    }

    /// The rasterisation for one monitor, built on first use.
    pub(crate) fn instance(&mut self, output: &Output) -> Option<&mut ShellSurface> {
        let name = output.name();
        if !self.instances.contains_key(&name) {
            match ShellSurface::new(self.declared.scene.clone(), &self.declared.properties) {
                Ok(surface) => {
                    self.instances.insert(name.clone(), surface);
                }
                Err(err) => {
                    // Once per monitor, not once per frame: a scene that will
                    // not load is a line in the log and a gap in the picture,
                    // not a session that stops.
                    tracing::error!(
                        ?err,
                        surface = self.declared.name,
                        scene = %self.declared.scene.display(),
                        "that surface would not load"
                    );
                    return None;
                }
            }
        }
        self.instances.get_mut(&name)
    }

    /// Forget every rasterisation for a monitor that is no longer there.
    pub(crate) fn keep_only(&mut self, live: &[String]) {
        self.instances
            .retain(|monitor, _| live.iter().any(|name| name == monitor));
    }
}

/// Find a scene on the QML search path.
///
/// A bare name is looked up the way an `import` would be -- the user's
/// directory first, then the one that ships -- so dropping
/// `~/.config/solium/qml/wallpaper.qml` in place replaces the shipped scene
/// without copying anything else, which is the same rule decorations follow.
/// A path, absolute or `~`-prefixed, is taken as given.
pub(crate) fn find_scene(name: &str) -> Option<PathBuf> {
    if let Some(rest) = name.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return Some(PathBuf::from(home).join(rest));
    }
    if name.starts_with('/') {
        return Some(PathBuf::from(name));
    }
    if let Some(user) = crate::qml::user_qml_dir() {
        let path = user.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    let own = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml")).join(name);
    own.is_file().then_some(own)
}

/// One JSON string, quoted and escaped.
///
/// The property bag is JSON text and the values in it come from a script,
/// which means from a user. An unescaped quote in a file name ends the string
/// early and takes the whole scene with it -- which presents as the surface
/// silently not existing.
pub(crate) fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
