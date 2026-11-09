//! **The configuration that ships may only ask for things the compositor has.**
//!
//! `init.lua` asked for an easing called `inOutCubic` and the engine had no
//! such curve. `parse_easing` warned, fell back to the default, and the session
//! carried on -- so the genie was quietly the wrong animation for as long as it
//! took somebody to read a hardware log and notice five identical warnings in
//! it. Nothing in the gate was looking, because nothing in the gate reads Lua
//! for anything but syntax: `solium --check` loads the scripts, and a name
//! inside a binding's body is not looked up until the binding runs.
//!
//! That is a class rather than an incident. A script names things -- easings,
//! events, layers, QML scenes, decorations, keys -- and every one of those
//! lookups either warns and carries on or, in the case of an event and a key,
//! misses in complete silence. So this walks `lua/*.lua` and resolves each name
//! through the same function the compositor uses at run time.
//!
//! **What it cannot see.** Only names written as literals. `shell.lua` takes
//! its scene from the configuration or the environment and `workspaces.lua`
//! builds `"super+" .. index` in a loop; a name assembled at run time is
//! outside this and outside any static check. Lua comments are stripped, so a
//! documented example that is deliberately a placeholder -- a path into
//! somebody's home directory -- does not fail a build.

use super::{Curve, Scripts, normalise_combo};

/// The Lua the compositor ships, as `(file, text)`.
///
/// From `CARGO_MANIFEST_DIR` rather than a path relative to the process,
/// because a test's working directory is the workspace root and this file
/// should not have to know that.
fn scripts() -> Vec<(String, String)> {
    let directory = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/lua"));
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<(String, String)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|end| end == "lua"))
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            Some((name, std::fs::read_to_string(&path).ok()?))
        })
        .collect();
    found.sort();
    // A check that walks an empty directory passes, which is the one way
    // this could be green and mean nothing at all.
    assert!(
        found.len() >= 8,
        "found {} shipped scripts in {}; the walk is broken, not the scripts",
        found.len(),
        directory.display()
    );
    found
}

/// One line with its Lua comment removed.
///
/// `--` outside a string starts a comment. Tracked rather than searched for
/// because `config.lua` has a `"module 'user' not found"` in it and a naive
/// cut would one day land inside a string like that one.
fn code(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quoted = false;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if quoted => index += 1,
            b'"' => quoted = !quoted,
            b'-' if !quoted && bytes.get(index + 1) == Some(&b'-') => return &line[..index],
            _ => {}
        }
        index += 1;
    }
    line
}

/// Every double-quoted literal that follows `marker`, with where it was.
///
/// `marker` is matched literally and the literal must come next, with only
/// spaces between: `easing =` finds `easing = "outCubic"` and `sol.on(`
/// finds `sol.on("open", ...)`. Anything else after the marker -- a
/// variable, a table, a concatenation -- is skipped rather than guessed at.
/// That is the limit this module states up front, and it is why
/// `shell.lua`'s `scene = chosen` never appears here.
fn named(text: &str, marker: &str) -> Vec<(usize, String)> {
    // An empty marker matches at every position and consumes none of them,
    // so the walk below would never move. Refused here rather than left to
    // a caller, because the symptom is a test run that never finishes --
    // which is how this was found.
    assert!(!marker.is_empty(), "a marker has to be something");
    let mut found = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = code(line);
        let mut from = 0;
        while let Some(at) = line[from..].find(marker) {
            let after = from + at + marker.len();
            from = after;
            let rest = line[after..].trim_start_matches(' ');
            let Some(rest) = rest.strip_prefix('"') else {
                continue;
            };
            let Some(end) = rest.find('"') else {
                continue;
            };
            found.push((number + 1, rest[..end].to_owned()));
        }
    }
    found
}

/// Every double-quoted literal in a chunk of Lua, in order.
///
/// For a list rather than an assignment: `{ "top", "left", ... }` has no
/// marker in front of each item, only in front of the whole thing.
fn quoted(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in text.lines() {
        let mut rest = code(line);
        while let Some(at) = rest.find('"') {
            let after = &rest[at + 1..];
            let Some(end) = after.find('"') else {
                break;
            };
            found.push(after[..end].to_owned());
            rest = &after[end + 1..];
        }
    }
    found
}

