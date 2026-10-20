# Working on Solium

For coding agents (Codex, Claude Code and the rest), and for anyone who wants
the short version. This file is a map and a list of rules; the documents it
points to hold the detail, and where the two disagree, they are right and this
file is out of date.

## Read first

1. [README.md](README.md), "How it is built": what Solium is, and why it is
   built the way it is.
2. [docs/architecture.md](docs/architecture.md), "The one idea" and "Design
   rules". They decide whether a change belongs in Rust, in Lua or in QML.
3. [CONTRIBUTING.md](CONTRIBUTING.md): branches, the gate, commits, and the
   rules a change is judged against.
4. [dev/README.md](dev/README.md): the instruments. Building, running nested,
   driving and recording a run without a keyboard, the checks, the TTY.
5. For settings, [docs/ricing.md](docs/ricing.md) and
   [crates/solium/lua/config.lua](crates/solium/lua/config.lua), which holds
   every setting with its documentation.

## The map

| Path | What is there |
|---|---|
| `crates/solium/src/` | The compositor. `state/` is the `Solium` struct and its Wayland handlers; `render.rs` draws a frame; `input/` is libinput, the seat and grabs; `script.rs` is the Lua API; `qml/` hosts Qt; `effect/` and `pass/` run effects; `models/` is what QML scenes read |
| `crates/solium/lua/` | The shipped configuration. `init.lua` loads it, `config.lua` has every setting, the modes are `floating.lua`, `tiling.lua`, `scrolling.lua` and `overview.lua`, and `meta/sol.lua` documents the Lua API |
| `crates/solium/qml/` | The QML: the `Solium` module's types, the window frame styles in `panes/`, the preview shell in `preview/` |
| `crates/solium/effects/` | The shipped effects, each a folder of Lua and GLSL: `blur`, `glass`, `genie`, `shade` |
| `crates/solium/tests/` | The command-line tests, and the scenarios the unit tests play |
| `crates/animation/`, `crates/layout/`, `crates/effects/` | Crates with no compositor in them: curves and springs, the window arrangements, and the effect format |
| `crates/wl-probe/` | A Wayland client that checks protocol support from outside |
| `dev/` | Setup, the gate, the build image, and the nested and TTY harnesses |
| `docs/` | The documentation site |

## Commands

Once, after cloning:

```sh
dev/setup.sh
```

It checks Rust and the system packages, builds the Fedora build image when the
packages are missing, installs the git hooks, and says what to install when it
cannot do it itself.

| To | Run |
|---|---|
| check a change, as every pull request must | `dev/gate.sh`, or `SOLIUM_GATE_IMAGE=localhost/solium-build:fc44 dev/gate.sh` in the image |
| build | `cargo build` |
| run the tests whose names contain a phrase | `cargo test -p solium <phrase>` (the crate is a binary: there is no `--lib`) |
| test another crate | `cargo test -p solium-animation`, `-p solium-layout`, `-p solium-effects` |
| check that the Lua configuration loads | `./target/debug/solium --check` |
| format | `cargo fmt --all` |
| build the documentation site | `dev/docs.sh` |

The gate also builds the documentation site as CI publishes it, so a doc
comment that links a renamed or test-only function fails it: link only what a
normal build compiles, and name the rest in backticks.

