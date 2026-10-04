# Where Solium ends and a shell begins

Solium is the compositor. A shell — the bar, the dock, the launcher — is a
project of its own, in its own repository, and it runs **inside** Solium as its
configuration: its QML is hosted in the compositor's own engine, beside the
window frames, the pointer and every other scene the compositor draws. That is
the line, and it is not where a Wayland tutorial would put it.

**Where it is today:**

- A shell is named in the configuration, `shell = { scene = "<its root QML
  file>" }`, and hosted in-process through `sol.surface`, by `lua/shell.lua`.
  The shipped configuration names none.
- A shell is QML written against Solium's own API: `import Solium` for
  `Theme`, `Keyboard`, `Monitors`, `Windows`, `WindowList`, `Grab` and the
  attached `Solium` object
  (`Solium.monitor`, `Solium.input`, `Solium.surface.reserve`,
  `Solium.keyboard`), and `Solium.send` for what it asks Lua to do
  ([below](#what-a-hosted-shell-is-given)). Quickshell support was removed
  (#172); Quickshell itself may add Solium support on its own side.
- A shell that runs as its own program — Waybar, a Quickshell instance run
  on its own, any `wlr-layer-shell` panel — is supported too, as an ordinary
  client. It needs nothing from the configuration and gets nothing from the
  compositor's engine.

The rest of this file is why hosting is the design, how to host one, and
exactly what a hosted shell is given and what it is not.

## The requirement that decides the architecture

> An object should be able to move from the dock into a window's titlebar.

Not "look similar in both". *Move* — one object, travelling, unbroken.

That single requirement rules out the conventional answer. If the shell is a
separate process painting its own pixels, then a dock icon and a titlebar are
two scene graphs in two processes with two stylesheets, and an object cannot
cross between them; the best available is a fake — dissolve one, appear in the
other — which is exactly the kind of thing this project exists not to build.

The same requirement, in a weaker form, rules it out again: "the shell changes
colour, so the decorations change too" is trivial when there is one theme
object, and a synchronisation problem forever when there are two.

## So: one engine

The compositor hosts **one QML engine**. Window decorations are scenes in it,
and so is a hosted shell's bar, dock and launcher. A scene that imports
`Solium` gets the same `Solium.Theme` singleton the frames read, because there
is only one of it.

That gives, in order of how hard they would otherwise be:

- **One design system.** `Solium.Theme`
  (`crates/solium/qml/Solium/Theme.qml`) is what the window frames and the
  loading window are drawn with, and any hosted scene that imports `Solium`
  can read it. Changing a colour there changes the titlebars and a shell that
  uses it together, with no rebuild, because they read the same object and
  not two copies. A shell brought in with a theme of its own keeps its own
  until it is pointed at this one. Two of the compositor's own scenes keep
  fixed colours instead: the fallback pointer, which has to stay legible over
  whatever a client drew, and the default wallpaper.
- **Objects that travel, planned.** The aim is that an item can leave the
  dock and land in a titlebar, one object the whole way. None of it is built,
  and it will not be a reparent: every scene has a `QQuickWindow` of its own,
  so each is its own scene graph, and an item cannot move from one to
  another. The plan is a flight or a morph: a third, live instance of the same
  component, drawn over both ends while they are hidden and moved on the
  compositor's clock. One engine is what lets that instance be the component
  itself, with the same theme, rather than a picture of it.
- **No protocol for shell geometry, planned.** A hosted dock's icon
  rectangles need not be published to the compositor, because the
  compositor's engine already has them. The plan is for a scene to name an
  item as an anchor (`Solium.region`) and for an animation to aim at that
  name, read again on every frame, so a genie follows an icon while it moves
  and can never animate against a stale copy. Nothing of that is built yet;
  it is tracked under Later in
  [#169](https://github.com/Lilium-Linux/solium/issues/169). Today a genie
  aims at a window, a `sol.surface` scene or a fixed rectangle
  ([modes.md](modes.md)). Only a dock that runs as its own program would need
  a protocol to hand its rectangles over.

This is the arrangement Apple has, and it is unavailable to anyone configuring
an existing compositor. It is the reason for writing one.

## Hosting a shell

### Naming it

In `~/.config/solium/user.lua` (or your own `config.lua`):

```lua
return {
    shell = { scene = "~/.config/solium/shell/shell.qml" },
}
```

`~` is expanded. The scene is drawn on every monitor, one instance each, over
the windows and across the whole monitor; each instance reads its own monitor
as `Solium.monitor`. `shell = { on = "primary" }`, or a connector name, draws
one instance instead
(`script::tests::the_shell_is_on_every_monitor_unless_the_configuration_names_one`).
It takes the pointer only where its items take input, as "Clickable only
where it takes input" under
[What a hosted shell is given](#what-a-hosted-shell-is-given) says; everywhere
else the pointer goes to the windows under it. `super+shift+r` picks up
a change to the setting, and tries again a scene that would not load, so a
typo in the shell costs a reload rather than the session
(`state::tests::real_client::a_reload_tries_again_a_scene_that_would_not_load`).
Editing a file under the scene's own directory
rebuilds the scene with no reload at all: that is checked every half second
while frames are being drawn.
`false`, the default, hosts nothing, and taking the setting out takes
the shell away on the next reload.

A reload does not rebuild a hosted scene. `sol.surface` declared again with the
same scene file writes the properties that changed into the live scene, so an
open popup, a running animation or a half-typed query survives
`super+shift+r`; only a different scene file builds the scene again
(`scripted::tests::a_redeclared_property_is_written_into_the_live_scene`).

`SOLIUM_SHELL_SCENE=<file>` overrides the setting for one run. It is how
`dev/run-shell.sh` hosts a shell under development without touching the
configuration you normally run; see `dev/README.md`.

### Installing one next to your configuration

A shell is its own repository. Clone it beside your configuration and name its
root file:

```sh
git clone <the shell's repository> ~/.config/solium/shell
```

```lua
shell = { scene = "~/.config/solium/shell/shell.qml" },
```

A shell that imports its own files by relative path needs nothing more.

`solium --check-qml <file>` loads one file without starting a compositor and
prints `ok` or what Qt reported, which is the quick way through a chain of
"type X unavailable" errors. It exits 0 either way.

## What a hosted shell is given

**A canvas.** One scene per monitor, each the size of its whole monitor, on
the `top` layer: over the windows, under a layer-shell client's own `top` layer
surfaces, and covered by a fullscreen window unless `fullscreen.covers` says
otherwise
(`scripted::tests::a_surface_on_every_monitor_has_one_live_scene_per_monitor`).
It gets pointer motion, every mouse button as itself, the wheel, and the
modifiers held, so a `MouseArea` or a `WheelHandler` works as it does anywhere
(`qml::hosted::tests::a_right_press_reaches_a_mouse_area_as_the_right_button`,
`qml::hosted::tests::the_wheel_reaches_a_wheel_handler_with_its_angle`).
Each event carries its time, so a double press is a double-click and a
`TapHandler` counts its taps, by Qt's own double-click interval and distance
(`qml::hosted::tests::a_double_press_on_a_mouse_area_is_one_double_click`,
`qml::hosted::tests::a_tap_handler_counts_taps_by_when_they_happened`,
`qml::hosted::tests::two_presses_further_apart_than_the_interval_are_two_single_clicks`).
The wheel with `super` held stays the compositor's
(`state::tests::real_client::reflow_on_close::hosted::super_and_the_wheel_stay_the_compositors_over_a_scene`),
and while the session is locked none of it reaches the scene
(`state::tests::real_client::lock_focus::the_wheel_over_a_hosted_scene_is_not_the_scenes_while_locked`,
`state::tests::real_client::lock_focus::a_button_let_go_behind_the_lock_is_not_held_after_it`).
A press the scene held when the session locked is cancelled, not clicked: a
`MouseArea`, a `Button` or a `TapHandler` holding it hears `canceled`
(`state::tests::real_client::lock_focus::the_lock_lets_go_of_a_hosted_scenes_press_and_its_hover`,
`qml::hosted::tests::a_press_let_go_of_unseen_is_cancelled_not_clicked`).
A monitor that arrives gets its instance there and then, and one that goes
takes its instance with it
(`scripted::tests::an_instance_goes_with_its_monitor_and_comes_with_a_new_one`).
It reads where it is from `Solium.monitor`, below; `screenInfo` is gone.

**Clickable only where it takes input.** The compositor asks the live item
tree under the pointer, so a point is the scene's only where a visible,
enabled item that is not fully transparent takes input: any item that accepts
mouse buttons — a `MouseArea`, a Qt Quick Controls control such as a `Button`
or a `TextField`, a `TextInput` or `TextEdit`, a `Flickable` (a `ListView`
among them), a `PathView`, a `MultiPointTouchArea` — a pointer handler other
than `HoverHandler`, such as a `TapHandler` or a `DragHandler`, a link in a
`Text` with an `onLinkActivated` handler, or an item marked
`Solium.input: true`.
An item at opacity 0, itself or through an ancestor, takes nothing, though Qt
would still deliver to it
(`qml::hosted::tests::the_item_tree_decides_what_a_point_claims`). The rest of
a `Text` takes no press, nor does a link nothing handles, and a plain
label takes nothing at all
(`qml::hosted::tests::a_text_takes_a_press_only_on_a_link`).
`Solium.input: "hover"` takes the pointer's motion and
leaves presses to what is under it, which is how an edge strip reveals a
hidden dock. A `HoverHandler` on an item that takes no press itself takes only
the motion in the same way, and a press there goes to what is under it.
`Solium.input: false` takes an item out. An item's shape counts,
through its `containmentMask`, so a rounded popup's corners pass clicks
through; a mask written in QML has to be typed,
`function contains(point: point): bool`, or Qt ignores it, and it answers for
the whole item, its bounds too. A disabled handler takes nothing
(`qml::hosted::tests::a_disabled_handler_claims_nothing`), and an open Qt
Quick Controls popup, a `Popup`, a `Menu` or a `ComboBox`'s list, takes the
points it is drawn on, though Qt draws it in the window's overlay rather than
under the scene's root
(`qml::hosted::tests::an_open_controls_popup_claims_its_press`), and
`Solium.input` written on the `Popup` is that item's
(`qml::hosted::tests::solium_input_on_a_controls_popup_is_its_items`). A
popup is laid out against the whole scene, so a `Menu` opens where it is
asked to
(`qml::hosted::tests::a_controls_popup_is_laid_out_against_the_whole_scene`),
and a modal one takes every point of its scene while it is open, as Qt's own
modality does
(`qml::hosted::tests::a_modal_popup_takes_every_point_of_its_scene_while_it_is_open`).
Everywhere else the window under the shell gets the press and the wheel
(`state::tests::real_client::reflow_on_close::hosted::the_wheel_where_the_shell_draws_nothing_is_the_windows_under_it`),
and where the scene takes a press, the window under it does not have the
pointer, nor the keyboard when focus follows the mouse. A press the scene
took is its until every button is up, wherever the pointer goes meanwhile,
and the pointer keeps the shape it had at the press
(`state::tests::real_client::reflow_on_close::hosted::a_press_where_the_shell_draws_nothing_reaches_the_window_under_it`,
`state::tests::real_client::reflow_on_close::hosted::a_release_after_dragging_off_a_shell_button_reaches_the_scene`,
`state::tests::real_client::reflow_on_close::hosted::a_press_a_scene_holds_keeps_its_shape_over_a_resize_border`,
`state::tests::real_client::reflow_on_close::hosted::focus_follows_the_mouse_through_a_shell_only_where_it_takes_no_press`,
`qml::hosted::tests::the_item_tree_decides_what_a_point_claims`).

**Popups that hold the pointer.** `Grab { name: "tray-menu"; target: menu;
active: menu.visible; onDismissed: menu.close() }` holds the pointer for the
scene while it is active
(`qml::hosted::tests::a_grab_is_held_while_active_and_dismissed_on_request`):
motion and the wheel go to it wherever the pointer is, and no window has the
pointer, nor its frame, nor the keyboard when focus follows the mouse
(`state::tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_the_pointer_is_the_scenes`,
`state::tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_the_wheel_is_the_scenes`,
`state::tests::real_client::reflow_on_close::hosted::while_a_grab_is_held_no_window_takes_focus_frame_or_cursor_from_the_pointer`).
Its `target` is an item or a Qt Quick Controls `Popup`, a `Menu` among them;
anything else has no points, so every press dismisses the grab, and the log
says so
(`qml::hosted::tests::a_controls_popup_is_a_grabs_target`).
A press inside its target is the scene's, even where no item there takes
input
(`state::tests::real_client::reflow_on_close::hosted::a_press_inside_the_grab_target_reaches_the_scene`).
With more than one `Grab` of a scene active, a press inside any of their
targets is the scene's, and the newest one's `name` is the one `outside_click`
is looked up by
(`qml::hosted::tests::a_scenes_newest_grab_is_reported_and_every_active_one_counts`).
A press outside it dismisses it: every active `Grab` of the scene hears
`dismissed`, newest first
(`qml::hosted::tests::a_scenes_newest_grab_is_reported_and_every_active_one_counts`),
and the press is then swallowed, its release with it, or passed on to
whatever is under it when `outside_click` says `"pass"`
(`state::tests::real_client::reflow_on_close::hosted::a_press_outside_a_grab_dismisses_it_and_is_swallowed_by_default`,
`state::tests::real_client::reflow_on_close::hosted::with_outside_click_pass_the_dismissing_press_reaches_the_window_under_it`).
However a grab ends, swallowed, passed, or let go of by its scene, the window
under the pointer has the pointer back at once, or, when the scene holds a
press as it ends, at that press's release, so a click there with no motion
before it reaches it
(`state::tests::real_client::reflow_on_close::hosted::a_swallowed_outside_press_gives_the_pointer_back_to_the_window_under_it`,
`state::tests::real_client::reflow_on_close::hosted::a_popup_that_closes_by_itself_gives_the_pointer_back_to_the_window_under_it`,
`state::tests::real_client::reflow_on_close::hosted::a_surface_taken_away_gives_the_pointer_back_to_the_window_under_it`,
`state::tests::real_client::reflow_on_close::hosted::a_popup_closed_during_a_press_inside_it_gives_the_pointer_back_at_the_release`).
`shell.outside_click` in `config.lua` is the hosted shell's, `"swallow"` by
default, and a table names grabs, `{ default = "swallow", ["tray-menu"] =
"pass" }`
(`script::tests::the_shell_takes_its_outside_click_from_the_configuration`,
`state::tests::real_client::reflow_on_close::hosted::a_policy_named_for_the_grab_beats_the_default`).
Any other value, in the word or in the table, fails the configuration's load
(`script::tests::an_unknown_outside_click_is_refused`).
One grab is held at a time, and one another scene takes dismisses it
(`state::tests::real_client::reflow_on_close::hosted::a_grab_another_scene_takes_dismisses_the_one_held`).
A surface the pointer does not reach, declared `interactive = false`, holds
no grab, and its scene's grabs dismiss none
(`state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_grab`);
one declared so while it holds a grab hears it dismissed
(`state::tests::real_client::reflow_on_close::hosted::a_surface_declared_again_out_of_the_pointers_reach_dismisses_the_grab_it_held`).
A press the scene already held when its grab began keeps the pointer until
its release, so a button whose press opens a menu is let go of as usual
(`state::tests::real_client::reflow_on_close::hosted::a_press_held_when_a_grab_begins_keeps_the_pointer_until_its_release`).
A game's pointer lock is let go while a grab is held, at once even while a
press of the game's own still keeps the pointer on it, and comes back after it
(`state::tests::real_client::reflow_on_close::hosted::a_grab_suspends_a_pointer_lock_and_the_lock_comes_back_after`,
`state::tests::real_client::reflow_on_close::hosted::a_grab_begun_during_a_press_on_a_locked_window_lets_go_of_the_lock_at_once`),
and locking the session dismisses it
(`state::tests::real_client::reflow_on_close::hosted::locking_the_session_dismisses_a_hosted_grab`).
No grab is held behind the lock; one a scene takes there is held once the
lock is gone
(`state::tests::real_client::lock_focus::a_grab_a_scene_takes_behind_the_lock_is_held_once_it_is_gone`).

**The keyboard, when an item asks.** `TextField { Solium.keyboard.wants:
activeFocus; Solium.keyboard.claims: [ "Escape", "Return", "Up", "Down" ] }`
takes the keyboard while it is visible and wants it
(`qml::hosted::tests::a_field_that_wants_the_keyboard_reports_its_claims`,
`qml::hosted::tests::an_invisible_field_does_not_hold_the_keyboard`). When
more than one visible item of a scene wants it, the one with active focus
holds it, else the one that came to want it last, and only that item's
`claims` count
(`qml::hosted::tests::the_holder_is_the_focused_wanting_item_else_the_one_that_wanted_last`).
Every hosted scene's window is active from the start, so `focus: true` gives a
field `activeFocus`; bind `wants` to it, because the compositor takes the
keyboard back by taking that focus away. Whatever `wants` is bound to, a
scene the keyboard was taken back from does not take it again until an item
asks anew: it comes to want it, is shown again, or takes active focus again
(`qml::hosted::tests::a_scene_let_go_of_takes_the_keyboard_again_only_when_asked_anew`).
Written on a container around the field, `wants` works too: the field inside
loses its focus when the keyboard is taken back, and taking it again asks
anew for the container
(`qml::hosted::tests::a_let_go_takes_the_focus_from_a_field_inside_the_container_that_wants_the_keyboard`).
Written on a Qt Quick Controls
`Popup`, which is no item, `Solium.keyboard` is the popup's own, its
`activeFocus` and its being open, so a search popup is `Popup { focus: true;
Solium.keyboard.wants: activeFocus; TextField { focus: true } }`
(`qml::hosted::tests::a_field_in_a_popup_that_wants_the_keyboard_takes_the_keys`);
on anything that is neither an item nor a `Popup` it holds nothing, and the
log says so. Such a popup holds the pointer with a `Grab` and the keyboard at
once: a press outside it dismisses it and the window under the pointer has
the pointer back, and the keyboard once the field lets go
(`state::tests::real_client::reflow_on_close::hosted::a_search_popup_holding_the_pointer_and_the_keyboard_gives_both_back_when_dismissed`). The
window that had the keyboard loses it meanwhile, still reads as the focused
window and is drawn focused, and gets it back when the field lets go
(`state::tests::real_client::reflow_on_close::hosted::the_window_gets_the_keyboard_back_when_the_shell_lets_go`).
Text comes from the compositor's own keyboard state, so typing works with any
layout active, Russian included
(`input::tests::russian_typed_through_the_compositor_reaches_a_hosted_text_field`,
`input::tests::while_the_shell_holds_the_keyboard_russian_letters_reach_it_as_cyrillic`),
and with Control held a letter is named by the one your Latin layout has on
that key, as Qt names it, so `ctrl+a` and `ctrl+z` work with Russian active
(`input::tests::ctrl_a_selects_all_in_a_hosted_text_field_on_russian`),
and a key held repeats at the keyboard's own rate
(`input::tests::a_held_key_repeats_into_the_scene_at_the_keyboards_rate`,
`input::tests::a_held_key_noticed_late_still_repeats_at_the_keyboards_rate`),
unless your keymap says it does not, as it says of a modifier, AltGr among
them, and of a group toggle such as `grp:alt_shift_toggle`
(`input::tests::a_held_modifier_does_not_repeat_into_the_scene`,
`input::tests::a_held_group_toggle_does_not_repeat_into_the_scene`). A group
toggle never repeats, even on a key that does, as `grp:alt_space_toggle`'s
is
(`input::tests::a_held_group_toggle_on_space_does_not_repeat_into_the_scene`).
The compositor's bindings keep working, `super+q` on Russian among them,
except the keys the item claims, which are the field's
(`input::tests::an_unclaimed_super_binding_still_fires_on_russian_while_the_shell_holds_the_keyboard`,
`input::tests::a_claimed_key_reaches_the_scene_and_not_its_binding`); a claim
is spelled as `sol.bind` spells a key. `shell.keyboard.bindings` in
`config.lua` is `"except_claimed"` by default, which is that: every binding
but the claimed keys. `"all"` keeps every binding, claimed or not, and
`"none"` gives the shell every key
(`input::tests::with_bindings_all_a_claimed_binding_wins`,
`input::tests::with_bindings_none_even_super_bindings_reach_the_scene`,
`script::tests::the_shell_takes_its_keyboard_bindings_from_the_configuration`).
Any other value fails the load too
(`script::tests::an_unknown_keyboard_bindings_is_refused`),
and a reload that changes it applies from the next key, while the shell holds
the keyboard
(`state::tests::real_client::reflow_on_close::hosted::a_bindings_policy_redeclared_while_the_shell_holds_the_keyboard_applies_at_once`).
The Ctrl+Alt escapes always work
(`input::tests::the_escape_hatches_beat_a_shell_that_holds_the_keyboard`).
Clicking a window or `sol.focus` takes the keyboard back
(`state::tests::real_client::reflow_on_close::hosted::clicking_a_window_ends_the_shells_hold`,
`state::tests::real_client::reflow_on_close::hosted::sol_focus_ends_the_shells_hold`),
and so does locking the session; no scene holds it behind the lock, and one
that asks there has it once the lock is gone
(`state::tests::real_client::lock_focus::no_scene_holds_the_keyboard_while_the_session_is_locked`).
The pointer crossing a window does not take it back, even with focus
following the mouse
(`state::tests::real_client::reflow_on_close::hosted::the_pointer_crossing_a_window_does_not_end_the_shells_hold`).
One scene holds it at a time
(`state::tests::real_client::reflow_on_close::hosted::a_hold_another_scene_takes_returns_to_the_window_the_first_took_it_from`),
and a scene whose monitor is unplugged gives it back
(`state::tests::real_client::reflow_on_close::hosted::a_monitor_unplugged_while_its_scene_holds_the_keyboard_gives_it_back`).
A surface the pointer does not reach, declared `interactive = false`, holds
no keyboard, as it holds no grab, since nothing could click it to take the
keyboard back; one declared so while its scene holds it gives it back
(`state::tests::real_client::reflow_on_close::hosted::a_surface_the_pointer_does_not_reach_holds_no_keyboard`,
`state::tests::real_client::reflow_on_close::hosted::a_surface_declared_again_out_of_the_pointers_reach_gives_the_keyboard_back`).
The compositor's own Qt loads no input method from your session, so IBus and
the like do not run inside it
(`launch::tests::the_compositors_qt_takes_no_input_method_from_the_session`);
the programs it starts still get yours
(`launch::tests::a_spawned_program_gets_the_input_method_the_compositors_qt_does_not`).

**The compositor's clock and frames.** Its animations advance on the same
clock as every window transform, a running animation asks for the next frame,
and the scene is redrawn only when Qt says it changed. Qt is served between
frames too: a `Timer` fires on time on an idle desktop, and what Qt waits on a
descriptor for — a socket, an answer from another thread — arrives when it
is ready, with no frame drawn unless the scene changed. A `Timer` is on that
same clock, so one beside an animation nothing draws still fires.

**The `Solium` QML module.** `Theme` above all: the colours, fonts and
metrics the frames are drawn with. Everything `import Solium` brings is
written unqualified, as `Theme` is: the singletons `Theme`, `Keyboard`
("The keyboard, live", below), `Monitors` ("Its monitor, live", below) and
`Windows` ("Windows, live", below); the types `Grab` ("Popups that hold the
pointer", above) and `WindowList` ("Windows, live"); the pane-style types
`PaneStyle` and `Layer`, and the keyboard pill's `KeyboardPill` and
`KeyboardPillLayer` (the [panes README](../crates/solium/qml/panes/README.md)); and the attached
`Solium` object, which any item can read, with exactly four members:
`Solium.monitor`, the monitor this instance of the scene is on (below);
`Solium.input`, `true`, `false` or `"hover"` (above);
`Solium.surface.reserve.top`, `right`, `bottom` and `left` ("Room of its own",
below); and `Solium.keyboard.wants` and `claims` (above)
(`qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`).
Nothing else is public: `Insets`, `ClientTreatment` and `ClientShadow` are
internal, and the row types have no name, so a shell's own `Monitor.qml` is
not shadowed
(`qml::hosted::tests::a_shell_file_named_like_a_row_is_still_the_shells`),
and `Window` is still Qt Quick's in a scene that imports both
(`qml::hosted::tests::a_quick_window_is_still_qt_quicks_beside_the_windows_model`).
A `Theme.qml` of your own in `~/.config/solium/qml/Solium/` is meant to
override the shipped one, and does not yet: the shipped module is found first
([#88](https://github.com/Lilium-Linux/solium/issues/88)).
The attached `Solium` object and `Grab` belong to a hosted scene, which
`sol.surface` builds on a monitor. Elsewhere (a pane's layers, the loading
window, the fallback pointer) they build and do nothing: `Solium.monitor` is an
absent row with an empty `name` and `present: false`, `Solium.surface.reserve`
reserves nothing, and `Solium.keyboard` and a `Grab` hold nothing
(`qml::hosted::tests::a_scene_hosted_on_no_monitor_reads_an_absent_monitor`,
`qml::hosted::tests::an_unhosted_scene_may_bind_a_reserve_and_reserves_nothing`,
`qml::hosted::tests::an_unhosted_scene_may_bind_the_keyboard_and_holds_nothing`).
`Solium.input` changes nothing there either, since what reaches a pane's
layers is decided by the pane, not by the scene's item tree. `Theme` and
`Keyboard` are the same in every scene.

**Its monitor, live.** `Solium.monitor` is the row of the monitor this
instance is on: `name`, `whole` and `area` (rectangles in the global space,
`area` being the work area), `scale`, `transform`, `primary`, and `present`
(`valid` reads the same). It changes in place, once per frame and all at once,
when the monitor does, and a monitor that goes reads `present: false` and
keeps its name; the same monitor coming back is the same row again
(`qml::hosted::tests::a_published_monitor_reaches_solium_monitor_in_its_scene`).
Until the compositor publishes a monitor, its row carries only the `name`, and
`present` and `valid` read false
(`qml::hosted::tests::the_attached_type_shares_the_solium_uri_with_the_shipped_module`).
A scene that wants its own coordinates subtracts `whole.x` and `whole.y`.
`transform` is spelled as `sol.monitors()` spells it: `"normal"`, `"_90"`,
`"_180"`, `"_270"`, `"flipped"`, `"flipped90"`, `"flipped180"` or
`"flipped270"`, and not `"90"` or `"flipped-90"` as `sol.monitors{ ... }` takes
it (`models::monitors::tests::a_turned_monitor_row_names_its_transform_as_smithay_does`).
`Monitors` is every monitor, as a list model with the same roles and a
`count`, and `Monitors.get(name)` one of them; a changed value is one
`dataChanged` for that role alone
(`qml::hosted::tests::the_monitors_model_lists_every_row_and_changes_one_role_at_a_time`).
Rows also say what is `reserved` on each edge (`top`, `right`, `bottom` and
`left`: what the work area lost there, layer-shell zones and hosted reserves
together), whether the monitor's `power` is `"on"` or `"off"`, whether the
`pointer` is on it and whether it is the `active` one, the monitor in front of
you (`models::monitors::tests::a_monitor_row_says_what_is_reserved_and_where_the_pointer_is`).

**Windows, live.** `Windows` is every window, a list model built from the
compositor's own panes and updated in place once per frame: `id`, `title`,
`appId`, `pid`, `xwayland`, `monitor`, `workspace`, `focused`, `focusOrder`
(0 is the most recently focused, and a window that closes leaves no gap),
`urgent` (it asked for attention nobody could see, until it is focused),
`fullscreen` and `maximized` (as the compositor last set them, without
waiting for the application), `modal`,
`parent`, `state` (`loading` from the click, `shown`, `closing`) and
`onStage`
(`state::tests::real_client::reflow_on_close::hosted::a_window_row_carries_where_it_lives_and_its_focus`,
`state::tests::real_client::reflow_on_close::hosted::focus_order_is_most_recent_first`,
`state::tests::real_client::reflow_on_close::hosted::a_closed_window_leaves_no_gap_in_focus_order`,
`state::tests::real_client::reflow_on_close::keyboard_at_open::a_refused_activation_marks_the_window_urgent_until_it_is_focused`,
`state::tests::real_client::reflow_on_close::hosted::a_maximised_window_reads_maximized_at_once`).
A window is listed from the moment it is launched, before its application
draws, unless `loading.reserves_a_slot` is off; until then it has no `pid`,
which reads `-1`
(`state::tests::real_client::reflow_on_close::hosted::a_window_still_loading_is_listed_as_loading`).
An X11 window's `pid` is the process Xwayland names for the window's own X
connection, `-1` when it names none, and never Xwayland's own
(`models::windows::tests::an_x11_window_is_never_given_the_pid_of_its_connection`).
`workspace` reads `""`: nothing declares workspaces yet
([#166](https://github.com/Lilium-Linux/solium/issues/166)).
`WindowList { monitor: Solium.monitor.name; sort: "mru" }` is a filtered,
sorted view, never reset: `monitor`, `workspace`, `app` and `onStage` filter,
an empty one (or `onStage` left unset) keeping every window, and `sort` is
`""`, the order windows opened, or `"mru"`, the most recently focused first.
`Windows.focused` is never null and follows focus, and with no window focused
it reads `present: false` and is empty, not the window focused last
(`qml::hosted::tests::the_focused_facade_is_empty_with_nothing_focused`);
`Windows.get(id)` is one window's row, which reads `valid: false` once the
window has gone. Rows say where a window lives, not where it is drawn this
frame
(`qml::hosted::tests::the_windows_model_filters_sorts_and_keeps_its_facades`).

**The keyboard, live.** `Keyboard`, written unqualified like `Theme`, is the
keyboard every scene reads, a window's frame as much as a shell: `layout`
(the live layout's index into `layouts`, from 0), `layoutName` (`"Russian"`),
`layoutShort` (`"RU"`, the short name xkb's own rules give it), `layouts`
(every layout's name, in order, as `layoutName` spells each), `caps` and
`num`, each notifying as it changes, and `changed(what)` once in each frame in
which the layout, Caps Lock or Num Lock really changed, `what` being
`"layout"`, `"caps"` or `"num"`, and never for ordinary typing
(`models::keyboard::tests::the_keyboard_singleton_changes_once_for_a_layout_switch_and_a_caps_toggle`).
Before the keymap is known, `layoutName` and `layoutShort` read `""`.
It is published once a frame, beside the monitors. It has no row types of its
own, so the one name it takes in `import Solium` is `Keyboard` itself, as
`Theme` takes `Theme`. It says what the keyboard is, where
`Solium.keyboard`, above, is how an item asks for it, and a switch made while
a scene holds the keyboard is told all the same
(`keyboard_change::tests::a_switch_made_while_a_scene_holds_the_keyboard_is_told_and_the_scene_types_with_it`).
Lua hears the same changes through
`sol.on("keyboard", ...)`, but for those made at the lock screen
(`state::tests::real_client::lock_focus::a_caps_toggle_at_the_lock_screen_is_not_told_to_the_configuration`),
and reads the focused text field's caret with `sol.text_input()`, hearing it
move through `sol.on("text_input", ...)`, once a pass of the event loop at
most (`text_input::tests::text_input_is_told_when_the_caret_moves_once_a_pass`).

**Room of its own.** A surface reserves edges of its monitor whatever its
size. Lua declares it, `reserve = { bottom = 48 }` on `sol.surface`, and the
scene can say it too, `Solium.surface.reserve.bottom: bar.hidden ? 0 : 48`,
which wins for each edge it sets
(`state::tests::real_client::reflow_on_close::hosted::a_scene_reserve_overrides_its_edge_and_reflows_the_layout_once`).
A negative value, such as `-1`, gives that edge back to the declaration
(`qml::hosted::tests::a_negative_scene_reserve_gives_the_edge_back_to_the_declaration`).
Each edge of the work area loses its layer-shell zone and every reserve on it,
added together
(`state::tests::real_client::reflow_on_close::hosted::two_surfaces_reserving_one_edge_take_both`),
and `sol.monitors()` and `Solium.monitor.area` are what is left
(`state::tests::real_client::reflow_on_close::hosted::a_declared_reserve_takes_its_edge_out_of_the_work_area`).
The reserving surface itself still covers its whole monitor
(`state::tests::real_client::reflow_on_close::hosted::a_reserving_surface_is_drawn_across_the_whole_monitor`),
so quick settings, a popup or a morph can grow out of a bar over the windows
without changing the reserve, and no window moves when it opens
(`state::tests::real_client::reflow_on_close::hosted::a_panel_growing_out_of_the_bar_moves_no_window`,
`qml::hosted::tests::a_panel_grown_out_of_the_bar_leaves_the_reserve_as_it_was`).
When the reserve does change, the layout runs once, in the dispatch that
changed it, and the windows glide into the new work area with the layout's
own motion from that instant, on the compositor's clock, which is the clock
the bar's animation runs on too
(`state::tests::real_client::reflow_on_close::hosted::a_reserve_the_scene_changes_at_a_press_reflows_the_tiled_windows_once_from_that_instant`,
`state::tests::real_client::reflow_on_close::hosted::a_reserve_declared_again_or_taken_away_reflows_the_windows_once_each`),
and that includes a change a `sol.on("surface", ...)` handler makes, which
re-flows the windows at the click that sent the action
(`state::tests::real_client::a_reserve_an_action_handler_changes_reflows_the_windows_in_the_clicks_dispatch`).
So bind it to where the bar is going, never to an animated value. A change
the scene makes on its own, from a `Timer`, is read after the frame it was
made in
(`state::tests::real_client::reflow_on_close::hosted::a_reserve_a_scene_changes_on_its_own_is_read_after_the_frame`).

**A way back to the configuration.** `Solium.send("windows.focus", { id:
model.id })` sends a named action with data, an object, a value or nothing
(`qml::hosted::tests::solium_send_queues_every_action_with_its_data_in_order`).
Lua hears it as `sol.on("surface", function(surface, action, data) ... end)`,
with the surface's name and the data as a table, a value or `nil`, in a
dispatch after the one that sent it, every action in the order sent, and
decides
(`script::tests::a_surface_action_reaches_lua_with_its_data`,
`script::tests::an_action_with_no_data_reaches_lua_as_nil`,
`state::tests::real_client::reflow_on_close::hosted::two_actions_from_one_frame_both_reach_lua_in_order`).
The shipped `lua/actions.lua` sends the vocabulary on to `sol.act`, the
compositor's one entry point for its verbs: `windows.focus`, `windows.close`,
`windows.fullscreen` and `windows.maximize`, each with `{ id = <window id> }`
(`state::tests::real_client::reflow_on_close::hosted::windows_focus_from_a_scene_focuses_the_window`).
`sol.act`'s data crosses as JSON, as a surface's `properties` do, nested at
most 64 deep and 65 536 values in all, a table counted once for each place it
is in: a table nested deeper or holding more, or one that contains itself, is
an error in the handler that sent it, not a crash or a stall of the compositor
(`script::tests::data_that_contains_itself_is_an_error_in_the_handler`,
`json::tests::a_table_of_more_than_65536_values_is_an_error_not_a_stall`).
`sol.act(action, data, done)` answers an attempt id, and `done(ok, reason)`
hears once whether it was done, after it was; `reason` is `"unknown-action"`,
`"unknown-window"`, `"bad-data"`, for a `windows.focus` behind the lock
`"locked"`, or, for a `windows.fullscreen` or `windows.maximize` of an X11
window, which the compositor cannot send there, `"unsupported"` (no test can
make an X11 window, so that one is read in `Solium::act`), and a window still
loading, one `sol.windows()` lists before its application has arrived, is
`"unknown-window"` to all but `windows.close`
(`script::tests::sol_act_returns_an_attempt_and_done_hears_the_outcome_once`,
`state::tests::real_client::reflow_on_close::hosted::sol_act_answers_why_it_could_not`,
`state::tests::real_client::reflow_on_close::hosted::sol_act_tells_done_once_the_window_was_asked_to_close`,
`state::tests::real_client::reflow_on_close::hosted::sol_act_on_a_window_still_loading_answers_unknown_window_but_closes_it`,
`state::tests::real_client::lock_focus::sol_act_focus_behind_the_lock_is_answered_locked`).
It is told once the whole dispatch that ran `sol.act` is applied, a
hotplug's or a reload's handlers included, and a `done` that acts again is
told again in that dispatch, 16 rounds at most, the rest at the next one
(`state::tests::real_client::reflow_on_close::hosted::done_is_told_after_every_command_of_the_batch_that_ran_its_act`,
`state::tests::real_client::reflow_on_close::hosted::a_sol_act_in_a_hotplugs_handler_hears_done_in_the_hotplugs_dispatch`,
`state::tests::real_client::reflow_on_close::hosted::a_sol_act_in_a_reloaded_configuration_hears_done_in_the_reloads_dispatch`,
`state::tests::real_client::reflow_on_close::hosted::a_done_that_acts_again_each_time_it_is_told_costs_rounds_not_the_session`).
Each `done` has the whole 100 ms handler deadline of its own, and one stopped
at it does not stop the next
(`script::tests::a_done_that_never_returns_is_stopped_and_the_next_done_still_hears_its_outcome`).
What a `done` asked for in the run that was stopped is dropped, the attempts it
started with it, so a retry that acts again and then never returns is stopped
once, not every round; and its stops are counted by function, as a listener's
are, so one stopped three times is not called again until a reload, while a
`done` written inline is a new function at each `sol.act`, and is stopped each
time rather than taken out
(`script::tests::what_a_stopped_done_asked_for_is_dropped`,
`script::tests::a_done_stopped_three_times_is_not_called_again`,
`script::tests::an_inline_done_is_stopped_each_time_and_never_taken_out`,
`state::tests::real_client::reflow_on_close::hosted::a_done_that_acts_again_and_never_returns_does_not_stall_every_dispatch`).
A `workspaces.*` action is the configuration's to answer, since the
compositor does not know what a workspace is: a file answers one with
`actions.override(name, function(data, surface) ... end)`, and until one
does, `sol.act` answers it `"unknown-action"`. An action outside the
vocabulary goes only to the listeners for its surface, as a tweak's does to
`tweaks.lua`
(`script::tests::actions_lua_routes_the_vocabulary_and_leaves_the_rest_alone`).
An override is a `surface` listener of its own, under the same 100 ms deadline
as every listener, so one stopped three times is taken out alone: every other
action is still routed, and its own goes to `sol.act` again; a later override
of the same name replaces it, and one of `nil` gives it back to `sol.act`
(`script::tests::an_override_stopped_three_times_is_taken_out_alone`,
`script::tests::a_later_override_of_the_same_name_replaces_the_earlier_one`,
`script::tests::an_override_of_nil_gives_its_action_back_to_the_compositor`).
A scene that sets a string property named `action`, as scenes did before
`Solium.send`, is heard the same way, after what it sent, with no data
(`surface::tests::queued_actions_come_first_and_the_old_action_property_last`).
Anything Lua can do — `sol.spawn`, switching workspaces, any binding — a
hosted shell can ask for this way.

## What it is not given

Said plainly, because a shell that loads is easy to mistake for one that works:

- **No compose or dead keys** in a hosted field, and no input method: a
  field types what the key types.
- **No clipboard of the session's.** `ctrl+c` and `ctrl+v` in a hosted
  field copy and paste within the compositor's own Qt: what a window copied
  cannot be pasted into it, nor the other way round.
- **No workspaces, and no icons.**
  Nothing tells a hosted scene which workspaces exist
  ([#166](https://github.com/Lilium-Linux/solium/issues/166)); it has the
  monitors and the windows, as `Solium.monitor`, `Monitors` and `Windows`.
  There is no `image://` provider for the icon theme. Driving the compositor
  goes through `Solium.send` and Lua.
- **No touch.** A scene takes no touch: a tap where it takes a press triggers
  nothing there, and neither reaches nor focuses the window under it;
  elsewhere a touch reaches the window under the shell, as the pointer would
  (`state::tests::real_client::reflow_on_close::hosted::a_touch_on_a_shell_button_neither_reaches_nor_focuses_the_window_under_it`).
  Scenes getting touch is
  [#181](https://github.com/Lilium-Linux/solium/issues/181); gestures are the
  touch epic [#7](https://github.com/Lilium-Linux/solium/issues/7), after
  v0.1.0.

## What is still a client

Ordinary applications, over `xdg-shell`.

**And any shell that is a program of its own**, over `wlr-layer-shell`:
Waybar, a wallpaper program, a Quickshell instance started as a process, a
lock screen. They are started like any other program and need nothing from
the configuration. Their `top` layer is drawn over a hosted shell, and the
compositor reserves their exclusive zones from the work area.

### Where a client's bar goes, with more than one monitor

A layer surface names the output it wants, and the compositor honours it. That
is how a client shell puts a bar on every screen: one surface per output, each
with its own exclusive zone, each reserving from *that* monitor's work area.

A surface that names no output gets the **primary** monitor — the one
`primary = true` picks out in `monitors`, or the first if nothing does. Not the
monitor the pointer is on, which is what it briefly was: a dock connects once,
at startup, and pinning it to whichever screen the mouse happened to be over
then means it appears somewhere different depending on where the mouse was
left. That looks like the compositor placing it at random, because it is.

`sol.monitors()` gives a script the list, with `primary` and `focused` flags,
so a shell written in Lua can decide for itself which screens get a bar.

## What hosting costs, honestly

A hosted shell cannot crash independently of the compositor. A separate
process can be restarted; a QML error in the dock takes the session with it
unless the compositor is careful. So:

- Every scene is loaded defensively. A scene that fails to load is skipped and
  logged; it never stops the compositor starting.
- Scene errors are contained per surface — a broken dock must not take the
  window frames with it.
- An edit that does not parse leaves the last scene drawing.

That is the trade being made deliberately: robustness through care inside one
process, in exchange for a desktop that can actually do what the design asks
for. A shell that would rather have a separate process's robustness can have
it, as a client, and gives up the one engine to get it.

## How the rest of the desktop starts

Whatever Solium loads itself starts with it and needs nothing below: its own
QML scenes, and everything `init.lua` declares. A shell that Solium loads
through its configuration is one of those, and needs none of this either.
Every other program of the desktop is a separate process, and on a systemd
machine it is started by the user's systemd, or by D-Bus when something first
asks for it. That covers a polkit authentication agent, a keyring, applets
such as `nm-applet`, a bar that is a program of its own, and the portals.

So Solium tells both where it is (#146), when it is started as the session.
The session file starts `solium-session`, which runs `solium --tty --session`.
Once its Wayland socket is up, Solium sends `WAYLAND_DISPLAY`,
`XDG_CURRENT_DESKTOP` (`Lilium`, unless the session file's `DesktopNames` set
it already) and `XDG_SESSION_TYPE=wayland` to systemd's user manager
(`SetEnvironment`) and to D-Bus activation (`UpdateActivationEnvironment`). It
sends them again with `DISPLAY` once XWayland has one. Then it starts
`solium-session.target`, which starts `graphical-session.target`, and
`solium-autostart.target`, which starts `xdg-desktop-autostart.target`.
`session.rs` has the tests, and
`the_calls_reach_systemd_and_dbus_as_their_methods` checks the calls on the
wire.

**On the way out** Solium stops `solium-session.target`, which takes the
autostart target with it, and unsets the variables in systemd, so a Plasma
login afterwards does not inherit a dead socket. It does so on a clean exit,
and on SIGTERM (how logind ends a session), SIGINT or SIGHUP (`signals.rs`). A
Solium that cannot stop, because something inside it has stopped answering,
ends itself `session.stop_timeout` (five seconds) after the signal, or at once
when the same signal comes again half a second or more later. Neither that nor
a crash gives Solium the chance to clean up, so `solium-session` does it once
Solium has gone, if the target is still active. If Solium failed before it
started the target, while it waited up to five seconds for XWayland,
`solium-session` unsets the variables, as long as no other desktop holds
`graphical-session.target` (`dev/install-check.sh` checks each case).

Two things are left behind even so. A SIGKILL of the whole session, which
systemd sends to whatever of it is still running once its stop timeout has
passed (45 seconds on Fedora 44), takes `solium-session` with it; the next
Solium session stops any target an earlier one left running before it says
anything. And under `dbus-daemon`, the D-Bus activation environment keeps the
variables, because D-Bus has no call that removes one. Fedora's `dbus-broker`
hands `UpdateActivationEnvironment` on to systemd's `SetEnvironment` (its
launcher names that method), so there the unset should clear them for D-Bus
activation too; that has still to be seen on a real session.

One Solium session runs per user at a time: `solium-session` holds a lock for
as long as its session runs, and refuses to start a second, which would take
the first one's target for a leftover and stop it.

**When Solium is not the session, it tells nobody.** `solium --tty` started by
hand, from a text console, leaves systemd and D-Bus alone: a desktop on
another VT shares them, and its display is not Solium's to replace. A nested
Solium leaves the session it runs in alone for the same reason. And a Solium
session that finds `graphical-session.target` already held by another desktop
of the same user leaves systemd and D-Bus to that desktop, and says so in the
log.

`session.systemd = false` turns all of it off. `session.autostart = false`
leaves out `solium-autostart.target`, and so XDG autostart, while
`solium-session.target` and every unit attached to it still start.
`dev/install.sh` puts both targets in `~/.config/systemd/user` and
`lilium-portals.conf` in `~/.config/xdg-desktop-portal`.

A program that should start with a Solium session can do it in one of two
ways.

**XDG autostart.** Put a `.desktop` file in `~/.config/autostart`, or in
`/etc/xdg/autostart` for every user. systemd's autostart generator turns each
into a service when `xdg-desktop-autostart.target` starts. It skips any whose
`OnlyShowIn=` does not name `Lilium`, whose `NotShowIn=` does, or that says
`X-systemd-skip=true`:

```ini
[Desktop Entry]
Type=Application
Name=Network applet
Exec=nm-applet --indicator
OnlyShowIn=Lilium;
```

**A systemd user unit.** It is tied to the graphical session, so it stops
when Solium does, and attached to `solium-session.target`, so it starts
whether autostart is on or not:

```ini
[Unit]
Description=A bar
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=/usr/bin/waybar
Restart=on-failure

[Install]
WantedBy=solium-session.target
```

Save it as `~/.config/systemd/user/bar.service` and enable it with
`systemctl --user enable bar.service`. `WantedBy=graphical-session.target`
also starts it under any other desktop that runs a systemd session, such as
Plasma or GNOME. `solium-session.target` starts it only under Solium.

**Polkit agents and keyrings start the same way, and most say which desktops
they are for.** Fedora's KDE polkit agent, for instance, ships
`/etc/xdg/autostart/polkit-kde-authentication-agent-1.desktop` with
`OnlyShowIn=KDE;` and `X-systemd-skip=true`, so autostart skips it under
Lilium, and a copy of it would be skipped too. It also ships
`plasma-polkit-agent.service`, which is already
`PartOf=graphical-session.target`, so a single command starts it with every
Solium session, autostart on or off:

```sh
systemctl --user add-wants solium-session.target plasma-polkit-agent.service
```

A keyring that D-Bus starts on demand, such as KWallet's `kwalletd6`, needs no
entry: once the environment is exported, the first program that asks for a
secret starts it. It is not unlocked with the login password, though. Under
Plasma, `plasma-kwallet-pam.service` runs `/usr/libexec/pam_kwallet_init`,
which hands the wallet what PAM kept at login, through the socket named by
`PAM_KWALLET5_LOGIN`. Solium does not export that variable to systemd, and the
script does nothing without it, so the wallet asks for its password the first
time it opens.

## Summary

| | Where it runs | How it reaches the compositor |
|---|---|---|
| Applications | Clients | `xdg-shell` |
| Window frames | Compositor's QML engine | directly |
| Loading windows, the pointer | The same QML engine | directly |
| The wallpaper, other scripted scenes | The same QML engine | `sol.surface` from Lua |
| A hosted shell: bar, dock, launcher | The same QML engine | `shell.scene`, through `sol.surface` |
| A client shell (Waybar and the like), wallpaper programs | Clients | `wlr-layer-shell` |
| A polkit agent, a keyring, applets and other separate programs | Clients, started by systemd or D-Bus | XDG autostart, or a unit `PartOf=graphical-session.target` |
| Which monitor a bar is on | Hosted: every monitor, or the one `shell.on` names. A client: the output it names | the output argument of `zwlr_layer_shell_v1.get_layer_surface` |
| Colours and metrics | `Solium.Theme`, one singleton | imported by every scene that wants it |
| What an animation *does* | Lua script | `sol.present_from`, `sol.on("open")` |

## History: the dock that proved it

The first in-compositor dock was compositor code — `shell.rs` and `sol.dock` —
and it was taken back out, as the compiled-in bar before it had been: it made
the compositor own a design, a font stack and a layout it had no reason to,
and it hid the question of how a *replaceable* shell attaches instead of
answering it. `layer.rs` records that for the bar, and `surface.rs` for the
dock. The answer is the one above: the shell is configuration, and the
compositor hosts whatever the configuration names.

The dock was a QML scene rendered by the same host that draws the window
frames, importing the same `Solium.Theme`, reserving its own strip out of
`work_area`. The morph was the whole argument made concrete: pressing an icon
fired a `dock` event carrying **the rectangle that icon occupies**, a script
spawned the program and handed that rectangle to `sol.present_from`, and the
window grew out of it. Captured frame by frame in `docs/morph.png`, a record
of 2026-09-06 and of a dock that no longer exists — 460x208 near the dock, then
928x504, 1192x672, settled at full size about a third of a second later.

`sol.present_from` is still there and still does that. What is missing is a
dock to give it a rectangle. `config.lua` carried a `dock.morph` duration for
the day one exists, and #117 removed it — nothing had ever read it, and a
setting configuring a component that does not exist cannot be told apart, from
outside, from one that is broken. `--check` reports that spelling as an
unrecognised key now, and a dock brings its settings back to that spot when it
arrives.

None of that is reachable from a separate process. A client dock can pass a
rectangle over IPC, but by the time the window exists the two are separate
scenes and nothing holds both at once to interpolate between them. That is why
a shell is hosted here rather than run beside the compositor as a program of
its own: hosted, its icons are in the same engine as the window frames, so the
flight planned above can be a live instance of the icon's own component rather
than a fake.
