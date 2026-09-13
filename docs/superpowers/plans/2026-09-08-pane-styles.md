# Pane Styles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A pane's look becomes a folder of QML declaring layers that sit behind the client, around it, or over it — each able to paint past the pane's own edge.

**Architecture:** A style is a `Pane.qml` declaring `Layer` children. Each layer is rasterised into its own scene and becomes its own render element, so the client's surface can be placed *between* them. A layer's canvas is the pane's outer rect grown by its declared `bleed`, and it is clipped to that. Bleed draws at the pane's own stacking depth; input stays clipped to the pane.

**Tech Stack:** Rust, Smithay 0.7, Qt 6 Quick, Lua (mlua).

**Spec:** `docs/superpowers/specs/2026-09-08-pane-styles-design.md`

**Depends on:** `docs/superpowers/plans/2026-09-08-qml-gpu-render-target.md`. Tasks 1–4 here are affordable on the software path and can be done first; Task 5 (bleed) is what needs the GPU, because a bleeding layer cannot use the band optimisation.

## Global Constraints

- Rust edition 2024. Workspace lints **deny** `unwrap_used`, `expect_used`, `panic`, `todo`. `unsafe_code` is `warn` — every `unsafe` block needs `#[expect(unsafe_code, reason = "…")]`.
- **All builds run in podman** via `dev/gate.sh`. The host has no Qt 6 development files.
- **Every shipped decoration must render byte-identically** after conversion. A conversion that changes a pixel is a bug in the conversion, not an improvement.
- **Insets are declared on `PaneStyle`, never on `Layer`.** The client is placed once; three layers declaring insets would be three answers to one question.
- **Bleed is a hard clip**, so damage stays bounded.
- `MAX_SIDE` for any scene is 8192 per side — `bleed` is author-controlled.
- Depth vocabulary is exactly `"behind"`, `"frame"`, `"above"`. No arbitrary z.
- Never launch `claude-desktop` inside Solium while testing.

---

## Amended 2026-09-13, after a drift check against the tree

This plan was written on 2026-09-08. The pane-ownership refactor, the GPU
render target, `crates/effects` and group addressing all landed since. **Task 9
is already done** — it became `docs/superpowers/plans/2026-09-12-pane-ownership.md`
and completed. Task 8 is phase 2. This plan is now Tasks 1–7.

### The one that would have shipped

**`scene.get_int("insets.top")` silently returns 0, always.** `host.cpp`'s
getter is `root->property(name)`, and `QObject::property` takes a *name*, not a
path — there is no `QQmlProperty` in `host.cpp` at all. Task 2's five tests
exercise `parse_depth` and `parse_bleed` and none of them touches `load()`, so
they go green while the client is drawn over the titlebar.

Task 2 must either reach the group object and then its property, or introduce
`QQmlProperty`. It must also have a test that reads a nested property back out
of a *loaded scene* rather than only parsing strings.

### Per task

**Task 2** — `qml::Scene::new` no longer exists; use `Scene::for_host(path, w,
h, None)`, because a GPU host refuses software scenes at construction. Written
against `Decorations::frames`/`bare`, both deleted; a pane now carries
`Frame::Pending | None | Styled(Decoration)`.

**Task 3** — `a_bare_name_prefers_the_users_directory` cannot pass against the
`resolve` in its own task: the candidate is not a directory, `user` is not
empty, so it falls through and returns `None`. The prose claims it is testable
without a filesystem; `is_dir()` means it is not.

**Task 4** — `layer_elements -> Vec<MemoryRenderBufferRenderElement<R>>` is
software-only; a frame is `Element::Chrome` or `Element::Screen` depending on
`Backing`, so the return type wants `render::Element`. Growing `Decoration` by a
`Vec` falsifies the measured "248 and 248" behind the
`#[expect(clippy::large_enum_variant)]` on `Frame::Styled` — re-measure and
update the reason. `Decorations` was **not** renamed to `StyleDefault` as Task 9
promised; it still exists with one field.

**Task 5** — `Solium::damage_for` does not exist; the path is
`self.drawn(id, pane_outer)` at `state.rs:904`. The point stands.

**Task 6** — clean. `insets_of` is now a match on `pane.frame()`, which makes
the invariant easier to hold than when this was written.

**Task 7** — three things:
* The eight shipped decorations **read** their own inset properties
  (`top.qml:91` is `height: frame.insetTop`; seven of eight do it). Deleting
  them gives eight `DIFFERS` with no clue why. They must become in-properties
  the compositor sets, like `contentWidth`.
* `dev/wirecheck/src/main.rs:1514` hardcodes
  `crates/solium/qml/decorations/top.qml`, which this task deletes — **and
  wirecheck runs on every gate.** Its "animating, both ways" control needs one
  process reading 1 for `quadrants.qml` and 0 for both `cursor.qml` and that
  decoration.
* `SOLIUM_DECORATION` lives in `decoration.rs` (six sites), not `dev.rs`. The
  file list also misses `config.lua:238`, `script.rs:1333` and `:2982`,
  `docs/decorations.md` and `docs/ricing.md`.

### Fixed in Task 1 rather than carried