/// Everything in the shipped Lua that follows `marker`, for every file.
fn everywhere(marker: &str) -> Vec<(String, usize, String)> {
    scripts()
        .into_iter()
        .flat_map(|(file, text)| {
            named(&text, marker)
                .into_iter()
                .map(move |(line, value)| (file.clone(), line, value))
        })
        .collect()
}

/// Where the QML that ships lives.
fn shipped_qml() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/qml"))
}

/// **Every easing the shipped configuration asks for exists.**
///
/// The one that did not: `sol.animate({ duration = 520, easing =
/// "inOutCubic" })`, in `init.lua` for `super+m` and again in `tweaks.lua`
/// for the genie tweak. `Curve::from_name` knew five names and that was not
/// one of them, so both fell back to `outCubic` -- a different animation,
/// chosen by nobody, announced only in a log line.
#[test]
fn motion_274_lua_springs_carry_physical_release_velocity() {
    use crate::present::Curve;
    use crate::script::easing_from;
    use mlua::{Lua, Table};
    let lua = Lua::new();
    let options: Table = lua.load("return { easing = { spring = { stiffness = 220, damping = 24, mass = 1.2, initial_velocity = -3.5, epsilon = 0.002 } } }").eval().expect("options");
    let Curve::Spring(spring) = easing_from(&options)
        .expect("spring options")
        .expect("curve")
    else {
        panic!("physical spring required")
    };
    assert_eq!(
        (
            spring.stiffness,
            spring.damping,
            spring.mass,
            spring.initial_velocity,
            spring.epsilon
        ),
        (220.0, 24.0, 1.2, -3.5, 0.002)
    );
    assert!((spring.value_at(0.0000001) / 0.0000001 + 3.5).abs() < 0.001);
    for invalid in [
        "mass=0",
        "mass=1e-30",
        "stiffness=-1",
        "damping=-1",
        "epsilon=0",
        "initial_velocity=1/0",
        "mass=0/0",
    ] {
        let options: Table = lua
            .load(format!(
                "return {{ easing = {{ spring = {{ {invalid} }} }} }}"
            ))
            .eval()
            .expect("options");
        assert!(easing_from(&options).is_err(), "{invalid}");
    }
}

#[test]
fn every_easing_named_is_one_the_engine_has() {
    let asked = everywhere("easing =");
    assert!(!asked.is_empty(), "no easings found; the scan is broken");
    for (file, line, name) in asked {
        assert!(
            Curve::from_name(&name).is_some(),
            "{file}:{line} asks for easing {name:?}, which Curve::from_name cannot read. \
             Names: {:?}",
            Curve::all().map(|(known, _)| known)
        );
    }
}

/// **Every effect the shipped configuration asks for is a shipped
/// folder.**
///
/// The easing case above, one layer over. `sol.present` takes `deform = {
/// effect = "..." }` and the name is an effect folder's, resolved by the
/// effect host where the command is applied -- which also means a
/// misspelling is not a Lua error but a problem on the overlay and an
/// undeformed window, so it is caught here before it ships.
#[test]
fn every_effect_named_is_one_the_engine_has() {
    let asked = everywhere("effect =");
    assert!(!asked.is_empty(), "no effects found; the scan is broken");
    let shipped = crate::assets::effects();
    for (file, line, name) in asked {
        assert!(
            shipped.join(&name).join("effect.lua").is_file(),
            "{file}:{line} asks for effect {name:?}, which is no folder with an effect.lua in {}",
            shipped.display()
        );
    }
}

/// **And every axis, which is worse when it is wrong.**
///
/// A deform's `axis` is refused outside its words when the binding runs,
/// which is a binding that does nothing, seen by nobody who was not
/// reading the log. `"auto"` is one of them: the side the target lies
/// on, picked when the flight starts.
#[test]
fn every_axis_named_is_one_the_engine_has() {
    let asked = everywhere("axis =");
    assert!(!asked.is_empty(), "no axes found; the scan is broken");
    for (file, line, name) in asked {
        assert!(
            name == "auto" || solium_effects::Axis::from_name(&name).is_some(),
            "{file}:{line} asks for axis {name:?}, which Axis::from_name cannot read. \
             Names: \"auto\" and {:?}",
            solium_effects::Axis::all().map(|(known, _)| known)
        );
    }
}

