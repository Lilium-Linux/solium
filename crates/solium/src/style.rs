//! A pane style: a folder of QML declaring how a window looks.
//!
//! The manifest is `Pane.qml` and it is QML rather than Lua on purpose —
//! style lives in QML and Lua configures the compositor, so a description of
//! how a thing looks belongs on the QML side of that line. It also means a
//! simple style is one file with inline layers and a complex one is a folder,
//! with no format to migrate between.
//!
//! This module is the *reading*. The types it reads are declared in
//! `qml/Solium/PaneStyle.qml` and `qml/Solium/Layer.qml`, and
//! `qml/panes/example/` is one written out in full.
//!
//! "Layer" is triple-booked in this tree and these are the third: `layer.rs` is
//! wlr-layer-shell, `scripted::Layer` is the `Background`/`Bottom`/`Top`/
//! `Overlay` a scripted surface sits in, and a [`LayerSpec`] here is one depth
//! within one pane's style. Nothing converts between them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::decoration::Insets;

/// Where the client's surface sits relative to a layer.
///
/// Three values, and a string in QML rather than an enumeration, so that a
/// fourth added later does not break the format of every style already
/// written — which is the ruling in the spec, not an implementation detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Depth {
    Behind,
    Frame,
    Above,
}

/// The vocabulary, and nothing else: `None` is "not one of the three".
///
/// Split from [`parse_depth`] so that [`load`] can *say* an unknown depth was
/// unknown. A fallback that cannot be distinguished from a correct answer is
/// the failure mode this whole task exists to avoid.
fn depth_from(name: &str) -> Option<Depth> {
    match name {
        "behind" => Some(Depth::Behind),
        "frame" => Some(Depth::Frame),
        "above" => Some(Depth::Above),
        _ => None,
    }
}

/// A depth name, with anything unrecognised put where decorations already are.
///
/// A typo puts the layer at `frame` rather than failing the style: a window
/// with no frame at all is a worse answer to a misspelling than a frame in the
/// ordinary place. But it is *said* — [`load`] warns, naming the bundle, the
/// layer and the word it did not know, because a layer that quietly becomes
/// `frame` is a style that looks wrong with nothing to grep for.
fn parse_depth(name: &str) -> Depth {
    depth_from(name).unwrap_or(Depth::Frame)
}

/// Logical pixels: rounded, and never negative.
///
/// A negative bleed would shrink the canvas below the pane and clip the frame
/// itself, which is the opposite of what the property is for.
fn pixels(value: f64) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a bleed is measured in screen pixels, and the cast saturates"
    )]
    let rounded = value.round() as i32;
    rounded.max(0)
}

/// How far past the pane's outer rect a layer may paint, per side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bleed {
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
    pub(crate) left: i32,
}

impl Bleed {
    /// Whether this layer needs a canvas larger than the pane at all.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the question Task 5 asks before growing a layer's canvas \
                      and its damage; the parser that answers it is here"
        )
    )]
    pub(crate) const fn any(self) -> bool {
        self.top > 0 || self.right > 0 || self.bottom > 0 || self.left > 0
    }
}

/// One named side of the object spelling of `bleed`.
///
/// A scan rather than a JSON parser, and that is a stated limit rather than a
/// shortcut: the only thing that ever produces this string is
/// `QJsonDocument::toJson(Compact)` in `qml/host.cpp`, the only keys are these
/// four, and none of the four is a substring of another. The day this has to
/// read something a person typed, it wants a real parser and not a fifth
/// special case.
fn side(raw: &str, name: &str) -> i32 {
    let key = format!("\"{name}\"");
    let Some(at) = raw.find(&key) else {
        return 0;
    };
    let Some(rest) = raw.get(at + key.len()..) else {
        return 0;
    };
    let number: String = rest
        .trim_start()
        .trim_start_matches(':')
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
        .collect();
    number.parse::<f64>().map_or(0, pixels)
}

/// `120`, or `{"top":160,"left":8}`.
///
/// Both spellings, because both are legal in QML and the property is a `var`:
/// a bare number bleeds on every side, an object names the sides it wants and
/// leaves the rest at zero. Per-side exists so that a bar throwing spikes
/// upward does not pay to rasterise three sides it never touches.
fn parse_bleed(raw: &str) -> Bleed {
    let raw = raw.trim();
    if let Ok(all) = raw.parse::<f64>() {
        let all = pixels(all);
        return Bleed {
            top: all,
            right: all,
            bottom: all,
            left: all,
        };
    }
    Bleed {
        top: side(raw, "top"),
        right: side(raw, "right"),
        bottom: side(raw, "bottom"),
        left: side(raw, "left"),
    }
}

/// One layer of a style, as declared.
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

