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
compiler, Qt 6.5 or newer (Quick, Qml and Network), and the Wayland,
libinput, libudev, libseat, xkbcommon, GBM, EGL and libdrm development files.

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

It checks the formatting (`cargo fmt --check`; `cargo fmt --all` fixes it),
then runs clippy with every warning denied, the tests, a build, then
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

The first five come from the Hyprland fork Solium replaces; the rest from
Solium's own first weeks. Each one cost real time, and the next person to hit
one will be one of us.

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
   ceiling. So window decorations are always the compositor's own. A bar or a
   dock may run inside the compositor as a hosted shell, in the same QML engine
   and theme as the frames, or as an ordinary layer-shell client; see
   [docs/shell-boundary.md](docs/shell-boundary.md).

And from Solium's first weeks:

- **Find out what a dependency's warning means before believing it.** Smithay
  logs `Unable to become drm master, assuming unprivileged mode` on runs that
  work. Its own comment three lines above says why: on a modern kernel the
  permission is granted implicitly when no other process is master, and it
  skips the error deliberately. Read as a failure, it once became a P1 issue, a
  rewritten document and a confident claim that the compositor had never drawn
  a frame on hardware, against the direct report of someone who had just used
  it. One look at the source undid all of it. **When the evidence and the
  person who was there disagree, check the evidence.**
- **Run a test more than once.** Three bugs in one week were found only by
  repetition. The clipboard flush failed about one run in three: the first run
  passed, the second passed, the third failed. A single green run is not
  evidence for anything involving two processes and a protocol.
- **Nested and hardware are different compositors.** The pointer was once
  invisible on the hardware and nothing noticed, because a nested session sits
  inside a host that draws its own cursor. The two backends also had different
  drawing policies — one drew every loop iteration, the other on damage — so
  every animation was developed against the one where a missing damage signal
  is invisible. They gate on the same test now; keep it that way. The answer is
  to make the nested backend able to *be* the hardware in the way that
  matters, not to test less: `SOLIUM_OUTPUTS=n` gives it that many monitors,
  and found a real bug within minutes of existing — the resize handler resized
  only the first output. On a TTY that is an hour and a session; nested it is a
  screenshot.
- **Take a backtrace before theorising.** An hour once went into deciding why
  the nested backend hung. `eu-stack -p $(pgrep -x solium)` answered it in a
  minute: `WlEglSurface::swap_buffers`.
- **"It broke when I changed X" is not evidence that X broke it.** A nested
  hang was once the host machine going to sleep, and the change blamed for it,
  twice, was innocent both times.
- **Measurement scripts lie more often than the compositor.** Three false
  alarms came from screenshot analysis: a "background" pixel that was the
  cursor, a sample row that landed in a tiling gap, and a synthetic drag that
  delivered twelve motions in one microsecond, so every client coalesced them
  into one. Look at the picture.
- **Run the examples in the documentation.** The mode guide's worked example
  once did not run: `require("modes")` failed, because the Lua search path was
  built from the chosen configuration's own directory, so writing your own
  `init.lua` lost every shipped module. That is the central configurability
  promise, and it had been documented as working since it was written.
- **A compositor cannot test its own protocol support from the inside.** "The
  global is advertised" is a different claim from "a client that uses it gets
  the right answers". Asked to anchor a bar, `wl-probe` found that layer
  surfaces had never been sent an initial configure: every bar and dock had
  been invisible for as long as the compositor had claimed to support them,
  and no shipped check used the protocol. A protocol added gets a check from a
  client's side, in `wl-probe` or as a real client in the test suite
  (`mod real_client` in `crates/solium/src/state/tests.rs`).

### Naming

- **Wayland protocols** carry the `solium_` prefix, because interface names are
  a global namespace. One concern per protocol: `solium_frame_v1`,
  `solium_window_v1`.
- **Rust is named for what it does**, never branded. A struct that transforms a
  window's presentation is `WindowPresentation`, not `SoliumWindowThing`. If a
  name only makes sense to someone who knows the project's history, it is
  wrong.

### Testing

- **Test evidence is captured independently of the thing under test.** A
  scene's or a shell's own screenshot of itself shows only what it believes it
  drew, and a host desktop's screenshot of the nested window shows nothing
  about what Solium composited. Read back the composited frame instead
  (`SOLIUM_CAPTURE`, in
  [dev/README.md](dev/README.md#capturing-a-frame)). The pictures in the
  documentation are captured the same way, by the compositor reading back its
  own framebuffer, because what they show is what it draws.
- A check that can only be run by hand is written down in
  [dev/README.md](dev/README.md), with what it asserts and why no automated
  test reaches it.

### History worth knowing

Solium began as a Hyprland fork. That work is archived, and its post-mortem —
most usefully, why an out-of-process QML shell was the wrong architecture — is
in the `lilium-de` repository of the [Lilium-Linux](https://github.com/Lilium-Linux)
organisation. Read it before
proposing a helper process.

## The documentation site

The docs are also a website, <https://lilium-linux.github.io/solium/>, built
with [mdBook](https://rust-lang.github.io/mdBook/) and published from `stage`
by `.github/workflows/docs.yml`. Every pull request builds it as well, so a
broken link inside it fails CI.

```sh
dev/docs.sh             # build it into target/book, and check its links
dev/docs.sh --serve     # and serve it on http://localhost:3000
SOLIUM_DOCS_IMAGE=localhost/solium-build:fc44 dev/docs.sh   # in the container
```

- **A doc in `docs/` is a page as it stands.** `docs/SUMMARY.md` is the table
  of contents; a new doc goes there too.
- **The reference pages are generated, and never committed.** The
  configuration comes from `crates/solium/lua/config.lua`, the key bindings
  from `solium --check`, the Lua API from `crates/solium/lua/meta/sol.lua` and
  the flags and environment from `crates/solium/environment.txt`. Change the
  source, not the page.
- **A new `sol.*` name needs its entry in `sol.lua`, and a new `SOLIUM_*`
  variable or flag its line in `environment.txt`.** The tests
  `sol_lua_documents_exactly_the_api_the_compositor_registers` and
  `the_environment_reference_lists_exactly_what_the_code_reads` fail until
  they have one.
- **Write links as paths in the repository**, the way GitHub reads them.
  `dev/docs/links.py` points each one at its page in the book, or at the file
  on GitHub.
