//! `solium --check`: will this configuration do what I wrote. Each check
//! writes to a [`Report`] and fails it rather than exiting, so every one is
//! tested; [`run`] turns the report into the exit status (spec §6.5, C7).
//! `--check` checks the configuration, then the scenes it declares at load,
//! your own pane styles, your own effect folders and the effects the
//! configuration names, and the scenes this run's environment names.
//! `tests::a_config_that_does_not_load_still_fails`,
//! `tests::a_file_that_does_not_load_fails`,
//! `tests::a_surface_whose_scene_does_not_load_fails`,
//! `tests::a_broken_style_of_your_own_fails`,
//! `tests::every_user_folder_and_every_named_effect_is_checked`.

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

            // A rule that does not parse is refused at load, so it fails
            // here too, by its number and its key
            // (`tests::a_broken_rule_fails_the_check`).
            if let Some(Err(errors)) = scripts.effects_at_load() {
                report.fail(&format!(
                    "  {} effect rule error(s) -- the rules as written are refused:",
                    errors.len()
                ));
                for error in errors {
                    report.line(&format!(
                        "    effects.rules, rule {}, `{}`: {}",
                        error.rule, error.key, error.message
                    ));
                }
            }

            // A refused engine key refuses the whole `sol.effects` call at
            // run time, its rules included, so it fails here, naming the key
            // (Ruling 9, `tests::a_refused_effects_key_fails_the_check`).
            if let Some(Err(error)) = scripts.effect_settings_at_load() {
                report
                    .fail("  sol.effects is refused -- neither its settings nor its rules apply:");
                report.line(&format!("    {error}"));
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

/// The one monitor every declared scene is built on, as
/// `models::monitors::rows` publishes one, so `Solium.monitor` is present in
/// what is built. `tests::a_scene_that_reads_its_monitor_passes`.
fn the_monitor() -> crate::models::diff::Row {
    let whole = crate::models::monitors::rect(smithay::utils::Rectangle::new(
        (0, 0).into(),
        (1920, 1080).into(),
    ));
    crate::models::diff::Row {
        key: MONITOR.to_owned(),
        values: std::collections::BTreeMap::from([
            ("name", crate::json::Json::Text(MONITOR.to_owned())),
            ("whole", whole.clone()),
            ("area", whole),
            ("scale", crate::json::Json::Number(1.0)),
            ("transform", crate::json::Json::Text("normal".to_owned())),
            ("primary", crate::json::Json::Bool(true)),
        ]),
    }
}

/// What the check's one monitor is called.
const MONITOR: &str = "check";

/// Every surface the configuration declares at load, built on one monitor and
/// drawn once, since some errors come only from the first polish and sync; a
/// scene fails on an error, or on a QML warning while it is built and drawn.
/// Only surfaces declared at load are seen: one declared later, by a handler,
/// is not, and the output says how many were.
/// `tests::a_surface_whose_scene_builds_is_listed_and_passes`,
/// `tests::a_scene_that_warns_while_it_is_built_fails`,
/// `tests::a_surface_whose_scene_does_not_load_fails`,
/// `tests::a_surface_whose_scene_file_is_missing_fails`,
/// `tests::a_surface_missing_a_required_property_fails`,
/// `tests::the_shipped_configuration_passes`.
pub(crate) fn scenes(scripts: &mut crate::script::Scripts, report: &mut Report) {
    use crate::models::diff::{diff, render};
    use crate::qml::hosted::{Model, apply_rows};

    let monitor = [the_monitor()];
    let published = apply_rows(Model::Monitors, &render(&diff(&[], &monitor)));
    let declared: Vec<crate::scripted::Declaration> = scripts
        .startup()
        .commands
        .into_iter()
        .filter_map(|command| match command {
            crate::script::Command::Surface(declared) => Some(*declared),
            _ => None,
        })
        .collect();
    report.line(&format!("  {} scene(s) declared at load:", declared.len()));
    for declaration in declared {
        let mark = crate::qml::warning_mark();
        let properties = declaration.properties.render();
        let rendered = crate::qml::Scene::for_monitor(
            &declaration.scene,
            400,
            200,
            Some(&properties),
            MONITOR,
        )
        .and_then(|mut scene| scene.render().map(|_| ()));
        let warned = crate::qml::warnings_since(mark);
        match rendered {
            Err(err) => report.fail(&format!("    {}: {err:#}", declaration.name)),
            // A warning fails, as a key nothing reads does: the scene built,
            // and is not doing what its file says (Ruling 19). The shipped
            // scenes declared at load build without one.
            // `tests::a_scene_that_warns_while_it_is_built_fails`,
            // `tests::the_shipped_configuration_passes`.
            Ok(()) if warned > 0 => report.fail(&format!(
                "    {}: {warned} QML warning(s) while it was built and drawn, logged above",
                declaration.name
            )),
            Ok(()) => report.line(&format!("    ok {}", declaration.name)),
        }
    }
    // Taken away again, so the process is left with the rows it had.
    if published {
        let _ = apply_rows(Model::Monitors, &render(&diff(&monitor, &[])));
    }
}

/// Your own pane-style bundles, every layer included, and the rules of each
/// one's `effects.lua`, read as the compositor reads them. The shipped ones
/// are `cargo test`'s, which builds them
/// (`decoration::tests::a_narrow_tile_hides_the_titlebars_pieces_in_every_shipped_style`).
/// `tests::a_broken_style_of_your_own_fails`,
/// `tests::a_broken_effects_lua_of_your_own_fails`.
pub(crate) fn styles(report: &mut Report, caps: crate::effect::settings::Caps) {
    styles_in(
        &crate::style::directories(),
        &crate::style::shipped(),
        caps,
        report,
    );
}

/// [`styles`], over directories handed in, so a test can hand in its own.
/// `tests::a_broken_style_of_your_own_fails`,
/// `tests::the_shipped_styles_are_left_to_cargo_test`.
fn styles_in(
    directories: &[std::path::PathBuf],
    shipped: &Path,
    caps: crate::effect::settings::Caps,
    report: &mut Report,
) {
    for directory in directories.iter().filter(|directory| *directory != shipped) {
        let Ok(bundles) = std::fs::read_dir(directory) else {
            continue;
        };
        let mut manifests: Vec<std::path::PathBuf> = bundles
            .flatten()
            .map(|bundle| bundle.path().join("Pane.qml"))
            .filter(|manifest| manifest.is_file())
            .collect();
        manifests.sort();
        if manifests.is_empty() {
            continue;
        }
        report.line(&format!(
            "  {} pane style(s) of your own in {}:",
            manifests.len(),
            directory.display()
        ));
        for manifest in manifests {
            qml_file(&manifest, report);
            let Some(dir) = manifest.parent() else {
                continue;
            };
            let file = dir.join("effects.lua");
            if !file.is_file() {
                continue;
            }
            // Under the configured sandbox, as the session reads it
            // (`tests::a_styles_effects_lua_is_checked_under_the_configured_sandbox`).
            let read = crate::style::rules_with(dir, caps);
            if read.problems.is_empty() {
                report.line(&format!(
                    "    ok {}: {} rule(s)",
                    file.display(),
                    read.rules.len()
                ));
            }
            say(report, &read.problems);
        }
    }
}

/// The pane style and the loading scene this run's environment names, each
/// found as the compositor finds it. `SOLIUM_SHELL_SCENE` is not among them:
/// `shell.lua` declares it at load (`script::tests::the_environment_overrides_the_configured_shell_scene`),
/// so [`scenes`] builds it, on its monitor.
/// `tests::a_pane_knob_naming_a_broken_style_fails`,
/// `tests::a_loading_knob_naming_a_missing_file_fails`.
fn knobs(pane: Option<&str>, loading: Option<&str>, report: &mut Report) {
    if let Some(pane) = pane {
        report.line(&format!("  the pane style this run names, {pane}:"));
        match crate::decoration::style_file(Some(pane)) {
            Some(file) => qml_file(&file, report),
            None => report.fail(&format!("{pane}: no pane style of that name")),
        }
    }
    if let Some(loading) = loading {
        report.line(&format!("  the loading scene this run names, {loading}:"));
        qml_file(&crate::pane::loading_source(Some(loading)), report);
    }
}

/// One effect folder: the sandbox, the schema, the lints, binding at the
/// defaults and at each rung, and, when `gpu` is given, the compile and the
/// formats its render node renders into (Rulings 9, 11). What it names
/// (`pixels`, a `fallback` naming an effect, a `use` stage) is looked for
/// beside it first and then in `shipped`, as at run time.
/// `tests::a_broken_effect_folder_fails_check`,
/// `tests::a_frag_reading_an_undeclared_param_fails_check_at_its_line`,
/// `tests::a_name_missing_from_uses_is_a_warning_not_a_failure`,
/// `tests::with_no_render_node_shaders_are_said_not_compiled`,
/// `tests::with_no_render_node_formats_are_said_not_checked`,
/// `tests::a_program_that_does_not_compile_fails_the_check`,
/// `tests::an_effect_needing_rgba16f_fails_where_the_probe_says_it_is_missing`.
pub(crate) fn effect_folder<C: crate::effect::host::Compiler>(
    report: &mut Report,
    dir: &Path,
    shipped: &Path,
    caps: crate::effect::settings::Caps,
    gpu: Option<(&mut C, crate::pool::Formats)>,
) {
    use crate::effect::host::{Host, Library};
    // `solium --check .` inside a folder: a path ending in `.` or `..` has no
    // name of its own to look the effect up by
    // (`tests::a_folder_named_through_dot_dot_is_checked_by_its_own_name`).
    let dir = if dir.file_name().is_some() {
        dir.to_path_buf()
    } else {
        dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
    };
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Said as it is: the folder is there, and nothing can name it
    // (`tests::a_folder_whose_name_cannot_name_an_effect_fails_saying_so`).
    if !crate::effect::host::is_name(&name) {
        report.fail(&format!(
            "    {}: `{name}` cannot name an effect: a name is lower-case letters, digits, `-` and `_`",
            dir.display()
        ));
        return;
    }
    let parent = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    // The folder's own place first, then the shipped folders: a user's copy of
    // `zoom` names the shipped `fade` (`tests::a_user_folder_naming_a_shipped_effect_passes`).
    let mut host: Host<C::Program> = Host::new(Library::with(Some(parent), shipped.to_path_buf()));
    // Under the configured sandbox, as the session loads it
    // (`tests::an_effect_is_checked_under_the_configured_sandbox`).
    host.set_caps(caps);
    // Known before the folder is bound, so a rung this GPU cannot render
    // into is dropped as at run time; with no render node every rung is kept.
    if let Some((_, formats)) = &gpu {
        host.set_formats(*formats);
    }
    host.want("check", [name.clone()]);
    let compiled = gpu.is_some();
    if let Some((compiler, _)) = gpu {
        host.compile_pending(compiler);
    }
    say(report, host.problems());
    if host
        .problems()
        .iter()
        .all(|each| each.severity != solium_effects::spec::Severity::Error)
    {
        report.line(&format!(
            "    {name}: ok{}",
            if compiled {
                ""
            } else {
                " (shaders not compiled: no render node; formats not checked)"
            }
        ));
    }
}

/// Every folder in the user's `effects/` and every effect the configuration
/// names, each with what it names in turn (Ruling 9). The shipped folders
/// are `cargo test`'s, as the shipped styles are.
/// `tests::every_user_folder_and_every_named_effect_is_checked`,
/// `tests::with_no_render_node_formats_are_said_not_checked`,
/// `cli::check_exits_1_on_a_broken_user_effect_folder`.
pub(crate) fn effects<C: crate::effect::host::Compiler>(
    report: &mut Report,
    library: &crate::effect::host::Library,
    wanted: &[String],
    caps: crate::effect::settings::Caps,
    mut gpu: Option<(&mut C, crate::pool::Formats)>,
) {
    report.line("  effects:");
    if gpu.is_none() {
        report.line("    shaders not compiled: no render node");
        report.line("    formats not checked: no render node");
    }
    let mut seen = std::collections::BTreeSet::new();
    for dir in library.user_folders() {
        if !dir.join("effect.lua").is_file() {
            report.fail(&format!("    {}: has no effect.lua", dir.display()));
            continue;
        }
        seen.insert(
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        effect_folder(
            report,
            &dir,
            library.shipped(),
            caps,
            gpu.as_mut()
                .map(|(compiler, formats)| (&mut **compiler, *formats)),
        );
    }
    for name in wanted.iter().filter(|name| !seen.contains(*name)) {
        match library.resolve(name) {
            Some(dir) => effect_folder(
                report,
                &dir,
                library.shipped(),
                caps,
                gpu.as_mut()
                    .map(|(compiler, formats)| (&mut **compiler, *formats)),
            ),
            None => report.fail(&format!(
                "    `{name}`: the configuration names an effect nobody ships"
            )),
        }
    }
}

/// The rules the configuration handed over as it loaded (none when it named
/// none), or `None` when they did not parse, or the engine's keys beside
/// them were refused, which refuses them too; [`config`] has failed both.
/// `tests::the_effects_a_rule_names_are_checked`,
/// `tests::rules_that_did_not_parse_are_not_called_ok`,
/// `tests::a_refused_effects_key_fails_the_check`.
pub(crate) fn configured_rules(
    scripts: &crate::script::Scripts,
) -> Option<Vec<crate::effect::rules::Rule>> {
    if let Some(Err(_)) = scripts.effect_settings_at_load() {
        return None;
    }
    match scripts.effects_at_load() {
        Some(Ok(rules)) => Some(rules),
        None => Some(Vec::new()),
        Some(Err(_)) => None,
    }
}

/// The `effects.sandbox` the configuration gives every effect's Lua, which
/// `--check` loads effects and styles under, as the session does: the
/// defaults when it names none or its `sol.effects` is refused.
/// `tests::an_effect_is_checked_under_the_configured_sandbox`,
/// `cli::check_of_a_folder_reads_the_configured_sandbox`.
pub(crate) fn configured_caps(scripts: &crate::script::Scripts) -> crate::effect::settings::Caps {
    match scripts.effect_settings_at_load() {
        Some(Ok(settings)) => settings.sandbox,
        _ => crate::effect::settings::Caps::default(),
    }
}

/// Every effect the rules name, each once: what [`effects`] checks beside
/// the user's folders. `tests::the_effects_a_rule_names_are_checked`.
pub(crate) fn wanted_by(rules: &[crate::effect::rules::Rule]) -> Vec<String> {
    crate::effect::rules::Rules::new(Vec::new(), Vec::new(), rules.to_vec(), 0).effects()
}

/// The rules bound as at load, GPU-free: a rule the compositor would refuse
/// fails, a tier this build cannot run included (Ruling 14), at the
/// effect's own file and line where it has one; rules that did not parse
/// are not bound, and not said to pass.
/// `tests::a_rule_reading_xray_fails_the_check`,
/// `tests::rules_that_did_not_parse_are_not_called_ok`.
pub(crate) fn rules(
    report: &mut Report,
    library: &crate::effect::host::Library,
    rules: Option<&[crate::effect::rules::Rule]>,
    caps: crate::effect::settings::Caps,
    formats: Option<crate::pool::Formats>,
) {
    report.line("  effects.rules:");
    // Rules that did not parse, or whose `sol.effects` was refused, which
    // `config` has failed, are not said to pass
    // (`tests::rules_that_did_not_parse_are_not_called_ok`).
    let Some(rules) = rules else {
        report.line("    rules not checked: they did not parse, or their sol.effects was refused");
        return;
    };
    let mut host = crate::effect::host::Host::new(library.clone());
    // Under the configured sandbox, as the session binds them
    // (`tests::an_effect_is_checked_under_the_configured_sandbox`).
    host.set_caps(caps);
    if let Some(formats) = formats {
        host.set_formats(formats);
    }
    host.want("check", wanted_by(rules));
    let config = crate::script::Scripts::config_path();
    let mut refused = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        if rule.fill == crate::effect::rules::Fill::Off {
            continue;
        }
        if let Err(problem) = crate::effect::plan::Chains::bind(&mut host, rule) {
            refused.push(crate::effect::plan::rule_problem(
                index + 1,
                problem,
                &config,
            ));
        }
    }
    say(report, &refused);
    if refused.is_empty() {
        report.line(&format!("    {} rule(s): ok", rules.len()));
    }
}

/// Problems as lines: an error fails the report, a warning is said.
/// `tests::a_name_missing_from_uses_is_a_warning_not_a_failure`.
fn say(report: &mut Report, problems: &[crate::effect::host::Problem]) {
    for problem in problems {
        let at = match problem.line {
            Some(line) => format!("{}:{line}", problem.file.display()),
            None => problem.file.display().to_string(),
        };
        match problem.severity {
            solium_effects::spec::Severity::Error => {
                report.fail(&format!("    {at}: {}", problem.message));
            }
            solium_effects::spec::Severity::Warning => {
                report.line(&format!("    warning: {at}: {}", problem.message));
            }
        }
    }
}

/// A headless renderer on the first render node, opened as wirecheck opens
/// one (`dev/wirecheck/src/main.rs`'s `open_gbm` and `make_renderer`), never
/// the card. `None` where there is none, as in the gate's container and in
/// COPR.
#[expect(
    unsafe_code,
    reason = "opening a headless renderer on a render node for --check"
)]
fn render_node() -> Option<smithay::backend::renderer::gles::GlesRenderer> {
    use smithay::backend::{
        allocator::gbm::GbmDevice,
        drm::DrmDeviceFd,
        egl::{EGLContext, EGLDisplay},
        renderer::gles::GlesRenderer,
    };
    use smithay::utils::DeviceFd;
    let node = std::fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("renderD"))
        })
        .min()?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(node)
        .ok()?;
    let gbm = GbmDevice::new(DrmDeviceFd::new(DeviceFd::from(
        std::os::fd::OwnedFd::from(file),
    )))
    .ok()?;
    // SAFETY: the display is made from a GBM device this function owns, and
    // lives as long as the renderer that holds its context.
    let display = unsafe { EGLDisplay::new(gbm) }.ok()?;
    let context = EGLContext::new(&display).ok()?;
    // SAFETY: the context is fresh and is current on no other thread.
    unsafe { GlesRenderer::new(context) }.ok()
}

