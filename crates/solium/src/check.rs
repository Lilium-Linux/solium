//! `solium --check`: will this configuration do what I wrote. Each check
//! writes to a [`Report`] and fails it rather than exiting, so every one is
//! tested; [`run`] turns the report into the exit status (spec §6.5, C7).
//! `tests::a_config_that_does_not_load_still_fails`,
//! `tests::a_file_that_does_not_load_fails`.

use std::{io::Write, path::Path};

/// What a check says, and whether anything failed.
pub(crate) struct Report<'a> {
    out: &'a mut dyn Write,
    failed: u32,
}

impl std::fmt::Debug for Report<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Report")
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl<'a> Report<'a> {
    pub(crate) fn new(out: &'a mut dyn Write) -> Self {
        Self { out, failed: 0 }
    }

    pub(crate) fn line(&mut self, text: &str) {
        let _ = writeln!(self.out, "{text}");
    }

    pub(crate) fn fail(&mut self, text: &str) {
        self.failed = self.failed.saturating_add(1);
        self.line(text);
    }

    pub(crate) fn passed(&self) -> bool {
        self.failed == 0
    }
}

/// Load the configuration and report what it would do.
///
/// Reports the bindings it registered as well as any error, because "it
/// parsed" is not the question a ricer is asking -- "did my binding survive
/// the edit" is.
///
/// ## Why a file that loads can still fail this
///
/// It used to answer that question for bindings alone, and answer "ok" to
/// everything else. `config.lua` merges a `user.lua` over its defaults key by
/// key and validated nothing, so `tilling = { split = 0.6 }` was merged in as a
/// new section, read by nothing, for ever -- and the one command whose entire
/// job is telling you whether your configuration worked said it loaded fine
/// (#117). It had. It just was not doing what the file said.
///
/// So an unrecognised setting fails the report, and that is deliberate. The
/// question is not "did Lua run" -- a syntax error already answered that -- it
/// is "will this configuration do what I wrote", and a key nothing reads means
/// no. It also makes this usable from a script or a pre-commit hook, which a
/// command that always succeeds is not.
///
/// `tests::a_config_that_does_not_load_still_fails`,
/// `tests::an_unrecognised_setting_still_fails`,
/// `tests::the_bindings_block_stays_where_generate_py_reads_it`.
pub(crate) fn config(path: &Path, report: &mut Report) -> Option<crate::script::Scripts> {
    report.line(&format!("checking {}", path.display()));
    match crate::script::Scripts::load(path) {
        Err(err) => {
            // Printed rather than only returned: this is a command someone
            // runs to read the answer, and the chain is where the answer is.
            report.fail("  failed:");
            for (depth, cause) in err.chain().enumerate() {
                report.line(&format!("    {:indent$}{cause}", "", indent = depth * 2));
            }
            None
        }
        Ok(scripts) => {
            let bindings = scripts.bindings();
            report.line(&format!("  ok: {} binding(s)", bindings.len()));
            for binding in bindings {
                // Padded so the notes line up into a column of their own, and
                // only when there is a note -- most configurations have none,
                // and trailing space on every line of the common case is
                // noise.
                match binding.note {
                    Some(note) => report.line(&format!("    {:<28}{note}", binding.combo)),
                    None => report.line(&format!("    {}", binding.combo)),
                }
            }

            // A binding taken away is the one thing the list above cannot
            // show: in a list of what survived, a key removed on purpose looks
            // exactly like one that never existed.
            let unbound = scripts.unbound();
            if !unbound.is_empty() {
                report.line(&format!(
                    "  {} binding(s) removed by the configuration:",
                    unbound.len()
                ));
                for binding in unbound {
                    match binding.note {
                        Some(note) => report.line(&format!("    {:<28}{note}", binding.combo)),
                        None => report.line(&format!("    {}", binding.combo)),
                    }
                }
            }

            let unknown = scripts.unknown_settings();
            if !unknown.is_empty() {
                report.fail(&format!(
                    "  {} unrecognised setting(s) -- written, merged, and read by nothing:",
                    unknown.len()
                ));
                for setting in unknown {
                    match setting.meant {
                        Some(meant) => {
                            report.line(&format!("    {:<28}did you mean {meant}?", setting.key));
                        }
                        None => report.line(&format!("    {}", setting.key)),
                    }
                }
            }
            Some(scripts)
        }
    }
}