`property QtObject insets` makes `insets.top: 32` **unassignable** — QML
resolves a grouped property against the property's *declared type*, not the
bound object, so `QtObject` has no `top`. This was in the brief, this plan, and
the spec's own example, in all three of `insets.top`, `client.radius` and
`client.shadow.blur`. The groups now have named types (`Insets`,
`ClientTreatment`, `ClientShadow`), `internal` in `qmldir`, so the public
surface is still the two types and the spec's examples now work as written.

---

### Task 1: `PaneStyle` and `Layer` QML types

**Files:**
- Create: `crates/solium/qml/Solium/PaneStyle.qml`
- Create: `crates/solium/qml/Solium/Layer.qml`
- Modify: `crates/solium/qml/Solium/qmldir`

**Interfaces:**
- Consumes: nothing.
- Produces: two QML types importable as `import Solium`. `PaneStyle` exposes `insets` (an object with `top`/`right`/`bottom`/`left`) and `layers` (its `Layer` children). `Layer` exposes `depth`, `bleed`, `source`, `name`.

- [ ] **Step 1: Write `Layer.qml`**

```qml
// One layer of a pane style.
//
// A layer is rasterised into its own scene and becomes its own element in the
// frame, which is what lets the client's surface sit between two layers the
// same style produced. Content is written inline or delegated with `source:`,
// and the syntax does not change between the two.

import QtQuick

Item {
    // Where the client's surface sits relative to this layer.
    //   "behind" — under the client
    //   "frame"  — where decorations are today
    //   "above"  — over the client
    property string depth: "frame"

    // How far past the pane's outer rect this layer may paint, in logical
    // pixels. A number for all four sides, or an object for per-side.
    //
    // It is a cost, not a permission: the canvas is this much larger, and
    // every pixel of it is rasterised, uploaded and repainted when the layer
    // animates. A bar throwing spikes upward should ask for `{ top: 160 }`
    // rather than 160, and not pay for three sides it never touches.
    property var bleed: 0

    // A QML file in the bundle, when the content is not inline.
    property string source: ""

    // For diagnostics: which layer a warning is about.
    property string name: ""
}
```

- [ ] **Step 2: Write `PaneStyle.qml`**

```qml
// A pane's whole appearance: its layers, and what they reserve.
//
// The entry point of a style bundle. `Pane.qml` in a folder under `panes/`
// declares one of these; the compositor reads it, then instantiates each
// Layer's content as its own scene.

import QtQuick

Item {
    id: style

    // Space reserved from the client, once, for the whole style.
    //
    // Not per layer: the client is placed once and every layer sees the same
    // client rect, so three layers each declaring insets would be three
    // answers to one question.
    property QtObject insets: QtObject {
        property int top: 0
        property int right: 0
        property int bottom: 0
        property int left: 0
    }

    // Reserved. Declared now so a style folder written today does not change
    // shape when client treatment is built — see the spec's *Reserved* section.
    // The compositor ignores these.
    property QtObject client: QtObject {
        property int radius: 0
        property QtObject shadow: QtObject {
            property int blur: 0
            property real opacity: 0
        }
    }

    // The Layer children, in declaration order. Read by the compositor.
    default property list<Item> layers
}
```

- [ ] **Step 3: Register both in `qmldir`**

```
module Solium
singleton Theme 1.0 Theme.qml
PaneStyle 1.0 PaneStyle.qml
Layer 1.0 Layer.qml
```

- [ ] **Step 4: Check a style loads**

Create `crates/solium/qml/panes/top/Pane.qml`:

```qml
import QtQuick
import Solium

PaneStyle {
    insets.top: 32
    Layer { depth: "frame"; name: "bar"; source: "Frame.qml" }
}
```

and `crates/solium/qml/panes/top/Frame.qml` as a copy of the existing
`crates/solium/qml/decorations/top.qml`.

```bash
cd /home/kotoxik/personal_projects/solium
./target/debug/solium --check-qml crates/solium/qml/panes/top/Pane.qml
```

Expected: `ok`. A QML error names the file and line.

- [ ] **Step 5: Commit**

```bash
git add crates/solium/qml/Solium crates/solium/qml/panes
git commit -m "qml: PaneStyle and Layer, the types a style bundle declares"
```

---

### Task 2: Read a style's layers from Rust

**Files:**
- Modify: `crates/solium/qml/host.h`
- Modify: `crates/solium/qml/host.cpp`
- Create: `crates/solium/src/style.rs`
- Modify: `crates/solium/src/main.rs`

**Interfaces:**
- Consumes: the QML types from Task 1.
- Produces:
  - FFI: `int solium_qml_scene_layer_count(const SoliumQmlScene *scene);` and `const char *solium_qml_scene_layer_field(const SoliumQmlScene *scene, int index, const char *field);`
  - `pub(crate) struct Style { pub(crate) insets: Insets, pub(crate) layers: Vec<LayerSpec>, pub(crate) dir: PathBuf }`
  - `pub(crate) struct LayerSpec { pub(crate) depth: Depth, pub(crate) bleed: Bleed, pub(crate) source: Option<PathBuf>, pub(crate) name: String, pub(crate) index: usize }`
  - `pub(crate) enum Depth { Behind, Frame, Above }`
  - `pub(crate) struct Bleed { pub(crate) top: i32, pub(crate) right: i32, pub(crate) bottom: i32, pub(crate) left: i32 }`
  - `pub(crate) fn load(dir: &Path) -> Result<Style>`

