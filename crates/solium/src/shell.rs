//! The shell's own surfaces, drawn by the compositor.
//!
//! A dock here is not a client. That is the entire reason it lives in this
//! process: an icon in this scene and a window on the screen are placed by the
//! same compositor, through the same presentation transform, so a window can
//! grow *out of* an icon and shrink back into it. A dock in a separate process
//! can only be a rectangle a window happens to fly past — the icon and the
//! window belong to different scenes, and nothing can interpolate between two
//! things that no single piece of code holds at once.
//!
//! That is also why this is QML rather than drawing calls: it is the same
//! engine, the same `Solium.Theme`, and the same host that draws the window
//! frames. An object lifted from the dock into a titlebar keeps its colours
//! because it never left the design system.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::Result;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    utils::{Logical, Rectangle, Transform},
};

use crate::qml;

/// How tall the dock is, and how far it sits off the bottom edge.
const HEIGHT: i32 = 72;
const MARGIN: i32 = 12;
/// One icon, and the space around it.
const ICON: i32 = 48;
const GAP: i32 = 9;
const PADDING: i32 = 12;

/// The dock.
#[derive(Debug)]
pub(crate) struct Dock {
    scene: qml::Scene,
    buffer: Option<MemoryRenderBuffer>,
    size: (i32, i32),
    items: Vec<String>,
    hovered: i32,
    /// Set when a press landed on an icon, read out after the pointer's lock
    /// is released — the same shape the window grabs use, and for the same
    /// reason.
    pub(crate) pressed: Option<usize>,

    /// What the scene was built from, so it can be built again.
    source: PathBuf,
    screen: String,
    /// The newest modification time seen in the QML, and when that was last
    /// checked. Shell work is edit-look-edit, and restarting a compositor to
    /// see a colour change is enough friction to stop anyone tuning anything.
    newest: Option<SystemTime>,
    checked: Duration,
}

/// How often the QML is checked for edits. Twice a second is well under what
/// anyone notices and well over what a directory scan costs.
const RELOAD_INTERVAL: Duration = Duration::from_millis(500);

impl Dock {
    pub(crate) fn new(screen: &str) -> Result<Self> {
        qml::start()?;
        // The shell's own dock declares `required property var screenInfo`,
        // and a required property must be supplied before the component is
        // built — afterwards is too late and it never exists at all.
        let source = qml_path();
        let scene = qml::Scene::with_properties(&source, 1, HEIGHT, Some(screen))?;
        let newest = newest_change(&source);
        Ok(Self {
            scene,
            source,
            screen: screen.to_owned(),
            newest,
            checked: Duration::ZERO,
            buffer: None,
            size: (0, 0),
            items: Vec::new(),
            hovered: -1,
            pressed: None,
        })
    }

    /// What the dock holds. Given by a script, because which programs belong
    /// on a dock is not the compositor's opinion.
    pub(crate) fn set_items(&mut self, items: Vec<String>) {
        if items != self.items {
            self.items = items;
            self.scene.set_string("items", &self.items.join("\t"));
        }
    }

    pub(crate) fn items(&self) -> &[String] {
        &self.items
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Where the surface sits: the whole output.
    ///
    /// A shell surface covers the screen and decides for itself what part of
    /// it to paint. The dock is the clear case — it hides against the bottom
    /// edge and reveals when the pointer arrives, so it needs to *see* the
    /// pointer well before it is over anything drawn. Sizing the surface to
    /// the painted part instead means the pointer only ever arrives once the
    /// dock is already out, which it never is.
    pub(crate) fn rect(&self, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        area
    }

    /// Where the compositor's own placeholder dock draws, for the genie.
    pub(crate) fn plate(&self, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        let count = i32::try_from(self.items.len()).unwrap_or(0).max(1);
        let width = PADDING * 2 + count * ICON + (count - 1) * GAP;
        let x = area.loc.x + (area.size.w - width) / 2;
        let y = area.loc.y + area.size.h - HEIGHT - MARGIN;
        Rectangle::new((x, y).into(), (width, HEIGHT).into())
    }

    /// Where one icon sits, in compositor coordinates.
    ///
    /// This is the rectangle the genie animates from, which is why it is
    /// computed here rather than inside QML: the compositor has to know it to
    /// hand it to a script, and a value QML owns would have to be read back
    /// every frame to stay true.
    pub(crate) fn icon_rect(
        &self,
        index: usize,
        area: Rectangle<i32, Logical>,
    ) -> Option<Rectangle<i32, Logical>> {
        if index >= self.items.len() {
            return None;
        }
        let dock = self.plate(area);
        let step = ICON + GAP;
        let x = dock.loc.x + PADDING + i32::try_from(index).unwrap_or(0) * step;
        let y = dock.loc.y + (HEIGHT - ICON) / 2;
        Some(Rectangle::new((x, y).into(), (ICON, ICON).into()))
    }

    /// Which icon a point is over.
    pub(crate) fn icon_at(&self, area: Rectangle<i32, Logical>, x: f64, y: f64) -> Option<usize> {
        (0..self.items.len()).find(|index| {
            self.icon_rect(*index, area)
                .is_some_and(|rect| rect.to_f64().contains((x, y)))
        })
    }

    /// Pointer input, in compositor coordinates. Returns whether the dock took
    /// it — a press on the dock belongs to the dock, not to what is behind it.
    pub(crate) fn pointer(
        &mut self,
        area: Rectangle<i32, Logical>,
        x: f64,
        y: f64,
        pressed: Option<bool>,
    ) -> bool {
        let surface = self.rect(area).to_f64();
        if !surface.contains((x, y)) {
            return false;
        }
        // Everything reaches the scene, because a surface that hides itself
        // has to watch the whole screen to know when to stop.
        self.scene
            .pointer(x - surface.loc.x, y - surface.loc.y, pressed);

        let over = self.icon_at(area, x, y);
        let index = over.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1);
        if index != self.hovered {
            self.hovered = index;
            self.scene.set_string("hovered", &index.to_string());
        }

        // Acted on release, so a press that lands on the wrong icon can be
        // dragged off it and abandoned.
        if pressed == Some(false) {
            self.pressed = over;
        }
        true
    }

