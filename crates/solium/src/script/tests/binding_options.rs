//! `sol.bind`'s options (#224): the third argument is a note, or a table that
//! holds the note and what else the binding asks for.

use super::Scripts;

/// `text` loaded as a configuration of its own, or what refused it.
fn load(name: &str, text: &str) -> anyhow::Result<Scripts> {
    let directory = std::env::temp_dir().join(format!(
        "solium-binding-options-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&directory);
    let entry = directory.join("init.lua");
    std::fs::write(&entry, text).expect("writing the test script");
    let loaded = Scripts::load(&entry);
    let _ = std::fs::remove_dir_all(&directory);
    loaded
}

/// **A binding takes its note and what it asks for as one table**, a
/// plain note as it always has, and a binding bound again with nothing
/// asked asks for nothing: the later call wins whole.
#[test]
fn a_binding_takes_its_note_and_what_it_asks_for_as_a_table() {
    let scripts = load(
        "table",
        r#"
        local function nothing() end
        sol.bind("XF86AudioRaiseVolume", nothing,
            { note = "laptop keys", repeating = true, locked = true })
        sol.bind("XF86AudioMute", nothing, { locked = true })
        sol.bind("super+q", nothing, "a plain note")
        sol.bind("super+w", nothing)
        sol.bind("XF86AudioLowerVolume", nothing, { repeating = true, locked = true })
        sol.bind("XF86AudioLowerVolume", nothing)
        "#,
    )
    .expect("loading the bindings");
    let reported: Vec<(String, Option<String>, bool, bool)> = scripts
        .bindings()
        .into_iter()
        .map(|binding| {
            (
                binding.combo,
                binding.note,
                binding.repeating,
                binding.locked,
            )
        })
        .collect();
    let row = |combo: &str, note: Option<&str>, repeating, locked| {
        (combo.to_owned(), note.map(str::to_owned), repeating, locked)
    };
    assert_eq!(
        reported,
        vec![
            row("super+q", Some("a plain note"), false, false),
            row("super+w", None, false, false),
            row("xf86audiolowervolume", None, false, false),
            row("xf86audiomute", None, false, true),
            row("xf86audioraisevolume", Some("laptop keys"), true, true),
        ],
        "(combination, note, repeating, locked)"
    );
}

/// **An option a binding does not have is refused, by name**, and so is
/// one that is not true or false. `repeat` is Lua's own word and can only
/// be written as `["repeat"]`, which is the likeliest slip of all, so its
/// refusal names the spelling that works.
#[test]
fn an_option_a_binding_does_not_have_is_refused_by_name() {
    for (case, options, says) in [
        ("repeat", r#"{ ["repeat"] = true }"#, "repeating"),
        ("unknown", "{ loud = true }", "loud"),
        ("not-a-boolean", r#"{ repeating = "yes" }"#, "repeating"),
        ("note-not-text", "{ note = 3 }", "note"),
    ] {
        let error = load(
            case,
            &format!("sol.bind(\"super+q\", function() end, {options})\n"),
        )
        .err()
        .map(|error| format!("{error:#}"))
        .unwrap_or_default();
        assert!(
            error.contains(says),
            "{case}: the configuration was not refused naming {says:?}: {error:?}"
        );
    }
}