/// `solium --check [<file>]`: the configuration, one QML file, or one effect
/// folder; the exit status is the report's.
/// `tests/cli.rs`'s `check_qml_on_a_broken_file_exits_one`,
/// `check_exits_1_on_a_broken_effect_folder`.
pub(crate) fn run(single: Option<&Path>) -> std::process::ExitCode {
    let mut out = std::io::stdout();
    let mut report = Report::new(&mut out);
    match single {
        // An effect folder: checked alone, with no Qt
        // (`cli::check_exits_0_on_a_good_effect_folder`).
        Some(dir) if dir.is_dir() && dir.join("effect.lua").is_file() => {
            // Under the configuration's `effects.sandbox`, as the session
            // loads the folder; the defaults when it does not load
            // (`cli::check_of_a_folder_reads_the_configured_sandbox`).
            let caps = crate::script::Scripts::load(&crate::script::Scripts::config_path())
                .map_or_else(
                    |_| crate::effect::settings::Caps::default(),
                    |scripts| configured_caps(&scripts),
                );
            let mut gpu = render_node();
            let formats = gpu.as_mut().map(crate::pool::probe_formats);
            let mut compiler = gpu.as_mut().map(crate::effect::GlCompiler);
            effect_folder(
                &mut report,
                dir,
                &crate::assets::effects(),
                caps,
                compiler.as_mut().zip(formats),
            );
        }
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
            if let Some(mut scripts) = config(&crate::script::Scripts::config_path(), &mut report) {
                // Read before `scenes` takes what the configuration handed
                // over as it loaded.
                let configured = configured_rules(&scripts);
                let caps = configured_caps(&scripts);
                // Software, as for one file: what is checked is whether each
                // scene builds, not what it looks like on this machine.
                crate::qml::renderer::decide(
                    crate::qml::renderer::Entry::CheckQml,
                    &crate::qml::renderer::Configured::default(),
                );
                match crate::qml::start() {
                    Ok(()) => {
                        scenes(&mut scripts, &mut report);
                        styles(&mut report, caps);
                        let mut gpu = render_node();
                        let formats = gpu.as_mut().map(crate::pool::probe_formats);
                        let mut compiler = gpu.as_mut().map(crate::effect::GlCompiler);
                        let library = crate::effect::host::Library::new();
                        effects(
                            &mut report,
                            &library,
                            &wanted_by(configured.as_deref().unwrap_or_default()),
                            caps,
                            compiler.as_mut().zip(formats),
                        );
                        rules(&mut report, &library, configured.as_deref(), caps, formats);
                        let pane = ["SOLIUM_PANE", "SOLIUM_DECORATION", "SOLIUM_QML_TITLEBAR"]
                            .into_iter()
                            .find_map(|name| std::env::var(name).ok());
                        let loading = std::env::var("SOLIUM_LOADING").ok();
                        knobs(pane.as_deref(), loading.as_deref(), &mut report);
                    }
                    Err(err) => report.fail(&format!("  Qt would not start: {err:#}")),
                }
            }
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

    use super::{Report, config, effect_folder, effects, knobs, qml_file, styles_in};

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

    /// A configuration in a temporary directory of its own, declaring one
    /// surface whose scene is `scene`.
    fn declaring(name: &str, scene: &Path) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("solium-check-test-{name}"));
        let _ = std::fs::create_dir_all(&directory);
        let path = directory.join("init.lua");
        std::fs::write(
            &path,
            format!(
                "sol.surface(\"bar\", {{ scene = {:?}, layer = \"top\" }})\n",
                scene.display().to_string()
            ),
        )
        .expect("the file");
        path
    }

    /// `config`, then `scenes` over what it loaded, as `run` does.
    fn check_scenes(config_path: &Path) -> (bool, String) {
        reported(|report| {
            if let Some(mut scripts) = config(config_path, report) {
                super::scenes(&mut scripts, report);
            }
        })
    }

    /// Whether the user's own files would be read in place of the shipped
    /// ones: their Lua, found first on `package.path`, or their QML, found
    /// first by `scripted::find_scene`. Then a test about the shipped
    /// configuration has nothing to say.
    fn the_users_own_files_are_in_the_way() -> bool {
        let lua = crate::script::Scripts::user_config_dir()
            .and_then(|own| std::fs::read_dir(own).ok())
            .is_some_and(|mut entries| {
                entries.any(|entry| {
                    entry.ok().is_some_and(|entry| {
                        entry.path().extension().is_some_and(|kind| kind == "lua")
                    })
                })
            });
        lua || crate::qml::user_qml_dir().is_some()
    }

    /// A surface whose scene builds and draws is listed, and passes.
    #[test]
    fn a_surface_whose_scene_builds_is_listed_and_passes() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = check_scenes(&declaring("bar", &fixture("surface/Bar.qml")));
            assert!(passed, "{text}");
            assert!(text.contains("  1 scene(s) declared at load:"), "{text}");
            assert!(text.contains("    ok bar"), "{text}");
        });
    }

    /// **A surface whose scene does not load fails `--check`**: it used to
    /// pass, because the check never built a scene.
    #[test]
    fn a_surface_whose_scene_does_not_load_fails() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = check_scenes(&declaring("broken-surface", &fixture("broken.qml")));
            assert!(!passed, "{text}");
        });
    }

    #[test]
    fn a_surface_whose_scene_file_is_missing_fails() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = check_scenes(&declaring(
                "missing-surface",
                Path::new("/nonexistent/solium-check.qml"),
            ));
            assert!(!passed, "{text}");
        });
    }

    #[test]
    fn a_surface_missing_a_required_property_fails() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = check_scenes(&declaring("needs", &fixture("needs/Needs.qml")));
            assert!(!passed, "{text}");
        });
    }

    /// A scene that reads its monitor passes: the check publishes one row.
    #[test]
    fn a_scene_that_reads_its_monitor_passes() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = check_scenes(&declaring("monitor", &fixture("monitor/Reads.qml")));
            assert!(passed, "{text}");
        });
    }

    /// **A scene that warns while it is built fails**, as an unrecognised
    /// setting does: it built, and it is not doing what the file says
    /// (Ruling 19).
    #[test]
    fn a_scene_that_warns_while_it_is_built_fails() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) = check_scenes(&declaring("warns", &fixture("warns/Warns.qml")));
            assert!(!passed, "{text}");
            assert!(text.contains("warning"), "{text}");
        });
    }

    /// The shipped configuration passes, as `generate.py` and the gate run it.
    #[test]
    fn the_shipped_configuration_passes() {
        if the_users_own_files_are_in_the_way() {
            return;
        }
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let shipped = crate::assets::lua().join("init.lua");
            let (passed, text) = check_scenes(&shipped);
            assert!(passed, "{text}");
        });
    }

    /// A style of your own fails on its broken layer, under `--check` alone.
    #[test]
    fn a_broken_style_of_your_own_fails() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let own = fixture("");
            let (passed, text) = reported(|report| {
                styles_in(
                    std::slice::from_ref(&own),
                    &crate::style::shipped(),
                    CAPS,
                    report,
                );
            });
            assert!(!passed, "{text}");
            assert!(text.contains("Ring.qml"), "{text}");
        });
    }

    /// **A style of your own whose `effects.lua` is broken fails**, at its
    /// file and line, as the overlay names it, though every layer builds.
    #[test]
    fn a_broken_effects_lua_of_your_own_fails() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let own = std::env::temp_dir().join(format!(
                "solium-check-test-{}-style-rules",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&own);
            let style = own.join("mine");
            std::fs::create_dir_all(&style).expect("a style folder");
            std::fs::write(
                style.join("Pane.qml"),
                "import Solium\nPaneStyle { Layer { depth: \"frame\"; name: \"bar\" } }\n",
            )
            .expect("writing the manifest");
            std::fs::write(
                style.join("effects.lua"),
                "return {\n  { part = = 'client' } }\n",
            )
            .expect("writing effects.lua");
            let (passed, text) = reported(|report| {
                styles_in(
                    std::slice::from_ref(&own),
                    &crate::style::shipped(),
                    CAPS,
                    report,
                );
            });
            let _ = std::fs::remove_dir_all(&own);
            assert!(!passed, "{text}");
            assert!(text.contains("effects.lua:2: "), "{text}");
        });
    }

    /// The shipped styles are not built again: `cargo test` checks them
    /// (`decoration::tests::a_narrow_tile_hides_the_titlebars_pieces_in_every_shipped_style`).
    #[test]
    fn the_shipped_styles_are_left_to_cargo_test() {
        let shipped = crate::style::shipped();
        let (passed, text) = reported(|report| {
            styles_in(std::slice::from_ref(&shipped), &shipped, CAPS, report);
        });
        assert!(passed, "{text}");
        assert!(!text.contains("Pane.qml"), "{text}");
    }

    /// `SOLIUM_PANE` naming a style is that style checked, layers and all.
    #[test]
    fn a_pane_knob_naming_a_broken_style_fails() {
        if std::env::var_os("SOLIUM_PANE").is_some()
            || std::env::var_os("SOLIUM_DECORATION").is_some()
            || std::env::var_os("SOLIUM_QML_TITLEBAR").is_some()
        {
            return;
        }
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let style = fixture("style").display().to_string();
            let (passed, text) = reported(|report| knobs(Some(&style), None, report));
            assert!(!passed, "{text}");
            assert!(text.contains("Ring.qml"), "{text}");
        });
    }

    /// `SOLIUM_LOADING` naming a file that is not there fails, and says so.
    #[test]
    fn a_loading_knob_naming_a_missing_file_fails() {
        if std::env::var_os("SOLIUM_LOADING").is_some() {
            return;
        }
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let (passed, text) =
                reported(|report| knobs(None, Some("/nonexistent/solium-loading.qml"), report));
            assert!(!passed, "{text}");
            assert!(text.contains("solium-loading.qml"), "{text}");
        });
    }

    /// The test effect folders (`tests/fixtures/effects/`).
    fn effect_fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/effects")
    }

    fn effect_fixture(name: &str) -> PathBuf {
        effect_fixtures().join(name)
    }

    /// The real shipped folders, as `--check` resolves a closure through them.
    fn shipped() -> PathBuf {
        crate::assets::effects()
    }

    type Counting = crate::effect::host::tests::Counting;

    /// `effects.sandbox` at its defaults, as a configuration naming none
    /// gives it.
    const CAPS: crate::effect::settings::Caps = crate::effect::settings::Caps::DEFAULT;

    /// **A broken effect folder fails the check**, naming the file.
    #[test]
    fn a_broken_effect_folder_fails_check() {
        let (passed, out) = reported(|report| {
            effect_folder::<Counting>(report, &effect_fixture("api2"), &shipped(), CAPS, None);
        });
        assert!(!passed, "{out}");
        assert!(
            out.contains("api2/effect.lua") && out.contains("api"),
            "{out}"
        );
    }

    /// **A geometry file that moves the window at rest fails the check**, at
    /// its `mesh`'s line, as it is refused at load.
    #[test]
    fn a_geometry_file_that_moves_the_window_at_rest_fails_the_check() {
        let (passed, out) = reported(|report| {
            effect_folder::<Counting>(report, &effect_fixture("mover"), &shipped(), CAPS, None);
        });
        assert!(!passed, "{out}");
        assert!(
            out.contains("mover/effect.lua:7") && out.contains("progress 0"),
            "{out}"
        );
    }

    /// **A `.frag` reading an undeclared param fails the check at its line**,
    /// with no GPU at all.
    #[test]
    fn a_frag_reading_an_undeclared_param_fails_check_at_its_line() {
        let (passed, out) = reported(|report| {
            effect_folder::<Counting>(report, &effect_fixture("typo"), &shipped(), CAPS, None);
        });
        assert!(!passed);
        assert!(out.contains("down.frag:2"), "{out}");
    }

    /// **A name missing from `uses` is a warning, not a failure.**
    #[test]
    fn a_name_missing_from_uses_is_a_warning_not_a_failure() {
        let place = crate::effect::host::tests::scratch("check-warning");
        let dir = crate::effect::host::tests::folder(
            &place,
            "soft",
            "return { api = 1, inputs = { 'self', 'backdrop' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_backdrop(uv); }\n",
            )],
        );
        let (passed, out) =
            reported(|report| effect_folder::<Counting>(report, &dir, &shipped(), CAPS, None));
        let _ = std::fs::remove_dir_all(place);
        assert!(passed, "{out}");
        assert!(
            out.contains("warning:") && out.contains("sol_backdrop"),
            "{out}"
        );
    }

    /// **A user's folder that names a shipped effect passes**: its closure
    /// (here a `fallback`; `pixels = "fade"` in a copy of `zoom` is the same
    /// path) resolves through the shipped folders, as at run time, and not
    /// only beside the folder checked.
    #[test]
    fn a_user_folder_naming_a_shipped_effect_passes() {
        let place = crate::effect::host::tests::scratch("check-closure");
        let dir = crate::effect::host::tests::folder(
            &place,
            "mine",
            "return { api = 1, inputs = { 'self' }, frag = 'effect.frag', fallback = { 'identity' } }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        let (passed, out) = reported(|report| {
            effect_folder::<Counting>(report, &dir, &effect_fixtures(), CAPS, None)
        });
        let _ = std::fs::remove_dir_all(place);
        assert!(
            passed,
            "the closure did not resolve through the shipped folders: {out}"
        );
    }

    /// **A folder's `use`s are checked with it**: a stage list that cannot be
    /// read fails at `effect.lua`, and so does an effect it uses that is
    /// broken or that nobody ships, while one using a shipped effect passes.
    #[test]
    fn a_folders_uses_are_checked_with_it() {
        let place = crate::effect::host::tests::scratch("check-uses");
        let check = |name: &str, stages: &str| {
            let dir = crate::effect::host::tests::folder(
                &place,
                name,
                &format!("return {{ api = 1, inputs = {{ 'self' }}, stages = {stages} }}"),
                &[],
            );
            reported(|report| {
                effect_folder::<Counting>(report, &dir, &effect_fixtures(), CAPS, None)
            })
        };
        let (passed, out) = check("fine", "{ { 'use', 'tint' } }");
        assert!(passed, "{out}");
        let (passed, out) = check("broken", "{ { 'use', 'api2' } }");
        assert!(!passed && out.contains("api2/effect.lua"), "{out}");
        let (passed, out) = check("missing", "{ { 'use', 'nowhere' } }");
        assert!(!passed && out.contains("`nowhere`"), "{out}");
        let (passed, out) = check("typo", "{ { 'pass', 'a.frag', scal = 1 } }");
        let _ = std::fs::remove_dir_all(&place);
        assert!(
            !passed && out.contains("typo/effect.lua") && out.contains("`scal`"),
            "{out}"
        );
    }

    /// **With no render node, shaders are said not compiled**, and that is
    /// not a failure: `rpm %check` in COPR has none.
    #[test]
    fn with_no_render_node_shaders_are_said_not_compiled() {
        let (passed, out) = reported(|report| {
            effect_folder::<Counting>(report, &effect_fixture("identity"), &shipped(), CAPS, None);
        });
        assert!(passed, "{out}");
        assert!(
            out.contains("shaders not compiled: no render node"),
            "{out}"
        );
    }

    /// With a compiler, a program that does not compile fails the check at
    /// the user's line (here the counting compiler's `FAIL`).
    #[test]
    fn a_program_that_does_not_compile_fails_the_check() {
        let place = crate::effect::host::tests::scratch("check-compile");
        let dir = crate::effect::host::tests::folder(
            &place,
            "bad",
            "return { api = 1, frag = 'effect.frag' }",
            &[("effect.frag", "vec4 sol_effect(vec2 uv) {\n  FAIL\n}\n")],
        );
        let mut compiler = Counting::default();
        let (passed, out) = reported(|report| {
            effect_folder(
                report,
                &dir,
                &shipped(),
                CAPS,
                Some((&mut compiler, crate::pool::Formats { rgba16f: true })),
            );
        });
        let _ = std::fs::remove_dir_all(place);
        assert!(!passed);
        assert!(out.contains("effect.frag:2"), "{out}");
    }

    /// **With no render node, formats are said not checked**, beside the
    /// shaders, for one folder and for plain `--check`'s effects: every rung
    /// is kept, and that is not a failure.
    #[test]
    fn with_no_render_node_formats_are_said_not_checked() {
        let (passed, out) = reported(|report| {
            effect_folder::<Counting>(report, &effect_fixture("identity"), &shipped(), CAPS, None);
        });
        assert!(passed && out.contains("formats not checked"), "{out}");
        let library = crate::effect::host::Library::with(None, effect_fixtures());
        let (passed, out) = reported(|report| {
            effects::<Counting>(report, &library, &["identity".to_owned()], CAPS, None);
        });
        assert!(
            passed && out.contains("    formats not checked: no render node"),
            "{out}"
        );
    }

    /// **An effect whose every rung needs `rgba16f` fails where the render
    /// node's probe says it is missing**, and passes where it renders: the
    /// formats are judged on the GPU the check opened.
    #[test]
    fn an_effect_needing_rgba16f_fails_where_the_probe_says_it_is_missing() {
        let place = crate::effect::host::tests::scratch("check-rgba16f");
        let dir = crate::effect::host::tests::folder(
            &place,
            "deep",
            "return { api = 1, inputs = { 'self' }, stages = { { 'pass', 'effect.frag', format = 'rgba16f' } } }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        let check = |rgba16f: bool| {
            let mut compiler = Counting::default();
            reported(|report| {
                effect_folder(
                    report,
                    &dir,
                    &shipped(),
                    CAPS,
                    Some((&mut compiler, crate::pool::Formats { rgba16f })),
                );
            })
        };
        let (missing, out) = check(false);
        assert!(!missing && out.contains("rgba16f"), "{out}");
        let (renders, out) = check(true);
        let _ = std::fs::remove_dir_all(place);
        assert!(renders, "{out}");
    }

    /// **A folder named through `..` is checked by its own name**, as
    /// `solium --check .` inside one is: a path whose last part is `.` or
    /// `..` has no name of its own to look the effect up by.
    #[test]
    fn a_folder_named_through_dot_dot_is_checked_by_its_own_name() {
        let place = crate::effect::host::tests::scratch("check-dot-dot");
        let dir = crate::effect::host::tests::folder(
            &place,
            "plain",
            "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        std::fs::create_dir_all(dir.join("inner")).expect("a folder inside");
        let (passed, out) = reported(|report| {
            effect_folder::<Counting>(report, &dir.join("inner/.."), &shipped(), CAPS, None);
        });
        let _ = std::fs::remove_dir_all(place);
        assert!(passed, "{out}");
        assert!(out.contains("plain: ok"), "{out}");
    }

    /// **A folder whose name cannot name an effect fails, saying so**, rather
    /// than that no effect of that name exists: it is there, and nothing can
    /// use it under that name.
    #[test]
    fn a_folder_whose_name_cannot_name_an_effect_fails_saying_so() {
        let place = crate::effect::host::tests::scratch("check-bad-name");
        let dir = crate::effect::host::tests::folder(
            &place,
            "Glow",
            "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        let (passed, out) =
            reported(|report| effect_folder::<Counting>(report, &dir, &shipped(), CAPS, None));
        let _ = std::fs::remove_dir_all(place);
        assert!(!passed, "{out}");
        assert!(
            out.contains("`Glow` cannot name an effect") && !out.contains("no effect called"),
            "{out}"
        );
    }

    /// **Every folder of the user's is checked, and every effect the
    /// configuration names**: a broken one of either fails, a folder with no
    /// `effect.lua` fails, and a name nobody ships fails.
    #[test]
    fn every_user_folder_and_every_named_effect_is_checked() {
        let place = crate::effect::host::tests::scratch("check-effects");
        let user = place.join("user");
        crate::effect::host::tests::folder(
            &user,
            "good",
            "return { api = 1, inputs = { 'self' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        std::fs::create_dir_all(user.join("empty")).expect("a folder with nothing in it");
        let library = crate::effect::host::Library::with(Some(user), effect_fixtures());
        let check = |wanted: &[String]| {
            reported(|report| effects::<Counting>(report, &library, wanted, CAPS, None))
        };
        let (passed, out) = check(&[]);
        assert!(!passed, "{out}");
        assert!(
            out.contains("empty: has no effect.lua") && out.contains("good: ok"),
            "{out}"
        );
        std::fs::remove_dir(place.join("user/empty")).expect("the empty folder goes");
        let (passed, out) = check(&["identity".to_owned()]);
        assert!(passed, "{out}");
        assert!(
            out.contains("identity: ok"),
            "a named effect was not checked: {out}"
        );
        let (passed, out) = check(&["typo".to_owned()]);
        assert!(!passed, "{out}");
        assert!(out.contains("down.frag:2"), "{out}");
        let (passed, out) = check(&["nowhere".to_owned()]);
        let _ = std::fs::remove_dir_all(place);
        assert!(!passed, "{out}");
        assert!(out.contains("`nowhere`"), "{out}");
    }

    /// **A broken rule fails the check**, by its number and its key, with
    /// the part it probably meant.
    #[test]
    fn a_broken_rule_fails_the_check() {
        let directory = crate::effect::host::tests::scratch("check-rule");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"sol.effects({ rules = { { match = "*", part = "regoin:titlebar", slot = "behind", effect = false } } })"#,
        )
        .expect("writing");
        let (passed, out) = reported(|report| {
            let _ = super::config(&config, report);
        });
        let _ = std::fs::remove_dir_all(directory);
        assert!(!passed, "{out}");
        assert!(
            out.contains("rule 1") && out.contains("region:titlebar"),
            "{out}"
        );
    }

    /// The rules an `init.lua` calling `sol.effects{ rules = <rules> }`
    /// hands over, as `--check` reads them.
    fn rules_from(directory: &Path, rules: &str) -> Option<Vec<crate::effect::rules::Rule>> {
        let config = directory.join("init.lua");
        std::fs::write(&config, format!("sol.effects({{ rules = {rules} }})")).expect("writing");
        let scripts = crate::script::Scripts::load(&config).expect("loading");
        super::configured_rules(&scripts)
    }

    /// **The effects a rule names are checked**, as `--check` checks the
    /// folders it is handed: a rule naming `typo` fails at its `.frag`'s line.
    #[test]
    fn the_effects_a_rule_names_are_checked() {
        let directory = crate::effect::host::tests::scratch("check-rule-effects");
        let rules = rules_from(
            &directory,
            r#"{ { match = "*", part = "client", slot = "behind", effect = { { "identity" }, { "typo" } } } }"#,
        )
        .expect("parsed");
        let _ = std::fs::remove_dir_all(directory);
        let wanted = super::wanted_by(&rules);
        assert_eq!(wanted, ["identity".to_owned(), "typo".to_owned()]);
        let library = crate::effect::host::Library::with(None, effect_fixtures());
        let (passed, out) =
            reported(|report| effects::<Counting>(report, &library, &wanted, CAPS, None));
        assert!(!passed, "{out}");
        assert!(
            out.contains("identity: ok") && out.contains("down.frag:2"),
            "{out}"
        );
    }

    /// **A rule reading xray fails the check** (Ruling 14), naming X2.1, as
    /// it is refused at load; with `source = "self"` it passes.
    #[test]
    fn a_rule_reading_xray_fails_the_check() {
        let place = crate::effect::host::tests::scratch("check-xray");
        let effects = place.join("effects");
        crate::effect::host::tests::folder(
            &effects,
            "soft",
            "return { api = 1, inputs = { 'backdrop' }, frag = 'effect.frag' }",
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        let library = crate::effect::host::Library::with(Some(effects.clone()), place.join("none"));
        let check = |rules: &str| {
            let rules = rules_from(&place, rules);
            reported(|report| super::rules(report, &library, rules.as_deref(), CAPS, None))
        };
        let (passed, out) =
            check(r#"{ { match = "*", part = "client", slot = "behind", effect = "soft" } }"#);
        assert!(!passed, "{out}");
        assert!(out.contains("rule 1") && out.contains("X2.1"), "{out}");
        let (passed, out) = check(
            r#"{ { match = "*", part = "client", slot = "behind", effect = { "soft", source = "self" } } }"#,
        );
        let _ = std::fs::remove_dir_all(place);
        assert!(passed, "{out}");
    }

    /// **Rules that did not parse are not called ok**: [`config`] has failed
    /// them already, and the rules section says it did not check them rather
    /// than "0 rule(s): ok".
    #[test]
    fn rules_that_did_not_parse_are_not_called_ok() {
        let directory = crate::effect::host::tests::scratch("check-rules-unparsed");
        let rules = rules_from(
            &directory,
            r#"{ { match = "*", part = "regoin:titlebar", slot = "behind", effect = false } }"#,
        );
        let _ = std::fs::remove_dir_all(&directory);
        assert!(rules.is_none(), "the premise: the rules did not parse");
        let library = crate::effect::host::Library::with(None, effect_fixtures());
        let (_, out) =
            reported(|report| super::rules(report, &library, rules.as_deref(), CAPS, None));
        assert!(!out.contains("ok") && out.contains("not checked"), "{out}");
    }

    /// **A refused `effects` key fails the check** (Ruling 9: what the
    /// session refuses, `--check` fails), naming the key; and the rules of
    /// the same refused `sol.effects` are not checked as if they applied.
    #[test]
    fn a_refused_effects_key_fails_the_check() {
        let directory = crate::effect::host::tests::scratch("check-settings");
        let config = directory.join("init.lua");
        std::fs::write(
            &config,
            r#"sol.effects({ rules = { { match = "*", part = "client", slot = "behind", effect = "identity" } }, sandbox = { load_ms = 1 } })"#,
        )
        .expect("writing");
        let mut scripts = None;
        let (passed, out) = reported(|report| scripts = super::config(&config, report));
        let _ = std::fs::remove_dir_all(directory);
        assert!(!passed, "{out}");
        assert!(out.contains("effects.sandbox.load_ms"), "{out}");
        let scripts = scripts.expect("the configuration loads");
        assert!(
            super::configured_rules(&scripts).is_none(),
            "the rules of a refused sol.effects were taken as applied"
        );
    }

    /// The caps `effects.sandbox = { memory_mib = 64 }` gives, with a long
    /// budget beside it so a busy machine building a string slowly is not
    /// what is tested.
    const ROOMY: crate::effect::settings::Caps = crate::effect::settings::Caps {
        load: std::time::Duration::from_millis(5000),
        memory: 64 << 20,
    };

    /// A 20 MiB string, built as an effect's Lua loads.
    const BIG: &str = "local big = string.rep('x', 20 * 1024 * 1024)\n";

    /// **An effect is checked under the configured `effects.sandbox`**: a
    /// folder that builds 20 MiB fails under the default 16 and passes
    /// under 64, as the session loads it; and so does a rule naming it.
    #[test]
    fn an_effect_is_checked_under_the_configured_sandbox() {
        let place = crate::effect::host::tests::scratch("check-caps");
        let dir = crate::effect::host::tests::folder(
            &place,
            "big",
            &format!("{BIG}return {{ api = 1, inputs = {{ 'self' }}, frag = 'effect.frag' }}"),
            &[(
                "effect.frag",
                "vec4 sol_effect(vec2 uv) { return sol_tex(uv); }\n",
            )],
        );
        let (passed, out) =
            reported(|report| effect_folder::<Counting>(report, &dir, &shipped(), CAPS, None));
        assert!(!passed, "the premise: 20 MiB under 16: {out}");
        let (passed, out) =
            reported(|report| effect_folder::<Counting>(report, &dir, &shipped(), ROOMY, None));
        assert!(passed, "{out}");
        let library = crate::effect::host::Library::with(Some(place.clone()), shipped());
        let rules = rules_from(
            &place,
            r#"{ { match = "*", part = "client", slot = "behind", effect = "big" } }"#,
        );
        let (passed, out) =
            reported(|report| super::rules(report, &library, rules.as_deref(), ROOMY, None));
        let _ = std::fs::remove_dir_all(place);
        assert!(passed, "{out}");
    }

    /// **A style's `effects.lua` is checked under the configured
    /// `effects.sandbox`**: one that builds 20 MiB fails under the default
    /// 16 and passes under 64.
    #[test]
    fn a_styles_effects_lua_is_checked_under_the_configured_sandbox() {
        crate::qml::qt_test::on_the_qt_thread(|| {
            software_qt();
            let own = std::env::temp_dir().join(format!(
                "solium-check-test-{}-style-caps",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&own);
            let style = own.join("roomy");
            std::fs::create_dir_all(&style).expect("a style folder");
            std::fs::write(
                style.join("Pane.qml"),
                "import Solium\nPaneStyle { Layer { depth: \"frame\"; name: \"bar\" } }\n",
            )
            .expect("writing the manifest");
            std::fs::write(style.join("effects.lua"), format!("{BIG}return {{}}\n"))
                .expect("writing effects.lua");
            let check = |caps| {
                reported(|report| {
                    styles_in(
                        std::slice::from_ref(&own),
                        &crate::style::shipped(),
                        caps,
                        report,
                    );
                })
            };
            let (refused, said) = check(CAPS);
            let (passed, out) = check(ROOMY);
            let _ = std::fs::remove_dir_all(&own);
            assert!(!refused, "the premise: 20 MiB under 16: {said}");
            assert!(passed, "{out}");
        });
    }

    /// **The shipped effects pass the check**, every one of them, GPU-free.
    #[test]
    fn the_shipped_effects_pass_the_check() {
        let shipped = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/effects"));
        let folders: Vec<_> = std::fs::read_dir(shipped)
            .expect("the shipped folders")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.join("effect.lua").is_file())
            .collect();
        assert!(
            !folders.is_empty(),
            "no shipped effect folder: the walk is broken"
        );
        for dir in folders {
            let (passed, out) = reported(|report| {
                effect_folder::<Counting>(report, &dir, shipped, CAPS, None);
            });
            assert!(passed, "{}: {out}", dir.display());
        }
    }
}