In the image, cargo runs inside `podman run`; dev/README.md, "Building", has
the line. One test can fail under load
([#176](https://github.com/Lilium-Linux/solium/issues/176)); dev/README.md,
"The gate", says which and how to tell.

## Running it, and looking at it

- **Nested only.** `./target/debug/solium` opens Solium as a window inside the
  session you are in. Never point anything at the live session: an empty
  `WAYLAND_DISPLAY` means the default socket, which is the real desktop.
- **Under `timeout`, one run at a time,** and stop what you started by its
  PID, never by name: the developer's own programs have the same names.
- **Drive it with the knobs, not a hand,** and record it the same way:
  `SOLIUM_TRIGGER_AT`, `SOLIUM_KEY_AT`, `SOLIUM_DRAG_AT`, `SOLIUM_CAPTURE`
  (dev/README.md, "The knobs"). For example, a terminal opened and the frame
  three seconds in:

  ```sh
  SOLIUM_TRIGGER_AT="1500:super+return" SOLIUM_CAPTURE=/tmp/shot.ppm \
      SOLIUM_CAPTURE_AT=3000 timeout -s TERM 5 ./target/debug/solium
  ```

- **Look at the frames.** A change that shows on screen is checked by looking
  at its captures, frame by frame, not by reading the log. Measurement scripts
  have been wrong more often than the compositor.
- **Nested is not the hardware.** Nested, QML renders in software and the host
  draws its own pointer. The GPU path, real input devices and frame pacing are
  checked on a TTY, and a TTY run is a person's.
- **No KDE applications as test clients.** They are started through D-Bus and
  open on the host session instead. `dev/foot` is a terminal that opens inside
  Solium.

## Where a change belongs

Most review findings are one of these.

- **A feature is configuration.** Rust and C++ provide protocols, data (the
  Lua API, QML models and properties, events) and generic primitives. What a
  user sees, such as a bar, an indicator, a frame or an effect, is shipped
  Lua, QML or GLSL that a user can copy and change. A search for the feature's
  name in `crates/solium/src` should find nothing.
- **Window frames are QML, always.** Never drawn in Rust, not even as a fast
  path for the default style.
- **Effects are files.** Blur, glass and genie are folders in
  `crates/solium/effects/`, written against the public API. The engine gets
  generic primitives, never a built-in effect.
- **Every number is a setting.** A threshold, a timeout or a cap gets a key in
  `config.lua`, with a default, its documentation, and a test that changing it
  changes the behaviour.
- **A new mode needs no new Rust.** If it does, the transform layer is missing
  something: fix the layer, do not special-case the mode.
- **Windows answer at once.** Opening, resizing and closing a window respond on
  the next frame, and the application catches up. Waiting for the application
  is an option a layout may set, never the default.
- **One answer to each question.** Which screen draws a window is
  `render::drawn_on`; what is under a point is `owns` in `state/hit_test.rs`;
  the keyboard moves only through `focus.rs`, and `crates/solium/clippy.toml`
  denies Smithay's own focus calls everywhere else. Never write a second copy.
- **One clock.** Compositor animations, QML animations and QML timers all run
  on the compositor's clock. Anything that ticks between frames goes through
  it.
- **Keys work under any layout.** A change to key handling is tested with a
  `us,ru` keymap switched to Russian, where `super+q` arrives as
  `super+Cyrillic_shorti`.
- **QML is designed for the GPU.** Scenes render on the GPU by default, and
  software is the fallback for machines whose GPU check fails. Design an
  effect, a scene or a performance change for the GPU path first.
- **One compositor for every form factor.** Phones, tablets, laptops and
  desktops run the same binary. What differs between them, such as a launch
  animation, a gesture or a bar, is configuration over generic primitives,
  never a code path of its own (docs/architecture.md, "Form factors").
- **Ready on the first frame.** The desktop is shown once its scenes and their
  data are ready, within a bounded wait (`startup.first_frame`), so nothing
  pops in after login. A new scene on the desktop joins that wait rather than
  appearing later.
- **Decide, do not defer.** Nothing ships switched off "until it is decided",
  and nothing half-finished is kept "for later": finish it, or remove it with
  its tests and its documentation.

## Code

- `unwrap`, `expect`, `panic!` and `todo!` are denied. A compositor that panics
  takes the whole session with it, so handle the error or pass it up.
- A test fails before the change and passes after. A behaviour comes with the
  test that shows it, and a bug fix with the test that reproduces it. Run
  anything that involves two processes more than once.
- Comments say what the code is and why, not how it got there.
- Nothing is copied from another project, whatever its licence: read it, write
  down what it does in your own words, and implement that. Credit the idea in
  [THIRD_PARTY.md](THIRD_PARTY.md).
- Show only real output: captures from the compositor, never a mock-up.

## Commits and pull requests

- Branch from `stage`, named for the issue: `fix-123-short-name`. One concern
  per branch.
- The subject says what is now true, prefixed with the issue:
  `#123: a tiled window keeps its edge`. The body says why.
- **No AI attribution.** No `Co-Authored-By` trailer naming an agent, and no
  "Generated with" footer, in commits, pull requests, issues or comments.
  `dev/hooks/commit-msg` strips them from commits; the rest is up to you.
- Pass `dev/gate.sh` before pushing.
- Push the branch and open a pull request against `stage`. Never push to
  `stage` or `release`, and never merge: the maintainer does.
- The queue is the open issues labelled `daily-drive`, ordered by `p1`, `p2`
  and `p3`.

## Your own machine

Rules for one machine, such as memory caps for builds, which account to push
with, or where private notes live, do not belong here. Put them in your agent's
own global instructions (for Codex, `~/.codex/AGENTS.md`).