- [ ] **Step 1: Write the failing test**

In `crates/solium/src/style.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{Bleed, Depth, parse_bleed, parse_depth};

    #[test]
    fn depth_names_map_to_the_three_we_have() {
        assert_eq!(parse_depth("behind"), Depth::Behind);
        assert_eq!(parse_depth("above"), Depth::Above);
        assert_eq!(parse_depth("frame"), Depth::Frame);
    }

    /// An unknown depth is `Frame` rather than an error: a typo should put the
    /// layer where decorations already are, not make the whole style fail to
    /// load and leave the window with no frame at all.
    #[test]
    fn an_unknown_depth_falls_back_to_frame() {
        assert_eq!(parse_depth("beneath"), Depth::Frame);
        assert_eq!(parse_depth(""), Depth::Frame);
    }

    #[test]
    fn a_bare_number_bleeds_on_every_side() {
        assert_eq!(
            parse_bleed("120"),
            Bleed { top: 120, right: 120, bottom: 120, left: 120 }
        );
    }

    /// Per-side, because a bar throwing spikes upward should not pay for the
    /// other three sides. Absent sides are zero.
    #[test]
    fn per_side_bleed_leaves_the_rest_at_zero() {
        assert_eq!(
            parse_bleed(r#"{"top":160,"left":8}"#),
            Bleed { top: 160, right: 0, bottom: 0, left: 8 }
        );
    }

    /// Negative bleed would shrink the canvas below the pane and clip the
    /// frame itself.
    #[test]
    fn negative_bleed_is_clamped_to_zero() {
        assert_eq!(
            parse_bleed("-40"),
            Bleed { top: 0, right: 0, bottom: 0, left: 0 }
        );
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cd /home/kotoxik/personal_projects/solium
podman run --rm --userns=keep-id --security-opt label=disable -v "$HOME:$HOME" \
  -e CARGO_HOME="$HOME/.cargo" -e PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin" \
  -w "$PWD" localhost/solium-build:fc44 sh -c 'cargo test -p solium -j2 style::tests'
```

Expected: FAIL — `unresolved import crate::style`.

- [ ] **Step 3: Add the FFI for enumerating layers**

In `host.h`:

```c
/* How many Layer children the style declares. -1 if the root is not a
 * PaneStyle, which is how a bundle with the wrong root object is reported. */
int solium_qml_scene_layer_count(const SoliumQmlScene *scene);

/* One field of one layer, as a string. "depth", "bleed", "source", "name".
 * `bleed` comes back as a bare number or a JSON object. NULL if absent.
 * The returned pointer is valid until the next call on this scene. */
const char *solium_qml_scene_layer_field(const SoliumQmlScene *scene, int index,
                                         const char *field);
```

In `host.cpp`:

```cpp
static QList<QObject *> style_layers(const SoliumQmlScene *scene)
{
    QList<QObject *> out;
    if (!scene || !scene->root) {
        return out;
    }
    const QVariant declared = scene->root->property("layers");
    if (!declared.isValid()) {
        return out;
    }
    const QQmlListReference list(scene->root, "layers");
    if (!list.isValid()) {
        return out;
    }
    for (qsizetype i = 0; i < list.count(); ++i) {
        out.append(list.at(i));
    }
    return out;
}

extern "C" int solium_qml_scene_layer_count(const SoliumQmlScene *scene)
{
    if (!scene || !scene->root) {
        return -1;
    }
    if (!scene->root->property("layers").isValid()) {
        return -1;
    }
    return static_cast<int>(style_layers(scene).count());
}

extern "C" const char *solium_qml_scene_layer_field(const SoliumQmlScene *scene,
                                                    int index, const char *field)
{
    const QList<QObject *> layers = style_layers(scene);
    if (index < 0 || index >= layers.count() || !field) {
        return nullptr;
    }
    const QVariant value = layers.at(index)->property(field);
    if (!value.isValid()) {
        return nullptr;
    }
    // A JSON round-trip, so `bleed` comes back the same way whether it was
    // written as a number or an object, and the Rust side has one parser.
    static thread_local QByteArray held;
    if (value.canConvert<QVariantMap>() && value.typeId() == QMetaType::QVariantMap) {
        held = QJsonDocument(QJsonObject::fromVariantMap(value.toMap())).toJson(QJsonDocument::Compact);
    } else {
        held = value.toString().toUtf8();
    }
    return held.constData();
}
```

- [ ] **Step 4: Write `style.rs`**

