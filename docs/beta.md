# The road to a public preview

**Written 2026-09-07.** Where the compositor stands, what has to be true before
strangers run it, and the order to do it in.

## Where it stands

Four days old, 111 commits, ~21k lines across Rust, Lua, QML and the Qt host.
E1 to E6 have landed: it boots on hardware, transforms and animates every
window through one engine, is scripted in Lua, has floating, tiling and
scrolling layouts, draws its own decorations from QML, and expresses every mode
as a script over the transform.

That last one was the bet. If overview had needed new Rust the architecture was
wrong and everything after would have been fighting it. It didn't — overview is
ninety lines of Lua.

It runs real applications: Firefox, LibreOffice, Steam, Konsole, Dolphin,
Okular, X11 clients through XWayland, with no protocol errors. Copy and paste
works in both directions across the X11 boundary. It is used to develop itself,
which is the only test that counts for a compositor.

Protocols: `xdg-shell`, `wlr-layer-shell`, `xdg-decoration`, `xdg-output`,
`xdg-activation`, `wp-viewporter`, `wp-fractional-scale`, `wp-presentation`,
`linux-dmabuf`, `relative-pointer`, `pointer-constraints`, `primary-selection`,
`xwayland-shell`.

## Call it a preview, not a beta

"Beta" tells people it is nearly ready for normal use. It is not, and a
compositor crash takes the session and whatever was unsaved in it. Spending
that trust once is enough to lose it.

A **public preview**, aimed at people who want to hack on a compositor rather
than people looking for a desktop, is honest and achievable. It also gets the
first-contact bugs sooner and smaller, from people who will not be angry about
them. Nobody but the author has ever run this: one machine, one NVIDIA GPU, one
monitor layout. First contact with AMD and Intel, laptop panels, other people's
configurations will produce a burst of bugs that no amount of local testing
predicts.

## What has to be true first

**Blocking.** A preview without these is not something a second person can
use.