/// **Every event a shipped script listens for is one the compositor sends.**
///
/// Worse than an unknown easing, because there is no warning at all:
/// `sol.on` puts the handler in a table under whatever name it was given,
/// and a name nothing dispatches is a handler that is simply never called.
/// A layout that misspells `"monitors"` does not fail -- it stops
/// rearranging when a screen is unplugged, which reads as a compositor bug.
///
/// The vocabulary is read out of `script/` itself rather than listed
/// here, because a list is a second copy and two copies of a vocabulary
/// drifting apart is the defect this whole module exists for.
#[test]
fn every_event_listened_for_is_one_that_is_sent() {
    // The production half of the module, each file cut at its first `#[cfg(test)]`
    // because everything below it -- including this test -- talks *about*
    // `call_listeners` and would otherwise be counted as calling it.
    let production = super::source::production();
    // Decision hooks (`sol.on("notification")`) are dispatched by
    // `call_deciders`, and are events like any other.
    let dispatched: Vec<String> = named(production, "call_listeners(lua,")
        .into_iter()
        .chain(named(production, "call_deciders(lua,"))
        .map(|(_, name)| name)
        .collect();

    // The scrape has to see every dispatch there is, or it narrows the
    // vocabulary in silence and this test starts agreeing with anything.
    // One `call_listeners` is the definition; the rest are calls.
    let calls = production.matches("call_listeners").count() - 1
        + production
            .matches("call_deciders")
            .count()
            .saturating_sub(1);
    assert_eq!(
        dispatched.len(),
        calls,
        "{calls} dispatches in script/ but only {} are `call_listeners(lua, \"name\")`; \
         one of them is written some other way and this check cannot see it",
        dispatched.len()
    );
    assert!(
        calls >= 8,
        "only {calls} dispatches found; the scan is broken"
    );

    for (file, line, event) in everywhere("sol.on(") {
        assert!(
            dispatched.contains(&event),
            "{file}:{line} listens for {event:?}, which nothing dispatches. \
             Events: {dispatched:?}"
        );
    }
}

/// **Every layer a shipped surface names is one that exists.**
///
/// `layer_named` answers `None` and `sol.surface` turns that into
/// `Background`, with no warning anywhere. A bar that asked for `"Top"` and
/// got the background is a bar drawn underneath every window on the screen,
/// and the symptom is that the bar has vanished.
#[test]
fn every_layer_named_is_one_that_exists() {
    let asked = everywhere("layer =");
    assert!(!asked.is_empty(), "no layers found; the scan is broken");
    for (file, line, name) in asked {
        assert!(
            crate::script::lua::layer_named(&name).is_some(),
            "{file}:{line} puts a surface on layer {name:?}, which is not one of \
             background, bottom, top, overlay"
        );
    }
}

/// **Every QML scene a shipped script names is one that ships.**
///
/// Three lookups, because there are three kinds of scene and a script
/// writes them the same way: a scripted surface's, found on the QML search
/// path by `scripted::find_scene`; a loading scene's, which is a name under
/// `qml/loading`; and the lock's (`lock.scene`), a name under `qml/lock`
/// (`lock_scene::find_scene`). A scene that is not there is an `ERROR` and
/// a hole in the picture -- `sol.surface` says "no such QML scene" and
/// draws nothing, and a lock shows its stand-in.
///
/// The resolved path has to land inside the shipped QML directory. Without
/// that, a developer with their own `~/.config/solium/qml/wallpaper.qml`
/// would have a green test for a file that does not ship.
#[test]
fn every_scene_named_is_one_that_ships() {
    let shipped = shipped_qml();
    let asked = everywhere("scene =");
    assert!(!asked.is_empty(), "no scenes found; the scan is broken");
    for (file, line, name) in asked {
        let surface = crate::scripted::find_scene(&name)
            .is_some_and(|path| path.starts_with(&shipped) && path.is_file());
        let loading = shipped
            .join("loading")
            .join(format!("{name}.qml"))
            .is_file();
        let lock = crate::lock_scene::find_scene(&name, None, &shipped.join("lock"))
            .is_some_and(|path| path.starts_with(&shipped) && path.is_file());
        assert!(
            surface || loading || lock,
            "{file}:{line} names scene {name:?}, and there is no {name} in \
             {}, its loading directory or its lock directory",
            shipped.display()
        );
    }
}