```rust
//! A pane style: a folder of QML declaring how a window looks.
//!
//! The manifest is `Pane.qml` and it is QML rather than Lua on purpose —
//! style lives in QML and Lua configures the compositor, so a description of
//! how a thing looks belongs on the QML side of that line. It also means a
//! simple style is one file with inline layers and a complex one is a folder,
//! with no format to migrate between.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::decoration::Insets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Depth {
    Behind,
    Frame,
    Above,
}

fn parse_depth(name: &str) -> Depth {
    match name {
        "behind" => Depth::Behind,
        "above" => Depth::Above,
        // A typo puts the layer where decorations already are rather than
        // failing the style: a window with no frame at all is a worse answer
        // to a misspelling than a frame in the ordinary place.
        _ => Depth::Frame,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bleed {
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
    pub(crate) left: i32,
}

impl Bleed {
    pub(crate) const fn any(self) -> bool {
        self.top > 0 || self.right > 0 || self.bottom > 0 || self.left > 0
    }
}

/// `120`, or `{"top":160,"left":8}`. Negative is clamped: a negative bleed
/// would shrink the canvas below the pane and clip the frame itself.
fn parse_bleed(raw: &str) -> Bleed {
    let raw = raw.trim();
    if let Ok(all) = raw.parse::<f64>() {
        #[expect(clippy::cast_possible_truncation, reason = "a bleed is screen-sized")]
        let all = (all.round() as i32).max(0);
        return Bleed { top: all, right: all, bottom: all, left: all };
    }
    let side = |name: &str| -> i32 {
        let Some(at) = raw.find(&format!("\"{name}\"")) else {
            return 0;
        };
        let rest = &raw[at + name.len() + 3..];
        let digits: String = rest
            .trim_start_matches(':')
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-')
            .collect();
        digits.parse::<i32>().unwrap_or(0).max(0)
    };
    Bleed {
        top: side("top"),
        right: side("right"),
        bottom: side("bottom"),
        left: side("left"),
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LayerSpec {
    pub(crate) depth: Depth,
    pub(crate) bleed: Bleed,
    /// The file this layer's content lives in, when it is not inline.
    pub(crate) source: Option<PathBuf>,
    pub(crate) name: String,
    /// Position among the style's layers, which is what identifies an inline
    /// layer that has no file of its own.
    pub(crate) index: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct Style {
    pub(crate) insets: Insets,
    pub(crate) layers: Vec<LayerSpec>,
    pub(crate) dir: PathBuf,
}

/// Read a bundle's `Pane.qml`.
///
/// The manifest scene is loaded at 1x1 and never rendered: it declares
/// structure, and instantiating it at any size would rasterise content nobody
/// is going to look at.
pub(crate) fn load(dir: &Path) -> Result<Style> {
    let manifest = dir.join("Pane.qml");
    if !manifest.is_file() {
        return Err(anyhow!("{} has no Pane.qml", dir.display()));
    }
    let scene = crate::qml::Scene::new(&manifest, 1, 1)
        .with_context(|| format!("loading {}", manifest.display()))?;

    let count = scene.layer_count();
    if count < 0 {
        return Err(anyhow!(
            "{} does not declare a PaneStyle at its root",
            manifest.display()
        ));
    }

    let mut layers = Vec::new();
    for index in 0..count as usize {
        let source = scene
            .layer_field(index, "source")
            .filter(|it| !it.is_empty())
            .map(|it| dir.join(it));
        layers.push(LayerSpec {
            depth: parse_depth(&scene.layer_field(index, "depth").unwrap_or_default()),
            bleed: parse_bleed(&scene.layer_field(index, "bleed").unwrap_or_default()),
            source,
            name: scene.layer_field(index, "name").unwrap_or_default(),
            index,
        });
    }
    if layers.is_empty() {
        return Err(anyhow!("{} declares no layers", manifest.display()));
    }

    Ok(Style {
        insets: Insets {
            top: scene.get_int("insets.top"),
            right: scene.get_int("insets.right"),
            bottom: scene.get_int("insets.bottom"),
            left: scene.get_int("insets.left"),
        },
        layers,
        dir: dir.to_path_buf(),
    })
}
```

Add `Scene::layer_count` and `Scene::layer_field` wrapping the new FFI, in the
same shape as the existing `take_string`. Add `mod style;` to `main.rs`.

- [ ] **Step 5: Run the tests**

Same command as Step 2. Expected: PASS, 5 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/qml/host.h crates/solium/qml/host.cpp \
        crates/solium/src/style.rs crates/solium/src/main.rs crates/solium/src/qml.rs
git commit -m "style: read a bundle's layers out of its Pane.qml"
```

---

### Task 3: Find a bundle by name

**Files:**
- Modify: `crates/solium/src/style.rs`

**Interfaces:**
- Consumes: `style::load`.
- Produces: `pub(crate) fn find(name: &str) -> Option<PathBuf>`.

- [ ] **Step 1: Write the failing test**

```rust
    /// The lookup order is the one decorations already use and the one
    /// `sol.surface` uses: the user's directory shadows the shipped one, name
    /// by name, so replacing a style means dropping in a folder rather than
    /// copying everything else.
    #[test]
    fn a_path_is_taken_as_given() {
        assert_eq!(
            super::resolve("/tmp/neon", None),
            Some(std::path::PathBuf::from("/tmp/neon"))
        );
    }

    #[test]
    fn a_bare_name_prefers_the_users_directory() {
        let user = std::path::Path::new("/home/someone/.config/solium/panes");
        assert_eq!(
            super::resolve("neon", Some(user)),
            Some(user.join("neon"))
        );
    }