/// A whole style, as declared.
#[derive(Clone, Debug)]
pub(crate) struct Style {
    /// Reserved from the client **once**, for the whole style.
    ///
    /// On `PaneStyle` and never on `Layer`: the client is placed once and every
    /// layer sees the same client rect, so three layers each declaring insets
    /// would be three answers to one question.
    pub(crate) insets: Insets,
    pub(crate) layers: Vec<LayerSpec>,
    pub(crate) dir: PathBuf,
}

/// The bundles that ship with the compositor.
///
/// Baked from `CARGO_MANIFEST_DIR`, which is what `qml::import_path` already
/// does with `qml/` and for the same reason: there is no install step in this
/// tree yet, so a path fixed at build time is at least true of the build that
/// fixed it. Both move together on the day there is one.
fn shipped() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/panes"))
}

/// Where a style bundle called `name` is, or `None`.
///
/// Split from [`find`] so the *order* is decided in one function that is handed
/// its directories rather than going looking for them — `find` supplies the
/// real ones, a test supplies its own.
///
/// The order is the one decorations already use and the one `sol.surface` uses:
/// the user's directory shadows the shipped one, name by name, so replacing a
/// style means dropping in a folder rather than copying everything else.
///
/// It **does** consult the filesystem, and cannot do otherwise: shadowing is a
/// question about which directories exist. The brief said this was testable
/// without one and it is not — a test asserting the user's copy wins has to
/// create the user's copy. See the plan's amendment of 2026-09-13.
fn resolve(name: &str, user: Option<&Path>) -> Option<PathBuf> {
    // `Path::join("")` is the directory itself, and `is_dir()` agrees, so
    // without this an empty name resolves to the whole `panes/` folder and the
    // failure surfaces later as a missing `Pane.qml` in a directory nobody
    // named. A name that is not a name has no bundle.
    if name.is_empty() {
        return None;
    }
    // A path is taken as given, existing or not: `load` then says "…has no
    // Pane.qml" and names it, where a `None` here would name nothing and leave
    // a typo indistinguishable from a style this build does not ship.
    if name.contains('/') {
        return Some(PathBuf::from(name));
    }
    if let Some(candidate) = user.map(|dir| dir.join(name))
        && candidate.is_dir()
    {
        return Some(candidate);
    }
    let own = shipped().join(name);
    own.is_dir().then_some(own)
}

/// Where the bundle called `name` is on this machine, or `None`.
///
/// `panes/` under the user's QML directory, because a style bundle is QML and
/// belongs beside the design system it imports — a `Solium/Theme.qml` dropped
/// in there is already what `Theme.titlebarHeight` resolves to inside a
/// bundle's `Pane.qml`, so splitting the two across different roots would mean
/// a style and the theme it is written against could come from different
/// places.
pub(crate) fn find(name: &str) -> Option<PathBuf> {
    let user = crate::qml::user_qml_dir().map(|dir| dir.join("panes"));
    resolve(name, user.as_deref())
}

/// Every term `requires` can name in this build, for a refusal to list.
const TERMS: [&str; 1] = ["gpu"];

/// Whether a process on `gpu` can offer `term`, or `None` for one this build
/// has never heard of.
///
/// > The software scene graph does not implement `ShaderEffect`, and `Canvas`
/// > appears not to paint on it; the GPU path does both. So which QML is legal
/// > depends on which machine you are on, and a style written on one hands a
/// > white rectangle to another, silently.
///
/// That is the whole reason `requires` exists, and `gpu` is its only term today.
fn provides(term: &str, gpu: bool) -> Option<bool> {
    match term {
        "gpu" => Some(gpu),
        _ => None,
    }
}

/// Refuse a style this process cannot run.
///
/// [`crate::qml::on_gpu`] and not `dev::qml_gpu()`: what Qt *did* with the
/// knob, not what it was asked for. It can refuse, and the answer is only known
/// once `start` has run — which by this point it has, because the scene being
/// read exists.
fn requirements(manifest: &Path, required: &[String]) -> Result<()> {
    requirements_on(manifest, required, crate::qml::on_gpu())
}