/// **Every surface a shipped selection names is one a shipped script
/// declares.**
///
/// `sol.group` is the third place a script writes a surface's name down, and
/// the quietest of the three. A name nothing has declared is not an error:
/// the selection simply does not contain it, so the group is declared, the
/// transform is applied, and the wallpaper stays exactly where it was —
/// which is the bug this whole item exists to remove, wearing a typo.
///
/// Both halves are read as literals, so what this can see is bounded the way
/// the module header says. The shipped desks build their surface names from
/// a workspace index (`wallpaper.for_desk`), and a name assembled at run time
/// is outside this and outside any static check — so the *membership* side
/// may legitimately be empty. What is asserted non-empty is the declaration
/// side, which proves the scan itself still works on this corpus; a literal
/// written into a group tomorrow is checked against it from that day on.
#[test]
fn every_surface_a_selection_names_is_one_that_is_declared() {
    let declared: Vec<String> = everywhere("sol.surface(")
        .into_iter()
        .map(|(_, _, name)| name)
        .collect();
    assert!(
        !declared.is_empty(),
        "no `sol.surface` declarations found; the scan is broken"
    );

    // Every literal inside a `surfaces = { ... }` list, with its file.
    let mut asked: Vec<(String, usize, String)> = Vec::new();
    for (file, text) in scripts() {
        for (line, rest) in surface_lists(&text) {
            for name in quoted(&rest) {
                asked.push((file.clone(), line, name));
            }
        }
    }
    for (file, line, name) in asked {
        assert!(
            declared.contains(&name),
            "{file}:{line} puts surface {name:?} in a selection, and nothing                  declares one by that name. Declared: {declared:?}"
        );
    }
}

/// The text of each `surfaces = { ... }` list in a chunk of Lua, with the
/// line it starts on.
///
/// A list rather than an assignment, so [`named`] cannot see it: there is no
/// marker in front of each item, only in front of the whole thing. Single
/// line only, which is how every one of them is written and what the whole
/// of this module can see.
fn surface_lists(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = code(line);
        let Some(at) = line.find("surfaces = {") else {
            continue;
        };
        let rest = &line[at..];
        let end = rest.find('}').unwrap_or(rest.len());
        found.push((number + 1, rest[..end].to_owned()));
    }
    found
}

/// **Every pane style the shipped configuration names is one that ships.**
///
/// A name that is nowhere is a frame that fails to build once per window --
/// every window undecorated, and a log line each.
///
/// Resolved through `decoration::ships` rather than by joining a path onto
/// the name here, which is this module's own rule -- "resolves each name
/// through the same function the compositor uses at run time" -- and which
/// this test was the one exception to. It matters rather than being a
/// tidy-up, and the move to style folders is where it would have bitten:
/// the eight names `config.lua` has always carried stopped being files
/// under `qml/decorations` and became folders under `qml/panes` on one
/// commit, and a hand-joined `<name>.qml` would have gone red for all eight
/// while the compositor drew them perfectly. Through the resolver there was
/// nothing to remember.
///
/// Both markers, because the setting is `pane` and `decoration` is still
/// read: a user.lua written before the rename still names a style, and a
/// name that has to resolve is a name this has to check. `everywhere` sees
/// only the shipped scripts, so what this really pins is that the alias in
/// `config.lua` cannot be left pointing at a style that was deleted.
#[test]
fn every_pane_style_named_is_one_that_ships() {
    let ships = crate::decoration::ships();
    // The walk before anything is decided by it: a catalogue that came back
    // empty would pass every name below by having no opinion, which is the
    // one way this can be green and mean nothing.
    assert!(
        ships.len() >= 8,
        "this build offers {} styles, which is fewer than the eight that \
         `config.lua` documents -- the walk is broken, not the configuration",
        ships.len()
    );
    let mut asked = everywhere("pane =");
    asked.extend(everywhere("decoration ="));
    assert!(
        !asked.is_empty(),
        "no pane styles found; the scan is broken"
    );
    for (file, line, name) in asked {
        // `none` is a real setting and draws no frame at all, deliberately.
        assert!(
            name == "none" || ships.iter().any(|offered| offered.name == name),
            "{file}:{line} asks for pane style {name:?}, and this build ships no style \
             of that name -- it ships {:?}",
            ships
                .iter()
                .map(|offered| offered.name.as_str())
                .collect::<Vec<_>>()
        );
    }
}