```

`resolve` takes the user directory as an argument rather than reading the
environment, so it is testable without one.

- [ ] **Step 2: Run it and watch it fail**

Expected: FAIL — `cannot find function resolve`.

- [ ] **Step 3: Implement**

```rust
/// Where a style bundle called `name` is, or `None`.
///
/// Split from `find` so the lookup order can be tested without an environment
/// or a filesystem: `find` supplies the real directories, this decides.
fn resolve(name: &str, user: Option<&Path>) -> Option<PathBuf> {
    if name.contains('/') {
        return Some(PathBuf::from(name));
    }
    if let Some(user) = user {
        let candidate = user.join(name);
        if candidate.is_dir() || user.as_os_str().is_empty() {
            return Some(candidate);
        }
    }
    let own = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/panes")).join(name);
    own.is_dir().then_some(own)
}

pub(crate) fn find(name: &str) -> Option<PathBuf> {
    let user = crate::qml::user_qml_dir().map(|dir| dir.join("panes"));
    resolve(name, user.as_deref())
}
```

- [ ] **Step 4: Run the tests**

Expected: PASS, 7 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/solium/src/style.rs
git commit -m "style: find a bundle, user directory first"
```

---

### Task 4: Render a style's layers, at their depths

**Files:**
- Modify: `crates/solium/src/decoration.rs`
- Modify: `crates/solium/src/render.rs`

**Interfaces:**
- Consumes: `Style`, `LayerSpec`, `Depth`.
- Produces:
  - `Decoration::from_style(style: &Style, width: i32, height: i32) -> Result<Decoration>` — builds one scene per layer.
  - `Decoration::layer_elements(&mut self, renderer, outer, depth, now, alpha, scale) -> Vec<MemoryRenderBufferRenderElement<R>>`

- [ ] **Step 1: Build a scene per layer**

In `decoration.rs`, `Decoration` gains:

```rust
    /// One scene per declared layer, in declaration order.
    ///
    /// Separate scenes rather than one image with passes: nothing else lets a
    /// client's surface sit *between* two layers the same style produced,
    /// which is the whole reason layers exist.
    layers: Vec<LayerScene>,

struct LayerScene {
    spec: crate::style::LayerSpec,
    scene: crate::qml::Scene,
    buffer: Option<MemoryRenderBuffer>,
}
```

`from_style` instantiates each layer: a layer with `source` loads that file; an
inline layer loads the manifest again and is told which layer it is, through a
`solium.layerIndex` property the compositor sets, so `Pane.qml` renders only
that child. Add to `PaneStyle.qml`:

```qml
    // Set by the compositor before a scene is rendered: which layer this
    // instance is drawing. An inline layer hides its siblings on it.
    property int layerIndex: -1
```

and in `Layer.qml`:

```qml
    // An inline layer draws only when the scene was built for it.
    visible: parent === null || parent.layerIndex < 0
             || parent.layers.indexOf(this) === parent.layerIndex
```

- [ ] **Step 2: Return elements per depth**

`render::elements` calls `layer_elements` three times per pane, at the three
points in the list where the depths belong. The existing single-frame call site
becomes the `Depth::Frame` call; `Depth::Above` goes immediately before it in
the list (so it draws over), and `Depth::Behind` immediately after the client's
window element (so it draws under).

- [ ] **Step 3: Check a one-layer style is unchanged**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh
SOLIUM_CAPTURE=/tmp/before SOLIUM_CAPTURE_FRAMES=1 SOLIUM_CAPTURE_AT=4000 \
  git stash && ./target/debug/solium & sleep 6; pkill -x solium