/// The decision, with the scene graph as an argument rather than as a global.
///
/// Split out because a test cannot choose one. Qt fixes its scene graph for the
/// life of the process, the gate's container has no render node, and there is
/// no way back — so a test of the real entry point only ever exercises the
/// software side, on every machine this is ever run on. Here both sides are
/// reachable.
///
/// Deliberately *not* the same move as making `resolve` take existence as a
/// parameter, which the brief asked for and this task did not do. A directory
/// is a fact a test can make, so faking one would swap a real check for a
/// pretend one. A scene graph is not.
///
/// **An unrecognised term is a refusal, not a warning.** `requires` is a list so
/// that `["gpu", "effects/2"]` is format versioning through the same mechanism,
/// and versioning that a build can ignore is not versioning: a style naming a
/// term this build has never heard of was written against a *later* one, so the
/// likeliest reading of it is "there is something here you do not know how to
/// draw". Warning and loading anyway would put back exactly the silence the
/// property exists to remove, one word further along — a style drawn wrong, on
/// a machine that had already been told it could not draw it.
///
/// The cost of being wrong either way decides it too. Refusing a style that
/// would in fact have looked fine costs one line deleted from a bundle, and the
/// error says which line. Loading one that does not costs a desktop that looks
/// broken with nothing naming the cause, which is the failure this feature is
/// for.
///
/// An author who wants a term to be advisory has somewhere to put it already:
/// out of `requires`. There is no way to spell the other direction.
fn requirements_on(manifest: &Path, required: &[String], gpu: bool) -> Result<()> {
    let unmet: Vec<String> = required
        .iter()
        .filter_map(|term| match provides(term, gpu) {
            Some(true) => None,
            Some(false) if term == "gpu" => Some(format!(
                "`{term}` needs the GPU scene graph and this process came up on the software \
                 one, which does not implement ShaderEffect and does not appear to paint Canvas"
            )),
            Some(false) => Some(format!("`{term}` is not available on this machine")),
            None => Some(format!(
                "`{term}` is not a requirement this build has heard of"
            )),
        })
        .collect();
    if unmet.is_empty() {
        return Ok(());
    }
    let available: Vec<&str> = TERMS
        .into_iter()
        .filter(|term| provides(term, gpu) == Some(true))
        .collect();
    Err(anyhow!(
        "{} requires [{}] and cannot be loaded here: {}. This build knows [{}] and provides [{}]",
        manifest.display(),
        required.join(", "),
        unmet.join("; "),
        TERMS.join(", "),
        available.join(", "),
    ))
}