/// One QML file, and, when it is a style's `Pane.qml`, every layer it names.
///
/// Software, whatever the machine: [`run`] decides `Entry::CheckQml` before
/// Qt starts, so whether a file loads does not depend on whether the machine
/// has a render node (`renderer::check_qml_is_software_in_every_mode`). So a
/// style that `requires: ["gpu"]` cannot be built here, and its layers are
/// not loaded: said, and not failed.
///
/// `tests::a_file_that_does_not_load_fails`,
/// `tests::a_style_bundle_fails_on_its_broken_layer`,
/// `tests::the_example_style_passes_with_its_layers`,
/// `tests::a_style_that_needs_the_gpu_is_not_failed_in_software`.
pub(crate) fn qml_file(path: &Path, report: &mut Report) {
    let Some(scene) = one(path, report) else {
        return;
    };
    if path.file_name().is_none_or(|name| name != "Pane.qml") {
        return;
    }
    let Some(bundle) = path.parent() else {
        return;
    };
    if scene
        .string_list("requires")
        .iter()
        .any(|term| term == "gpu")
    {
        report.line(&format!(
            "{}: needs the GPU, and the check runs in software: its layers are not loaded",
            bundle.display()
        ));
        return;
    }
    drop(scene);
    match crate::style::load(bundle) {
        Err(err) => report.fail(&format!("{}: {err:#}", bundle.display())),
        // Each layer through `one`, not `qml_file`: a layer is a file, and one
        // named `Pane.qml` is not a style to follow again.
        // `tests::a_layer_that_is_its_own_pane_qml_is_checked_once`.
        Ok(style) => {
            for source in style
                .layers
                .iter()
                .filter_map(|layer| layer.source.as_deref())
            {
                let _ = one(source, report);
            }
        }
    }
}

/// Load one file, and say whether it did.
fn one(path: &Path, report: &mut Report) -> Option<crate::qml::Scene> {
    match crate::qml::Scene::software(path, 400, 200) {
        Ok(scene) => {
            report.line(&format!("ok {}", path.display()));
            Some(scene)
        }
        Err(err) => {
            let message = err.to_string();
            // A `pragma Singleton` file cannot be built as a component, so Qt
            // answers with nothing rather than a reason. Say so, instead of
            // printing a blank line that reads like success.
            if message.trim().is_empty() || message.trim().ends_with(':') {
                report.fail(&format!(
                    "{}: no component: a singleton, or an empty error",
                    path.display()
                ));
            } else {
                report.fail(&format!("{}: {message}", path.display()));
            }
            None
        }
    }
}

