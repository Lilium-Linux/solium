# Contributing to Solium

Thank you for wanting to help. This file says how work gets into Solium: where
it goes, how to build and check it, how to write the commits, and the design
rules a change is judged against.

Report security problems privately, not as issues: see [SECURITY.md](SECURITY.md).
Everyone taking part is expected to follow the [code of
conduct](CODE_OF_CONDUCT.md).

## Branches

- **`stage` is the trunk.** Every change is a pull request against it. There is
  no `main`.
- **`release`** is what releases are cut from. Merging `stage` into it runs
  `.github/workflows/release.yml`, which verifies the tree, bumps the version
  and drafts a GitHub release. Only the maintainer merges into `release`.

## Making a change

1. **Find or open an issue first** for anything bigger than a typo, so the
   approach can be agreed before the work is done. The `daily-drive` label is
   the list of what matters most right now.
2. **Branch from `stage`**, named for the issue: `fix-123-short-name`.
3. **Keep it to one concern.** A pull request that fixes a bug and tidies the
   module around it is two pull requests. Small and explained is easier to
   review than large and complete.
4. **Run the gate** (below) before opening the pull request, and again after
   rebasing on a `stage` that moved.
5. **Open the pull request against `stage`.** The template asks for the checks
   and carries the contributor agreement sentence; leave that sentence in.
   Say what changed and why, and link the issue.
6. **Review is a conversation.** Push follow-up commits rather than
   force-pushing over what was reviewed, so the reviewer can see what changed.
   The maintainer merges; please do not merge your own pull request.

## Building

The README's [Building](README.md#building) section lists the packages for
each distribution. In short: Rust 1.88 or newer (edition 2024), a C++17
compiler, Qt 6.5 or newer (Quick and Qml), and the Wayland, libinput, libudev,
libseat, xkbcommon, GBM, EGL and libdrm development files.

```sh
cargo build
./target/debug/solium          # nested, as a window inside your session
```

Test nested. **Never test against the session you are working in**: an empty
`WAYLAND_DISPLAY` does not mean "no display", it means the default one, which
is your real desktop. [dev/README.md](dev/README.md) has the knobs for driving
and photographing the compositor without a hand on the keyboard, and the
checks that need a running session.

## The gate

```sh
dev/gate.sh
```

It runs `cargo fmt`, clippy with every warning denied, the tests, a build, then
`solium --check` (the Lua configuration loads) and `dev/wirecheck` (the QML GPU
path against your machine's render node, skipped if there is none). It exits
non-zero if anything fails, and a pull request is expected to pass it.

It runs cargo natively. On Fedora without the development packages, it can
build in an image of your Fedora release instead (`FEDORA_VERSION` in
`dev/Containerfile`, 44 by default), which needs only podman and rustup:

```sh
podman build -t solium-build:fc44 -f dev/Containerfile dev/
SOLIUM_GATE_IMAGE=localhost/solium-build:fc44 dev/gate.sh
```

`SOLIUM_GATE_JOBS`, `SOLIUM_GATE_CPUS`, `SOLIUM_GATE_PODMAN_ARGS` and
`SOLIUM_GATE_NO_GPU` limit what it uses; see [dev/README.md](dev/README.md#the-gate).
On another distribution, install the packages natively: what the image builds
links Fedora's libraries and does not run elsewhere. CI runs the same checks
except the GPU one, on Fedora, so a contributor whose distribution ships a Qt
older than 6.5, which cannot run Solium at all, can rely on CI for them.

**Lints are denied, not warned.** `unwrap_used`, `expect_used`, `panic` and
`todo` are denied for the whole workspace, because a compositor crash takes the
session with it: there is no supervisor to restart it and nothing for the user
to look at but a black screen. Handle the error or propagate it. `unsafe_code`
warns rather than denies; each use says why it is sound, in a comment.

**A test counts only when it fails before the change and passes after.** A new
behaviour comes with a test that shows it, and a bug fix with one that
reproduces the bug.

## Commits

- One logical change per commit, and the tree builds and passes the gate at
  every commit.
- The subject says what is now true, in the present tense, prefixed with the
  issue it belongs to: `#115: the layouts take each window's own floor`. A
  follow-up from review says so: `#115 review: small corrections`.
- The body says why. The diff already says what.
- Comments describe the code as it is, not how it got there. A comment that
  stops being true when the code changes is changed with it.

## Licence and the contributor agreement

Solium is published under the [GNU General Public License, version 3
only](LICENSE). Before a first contribution is merged, each contributor agrees
once to the [Contributor License Agreement](CLA.md): the pull request template
carries the one sentence that does it. You keep the copyright in your work.

If a change includes code from another project, say so in the pull request and
name its source and licence, and add it to [THIRD_PARTY.md](THIRD_PARTY.md).
Behaviour reimplemented from another project, with no code from it, is
credited there too.

## Design rules

These decide whether a change belongs in Rust, in a script or somewhere else.
Read [docs/architecture.md](docs/architecture.md) as well, particularly "The
one idea".

### The test that governs design

Every mode — overview, app switcher, peek, genie, tiling, scrolling — is the
same operation: place a window's texture somewhere other than its real geometry
and animate between the two.

**If a new mode needs new Rust, the transform layer is missing something.** Fix
the layer; do not special-case the mode.

### Rules learned the hard way

These come from the Hyprland fork Solium replaces, and each one cost real time.

1. **One authority for any piece of state.** Two models of window state
   produced a deadlock where a decoration was needed to learn the geometry that
   decided whether to create a decoration.
2. **Geometry the compositor animates against must arrive with the frame that
   shows it.** Never a side channel: that is a mirror, and mirrors drift.
3. **Commands are not state.** `focus_window` is a verb; the focus change comes
   back through the event stream.
4. **A protocol carries one concern.** A name that is hard to choose means the
   interface does too much.
5. **In-process for anything window-coupled.** An out-of-process shell painting
   decorations was measured at ~15 fps and 39% CPU; the boundary was the
   ceiling. Everything else that merely sits on a screen — a bar, a dock — is a
   client; see [docs/shell-boundary.md](docs/shell-boundary.md).

### Naming

- **Wayland protocols** carry the `solium_` prefix, because interface names are
  a global namespace. One concern per protocol: `solium_frame_v1`,
  `solium_window_v1`.
- **Rust is named for what it does**, never branded. A struct that transforms a
  window's presentation is `WindowPresentation`, not `SoliumWindowThing`. If a
  name only makes sense to someone who knows the project's history, it is
  wrong.

### Testing

- Capture screenshots with an independent tool, not through the thing under
  test.
- A check that can only be run by hand is written down in
  [dev/README.md](dev/README.md), with what it asserts and why no automated
  test reaches it.

### History worth knowing

Solium began as a Hyprland fork. That work is archived, and its post-mortem —
most usefully, why an out-of-process QML shell was the wrong architecture — is
in the `lilium-de` repository of the [Lilium-Linux](https://github.com/Lilium-Linux)
organisation. Read it before
proposing a helper process.