/// **Every key a shipped script binds is spelled the way a key arrives.**
///
/// A binding is a table lookup on a string. `input::combo_for` builds that
/// string from `xkb::keysym_get_name` and `normalise_combo` lowercases it,
/// so a binding whose key is not that exact spelling is not a binding at
/// all -- it is an entry in a table nothing will ever look up, with no
/// warning at load and nothing at the press but a `no script has bound
/// this` at info.
///
/// Round-tripped rather than merely resolved: `keysym_from_name` is
/// forgiving and `keysym_get_name` is not, and it is the second one that
/// decides what a key is called at run time.
///
/// Only the bindings written as literals after `sol.bind(`. `workspaces.lua`
/// builds nine of them from a loop counter and they cannot be read from the
/// text -- see this module's header -- and `scrolling.lua` binds through a
/// local `bind` helper this marker does not match.
///
/// And it checks the *spelling*, not that the key can be reached. Those are
/// not the same question: `super+shift+1` is spelled perfectly, and until
/// #121 no key on a `us` keyboard produced it, because `shift+1` arrives as
/// `exclam` and that was the only name a press was looked up by.
/// Reachability is `every_shipped_binding_is_reachable_on_us` below, which
/// reads the bindings from the loaded scripts rather than from the text and
/// so sees the loop-built and helper-built ones this one skips.
#[test]
fn every_key_bound_is_spelled_the_way_it_arrives() {
    use smithay::input::keyboard::xkb;

    let bound = everywhere("sol.bind(");
    assert!(!bound.is_empty(), "no bindings found; the scan is broken");
    for (file, line, combo) in bound {
        let combo = normalise_combo(&combo);
        let Some(key) = combo.rsplit('+').next() else {
            continue;
        };
        // A combo built by concatenation -- `"super+" .. index` -- reaches
        // this as a bare `super+` with nothing after it. There is no key
        // here to check; the header says so.
        if key.is_empty() {
            continue;
        }
        let keysym = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
        assert!(
            keysym != xkb::keysyms::KEY_NoSymbol.into(),
            "{file}:{line} binds {combo:?}, and xkb has no key called {key:?}"
        );
        let canonical = xkb::keysym_get_name(keysym).to_ascii_lowercase();
        assert_eq!(
            canonical, key,
            "{file}:{line} binds {combo:?}, but that key arrives called {canonical:?} -- \
             the binding would never fire"
        );
    }
}

/// A `us` keymap, compiled from the real rules, and its modifier indices.
///
/// Real rather than described, because the question these tests ask is
/// what xkb does and not what anyone believes it does.
fn us_keymap() -> (smithay::input::keyboard::xkb::Keymap, UsModifiers) {
    use smithay::input::keyboard::xkb;

    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let Some(keymap) = xkb::Keymap::new_from_names(
        &context,
        "",
        "",
        "us",
        "",
        None,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    ) else {
        // No xkb rules on this machine means no keymap to ask, and a test
        // that invented an answer here would be worse than one that says
        // it could not look.
        panic!("no `us` keymap; xkb data is missing, so this proves nothing");
    };
    let modifiers = UsModifiers {
        shift: keymap.mod_get_index(xkb::MOD_NAME_SHIFT),
        ctrl: keymap.mod_get_index(xkb::MOD_NAME_CTRL),
        alt: keymap.mod_get_index(xkb::MOD_NAME_ALT),
        logo: keymap.mod_get_index(xkb::MOD_NAME_LOGO),
    };
    (keymap, modifiers)
}

/// Where `us_keymap` keeps each modifier the combination syntax can name.
struct UsModifiers {
    shift: u32,
    ctrl: u32,
    alt: u32,
    logo: u32,
}