| | why |
|---|---|
| packaging | there is none. A preview nobody can install is a preview nobody tries |
| ~~[#41](https://github.com/Lilium-Linux/solium/issues/41) multi-monitor~~ | **done.** A pipeline per monitor, one global space, layouts and workspaces per screen |
| ~~[#39](https://github.com/Lilium-Linux/solium/issues/39) HiDPI~~ | **done.** Scale per monitor, chrome rasterised at it, chosen from the panel's dpi |
| ~~[#28](https://github.com/Lilium-Linux/solium/issues/28) screen capture~~ | **done.** `wlr-screencopy`, so grim, wf-recorder and the portal all work |
| packaging | there is none. A preview nobody can install is a preview nobody tries |

**Shippable as documented gaps.** Real holes, but ones a preview can name and
survive: [#27](https://github.com/Lilium-Linux/solium/issues/27) session lock,
[#26](https://github.com/Lilium-Linux/solium/issues/26) IME,
[#36](https://github.com/Lilium-Linux/solium/issues/36) idle-inhibit,
[#33](https://github.com/Lilium-Linux/solium/issues/33) the ~190KB-per-window
leak.

**Not features, and do them anyway.** CI runs the gate but not the checks under
`dev/`. And the compositor has never been soaked — left running unattended for
hours with window churn — because that could not be done on the development
host. That needs solving rather than skipping: a preview that dies after six
hours is worse than one that is missing a lock screen.

## Order, and why

1. ~~**[#41](https://github.com/Lilium-Linux/solium/issues/41)
   multi-monitor**~~ — done and on `main`, verified on two monitors at 260 Hz
   and 75 Hz. It was the biggest and the one everything else is easier after,
   which is why it went first. Left over as its own issues:
   [#42](https://github.com/Lilium-Linux/solium/issues/42) absolute devices,
   [#43](https://github.com/Lilium-Linux/solium/issues/43) hotplug,
   [#44](https://github.com/Lilium-Linux/solium/issues/44) matching a monitor
   by what it is rather than which port it is in,
   [#45](https://github.com/Lilium-Linux/solium/issues/45) mirroring,
   [#46](https://github.com/Lilium-Linux/solium/issues/46) 10-bit.
2. ~~**[#39](https://github.com/Lilium-Linux/solium/issues/39) HiDPI**~~ —
   done, and #41 was indeed most of it: every call site already took an output
   or a screen rect. The part that was not plumbing was the QML host, which
   needed the *distinction* between logical layout and device rasterisation
   rather than a bigger canvas — a scene given the device size lays out in it
   and comes out half the size it should be.
3. ~~**[#28](https://github.com/Lilium-Linux/solium/issues/28) screencopy**~~ —
   done, and it was self-contained as expected. `ext-image-copy-capture-v1` is
   the successor and is #47; the `wayland-protocols` release this builds
   against does not carry it and nothing installed speaks it.
4. **Soak and package.** Both are the difference between working here and
   working anywhere.
5. Then [#27](https://github.com/Lilium-Linux/solium/issues/27),
   [#36](https://github.com/Lilium-Linux/solium/issues/36),
   [#33](https://github.com/Lilium-Linux/solium/issues/33),
   [#38](https://github.com/Lilium-Linux/solium/issues/38) in whatever order
   suits, and the P3s after the preview is out.

Priorities on the tracker are by **whether an application can be used at all
without the thing**, not by effort. That is why `cursor-shape-v1` is P3 — its
absence costs nothing, because clients fall back to `wl_pointer.set_cursor` —
and multi-monitor is P1.

## Traps, paid for in full

Every one of these cost real time in the first four days. They are here because
the next person to hit them will be us.

**Find out what a dependency's warning means before believing it.** Smithay
logs `Unable to become drm master, assuming unprivileged mode` on every run on
this machine, including every run that worked. Its own comment three lines
above says why: on a modern kernel the permission is granted implicitly when
no other process is master, and it skips the error deliberately. Read as a
failure, it turned into a P1 issue, a rewritten section of this document, and
a confident claim that the compositor had never drawn a frame on hardware —
against the author's direct report that he had just used it. It took one look
at the source to undo all of it. **When the evidence and the person who was
there disagree, check the evidence.**

**Run a test more than once.** Three separate bugs this week were found only by
repetition. The clipboard flush failed about one run in three: the first test
passed, the second passed, the third failed. A single green run is not evidence
for anything involving two processes and a protocol.

**Nested and hardware are different compositors.** The cursor was invisible for
the project's entire life and nothing noticed, because a nested session sits
inside a host that draws its own cursor. The two backends also had different
drawing policies — one drew every loop iteration, the other on damage — so
every animation was developed against the one where a missing damage signal is
invisible. They gate on the same test now. Keep it that way.

The answer to this is to make the nested backend able to *be* the hardware in
the way that matters, not to test less. `SOLIUM_OUTPUTS=n` gives it that many
monitors and found a real bug within minutes of existing — the resize handler
only resized the first output, so the left screen covered the right one and a
pointer over the second monitor was answered with the first. On a TTY that is
an hour and a session; nested it is a screenshot.

**Take a backtrace before theorising.** An hour went into deciding why the
nested backend hung. `eu-stack -p $(pgrep -x solium)` answered it in a minute:
`WlEglSurface::swap_buffers`.

**"It broke when I changed X" is not evidence that X broke it.** Twice. The
nested hang was the machine going to sleep. The presentation change was
innocent both times I blamed it.

**Measurement scripts lie more often than the compositor.** Three false alarms
came from screenshot analysis: a "background" pixel that was the cursor, a
sample row that landed in a tiling gap, a synthetic drag that delivered twelve
motions in one microsecond so every client coalesced them into one. Look at the
picture.

**Run the examples in the documentation.** The mode guide's worked example did
not run — `require("modes")` failed, because the Lua search path was built from
the chosen config's own directory. Writing your own `init.lua` lost every
shipped module, which is the central configurability promise and had been
documented as working since it was written.

## The tools that exist now

| | |
|---|---|
| `dev/gate.sh` | fmt, clippy, tests, build, and that the Lua configuration loads |
| `dev/app-check.sh <program>` | a client runs, draws, and provokes no protocol error |
| `dev/cursor-check.sh` | the pointer is visible over empty desktop |
| `dev/clipboard-check.sh` | copy and paste across the X11 boundary, all four ways |
| `cargo run -p wl-probe` | the protocols answer, from a real client's side |
| `SOLIUM_OUTPUTS=2 dev/run-nested.sh` | two monitors, without a second monitor |
| `WL_PROBE_BAR=… WL_PROBE_HOLD=…` | a bar lands on the monitor it named, and takes its zone from that one |

`wl-probe` is the one to extend, and extending it paid for itself the same
afternoon: asked to anchor a bar, it found that layer surfaces had never been
sent an initial configure. Every bar and every dock had been invisible for as
long as the compositor has claimed to support them, and nothing else was ever
going to notice — no shipped check used the protocol, and nothing installed on
this machine does. A compositor cannot test its own protocol
support from the inside, and "the global is advertised" is a different claim
from "a client that uses it gets the right answers". Every protocol added from
here should get a check in it.