```

Capture one frame on the current build and one on the new build with
`pane = "top"`, and compare:

```bash
cmp /tmp/before-000 /tmp/after-000 && echo "identical"
```

Expected: `identical`. A one-layer `frame` style with the same QML must produce
the same pixels; anything else is a bug in the conversion.

- [ ] **Step 4: Commit**

```bash
git add crates/solium/src/decoration.rs crates/solium/src/render.rs crates/solium/qml/Solium
git commit -m "style: a scene per layer, drawn at the depth it asked for"
```

---

### Task 5: Bleed

**Files:**
- Modify: `crates/solium/src/decoration.rs`
- Modify: `crates/solium/src/render.rs`

**Interfaces:**
- Consumes: `Bleed`, `LayerScene`.
- Produces: `LayerScene::canvas(outer: Rectangle<i32, Logical>) -> Rectangle<i32, Logical>`.

- [ ] **Step 1: Write the failing test**

In `decoration.rs`:

```rust
    use crate::style::Bleed;
    use smithay::utils::{Logical, Rectangle};

    /// The canvas is the pane grown by the bleed, and the pane's own corner
    /// moves within it — which is why a layer is told `bleedLeft` and
    /// `bleedTop`: `anchors.fill: parent` covers the canvas, and QML needs a
    /// known origin to position the window's own corner against.
    #[test]
    fn a_canvas_is_the_pane_grown_by_its_bleed() {
        let outer = Rectangle::<i32, Logical>::new((100, 200).into(), (800, 600).into());
        let bleed = Bleed { top: 40, right: 10, bottom: 0, left: 20 };
        let canvas = super::canvas(outer, bleed);
        assert_eq!(canvas.loc.x, 80);
        assert_eq!(canvas.loc.y, 160);
        assert_eq!(canvas.size.w, 830);
        assert_eq!(canvas.size.h, 640);
    }

    #[test]
    fn no_bleed_means_the_canvas_is_the_pane() {
        let outer = Rectangle::<i32, Logical>::new((0, 0).into(), (400, 300).into());
        assert_eq!(super::canvas(outer, Bleed::default()), outer);
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
podman run --rm --userns=keep-id --security-opt label=disable -v "$HOME:$HOME" \
  -e CARGO_HOME="$HOME/.cargo" -e PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin" \
  -w "$PWD" localhost/solium-build:fc44 sh -c 'cargo test -p solium -j2 decoration::tests::canvas'
```

Expected: FAIL — `cannot find function canvas`.

- [ ] **Step 3: Implement**

```rust
/// The rectangle a layer is rasterised into: the pane, grown by its bleed.
pub(crate) fn canvas(
    outer: Rectangle<i32, Logical>,
    bleed: crate::style::Bleed,
) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (outer.loc.x - bleed.left, outer.loc.y - bleed.top).into(),
        (
            outer.size.w + bleed.left + bleed.right,
            outer.size.h + bleed.top + bleed.bottom,
        )
            .into(),
    )
}
```

In `layer_elements`, size the scene to `canvas` rather than `outer`, position
the element at `canvas.loc`, and set `bleedLeft`/`bleedTop`/`paneWidth`/
`paneHeight` on the scene each frame.

**The band optimisation must be skipped for any layer where `bleed.any()`.**
Bands are the inset strips, and a bleeding layer paints outside all of them —
copying only the bands would upload the titlebar and drop the spikes.

**The damage rect is the canvas, not the pane.** The element's geometry is
already `canvas`, so Smithay's damage tracking follows it — but the pane's own
damage, raised when a window moves or animates, is computed from `pane_outer`
in `Solium::damage_for`. A bleeding layer must contribute `canvas` there
instead, or an animating bleed leaves trails across its neighbours: the region
it vacated is never marked dirty and never repainted.

- [ ] **Step 4: Run the tests**

Expected: PASS.

- [ ] **Step 5: Check the bleed actually escapes, and is clipped**

Write `crates/solium/qml/panes/bleedtest/Pane.qml` declaring one `above` layer
with `bleed: 100` that fills its whole canvas with opaque red, then:

```bash
SOLIUM_PANE=bleedtest SOLIUM_CAPTURE=/tmp/bleed SOLIUM_CAPTURE_FRAMES=1 \
  SOLIUM_CAPTURE_AT=5000 ./target/debug/solium &
sleep 3; WAYLAND_DISPLAY=wayland-1 ./target/debug/wl-probe & sleep 4; pkill -x solium
```

Then assert red exists outside the window's own rect and stops exactly 100px
out:

```bash
python3 - <<'EOF'
d = open('/tmp/bleed-000','rb').read(); p = d.split(b'\n',3)
w,h = map(int,p[1].split()); px = p[3]
red = [(x,y) for y in range(0,h,2) for x in range(0,w,2)
       if px[(y*w+x)*3:(y*w+x)*3+3] == b'\xff\x00\x00']
xs = [x for x,_ in red]; ys = [_y for _,_y in red]
print(f"red spans x {min(xs)}..{max(xs)} y {min(ys)}..{max(ys)}")
EOF
```

Expected: the span is the window's rect grown by 100 on each side, and no more.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/decoration.rs crates/solium/src/render.rs crates/solium/qml/panes
git commit -m "style: a layer may paint past its pane, exactly as far as it said"
```

---

### Task 6: Input stays inside the pane

**Files:**
- Modify: `crates/solium/src/state.rs`

**Interfaces:**
- Consumes: `Decoration`, `canvas`.
- Produces: no new API. `frame_under` and `decorated_under` keep testing the pane's outer rect.

- [ ] **Step 1: Write the failing test**

```rust
    /// Drawing is unclipped; input is not. A spike reaching over the next
    /// window must not eat that window's clicks — the failure mode is a
    /// neighbour that has silently stopped responding, with nothing on screen
    /// to explain it.
    #[test]
    fn a_point_in_the_bleed_is_not_in_the_pane() {
        let outer = Rectangle::<i32, Logical>::new((100, 100).into(), (200, 200).into());
        let bleed = crate::style::Bleed { top: 50, right: 50, bottom: 50, left: 50 };
        let canvas = crate::decoration::canvas(outer, bleed);
        let in_bleed = smithay::utils::Point::<f64, smithay::utils::Logical>::from((70.0, 120.0));
        assert!(canvas.to_f64().contains(in_bleed));
        assert!(!outer.to_f64().contains(in_bleed));
    }
```

- [ ] **Step 2: Run it and watch it fail**

Expected: FAIL if any hit-test was changed to use the canvas. If it passes
immediately, the invariant is already held — keep the test, it is what stops a
later change from breaking it.

- [ ] **Step 3: Audit every hit-test**

`grep -n "canvas" crates/solium/src/state.rs crates/solium/src/input/mod.rs`
must return nothing outside rendering. `frame_under`, `decorated_under`,
`window_under`, `surface_under` and `resize_target` all use `pane_outer` and
must keep doing so.

- [ ] **Step 4: Verify with a real pointer**

Using the `bleedtest` style from Task 5, place a second window under the bleed
and drive a real click into the bleed region:

