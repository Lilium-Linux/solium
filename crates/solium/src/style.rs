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

/// Read a bundle's `Pane.qml`.
///
/// The manifest scene is loaded at 1x1 and never rendered: it declares
/// structure, and instantiating it at any size would rasterise content nobody
/// is going to look at. `Decoration::new` builds its GPU scenes the same way
/// and for the same reason.
///
/// `requires` is deliberately not read here. Refusing a style the machine
/// cannot run is the loader's job, and the loader is Task 3.
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
    let count = usize::try_from(declared).unwrap_or(0);

    let mut layers = Vec::with_capacity(count);
    for index in 0..count {
        let name = scene.layer_field(index, "name").unwrap_or_default();
        let source = scene
            .layer_field(index, "source")
            .filter(|it| !it.is_empty())
            .map(|it| dir.join(it));
        let declared_depth = scene.layer_field(index, "depth");
        let depth = match declared_depth.as_deref().and_then(depth_from) {
            Some(depth) => depth,
            None => {
                // Said rather than silently corrected. `layers` is a
                // `list<Item>`, so `None` here is an item that is not a `Layer`
                // at all, and a word that is not one of the three is a typo —
                // both end up at `frame`, and neither should have to be found
                // by looking at the screen and wondering.
                tracing::warn!(
                    bundle = %dir.display(),
                    layer = name,
                    index,
                    depth = declared_depth.as_deref().unwrap_or("<no depth property>"),
                    "unknown layer depth, drawing it at `frame`; the vocabulary \
                     is `behind`, `frame`, `above`"
                );
                Depth::Frame
            }
        };
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

    /// A bundle written into a directory of its own, for the tests that need
    /// values nothing else in the tree can shadow.
    fn fixture(name: &str, manifest: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("solium-style-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
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
}