/// Every name a press of xkb keycode `code` answers to while `held` is
/// down -- the list `input::keyboard` tries bindings in.
///
/// Built the way that filter builds it: `modified` is `key_get_one_sym`
/// under the held modifiers, which is what smithay's `modified_sym` calls,
/// and `raw` is the key's first keysym at level 0 of its layout, which is
/// what the filter's `raw_latin_sym_or_raw_current_sym` comes to on a
/// keymap with one layout -- there is no other to look in, and `us_keymap`
/// makes no other kind. The filter itself, on two layouts, is
/// `input::tests::every_shipped_binding_fires_on_both_groups_of_us_ru`.
/// Then the same
/// `input::combos_for`, so the string compared below is the string the
/// compositor looks up and not a second opinion of it.
fn names_for(
    keymap: &smithay::input::keyboard::xkb::Keymap,
    indices: &UsModifiers,
    code: u32,
    held: &smithay::input::keyboard::ModifiersState,
) -> Vec<String> {
    use smithay::input::keyboard::xkb;

    let mut mask = 0;
    for (on, index) in [
        (held.shift, indices.shift),
        (held.ctrl, indices.ctrl),
        (held.alt, indices.alt),
        (held.logo, indices.logo),
    ] {
        if on {
            mask |= 1 << index;
        }
    }
    let mut state = xkb::State::new(keymap);
    state.update_mask(mask, 0, 0, 0, 0, 0);
    let code = xkb::Keycode::new(code);
    let modified = state.key_get_one_sym(code);
    let raw = keymap
        .key_get_syms_by_level(code, state.key_get_layout(code), 0)
        .first()
        .copied();
    crate::input::combos_for(held, modified, raw)
}

/// The modifiers a canonical combination names, as a keyboard would hold
/// them.
fn held_for(combo: &str) -> smithay::input::keyboard::ModifiersState {
    let named: Vec<&str> = combo.split('+').collect();
    let modifiers = named
        .split_last()
        .map(|(_, modifiers)| modifiers)
        .unwrap_or_default();
    smithay::input::keyboard::ModifiersState {
        ctrl: modifiers.contains(&"ctrl"),
        alt: modifiers.contains(&"alt"),
        shift: modifiers.contains(&"shift"),
        logo: modifiers.contains(&"super"),
        ..Default::default()
    }
}

/// **The two height binds in `tiling.lua` are combinations a `us` keyboard
/// can actually produce.**
///
/// Reachability, which the spelling test above deliberately does not check:
/// it asks whether a key is *spelled* the way xkb names it, and
/// `underscore` passes that while being a combination no one can press
/// with the modifiers the binding also names.
///
/// So this goes the other way round. It starts from the physical keys --
/// `AE11` and `AE12`, the `-` and `=` of the top row, which are evdev 12
/// and 13 and so xkb keycodes 20 and 21 -- presses them under the modifiers
/// the bindings name, and asks `names_for` what that press is called.
///
/// These were bound on ctrl because until #121 the shift spelling was
/// dead: shift+`-` arrives as `underscore`, and that was the only name a
/// press had. Ctrl selects no shift level, so it is right before that fix
/// and after it. The second half pins the fix itself for this key: the
/// spelling #120 started with is now one a press answers to, so the next
/// reader who tidies these back to shift gets a working binding rather
/// than a dead one.
#[test]
fn every_height_bind_is_a_key_that_arrives() {
    use smithay::input::keyboard::ModifiersState;

    let (keymap, indices) = us_keymap();
    let minus = 20;
    let equal = 21;
    let with_ctrl = ModifiersState {
        ctrl: true,
        logo: true,
        ..Default::default()
    };
    let with_shift = ModifiersState {
        shift: true,
        logo: true,
        ..Default::default()
    };

    // What `tiling.lua` binds, and what the keyboard sends. These have to
    // be the same string or the binding is an entry in a table nothing
    // looks up.
    for (code, bound) in [(minus, "super+ctrl+minus"), (equal, "super+ctrl+equal")] {
        let sent = names_for(&keymap, &indices, code, &with_ctrl);
        assert!(
            sent.contains(&normalise_combo(bound)),
            "`tiling.lua` binds {bound:?}, and that keypress answers only to {sent:?}"
        );
    }

    // And the spelling it started as, which #121 made reachable. Shift
    // still changes the keysym -- the press is called `underscore` first
    // -- but it also answers to the key it was.
    for (code, shifted, bound) in [
        (minus, "super+shift+underscore", "super+shift+minus"),
        (equal, "super+shift+plus", "super+shift+equal"),
    ] {
        let sent = names_for(&keymap, &indices, code, &with_shift);
        assert_eq!(
            sent,
            [normalise_combo(shifted), normalise_combo(bound)],
            "shift on this key should arrive as {shifted:?} and fall back to \
             {bound:?}, modified spelling first"
        );
    }
}