```bash
SOLIUM_PANE=bleedtest SOLIUM_DRAG_AT="6000:<x>,<y>>-<x>,<y>" ./target/debug/solium
```

where `<x>,<y>` is inside the first window's bleed and over the second window.
`SOLIUM_DRAG_AT` goes through the real pointer path; `SOLIUM_CLICK_AT` does not
and would pass while proving nothing.

Expected: the second window takes focus.

- [ ] **Step 5: Commit**

```bash
git add crates/solium/src/state.rs
git commit -m "style: bleed draws over a neighbour and never takes its clicks"
```

---

### Task 7: Convert the shipped decorations, and rename the setting

**Files:**
- Create: `crates/solium/qml/panes/{top,left,bottom,border,reactive,proximity,reveal,pulse}/`
- Delete: `crates/solium/qml/decorations/`
- Modify: `crates/solium/lua/config.lua`
- Modify: `crates/solium/src/script.rs`
- Modify: `crates/solium/src/dev.rs`
- Modify: `docs/ricing.md`, `crates/solium/qml/panes/README.md`

**Interfaces:**
- Consumes: `style::find`, `style::load`.
- Produces: `sol.pane(name)` replacing `sol.decoration(name)`; `decoration` accepted as a silent alias.

- [ ] **Step 1: Convert each of the eight**

For each name, create `panes/<name>/Pane.qml` declaring the insets its QML used
to declare, one `Layer { depth: "frame"; source: "Frame.qml" }`, and move the
old `decorations/<name>.qml` to `panes/<name>/Frame.qml` with its
`insetTop`/`insetRight`/`insetBottom`/`insetLeft` properties **removed** — they
live in `Pane.qml` now.

- [ ] **Step 2: Prove each is byte-identical**

For each of the eight, capture a frame on the pre-conversion build and the
post-conversion build and compare:

```bash
for name in top left bottom border reactive proximity reveal pulse; do
  SOLIUM_PANE=$name SOLIUM_CAPTURE=/tmp/after-$name SOLIUM_CAPTURE_FRAMES=1 \
    SOLIUM_CAPTURE_AT=4000 ./target/debug/solium & sleep 6; pkill -x solium
  cmp -s /tmp/before-$name-000 /tmp/after-$name-000 \
    && echo "$name identical" || echo "$name DIFFERS"
done
```

Expected: eight `identical`. A `DIFFERS` is a conversion bug — most likely an
inset that moved but changed value, or a `Frame.qml` still declaring its own.

- [ ] **Step 3: Rename the setting, with an alias**

In `config.lua`, `decoration = "top"` becomes `pane = "top"`, with the comment
explaining that a style is a folder. In `script.rs`, `sol.pane` replaces
`sol.decoration`, and `sol.decoration` stays as a one-line alias:

```rust
    // The old name for `sol.pane`. A style used to be a single QML file and
    // is now a folder; the name changed with it. Kept so no configuration
    // written before the change breaks, and cheap enough to keep until there
    // is a reason to remove it.
    sol.set("decoration", sol.get::<mlua::Function>("pane")?)?;
```

Same for `SOLIUM_DECORATION` → `SOLIUM_PANE` in `dev.rs`.

- [ ] **Step 4: Move the contract documentation**

`crates/solium/qml/decorations/README.md` becomes
`crates/solium/qml/panes/README.md`, rewritten for the bundle format: what
`Pane.qml` declares, the `Layer` properties, what the compositor sets on every
layer, and the cost of bleed. Update the `docs/ricing.md` recipe and the
`docs/decorations.md` link.

- [ ] **Step 5: Gate and commit**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh
git add -A
git commit -m "style: the shipped decorations become bundles, and decoration becomes pane"
```

---

### Task 8: A rule picks the style

**Files:**
- Modify: `crates/solium/src/script.rs`
- Modify: `crates/solium/src/state.rs`
- Modify: `crates/solium/lua/config.lua`
- Create: `crates/solium/lua/rules.lua`

**Interfaces:**
- Consumes: `style::find`, `style::load`, `Decoration::from_style`.
- Produces: `sol.pane(name)` gains an optional pane id and options table:
  `sol.pane(id, name, options)` sets one pane's style; `sol.pane(name)` sets the
  default. `WindowInfo` gains `app_id: String`.

- [ ] **Step 1: Expose `app_id` to scripts**

`WindowInfo` gains `pub(crate) app_id: String`, filled from the existing
`Solium::window_app_id`. The shell's JSON already carries it; `sol.windows()`
does not, and a rule cannot match without it.

- [ ] **Step 2: Write `rules.lua`**

```lua
-- Which style each window gets.
--
-- The compositor knows nothing about applications. This reads
-- `config.rules`, matches on `app_id`, and asks for a style — the same call a
-- global default makes, with a pane id in front of it.

local config = require("config")

local rules = {}

function rules.for_window(window)
    for _, rule in ipairs(config.rules or {}) do
        if rule.app_id and window.app_id == rule.app_id then
            return rule
        end
    end
    return nil
end

function rules.apply(id)
    for _, window in ipairs(sol.windows()) do
        if window.id == id then
            local rule = rules.for_window(window)
            if rule and rule.pane then
                sol.pane(id, rule.pane, rule.options)
            end
            return
        end
    end
end