    /// Rebuild the scene if the QML changed on disk.
    ///
    /// The whole scene, not a patch of it: QML has no way to apply an edit to
    /// a live object tree, and a rebuilt scene is indistinguishable from a
    /// fresh one anyway. State the shell holds is lost, which is the honest
    /// cost of reloading and the reason this is not on by default in a
    /// session anyone is using.
    fn reload_if_changed(&mut self, now: Duration) {
        if now.saturating_sub(self.checked) < RELOAD_INTERVAL {
            return;
        }
        self.checked = now;

        let newest = newest_change(&self.source);
        if newest == self.newest {
            return;
        }
        self.newest = newest;

        // Without this the engine hands back what it compiled last time.
        qml::clear_cache();

        match qml::Scene::with_properties(&self.source, 1, HEIGHT, Some(&self.screen)) {
            Ok(scene) => {
                self.scene = scene;
                // The buffer is thrown away with the scene: the new one may
                // want a different size, and a stale buffer would be drawn
                // once at the old one.
                self.buffer = None;
                self.size = (0, 0);
                if !self.items.is_empty() {
                    self.scene.set_string("items", &self.items.join("\t"));
                }
                tracing::info!("shell reloaded");
            }
            // The old scene keeps drawing. An edit that does not parse should
            // leave the screen as it was, not blank it.
            Err(err) => tracing::warn!(?err, "reload failed, keeping the last scene"),
        }
    }

    /// Draw the dock.
    pub(crate) fn element<R>(
        &mut self,
        renderer: &mut R,
        area: Rectangle<i32, Logical>,
        now: Duration,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        self.reload_if_changed(now);

        let rect = self.rect(area);
        let size = (rect.size.w.max(1), rect.size.h.max(1));
        self.scene.resize(size.0, size.1);
        self.scene.advance(now);

        let rendered = match self.scene.render() {
            Ok(rendered) => rendered,
            Err(err) => {
                tracing::warn!(?err, "the dock did not render");
                return None;
            }
        };

        let resized = self.size != size;
        if self.buffer.is_none() || resized {
            self.buffer = Some(MemoryRenderBuffer::new(
                Fourcc::Argb8888,
                size,
                1,
                Transform::Normal,
                None,
            ));
            self.size = size;
        }
        let buffer = self.buffer.as_mut()?;

        if rendered.changed || resized {
            let mut context = buffer.render();
            let copy = context.draw(|target| {
                let row_bytes = usize::try_from(size.0.max(0)).unwrap_or_default() * 4;
                for (row, destination) in target.chunks_exact_mut(row_bytes).enumerate() {
                    let start = row * rendered.stride;
                    let Some(source) = rendered.pixels.get(start..start + row_bytes) else {
                        return Err(());
                    };
                    destination.copy_from_slice(source);
                }
                Ok(vec![Rectangle::from_size(size.into())])
            });
            if copy.is_err() {
                tracing::warn!("the dock image was smaller than its buffer");
                return None;
            }
        }

        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (f64::from(rect.loc.x), f64::from(rect.loc.y)),
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .inspect_err(|err| tracing::warn!(?err, "could not upload the dock"))
        .ok()
    }
}

/// The newest modification time anywhere the shell's QML lives.
///
/// The file itself, and the tree it was loaded from when one was named —
/// editing a widget three directories away is still editing the shell.
fn newest_change(source: &Path) -> Option<SystemTime> {
    fn newest_in(directory: &Path, best: &mut Option<SystemTime>, depth: usize) {
        if depth > 4 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                newest_in(&path, best, depth + 1);
            } else if path.extension().is_some_and(|kind| kind == "qml") {
                if let Ok(time) = entry.metadata().and_then(|data| data.modified()) {
                    if best.is_none_or(|current| time > current) {
                        *best = Some(time);
                    }
                }
            }
        }
    }

    let mut newest = std::fs::metadata(source)
        .and_then(|data| data.modified())
        .ok();
    if let Some(root) = std::env::var_os("SOLIUM_SHELL_WATCH") {
        newest_in(Path::new(&root), &mut newest, 0);
    } else if let Some(parent) = source.parent() {
        newest_in(parent, &mut newest, 0);
    }
    newest
}

fn qml_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SOLIUM_QML_DOCK") {
        return PathBuf::from(path);
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/dock.qml"))
}