/// Read a bundle's `Pane.qml`.
///
/// The manifest scene is loaded at 1x1 and never rendered: it declares
/// structure, and instantiating it at any size would rasterise content nobody
/// is going to look at. `Decoration::new` builds its GPU scenes the same way
/// and for the same reason.
///
/// `requires` is checked *here*, and not by [`find`] or by a caller, because
/// this is the only door: a [`Style`] that exists has already been found
/// runnable, and there is no path that produces one without passing this.
/// See [`requirements`] for what an unknown term does.
pub(crate) fn load(dir: &Path) -> Result<Style> {
    let manifest = dir.join("Pane.qml");
    if !manifest.is_file() {
        return Err(anyhow!("{} has no Pane.qml", dir.display()));
    }
    // As `Decoration::new` does before building a frame. Idempotent, and the
    // only thing that makes this callable from a test — nothing else in the
    // process has brought Qt up.
    crate::qml::start()?;
    // `for_host` and not `software`: a host that came up on the GPU refuses a
    // software scene at construction, so the path-aware constructor is the only
    // correct one anywhere but `--check-qml`.
    let mut scene = crate::qml::Scene::for_host(&manifest, 1, 1, None)
        .with_context(|| format!("loading {}", manifest.display()))?;

    let declared = scene.layer_count();
    if declared < 0 {
        return Err(anyhow!(
            "{} does not declare a PaneStyle at its root",
            manifest.display()
        ));
    }
    // Before the layers, and after the root check: a bundle that is not a
    // PaneStyle has not asked for anything, and there is no point cataloguing
    // the layers of a style that is about to be refused.
    requirements(&manifest, &scene.string_list("requires"))?;
    let count = usize::try_from(declared).unwrap_or(0);

    let mut layers = Vec::with_capacity(count);
    for index in 0..count {
        let name = scene.layer_field(index, "name").unwrap_or_default();
        let source = scene
            .layer_field(index, "source")
            .filter(|it| !it.is_empty())
            .map(|it| dir.join(it));
        let declared_depth = scene.layer_field(index, "depth");
        // Through `parse_depth` and not a second copy of its fallback. The
        // fallback is what `an_unknown_depth_falls_back_to_frame` pins, and a
        // `load` that decided it again for itself is a test passing while the
        // path it is named after does something else — which is the shape of
        // defect this whole task is about.
        let depth = parse_depth(declared_depth.as_deref().unwrap_or_default());
        if declared_depth.as_deref().and_then(depth_from).is_none() {
            // Said rather than silently corrected. `layers` is a `list<Item>`,
            // so a missing property here is an item that is not a `Layer` at
            // all, and a word that is not one of the three is a typo — both
            // end up at `frame`, and neither should have to be found by
            // looking at the screen and wondering.
            tracing::warn!(
                bundle = %dir.display(),
                layer = name,
                index,
                depth = declared_depth.as_deref().unwrap_or("<no depth property>"),
                "unknown layer depth, drawing it where decorations already are; \
                 the vocabulary is `behind`, `frame`, `above`"
            );
        }
        layers.push(LayerSpec {
            depth,
            bleed: parse_bleed(&scene.layer_field(index, "bleed").unwrap_or_default()),
            source,
            name,
            index,
        });
    }
    if layers.is_empty() {
        return Err(anyhow!("{} declares no layers", manifest.display()));
    }

    Ok(Style {
        // Dotted paths, which is the whole of what `host.cpp` had to learn:
        // `insets` is a grouped property, so it is a child object held in a
        // property and `QObject::property("insets.top")` finds nothing and
        // reads back 0. See `solium_qml_scene_get_int`.
        insets: Insets {
            top: scene.get_int("insets.top").max(0),
            right: scene.get_int("insets.right").max(0),
            bottom: scene.get_int("insets.bottom").max(0),
            left: scene.get_int("insets.left").max(0),
        },
        layers,
        dir: dir.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::{Bleed, Depth, load, parse_bleed, parse_depth};
    use crate::qml::qt_test::on_the_qt_thread;

    use std::path::{Path, PathBuf};

    #[test]
    fn depth_names_map_to_the_three_we_have() {
        assert_eq!(parse_depth("behind"), Depth::Behind);
        assert_eq!(parse_depth("above"), Depth::Above);
        assert_eq!(parse_depth("frame"), Depth::Frame);
    }

    /// An unknown depth is `Frame` rather than an error: a typo should put the
    /// layer where decorations already are, not make the whole style fail to
    /// load and leave the window with no frame at all.
    ///
    /// `load` warns when it takes this path, which is the half of the answer
    /// the brief left out — see `depth_from`.
    #[test]
    fn an_unknown_depth_falls_back_to_frame() {
        assert_eq!(parse_depth("beneath"), Depth::Frame);
        assert_eq!(parse_depth(""), Depth::Frame);
        assert_eq!(super::depth_from("beneath"), None);
        assert_eq!(super::depth_from("frame"), Some(Depth::Frame));
    }

    #[test]
    fn a_bare_number_bleeds_on_every_side() {
        assert_eq!(
            parse_bleed("120"),
            Bleed {
                top: 120,
                right: 120,
                bottom: 120,
                left: 120
            }
        );
        assert!(parse_bleed("120").any());
        assert!(!parse_bleed("0").any());
    }

    /// Per-side, because a bar throwing spikes upward should not pay for the
    /// other three sides. Absent sides are zero.
    #[test]
    fn per_side_bleed_leaves_the_rest_at_zero() {
        assert_eq!(
            parse_bleed(r#"{"top":160,"left":8}"#),
            Bleed {
                top: 160,
                right: 0,
                bottom: 0,
                left: 8
            }
        );
    }

    /// Negative bleed would shrink the canvas below the pane and clip the
    /// frame itself.
    #[test]
    fn negative_bleed_is_clamped_to_zero() {
        assert_eq!(
            parse_bleed("-40"),
            Bleed {
                top: 0,
                right: 0,
                bottom: 0,
                left: 0
            }
        );
        assert_eq!(parse_bleed(r#"{"top":-40,"right":12}"#).top, 0);
        assert_eq!(parse_bleed(r#"{"top":-40,"right":12}"#).right, 12);
    }

    #[test]
    fn a_directory_with_no_manifest_is_refused() {
        // No Qt in this one: the manifest is checked before anything is built.
        let err = load(Path::new("/nonexistent/solium/style")).expect_err("there is no Pane.qml");
        assert!(err.to_string().contains("has no Pane.qml"), "{err}");
    }

    /// An empty directory of this test's own, on the real filesystem.
    ///
    /// Cleared first, because these are named after the test and the process
    /// and a second run of the same test in the same binary would otherwise
    /// find its own leftovers.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("solium-style-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    /// A bundle written into a directory of its own, for the tests that need
    /// values nothing else in the tree can shadow.
    fn fixture(name: &str, manifest: &str) -> PathBuf {
        let dir = scratch(name);
        std::fs::write(dir.join("Pane.qml"), manifest).expect("writing the manifest");
        dir
    }

    /// **The wire test**, and the one the other four cannot be.
    ///
    /// `parse_depth` and `parse_bleed` are string functions and pass whether or
    /// not anything in Rust can read a QML scene at all. This reads a *grouped*
    /// property back out of a genuinely loaded one, which is the call that
    /// silently returned 0 before this task: `QObject::property("insets.top")`
    /// looks the whole dotted string up in the metaobject, finds nothing, and
    /// `toInt()` makes it a perfectly plausible zero — a style reserving space
    /// at the top, read back as reserving none, with the client then drawn over
    /// its own titlebar and five green tests.
    ///
    /// So the four sides carry four *different* non-zero values, which also
    /// catches the version of this where the sides are read in the wrong order.
    /// `client.shadow.blur` is here because it is two levels deep and the spec
    /// writes it that way; nothing reads `client` in anger yet.
    #[test]
    fn a_grouped_property_is_read_through_its_group() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "grouped",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    insets.top: 7
                    insets.right: 11
                    insets.bottom: 13
                    insets.left: 17

                    client.radius: 5
                    client.shadow.blur: 40

                    Layer { depth: "frame"; name: "bar" }
                }
                "#,
            );
            let style = load(&dir).expect("the fixture loads");

            assert_eq!(style.insets.top, 7, "insets.top read through the group");
            assert_eq!(style.insets.right, 11);
            assert_eq!(style.insets.bottom, 13);
            assert_eq!(style.insets.left, 17);
            assert_eq!(style.dir, dir, "a style knows where it was read from");

            // Two levels, which is what `client.shadow.blur` costs and what a
            // hand-rolled one-level walk would not have reached.
            let mut scene =
                crate::qml::Scene::for_host(&dir.join("Pane.qml"), 1, 1, None).expect("a scene");
            assert_eq!(scene.get_int("client.radius"), 5);
            assert_eq!(scene.get_int("client.shadow.blur"), 40);
            // And a path that resolves to nothing still reads 0 rather than
            // doing anything exciting.
            assert_eq!(scene.get_int("insets.sideways"), 0);
            assert_eq!(scene.get_int("nothing.at.all"), 0);
            drop(scene);

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// The shipped example, read back exactly as it is written.
    ///
    /// `panes/example/` exists to be the format written out once, so this is
    /// also the test that the format still parses after anyone edits it. The
    /// layers are asserted **in declaration order** and the two depths are out
    /// of stacking order in the file on purpose over in the fixture above;
    /// here the interesting part is that all three spellings coexist — inline
    /// content with a bare-number bleed, a delegated frame with none, and a
    /// delegated overlay with a per-side one.
    #[test]
    fn the_example_bundle_reads_back_as_it_is_written() {
        on_the_qt_thread(|| {
            let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/panes/example"));
            let style = load(dir).expect("the shipped example loads");

            let names: Vec<&str> = style.layers.iter().map(|it| it.name.as_str()).collect();
            assert_eq!(
                names,
                ["glow", "bar", "spikes"],
                "declaration order, not stacking order"
            );
            let depths: Vec<Depth> = style.layers.iter().map(|it| it.depth).collect();
            assert_eq!(depths, [Depth::Behind, Depth::Frame, Depth::Above]);
            let indices: Vec<usize> = style.layers.iter().map(|it| it.index).collect();
            assert_eq!(indices, [0, 1, 2]);

            // Both spellings of `bleed`, from one file, through one parser.
            assert_eq!(
                style.layers[0].bleed,
                Bleed {
                    top: 24,
                    right: 24,
                    bottom: 24,
                    left: 24
                },
                "a bare number bleeds on every side"
            );
            assert_eq!(style.layers[1].bleed, Bleed::default());
            assert_eq!(
                style.layers[2].bleed,
                Bleed {
                    top: 48,
                    right: 0,
                    bottom: 0,
                    left: 0
                },
                "an object names the side it wants and leaves the rest alone"
            );

            // Inline content has no file; delegated content is resolved
            // against the bundle rather than the working directory.
            assert_eq!(style.layers[0].source, None);
            assert_eq!(style.layers[1].source, Some(dir.join("Frame.qml")));
            assert_eq!(style.layers[2].source, Some(dir.join("Spikes.qml")));

            // `insets.top` is `Theme.titlebarHeight`, so this is also the
            // assertion that the design system resolved — and the reason for
            // the guard: `~/.config/solium/qml` comes first on the import path
            // and a theme dropped in there is entitled to a different number.
            if crate::qml::user_qml_dir().is_none() {
                assert_eq!(style.insets.top, 32, "Theme.titlebarHeight");
            } else {
                assert!(style.insets.top > 0, "a titlebar reserves something");
            }
            assert_eq!(style.insets.right, 0);
            assert_eq!(style.insets.bottom, 0);
            assert_eq!(style.insets.left, 0);
        });
    }

    /// The fallback, taken by `load` rather than by `parse_depth` alone.
    ///
    /// Both ways in: a word that is not one of the three, and an item in
    /// `layers` that is not a `Layer` at all — `default property list<Item>`
    /// accepts any `Item`, so a `Rectangle` in there has no `depth` property to
    /// read. Each warns, naming the bundle and the layer, and each lands at
    /// `frame`. The style still loads, because a window with no frame at all is
    /// a worse answer to a misspelling than a frame in the ordinary place.
    #[test]
    fn an_unknown_depth_still_loads_the_style() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "bad-depth",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    Layer { depth: "beneath"; name: "typo" }
                    Rectangle { width: 1; height: 1 }
                }
                "#,
            );
            let style = load(&dir).expect("a typo does not fail the style");

            assert_eq!(style.layers.len(), 2);
            assert_eq!(style.layers[0].depth, Depth::Frame, "`beneath` is a typo");
            assert_eq!(style.layers[0].name, "typo");
            assert_eq!(
                style.layers[1].depth,
                Depth::Frame,
                "a Rectangle has no depth property at all"
            );
            assert_eq!(style.layers[1].name, "");

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// `-1` and `0` are different answers and both are the loader's to report.
    ///
    /// A bundle whose `Pane.qml` declares the wrong root object is a mistake
    /// about the *format*; a `PaneStyle` with no layers in it is a mistake
    /// about the *style*. Collapsing them into "no layers" would send someone
    /// looking for a missing `Layer` in a file that never had a `PaneStyle`.
    #[test]
    fn a_root_that_is_not_a_pane_style_is_told_apart_from_one_with_no_layers() {
        on_the_qt_thread(|| {
            let wrong_root = fixture(
                "wrong-root",
                "import QtQuick\n\nItem { property int insetTop: 32 }\n",
            );
            let err = load(&wrong_root).expect_err("an Item is not a PaneStyle");
            assert!(
                err.to_string().contains("does not declare a PaneStyle"),
                "{err}"
            );
            let _ = std::fs::remove_dir_all(&wrong_root);

            let empty = fixture(
                "no-layers",
                "import QtQuick\nimport Solium\n\nPaneStyle {}\n",
            );
            let err = load(&empty).expect_err("a style with no layers draws nothing");
            assert!(err.to_string().contains("declares no layers"), "{err}");
            let _ = std::fs::remove_dir_all(&empty);
        });
    }

    /// A path is a path, and is handed back whether or not it exists.
    ///
    /// Deliberately not checked here: a mistyped path that comes back unchanged
    /// is reported by [`load`] as "… has no Pane.qml" and *names the thing that
    /// was wrong*, where a `None` would name nothing and leave the caller unable
    /// to tell a typo from a style this build does not ship.
    #[test]
    fn a_path_is_taken_as_given() {
        assert_eq!(
            super::resolve("/tmp/solium-no-such-style", None),
            Some(PathBuf::from("/tmp/solium-no-such-style"))
        );
        // Relative too: what makes it a path is the separator, not the root.
        assert_eq!(
            super::resolve("./neon", None),
            Some(PathBuf::from("./neon"))
        );
        // And a path wins over the lookup entirely, so a user directory that
        // happens to hold a folder of the same spelling does not capture it.
        assert_eq!(
            super::resolve("/tmp/solium-no-such-style", Some(Path::new("/tmp"))),
            Some(PathBuf::from("/tmp/solium-no-such-style"))
        );
    }

    /// The user's directory shadows the shipped one, name by name, so replacing
    /// a style means dropping in a folder rather than copying everything else.
    ///
    /// Two things this test does that the brief's version could not:
    ///
    /// * **It creates the directory.** `resolve` asks `is_dir()`, so a user
    ///   path nobody made falls straight through to the shipped tree — the
    ///   brief's `/home/someone/…` returns `None`, and the claim that the
    ///   lookup is testable without a filesystem is not true of a lookup that
    ///   consults one. See the plan's amendment of 2026-09-13.
    /// * **It uses a name that also ships.** `example` exists in both places, so
    ///   this pins the *order*. A name only the user has would be found first
    ///   either way and would pass against a reversed lookup.
    #[test]
    fn a_bare_name_prefers_the_users_directory() {
        let user = scratch("user-panes");
        std::fs::create_dir_all(user.join("example")).expect("a user bundle");

        assert_eq!(
            super::resolve("example", Some(&user)),
            Some(user.join("example")),
            "the user's `example` shadows the shipped one"
        );
        assert_ne!(
            super::resolve("example", Some(&user)),
            Some(shipped_example())
        );

        let _ = std::fs::remove_dir_all(&user);
    }

    /// With nothing of that name in the user's directory, the shipped one.
    ///
    /// Both ways of having nothing: no user directory at all — which is what
    /// `user_qml_dir` returns on a machine with no `~/.config/solium/qml` — and
    /// one that exists but does not hold this name.
    #[test]
    fn a_bare_name_falls_back_to_the_shipped_bundle() {
        let user = scratch("user-panes-empty");

        assert_eq!(super::resolve("example", None), Some(shipped_example()));
        assert_eq!(
            super::resolve("example", Some(&user)),
            Some(shipped_example()),
            "an empty user directory is not an answer"
        );

        let _ = std::fs::remove_dir_all(&user);
    }

    /// A name in neither place is `None` rather than a path that is not there.
    ///
    /// The empty name is here because it is the one input that made the old
    /// shape of this return something absurd: `Path::join("")` is the directory
    /// itself, which `is_dir()` happily confirms, so `find("")` answered with
    /// the whole `panes/` folder.
    #[test]
    fn a_name_that_is_nowhere_is_none() {
        let user = scratch("user-panes-nowhere");
        assert_eq!(super::resolve("neon", Some(&user)), None);
        assert_eq!(super::resolve("neon", None), None);
        assert_eq!(super::resolve("", Some(&user)), None);
        assert_eq!(super::resolve("", None), None);
        let _ = std::fs::remove_dir_all(&user);
    }

    /// `find` against this machine, which must reach the shipped example.
    ///
    /// The one assertion `resolve`'s tests cannot make: that the directory
    /// `find` builds out of `user_qml_dir` is the one styles are actually in.
    /// Guarded, because a user bundle called `example` is entitled to win —
    /// that is the feature — and then the shipped path is the wrong assertion.
    #[test]
    fn find_reaches_the_shipped_example() {
        let user = crate::qml::user_qml_dir().map(|dir| dir.join("panes"));
        let shadowed = user.is_some_and(|dir| dir.join("example").is_dir());
        if shadowed {
            assert!(
                super::find("example").is_some(),
                "a user bundle is still a bundle"
            );
        } else {
            assert_eq!(super::find("example"), Some(shipped_example()));
        }
        assert_eq!(super::find("no-such-style-ships-here"), None);
    }

    /// The bundle this tree ships, by the same route `resolve` reaches it.
    fn shipped_example() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml/panes")).join("example")
    }

    /// A style is refused when the path cannot give it what it asked for.
    ///
    /// Which way this test runs is decided by the process, not by the assertion:
    /// `qml::start()` is called first so `on_gpu()` is answering about a host
    /// that exists, and then the same bundle must load on one path and be
    /// refused on the other. The container the gate runs in has no render node,
    /// so the refusal is the branch that is actually exercised there.
    #[test]
    fn a_style_the_path_cannot_run_is_refused() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "needs-gpu",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    requires: ["gpu"]
                    Layer { depth: "frame"; name: "shader" }
                }
                "#,
            );
            crate::qml::start().expect("Qt starts");

            if crate::qml::on_gpu() {
                let style = load(&dir).expect("a GPU build runs a GPU style");
                assert_eq!(style.layers.len(), 1);
            } else {
                let err = load(&dir).expect_err("the software path cannot run a GPU style");
                let said = err.to_string();
                // What was required, why it could not be had, and what is here.
                // A refusal that says only "no" sends someone to the wrong file.
                assert!(said.contains("requires [gpu]"), "{said}");
                assert!(said.contains("software"), "{said}");
                assert!(said.contains("knows [gpu] and provides []"), "{said}");
            }

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// Both sides of the decision, which no process running this can supply.
    ///
    /// Every other test here runs on whichever scene graph Qt came up on, and in
    /// the container the gate builds in that is always the software one: no
    /// render node, and Qt fixes the choice for the life of the process. So the
    /// GPU side of `requirements` is unreachable from a test of the real entry
    /// point, on every machine this will ever be run on, and the decision takes
    /// the answer as an argument so that it is reachable here.
    ///
    /// What stays untested is one line — `requirements` binding the argument to
    /// `qml::on_gpu()`. That is the honest size of the gap, and it is smaller
    /// than the whole rule going unexercised in one direction.
    #[test]
    fn the_gpu_term_is_decided_by_the_path_the_process_came_up_on() {
        let manifest = Path::new("/panes/neon/Pane.qml");
        let gpu = ["gpu".to_string()];

        super::requirements_on(manifest, &gpu, true).expect("a GPU process runs a GPU style");

        let err =
            super::requirements_on(manifest, &gpu, false).expect_err("a software process cannot");
        let said = err.to_string();
        assert!(said.contains("/panes/neon/Pane.qml"), "{said}");
        assert!(said.contains("requires [gpu]"), "{said}");
        assert!(said.contains("came up on the software one"), "{said}");
        assert!(said.contains("knows [gpu] and provides []"), "{said}");

        // What is available is reported from the same place the decision is
        // made, so the two cannot drift into disagreeing.
        let future = ["effects/2".to_string()];
        let on_gpu = super::requirements_on(manifest, &future, true)
            .expect_err("unknown is unknown on either path")
            .to_string();
        assert!(
            on_gpu.contains("knows [gpu] and provides [gpu]"),
            "{on_gpu}"
        );
        assert!(super::requirements_on(manifest, &future, false).is_err());

        // Nothing asked for is nothing to refuse, on either path.
        super::requirements_on(manifest, &[], false).expect("portable");
        super::requirements_on(manifest, &[], true).expect("portable");
    }

    /// An unrecognised requirement is a refusal, on either path.
    ///
    /// `requires` is a list so that versioning goes through the same mechanism,
    /// and a build that shrugs at a term it does not know is not versioned. A
    /// style naming `effects/2` was written against a later build than this
    /// one; loading it anyway is the silence the property exists to remove.
    #[test]
    fn an_unknown_requirement_is_refused() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "needs-the-future",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    requires: ["effects/2"]
                    Layer { depth: "frame"; name: "bar" }
                }
                "#,
            );
            let err = load(&dir).expect_err("this build has never heard of effects/2");
            let said = err.to_string();
            assert!(said.contains("`effects/2` is not a requirement"), "{said}");
            assert!(said.contains("knows [gpu]"), "{said}");

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// Declaring nothing is portable, both ways of declaring it.
    ///
    /// `requires: []` and no `requires` at all are the same answer, which is
    /// what makes the property something a style can leave out. The shipped
    /// bundle is the `[]` case — see
    /// `the_example_bundle_reads_back_as_it_is_written`, which loads it — and
    /// this is the absent one, read back through the list itself rather than
    /// only through the style loading.
    #[test]
    fn a_style_that_requires_nothing_is_portable() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "requires-nothing",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    Layer { depth: "frame"; name: "bar" }
                }
                "#,
            );
            load(&dir).expect("a style asking for nothing loads anywhere");

            let scene =
                crate::qml::Scene::for_host(&dir.join("Pane.qml"), 1, 1, None).expect("a scene");
            assert!(scene.string_list("requires").is_empty());
            // And a property that is not a list at all is not read as one.
            assert!(scene.string_list("nothing.at.all").is_empty());
            drop(scene);

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// The shapes a list can arrive in, all read the same way.
    ///
    /// Measured rather than assumed, and the measurement is the reason the
    /// unwrap in `solium_qml_scene_string_at` is written the way it is:
    ///
    /// * `list<string>` is a `QStringList` in the metaobject and arrives plain.
    ///   Dropping the QJSValue unwrap changes nothing about `requires` — run as
    ///   a control, all nineteen tests stayed green — so the unwrap is **not**
    ///   what makes that property work, and a comment claiming it was would be
    ///   the second thing in this file to describe a branch that never fires.
    /// * `var` is the shape that needs it. A QML `var` hands its value back
    ///   wrapped, exactly as `bleed` does, and without the unwrap a `var` list
    ///   reads as empty. That is what `tags` here pins.
    /// * A single value where a list was expected is one element, not none.
    ///   `QVariant::toList()` answers empty for anything that is not already a
    ///   list, and for `requires` "declares nothing" is precisely the wrong
    ///   default — a requirement silently dropped is what the property exists
    ///   to prevent.
    #[test]
    fn a_var_list_and_a_lone_value_read_like_a_string_list() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "list-shapes",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    property var tags: ["one", "two"]
                    property var lone: "only"
                    property var nothing: []

                    Layer { depth: "frame"; name: "bar" }
                }
                "#,
            );
            let scene =
                crate::qml::Scene::for_host(&dir.join("Pane.qml"), 1, 1, None).expect("a scene");

            assert_eq!(scene.string_list("tags"), ["one", "two"], "a var list");
            assert_eq!(
                scene.string_list("lone"),
                ["only"],
                "one value is one element"
            );
            assert!(scene.string_list("nothing").is_empty());
            drop(scene);

            let _ = std::fs::remove_dir_all(&dir);
        });
    }

    /// Two terms, read in order and reported together.
    ///
    /// The list is what the FFI is for: a joined string would have made
    /// `["gpu", "effects/2"]` one unparseable term, and the refusal has to name
    /// each of them separately to be worth reading.
    #[test]
    fn every_declared_requirement_is_read_and_reported() {
        on_the_qt_thread(|| {
            let dir = fixture(
                "needs-two",
                r#"
                import QtQuick
                import Solium

                PaneStyle {
                    requires: ["gpu", "effects/2"]
                    Layer { depth: "frame"; name: "bar" }
                }
                "#,
            );

            let scene =
                crate::qml::Scene::for_host(&dir.join("Pane.qml"), 1, 1, None).expect("a scene");
            assert_eq!(scene.string_list("requires"), ["gpu", "effects/2"]);
            drop(scene);

            let err = load(&dir).expect_err("effects/2 is unknown on either path");
            let said = err.to_string();
            assert!(said.contains("requires [gpu, effects/2]"), "{said}");
            assert!(said.contains("`effects/2`"), "{said}");

            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