-- A window's app_id is known when it opens, not when its pane is created.
sol.on("open", rules.apply)

return rules
```

Add `require("rules")` to `init.lua`, and a commented `rules = { … }` example
to `config.lua`.

- [ ] **Step 3: Per-pane style in the compositor**

`Decorations::style` becomes the *default*; a pane whose `Frame` carries its
own `Style` uses that. `Command::Pane { id: Option<u64>, name: String, options: String }`
sets one or the other. `options` is the JSON from `json_object`, set as
properties on each layer scene's root — the `PaneStyle` root, per the spec, so
three layers do not each hold a copy that can disagree.

- [ ] **Step 4: Verify a rule selects a different style**

```bash
cat > /tmp/ruletest/solium/user.lua <<'LUA'
return { pane = "top", rules = { { app_id = "wl-probe", pane = "border" } } }
LUA
XDG_CONFIG_HOME=/tmp/ruletest SOLIUM_CAPTURE=/tmp/rule SOLIUM_CAPTURE_FRAMES=1 \
  SOLIUM_CAPTURE_AT=6000 ./target/debug/solium &
sleep 3; WAYLAND_DISPLAY=wayland-1 WL_PROBE_WINDOWS=10 ./target/debug/wl-probe &
sleep 5; pkill -x solium
```

Expected: the probe's windows wear `border` (no titlebar) while anything else
wears `top`. Compare against a capture with the rule removed.

- [ ] **Step 5: Gate and commit**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh
git add -A
git commit -m "style: a rule gives one application its own pane style"
```

---

### Task 9: The pane owns its frame

**Files:**
- Modify: `crates/solium/src/pane.rs`
- Modify: `crates/solium/src/decoration.rs`
- Modify: `crates/solium/src/state.rs`

**Interfaces:**
- Consumes: `Decoration`, `Style`.
- Produces: `Pane::frame(&self) -> &Frame`, `Pane::frame_mut(&mut self) -> &mut Frame`, `enum Frame { Pending, None, Styled(Box<Decoration>) }`. `Decorations` keeps only `style` and becomes `StyleDefault`.

- [ ] **Step 1: Write the failing test**

In `pane.rs`:

```rust
    /// `frames` and `bare` were two answers to one question and nothing
    /// stopped a pane being in both — `insets_of` checked `frames` first, so
    /// `bare` was silently shadowed and nothing tested it. One value makes
    /// the state unrepresentable.
    #[test]
    fn a_pane_has_exactly_one_frame_state() {
        // `Pane::loading` is how `pane.rs`'s existing tests build one; there
        // is no bare constructor and adding one for a test would be a second
        // way to make a pane.
        let mut pane = Pane::loading(
            "test",
            Some(1),
            slot(),
            std::path::PathBuf::new(),
            None,
            Duration::ZERO,
        );
        assert!(matches!(pane.frame(), Frame::Pending));
        pane.set_frame(Frame::None);
        assert!(matches!(pane.frame(), Frame::None));
    }

    /// A pane whose client has not arrived keeps reserving its insets, which
    /// is what `bare` existed to distinguish from "never has a frame".
    #[test]
    fn pending_reserves_and_none_does_not() {
        assert!(super::Frame::Pending.reserves());
        assert!(!super::Frame::None.reserves());
    }
```

- [ ] **Step 2: Run it and watch it fail**

Expected: FAIL — `no method named frame`.

- [ ] **Step 3: Move the state in**

```rust
/// Whether this pane has a frame, and which.
///
/// One value, because two collections keyed by pane could disagree: a pane in
/// both `frames` and `bare` had `bare` silently ignored, and nothing tested
/// it. `Pending` and `None` are genuinely different — one keeps reserving
/// room for a frame that is coming, the other never will have one.
#[derive(Debug, Default)]
pub(crate) enum Frame {
    #[default]
    Pending,
    None,
    Styled(Box<crate::decoration::Decoration>),
}

impl Frame {
    pub(crate) const fn reserves(&self) -> bool {
        matches!(self, Self::Pending | Self::Styled(_))
    }
}
```

`Pane` gains `frame: Frame`. `Solium::insets_of` becomes a match on it.
`Decorations::frames` and `Decorations::bare` go, and with them the
`self.decorations.retain(…)` line in `sync_panes`. `closing` and `asked` move
in the same way, as `Pane::closing_at` and `Pane::asked_at`.

`hovered_frame` stays on `Solium`: it is which pane the pointer is over, a
property of the pointer rather than of a pane, and moving it in would mean a
bool on every pane and a scan to find the one that is true.

- [ ] **Step 4: Run the tests**

Expected: PASS.

- [ ] **Step 5: Check a crashing client leaves nothing behind**

This is the bug that produced the old design — a client that crashed used to
leave its frame forever.

```bash
./target/debug/solium &
sleep 3; WAYLAND_DISPLAY=wayland-1 ./target/debug/wl-probe &
sleep 2; pkill -9 wl-probe
sleep 2; # the frame must go with the pane
```

Expected: the frame disappears with the window. With `SOLIUM_MEMDIAG=1`, the
scene count returns to what it was before the client connected.

- [ ] **Step 6: Gate and commit**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh
git add -A
git commit -m "style: a pane owns its frame, so nothing has to remember to tidy up"
```