/// **The held volume and brightness keys repeat, and the laptop keys fire
/// at the lock -- and nothing else that ships does either** (#224).
///
/// Mute, the media keys and Print do not repeat: a toggle or a skip
/// repeating at 25 a second is a key that cannot be pressed once. Print
/// does not fire at the lock, which has nothing on screen worth keeping.
/// The second half is the one that matters most: every binding allowed at
/// the lock is one more thing a lock screen does, so the list of them is
/// pinned whole, and a shipped binding that starts firing there fails this
/// until it is added on purpose.
#[test]
fn the_held_volume_keys_repeat_and_only_the_laptop_keys_fire_at_the_lock() {
    let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
    let directory = std::env::temp_dir().join("solium-shipped-binding-options");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("creating the entry directory");
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!("package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"),
    )
    .expect("writing the entry point");
    let scripts = Scripts::load(&entry).expect("loading the shipped init.lua");
    let bindings = scripts.bindings();
    let with = |asked: fn(&super::Binding) -> bool| -> Vec<&str> {
        bindings
            .iter()
            .filter(|binding| asked(binding))
            .map(|binding| binding.combo.as_str())
            .collect()
    };
    assert_eq!(
        with(|binding| binding.repeating),
        vec![
            "shift+xf86audiolowervolume",
            "shift+xf86audioraisevolume",
            "xf86audiolowervolume",
            "xf86audioraisevolume",
            "xf86monbrightnessdown",
            "xf86monbrightnessup",
        ],
        "the shipped bindings that repeat while held"
    );
    assert_eq!(
        with(|binding| binding.locked),
        vec![
            "shift+xf86audiolowervolume",
            "shift+xf86audioraisevolume",
            "xf86audiolowervolume",
            "xf86audiomicmute",
            "xf86audiomute",
            "xf86audionext",
            "xf86audiopause",
            "xf86audioplay",
            "xf86audioprev",
            "xf86audioraisevolume",
            "xf86monbrightnessdown",
            "xf86monbrightnessup",
        ],
        "the shipped bindings that fire at the lock"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The shipped media keys act on Solium's own media, and spawn
/// `playerctl` only when it follows no players** (#379).
#[test]
fn the_shipped_media_keys_act_on_the_active_player_and_fall_back_to_playerctl() {
    let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
    let directory = std::env::temp_dir().join("solium-shipped-media-keys");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("creating the entry directory");
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!("package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"),
    )
    .expect("writing the entry point");
    let mut scripts = Scripts::load(&entry).expect("loading the shipped init.lua");
    let following = super::Snapshot {
        media: crate::services::media::Seen {
            view: std::rc::Rc::new(crate::services::media::View {
                available: true,
                ..crate::services::media::View::default()
            }),
            now: std::time::Duration::ZERO,
        },
        ..super::Snapshot::default()
    };
    let acted = |outcome: &super::Outcome| -> Vec<String> {
        outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                super::Command::Act { action, .. } => Some(action.clone()),
                _ => None,
            })
            .collect()
    };
    let spawned = |outcome: &super::Outcome| -> Vec<(String, Vec<String>)> {
        outcome
            .commands
            .iter()
            .filter_map(|command| match command {
                super::Command::Spawn { program, args } => Some((program.clone(), args.clone())),
                _ => None,
            })
            .collect()
    };
    for (key, action) in [
        ("XF86AudioPlay", "media.play_pause"),
        ("XF86AudioPause", "media.play_pause"),
        ("XF86AudioNext", "media.next"),
        ("XF86AudioPrev", "media.previous"),
    ] {
        let outcome = scripts.key(key, following.clone());
        assert_eq!(acted(&outcome), [action], "{key}");
        assert!(spawned(&outcome).is_empty(), "{key} spawned nothing");
    }
    let alone = scripts.key("XF86AudioNext", super::Snapshot::default());
    assert!(acted(&alone).is_empty());
    assert_eq!(
        spawned(&alone),
        [("playerctl".to_owned(), vec!["next".to_owned()])]
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// **Every key a shipped script binds can be pressed on a `us` keyboard.**
///
/// The question the spelling test cannot ask. This one loads the shipped
/// `init.lua` -- so the bindings are what the running compositor has, the
/// nine `super+shift+N` built by a loop in `workspaces.lua` and the keys
/// `scrolling.lua` binds through its own helper included -- and for each,
/// looks for a physical key that, held under the modifiers the binding
/// names, answers to that exact string.
///
/// Eleven failed this before #121: all nine send-to-workspace keys and both
/// `super+shift+bracketleft`/`bracketright`, because shift turned the key
/// into `exclam` or `braceleft` and that was the only name looked up.
///
/// `package.path` is written by the entry rather than left to
/// `Scripts::load`, which prepends the developer's own `~/.config/solium`
/// and would make this a test of somebody's configuration -- the trap the
/// layout harness below documents.
#[test]
fn every_shipped_binding_is_reachable_on_us() {
    let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/lua");
    let directory = std::env::temp_dir().join("solium-reachable");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("creating the entry directory");
    let entry = directory.join("init.lua");
    std::fs::write(
        &entry,
        format!("package.path = {shipped:?} .. \"/?.lua\"\ndofile({shipped:?} .. \"/init.lua\")\n"),
    )
    .expect("writing the entry point");
    let scripts = Scripts::load(&entry).expect("loading the shipped init.lua");
    let bound: Vec<String> = scripts
        .bindings()
        .into_iter()
        .map(|binding| binding.combo)
        .collect();

    // The ones this is for, by name, so a load that quietly bound nothing
    // -- or a scan that lost the loop -- cannot pass by having nothing to
    // check.
    let expected = (1..=9).map(|index| format!("shift+super+{index}")).chain([
        "shift+super+bracketleft".to_owned(),
        "shift+super+bracketright".to_owned(),
        // #150's, from `direction.lua` and `modes.lua` by way of `init.lua`.
        "super+left".to_owned(),
        "super+l".to_owned(),
        // #281's lock key, from `init.lua`.
        "ctrl+super+l".to_owned(),
        "shift+super+down".to_owned(),
        "shift+super+h".to_owned(),
        "alt+super+k".to_owned(),
        // #151's laptop keys, from `init.lua`: every key a laptop's
        // function row has, so on a `us` keyboard each must be a key.
        "xf86audioraisevolume".to_owned(),
        "xf86audiolowervolume".to_owned(),
        "shift+xf86audioraisevolume".to_owned(),
        "xf86audiomute".to_owned(),
        "xf86audiomicmute".to_owned(),
        "xf86monbrightnessup".to_owned(),
        "xf86monbrightnessdown".to_owned(),
        "xf86audioplay".to_owned(),
        "xf86audionext".to_owned(),
        "xf86audioprev".to_owned(),
        "print".to_owned(),
        "shift+print".to_owned(),
        "super+f".to_owned(),
        "shift+super+m".to_owned(),
        "shift+super+space".to_owned(),
    ]);
    for combo in expected {
        assert!(
            bound.contains(&combo),
            "the shipped scripts no longer bind {combo:?}; they bind {bound:?}"
        );
    }

    // Every one that fails, not the first: eleven at once is a different
    // bug from one, and a report that stops at the first cannot say which.
    let (keymap, indices) = us_keymap();
    let codes = keymap.min_keycode().raw()..=keymap.max_keycode().raw();
    let dead: Vec<&String> = bound
        .iter()
        .filter(|combo| {
            let combo = normalise_combo(combo);
            let held = held_for(&combo);
            !codes
                .clone()
                .any(|code| names_for(&keymap, &indices, code, &held).contains(&combo))
        })
        .collect();
    assert!(
        dead.is_empty(),
        "shipped scripts bind {dead:?}, and for each no key on a `us` keyboard, held \
         under the modifiers it names, answers to that name -- they can never fire"
    );
}