/// `solium --check [<file>]`: the configuration, or one QML file; the exit
/// status is the report's. `tests/cli.rs`'s `check_qml_on_a_broken_file_exits_one`.
pub(crate) fn run(single: Option<&Path>) -> std::process::ExitCode {
    let mut out = std::io::stdout();
    let mut report = Report::new(&mut out);
    match single {
        Some(file) => {
            // Software in every mode, so the scene is a software scene by
            // construction rather than by preference -- the one caller in the
            // compositor that is right not to go through
            // `qml::Scene::for_host`. Nothing here is ever drawn.
            crate::qml::renderer::decide(
                crate::qml::renderer::Entry::CheckQml,
                &crate::qml::renderer::Configured::default(),
            );
            match crate::qml::start() {
                Ok(()) => qml_file(file, &mut report),
                Err(err) => report.fail(&format!("Qt would not start: {err:#}")),
            }
        }
        None => {
            let _ = config(&crate::script::Scripts::config_path(), &mut report);
        }
    }
    if report.passed() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Report, config, qml_file};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/check")
            .join(name)
    }

    /// Whether `check` passed, and what it said.
    fn reported(check: impl FnOnce(&mut Report)) -> (bool, String) {
        let mut out = Vec::new();
        let mut report = Report::new(&mut out);
        check(&mut report);
        let passed = report.passed();
        (passed, String::from_utf8_lossy(&out).into_owned())
    }

    /// As `run` brings Qt up for one file: software, whatever the machine.
    fn software_qt() {
        crate::qml::renderer::decide(
            crate::qml::renderer::Entry::CheckQml,
            &crate::qml::renderer::Configured::default(),
        );
        crate::qml::start().expect("Qt starts");
    }

    /// A configuration that does not load fails, and says why.
    #[test]
    fn a_config_that_does_not_load_still_fails() {
        let directory = std::env::temp_dir().join("solium-check-test-broken-lua");
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join("init.lua");
        std::fs::write(&path, "sol.bind(").expect("the file");
        let mut loaded = true;
        let (passed, text) = reported(|report| loaded = config(&path, report).is_some());
        assert!(!loaded);
        assert!(!passed, "{text}");
        assert!(text.contains("failed:"), "{text}");
    }

    /// A key nothing reads fails, as `config`'s doc comment argues. Set up as
    /// `script::tests::shipped_init_with_user` sets up its fixture: the
    /// shipped `init.lua` copied beside a `user.lua`, which `Scripts::load`
    /// finds on `package.path` unless the user's own directory has Lua in it,
    /// which comes first; then there is nothing this test can say.
    #[test]
    fn an_unrecognised_setting_still_fails() {
        if let Some(own) = crate::script::Scripts::user_config_dir()
            && let Ok(entries) = std::fs::read_dir(&own)
            && entries
                .filter_map(Result::ok)
                .any(|entry| entry.path().extension().is_some_and(|kind| kind == "lua"))
        {
            return;
        }
        let directory = std::env::temp_dir().join("solium-check-test-unknown-key");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("a directory");
        std::fs::write(
            directory.join("user.lua"),
            "return { tilling = { split = 0.6 } }\n",
        )
        .expect("user.lua");
        let path = directory.join("init.lua");
        std::fs::copy(crate::assets::lua().join("init.lua"), &path).expect("the shipped init.lua");
        let (passed, text) = reported(|report| {
            let _ = config(&path, report);
        });
        assert!(!passed, "{text}");
        assert!(text.contains("tilling"), "{text}");
    }

    /// The bindings block is where `dev/docs/generate.py` reads it: after
    /// `  ok: N binding(s)`, every line indented four spaces until one is not.
    #[test]
    fn the_bindings_block_stays_where_generate_py_reads_it() {
        let directory = std::env::temp_dir().join("solium-check-test-bindings");
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join("init.lua");
        std::fs::write(&path, "sol.bind(\"super+x\", function() end)\n").expect("the file");
        let (passed, text) = reported(|report| {
            let _ = config(&path, report);
        });
        assert!(passed, "{text}");
        let mut lines = text.lines().skip_while(|line| !line.starts_with("  ok: "));
        assert!(
            lines
                .next()
                .is_some_and(|line| line.ends_with("binding(s)")),
            "{text}"
        );
        assert!(
            lines
                .take_while(|line| line.starts_with("    "))
                .any(|line| line.trim_start().starts_with("super+x")),
            "the binding is not in the block generate.py reads: {text}"
        );
    }

    /// **A QML file that does not load fails**: it used to exit 0.
    #[test]
    fn a_file_that_does_not_load_fails() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = reported(|report| qml_file(&fixture("broken.qml"), report));
            assert!(!passed, "{text}");
        });
    }

    /// **A style bundle fails on its broken layer**, which the old check never
    /// loaded: a `Ring.qml` with an error passed.
    #[test]
    fn a_style_bundle_fails_on_its_broken_layer() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = reported(|report| qml_file(&fixture("style/Pane.qml"), report));
            assert!(!passed, "{text}");
            assert!(
                text.contains("Ring.qml"),
                "the failure does not name the layer: {text}"
            );
        });
    }

    /// The format written out in full, which `qml/panes/README.md` tells an
    /// author to check, passes with every layer loaded.
    #[test]
    fn the_example_style_passes_with_its_layers() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let example =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/panes/example/Pane.qml");
            let (passed, text) = reported(|report| qml_file(&example, report));
            assert!(passed, "{text}");
            assert!(
                text.contains("Frame.qml") && text.contains("Spikes.qml"),
                "{text}"
            );
        });
    }

    /// A layer whose source is its own `Pane.qml` is loaded once as a file,
    /// not followed as a style again and again until the stack runs out.
    #[test]
    fn a_layer_that_is_its_own_pane_qml_is_checked_once() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = reported(|report| qml_file(&fixture("looped/Pane.qml"), report));
            assert!(passed, "{text}");
            assert_eq!(text.matches("ok ").count(), 2, "{text}");
        });
    }

    /// The check runs in software, so a style that `requires: ["gpu"]` cannot
    /// be built by it. That is the check's limit, not the style's fault: said,
    /// and not a failure.
    #[test]
    fn a_style_that_needs_the_gpu_is_not_failed_in_software() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = reported(|report| qml_file(&fixture("gpu/Pane.qml"), report));
            assert!(passed, "{text}");
            assert!(text.contains("needs the GPU"), "{text}");
        });
    }
}
