# Does the compositor's half of the QML GPU path still work?

    cd dev/wirecheck && cargo build        # in the container, as always
    ./target/debug/wirecheck               # on the host, which is where the GPU is

`dev/gate.sh` runs both steps. Exit status is the result; there is nothing to
read unless it fails.

This drives the **real** `crates/solium/qml/host.cpp` — compiled from source by
`build.rs`, not linked out of `target/`, because a glob over
`target/debug/build/solium-*/out/` silently picks a stale archive — and the
**real** `crates/solium/src/qml/target.rs`, included by path. Both are relative
to this crate, so a worktree or somebody else's clone checks its own code.

Against them it stands up a genuine smithay `GlesRenderer`, built the way
`tty::State::open_gpu` builds one, and runs the sequence `ShellSurface::on_gpu`
runs:

    scene_new_gpu → restore our EGL context → render_gpu → restore
      → EGLFence::import → Renderer::wait → import_dmabuf
      → TextureRenderElement::draw into an offscreen target → read back

That sequence lives in `crates/solium/src/qml/paint.rs`, which is where the
wallpaper, the window frames and the pointer all reach it from — the harness
runs the same order by hand because nothing here links the compositor crate.

It needs a GPU and a Qt installation at run time, which is why it is here and
not a `#[test]`. It sets the `QT_QPA_EGLFS_*` environment itself, the same way
`qml::keep_qt_off_the_hardware` does, so it opens the render node and never the
card — it is safe to run inside a live session.

## What it asserts, and why each one is not obvious

**The picture matches the software path, byte for byte.** Not "looks right": the
same known image is uploaded through `import_memory` — which is top-down by
definition and is what the software shell path uses — and drawn through
identical element parameters. Comparing the two readbacks cancels whatever
convention the offscreen target itself has, which is the only way to check
orientation without asserting one. Run at scale 1, 2 and 1.25, because the
element's `src` is in device pixels and its `size` is logical, and a transform
that is right at 1 can be wrong everywhere else.

**Frames 2..N, not just the first.** The first frame after a scene is built is
safe under bugs the rest are not, because `initialize()` left Qt's context
current and nothing has taken it yet. Every one-frame probe this project has
written passed while the second frame was broken.

**A resize keeps the QML object tree.** `solium_qml_scene_rebind` puts a scene on
a new buffer without rebuilding it, and the cost of the alternative was never
mainly the allocation: a rebuilt scene is a *new object tree*, so every
animation, transition and stored property in it restarts from zero. A pane is
sized from an animating rectangle, so that happened once per frame for the
length of every window animation — an animation restarted every frame never
advances. Asserted with a counter `quadrants.qml` holds and nothing outside the
tree remembers, so a fresh tree hands back the declared default; and then with
the picture at the new size, through the same reference comparison as above,
because a rebind that returned true and left Qt on the *old* texture would carry
the counter across perfectly.

**And a running animation keeps running across it.** The counter is a proxy and
was always only a proxy: a tree that survived with every animation reset to its
`from:` carries the counter across perfectly and is still the bug. So
`quadrants.qml` also holds a `NumberAnimation` on `spin`, the harness advances
the compositor's clock with `solium_qml_tick` once per frame the way
`render::prepare` does, and the case asserts `spin` is *larger* after the rebind
than before it. Two instrument checks go with it, because a property that never
moves reads the same number twice whichever way the rebind went: `spin` must be
non-zero before the rebind (or nothing was animating and nothing was ticking),
and `WIRECHECK_STOP_THE_CLOCK` below is its negative control.

Measured at scale 1: `spin` reads 80 after five ticked frames and 96 after the
rebind and one more. Under `WIRECHECK_REBUILD_ON_RESIZE` it reads 80 -> 0, which
is printed in that control's own failure message.

**Every scene the compositor builds, on a GPU host.** The compositor's real
`qml/cursor.qml` and `qml/decorations/top.qml`, built through
`solium_qml_scene_new_gpu` and rendered. Before Task 7 those two went down the
*software* constructor, which a GPU host refuses outright — so `SOLIUM_QML_GPU=1`
gave a desktop with a wallpaper on it and no window frames and no pointer, each
refusal logged by its own caller as its own unrelated failure.

The files themselves and not a stand-in, because what is in question is whether
*these* come up under the RHI scene graph: `cursor.qml` draws through
`QtQuick.Shapes` with the curve renderer and `top.qml` lays out text and an
animated `Behavior`, none of which the four flat rectangles in `quadrants.qml`
touch. What it cannot check is the picture — there is no reference for a titlebar
here, and inventing one would assert today's design system rather than the path
— so it wipes the buffer through the compositor's own renderer first and asks
whether anything came back. That proves Qt built the component, brought up an
RHI for it, imported *our* dmabuf and wrote into it. It proves nothing about
what it drew.

It does **not** stand in for the defect on the Rust side. Nothing in this binary
links the compositor crate, so which constructor `cursor.rs` calls is invisible
here; what stops that regressing is that `Scene::gpu_sized` and
`Scene::with_properties` are now private to `qml.rs` and `Scene::for_host` is the
only way in from outside it.

**And whether the host can say that a scene is animating — both ways.**
`solium_qml_scene_animating`, which is what `render::Drawn` gates the next frame
on. It is a claim about *Qt* and not about anything in this repository: that a
`QQuickAbstractAnimation` driven by a `Behavior` or a `NumberAnimation on` sets
its own `running` property and clears it when it is done. `cargo test` cannot
ask — asking needs a live Qt — and the compositor's whole animation loop hangs
on the answer, because Qt's dirty flag means "something changed", which a
running animation does not do on every tick.

The two readings are taken in one run, in one process, against one driver, and
**they are this instrument's negative control**: no edited copy of anything is
needed, because a stub cannot satisfy both. `quadrants.qml` carries an animation
with `loops: Animation.Infinite` and is asserted to read 1, in the resize case,
on the same line that already asserts `spin` has moved. `cursor.qml` and
`decorations/top.qml` are built and rendered in the scene case above with
nothing written to them, and are asserted to read 0.

Verified by stubbing `solium_qml_scene_animating`'s return in a control copy,
both ways, with the harness otherwise untouched:

```
return 1;  ->  Error: `solium_qml_scene_animating` says the cursor scene
               (crates/solium/qml/cursor.qml) is animating, and nothing has
               written a property on it or declared an animation in it.
return 0;  ->  Error: `solium_qml_scene_animating` says nothing is animating in
               a scene whose `spin` just moved 80 units under an
               `Animation.Infinite`.
```

The `cursor.qml` reading is the one that matters most, and it is there to rule
out a specific wrong answer rather than a hypothetical one.
`QAnimationDriver::isRunning()` reads like the direct question and is not: a
driver is one object for the whole process, `quadrants.qml` has been animating
since long before the scene case runs, and `QAnimationDriver::advanceAnimation`
ends in `QUnifiedTimer::localRestart`, which starts the driver again whenever it
is not running **and nothing is registered at all** (qtbase v6.11.2,
`src/corelib/animation/qabstractanimation.cpp:333`). So a process-wide answer is
`true` on that line for ever, and a compositor gated on it never sleeps again —
measured separately at 400 frames of 400, never idle, once anything else damaged
the screen while an animation was finishing. This case fails on it.

Neither assertion allocates a GL object, so the teardown control's destroyed
list is unmoved by them: re-measured after adding this case, C-1 still reports
the same `[('b', 1), ('f', 1), ('r', 1), ('b', 2), ('r', 2), ('b', 3)]`.

**And whether an animation started from rest actually *runs*.** The question
both of the readings above are blind to, and the one the hardware was still
failing after them: an animation advanced past its own end in a single tick
satisfies `dirty` and `solium_qml_scene_animating` perfectly. `dirty` is raised,
because the value did change; `animating` is `true` on the frame the `Behavior`
starts and `false` a tick later, because the animation really has finished. The
compositor then asks for exactly the frames it should, and every one of them
shows the final value — a titlebar that does not slide out, it is simply there.

So this case reads the animated value itself. `appear.qml` beside this file is
one property with a `Behavior` on it, sliding 34 units over 260ms on a linear
curve, mirrored into an `int` so it can be read back out of the object tree. It
is settled for forty frames, `pointerInside` is written the way
`Decoration::tell` writes it, and the twenty readings that follow must include
at least one strictly between the two ends. Not that it *reaches* its end — it
does that either way, instantly, which is the bug.

**It has to run first, before any other scene exists**, and that is not a
stylistic choice. Qt zeroes its animation reference only on the edge from *no*
animations in the process to one (`QUnifiedTimer::startTimers`, qtbase v6.11.2,
`src/corelib/animation/qabstractanimation.cpp:378-389`), and `quadrants.qml`'s
`Animation.Infinite` holds that registry open from the moment it is built until
the process exits. On any later line the edge never happens, every delta is
16ms, and this case passes with the defect fully present. The precondition is
asserted through `wirecheck_anything_animating` rather than left to a comment,
so a case added ahead of it fails loudly instead of quietly making this one
vacuous.

It also has to settle for long enough that the clock passes the animation's own
duration. A harness whose clock starts at zero cannot see this defect at all,
because the bad delta *is* the clock: the forty settling frames put it at 640ms,
and a desktop where somebody hovers a window has been up for minutes.

**The clock control** — put the driver back to reporting the compositor's
uptime, which is a two-line diff and touches nothing else:

```sh
cd dev/wirecheck
sed -e 's|return m_elapsed - m_origin;|return m_elapsed; /* control */|' \
    -e 's|^            m_origin = elapsed;$|            /* control */ (void) anything_animating;|' \
    ../../crates/solium/qml/host.cpp > host-control-clock.cpp

WIRECHECK_HOST_CPP="$PWD/host-control-clock.cpp" cargo build --target-dir target-control-clock
./target-control-clock/debug/wirecheck   # must fail
```

```
  slid, frame by frame: [-34, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
  readings strictly between -34 and 0: 0
Error: the appear animation never took a step: ... from -34 to 0 with nothing
in between, over 20 frames of a 260ms animation.
```

against the shipped file's

```
  slid, frame by frame: [-34, -30, -28, -26, -24, -21, -19, -17, -15, -13, -11, -9, -7, -5, -3, -1, 0, 0, 0, 0]
  readings strictly between -34 and 0: 15
```

Both runs print `scene animating = true` on the frame that writes the property.
That is the point of printing it: the reading the previous case exists for
separates nothing here, and neither does `dirty`. The control's scene is
`running` on that frame and settled one tick later, which is exactly what a
healthy animation that has just finished looks like — because it has.

The scene is kept alive rather than freed, like the two in the scene case below
and for the same reason — a free here would reach C-1's defect in C-1's own
ordering, ahead of C-1's census.

**The pointer's route: the dmabuf read back and uploaded as memory.** The one
scene that does not reach the screen as a texture. smithay reaches a DRM cursor
plane only through `RenderElement::underlying_storage`, whose two variants are
`Wayland` and `Memory` (`renderer/element/mod.rs:103-109`); a
`TextureRenderElement` implements none and inherits `None`, so
`copy_element_to_cursor_bo` gives up on its first line. And nothing catches it
underneath: smithay's pixman fallback is behind `#[cfg(feature =
"renderer_pixman")]`, which is not in the compositor's feature list
(`crates/solium/Cargo.toml:24-43`), so the arm that compiles is the
`#[cfg(not(...))]` one at `drm/compositor/mod.rs:3244` and its failure branch is
a plain `return None`. A GPU pointer drawn as a texture loses the plane
silently, and every pointer motion on a TTY becomes a full composite and page
flip. So `cursor.rs` lets Qt draw into the dmabuf and then reads it straight
back into a `MemoryRenderBuffer`.

That hangs on a claim nobody had checked: what `copy_framebuffer` hands back is
byte-identical to what `import_memory` is given on the software path. Three
conventions meet there — Qt's render target, which `mirror_for_the_compositor`
flips; smithay's readback; and `MemoryRenderBuffer`'s top-down rows — and two of
them cancelling is not the same as all three agreeing. Checked against the known
picture directly and then through an element built from it.

It sits **before** the frame loop, deliberately, and reads the first frame: that
is the one frame every configuration of this harness draws correctly, so this
case fails only on its own defect. Reading a buffer the frame loop had already
found wrong would make it fail on the loop's defect and report it as a readback
fault — and an all-zero buffer, which is what the render control leaves, is
called out separately and sent back to the frame comparison rather than
described as a flipped or swizzled one.

It also prints what the round trip costs, because the whole argument for the
route is that the pointer pays it once per size per change rather than per
frame. The cost is **~65 µs fixed plus 3.5–4 ns per pixel** on this machine —
fitted over a sweep rather than read off a pair, since one run at two sizes
cannot separate the two terms:

| size | pixels | each |
|---|---|---|
| 24x24 | 576 | 66.9 µs |
| 48x48 | 2304 | 69.1 µs |
| 96x96 | 9216 | 110.8 µs |
| 192x192 | 36864 | 192.3 µs |
| 384x384 | 147456 | 605.8 µs |

**The pixel term is negligible at cursor sizes and only at cursor sizes.** At
24x24 it is ~2 µs of ~67, three per cent, so what the pointer pays really is the
round trip — and the `Kept` cache in `cursor.rs` makes even that once per size
per change. Read that scoped, because it expires immediately above the pointer:
at 384x384 the pixel term alone is ~540 µs and the whole call is 606, nine times
what the cursor pays for its entire readback. "It is the round trip, not the
pixels" is a claim about 24- and 48-pixel squares, and is not a reason to trim
the cache.

Set `WIRECHECK_LOGICAL` and `WIRECHECK_SCALE` to move the size this reports, and
re-run it several times at one size before comparing two: the spread within a
single size is wider than the gap between 24 and 48. Seven runs each gave
medians of 68.0 µs at 24x24 and 67.1 at 48x48, with 24x24 spanning 60.2–72.1 and
48x48 spanning 65.6–80.2. This file previously reported a single run in which
24x24 (63.5 µs) came out *slower* than 48x48 (59.8 µs) and treated that
inversion as evidence; it was noise, and at n=1 there was nothing there to read.

If the fixed term were ever milliseconds, the cache would not be enough and the
design would need revisiting — which is why the number is printed rather than
remembered.

**The first rebind, on a scene that has never rendered.** The shape production is
actually in, and the one the resize case above cannot reach — it renders five
frames first. Nothing on screen does: `ShellSurface::new` builds at 1x1 with its
size recorded as `(0, 0)`, and a `Decoration` is built at the client's size and
has to end up on the *outer* rect at the monitor's scale. Both take the rebind
branch on their very first frame, with `QQuickRenderControl::initialize()` the
only thing that has ever made the scene's context current — so `take_the_thread`
is working from what the *build* recorded, and the thread-local
`clear_stale_current_context` reasons about is either null or another scene's.
Checked with the same reference comparison as the resize case, because a rebind
that returned true and left Qt on the 1x1 texture would sail past a non-zero
check.

Neither of those two cases frees its scenes. That is deliberate: a free with the
compositor's context current is C-1's own ordering, so freeing here would reach
that defect before C-1's census is taken — and under the teardown control with
`WIRECHECK_KEEP_RESIZED_SCENE` set it would stop the run in the resize case's
place, putting C-1 back to only ever executing in the passing state. Nothing is
leaked past the run; Qt keeps its own reference to each buffer and the process is
about to exit. Verified: with those cases in place, the teardown control still
reaches C-1 and C-1 still reports the identical five objects.

That byte comparison is **not** a second check on the render path, and it is
worth knowing before anyone trims the frame loop on the strength of it. Measured
under the render control: the resize case reports `0 of 65536 bytes differ` and
passes, three runs of three, while the frame loop above it reports `frame 2:
10240 of 16384 bytes differ` and fails the run. It cannot see that defect and it
is right not to — `release_the_thread` at the end of a rebind leaves the thread
empty, so the `beginFrame` after one correctly takes it whether or not
`solium_qml_scene_render_gpu` clears a stale belief first. The frame loop is
still the only thing that catches it.

**A scene built and freed without ever rendering.** The path that reaches
`clear_stale_current_context`'s null-thread-local early return. It used to be
the hot path by accident, when every resize built one scene and freed another;
now that a resize does neither, this case is the only thing that takes it on the
free side.

**A scene freed with the compositor's context current** — the ordering the rest
of the harness cannot produce, since building the replacement first leaves the
*new* scene's context current and Qt then does the right thing by accident.

That last one is checked with a census of live GL object names
(`glIsTexture`/`glIsBuffer`/`glIsFramebuffer`/`glIsProgram`/`glIsRenderbuffer`
over 1..64) in the compositor's context, taken before and after the free and
diffed. A third census is taken **before Qt is started at all**, so anything
destroyed that appears in it is unambiguously the compositor's.

The census is the assertion and it was not the first thing tried. Guard textures
drawn and compared across the free once passed with the bug present *and*
absent, because Qt's deferred releases for that scene were buffers and a
framebuffer and a texture-only check cannot see a `glDeleteBuffers`.

Do not read them as an expectation in either direction. Whether a guard is
visibly clobbered depends on Qt's deferred deletes landing on the integers the
compositor's own textures happen to have been given, and that collision is not
stable run to run. The census is: it has been identical in every run of every
configuration anyone has logged here.

The frequency, since a number is what stops the guessing. **Sample: 30
consecutive runs on this machine** of the one configuration this file gives an
`Expect` for (`WIRECHECK_KEEP_RESIZED_SCENE=1`, below). The guards fired in
**5 of 30**, and every one of those five fired *completely* — `=> 0 of 8
compositor textures survived`, `(8 of 8 guard textures visibly clobbered)`. The
other 25 read `=> 8 of 8 compositor textures survived` and `(0 of 8 …)`. All 30
failed on the census, with the identical five objects and the identical
attribution to `[('b', 1), ('b', 2)]`.

Quote the sample size alongside the number, because this number moves. This file
previously reported 0 of 8 in 30 of 30 here, and **6 of 8** when they fire
elsewhere, and neither reproduces: the magnitude measured now is all-or-nothing.
That is not a regression, it is the paragraph above being true. What Qt's
deferred deletes destroy depends on which integers the compositor's own objects
happen to have by the time the free runs, and every case added ahead of C-1
since those runs shifts exactly that. Expect the next case added here to move it
again — re-measure rather than reasoning from this paragraph.

So neither reading is a finding. A control run reporting `8 of 8 compositor
textures survived` has **not** stopped working, and one reporting `0 of 8
compositor textures survived` has **not** found a new bug — it is the same
defect, caught by an instrument that only sometimes has anything to catch it
with. Check the census. The guards illustrate what the damage means; the census
is the proof that it happened.

The free-path case also asserts its own precondition, through
`wirecheck_belief_names_scene`: Qt's thread-local must name *this* scene's
context and EGL must disagree. Neither half alone is enough. A null belief is
safe with or without the fix, and so is a belief naming a different live scene,
because `ensureContext()` compares it against its own `ctx` and corrects. The
first version of that guard asked only whether *some* belief existed and passed
with the fix reverted.

## Negative controls

A harness that passes both ways proves nothing, and this one has been in that
state five times: four blind instruments, and once a control that could not
observe the thing it named. A sixth was a different shape and is worth knowing
about separately — the instrument was truthful and this file overclaimed it, so
a documented expectation that reproduced about one run in six taught the reader
to distrust a working control. Hence the frequencies quoted below rather than
whichever result the last run happened to give.

`clear_stale_current_context` has two call sites and each guards a different
defect, so there are two controls there and both are worth running after any
change here.

Keep the copies inside the repository. The build runs in the container, which
mounts `$HOME` and nothing else, so a control under `/tmp` compiles to
`fatal error: no such file`. Both filenames and both target directories are
gitignored.

**The teardown control** — drop the call in `solium_qml_scene_free`, leaving the
one in `solium_qml_scene_render_gpu`:

```sh
cd dev/wirecheck
awk '/^extern "C" void solium_qml_scene_free/ {f=1}
     /^extern "C" void solium_qml_scene_resize/ {f=0}
     f && /clear_stale_current_context\(scene\);/ {print "    /* control */"; next}
     {print}' ../../crates/solium/qml/host.cpp > host-control.cpp

WIRECHECK_HOST_CPP="$PWD/host-control.cpp" cargo build --target-dir target-control
./target-control/debug/wirecheck        # must fail
```

Expect `DESTROYED in our context by freeing the resized scene: [('b', 1),
('f', 1), ('r', 1), ('b', 2), ('r', 2), ('b', 3)]`, of which buffers **1 and 2**
appear in the pre-Qt census. Measured identical in 5 of 5 runs.

That list was seven objects until the pointer's readback case was added above it
— `('f', 2)` is no longer among them — and the reason is worth knowing rather
than being surprised by later. What Qt's deferred deletes destroy depends on
which integers the compositor's own objects happen to have been given by the
time the free runs, and any case added before this one shifts that. So **the
list is illustrative and the emptiness is the assertion**: the harness fails on
`!lost.is_empty()`, not on a particular set. Re-measure it after adding a case
here rather than treating a changed list as a finding. The pre-Qt census below
has not moved.

Two of them and not three, and that distinction is the whole value of the line.
The pre-Qt census is `[('b', 1), ('b', 2), ('p', 3), ('p', 4), ('p', 5),
('p', 6), ('p', 7), ('p', 8), ('p', 9)]` — invariant across 25 logged runs of
every configuration. `('p', 3)` is a *program* named 3, and programs and buffers
are separate GL namespaces, so there is no buffer 3 for Qt to have destroyed:
`('b', 3)` is Qt's own.

That comparison is **yours to make in this run**, and the two lines to make it
from are both printed: the pre-Qt census above, and the `DESTROYED …` list. This
run does not attribute them for you. It ends:

```
  DESTROYED in our context by freeing the resized scene: [('b', 1), ('f', 1), ('r', 1), ('b', 2), ('r', 2), ('b', 3)]
Error: freeing the resized scene destroyed 6 of the compositor's GL objects
```

and that is the whole of it. The attributed line — `...of which existed before
Qt was started, so are certainly ours` — is printed at `src/main.rs:1603`, which
is inside C-1, and this run never reaches C-1. Do not go looking for it here;
read it off the `WIRECHECK_KEEP_RESIZED_SCENE=1` run below, which does print it,
against the same unchanged pre-Qt census.

That is the **resize case** failing, not C-1: the resized scene is freed in
exactly C-1's ordering, so it reaches the same defect first and the run stops
there — C-1 was not reached in any of those 6 runs. Run the control a second
time with the free skipped, so C-1 executes its own instrument:

```sh
WIRECHECK_KEEP_RESIZED_SCENE=1 ./target-control/debug/wirecheck   # must fail
```

Expect `DESTROYED by Qt's teardown, in our context: [('b', 1), ('f', 1),
('r', 1), ('b', 2), ('f', 2)]` and `...of which existed before Qt was started,
so are certainly ours: [('b', 1), ('b', 2)]` — identical in 30 of 30 runs on
this machine. The guard line is **not** part of the expectation: across those
same 30 it read `8 of 8 compositor textures survived` in 25 and `0 of 8` in 5,
with nothing in between, and see the guards paragraph above for why either is
neither surprising nor a problem.

Both runs matter and they check different things: the first that the defect is
caught, the second that C-1's own census, precondition assertion and guard
textures still execute at all. Without the knob they only ever run in the
passing state, where a regression in the instrument itself would be invisible.

Two things measurement settled against what was written here first, both of
which had been reasoned rather than run:

* C-1 does **not** depend on the resize case's census to protect its baseline.
  With that free skipped, C-1's diff is the full five objects rather than a
  clean one. The resize case's census earns its place by catching the damage
  where it happens, not by shielding C-1.
* "Most of the guard textures clobbered outright" was right that the guards are
  unreliable and has now been wrong twice about the magnitude — first "most of
  them", then "6 of 8". Measured here across 30 runs of this configuration: they
  fire in 5, and when they fire it is all eight. The reliability half is the
  durable part; the magnitude is whatever this build's GL name allocation
  happens to produce.

**The render control** — drop the call in `solium_qml_scene_render_gpu` instead:

```sh
awk '/^extern "C" int solium_qml_scene_render_gpu/ {f=1}
     f && /clear_stale_current_context\(scene\);/ {print "    /* control */"; f=0; next}
     {print}' ../../crates/solium/qml/host.cpp > host-control-render.cpp

WIRECHECK_HOST_CPP="$PWD/host-control-render.cpp" cargo build --target-dir target-control-render
./target-control-render/debug/wirecheck  # must fail
```

Expect `frame 2: 10240 of 16384 bytes differ from the reference`, 10240 being
the reference's exact non-zero byte count — Qt issues the frame against the
compositor's context and writes nothing, so what is read back is the wipe.

That line is the expectation, not the exit: the frame loop only records, and
`worst` is checked at the very end of the run.

It used to stop well before that, at the resize case, with `the resize left GL
error 0x501 in the compositor's context` — and that was the harness's fault, not
the control's. `glGetError` pops one error per call, and none of the four probes
drained before the operation they were probing, so an error generated hundreds
of lines earlier was reported against the rebind and the reader was sent to code
that is fine. Each probed operation is now bracketed: `drain_gl_errors` empties
the queue immediately before it and prints anything it found as *not* the
operation's, and the probe reads once immediately after. Under this control the
run now prints `GL error(s) ["0x501"] already pending BEFORE the rebind` and
`glGetError after it: 0x0`, and goes on to fail where it should, on `the GPU
path does not match the software path`.

Dormant on this machine in the passing configuration — 3 of 3 runs drain
nothing — so this is an instrument that was wrong rather than one that was
failing.

That control only works *because* of the wipe. `quadrants.qml` paints an
unchanging picture, so before the wipe existed the dmabuf still held frame 1's
identical pixels and the comparison read zero: "Qt did not write" and "Qt wrote
the same thing" were the same measurement, and this control exited 0 with the
defect present. Do not remove the wipe, and do not make the probe scene static
in a way that survives it.

### Verify the substitution reached the *binary*

Two different questions, and for a long time only the first was asked here:

```sh
# 1. Was the control written? Exactly one changed line each.
diff ../../crates/solium/qml/host.cpp host-control.cpp
diff ../../crates/solium/qml/host.cpp host-control-render.cpp

# 2. Was the binary built from it? The build must be NEWER than the file.
for c in control control-render; do
    if [ "target-$c/debug/wirecheck" -nt "host-$c.cpp" ]; then
        echo "$c: binary is newer than the control it was built from"
    else
        echo "$c: STALE — this binary predates its own control"
    fi
done
```

The second check is not pedantry, it is a bug this harness actually had.
`build.rs` emitted `rerun-if-changed` for `host.cpp` and
`rerun-if-env-changed` for `WIRECHECK_HOST_CPP`, but nothing for the file that
variable *points at*. So editing a control copy while `host.cpp` was untouched
changed neither, cargo never re-ran the build script, and the stale object file
was linked.

Measured on this machine, with that line missing: `host-control-render.cpp` was
overwritten with a byte-identical copy of `host.cpp` — no control in it at all —
and rebuilt with the same env var and target directory. The build reported
`Finished ... in 0.04s` and compiled nothing, `diff` then declared the two files
identical, and the binary went on failing with `frame 2: 10240 of 16384 bytes
differ` — biting on a control that was no longer in its source. With the
`rerun-if-changed` for `host` restored, the identical sequence recompiled
(`Compiling wirecheck`, 6.5s) and the run came back `EXIT=0`,
`frame 2: 0 of 16384 bytes differ`, which is the correct answer for a binary
with no control in it.

`strings` on the binary cannot substitute for this: the awk replaces a *call*
with a comment, and neither is a string literal, so nothing distinguishes the
two binaries by content. The timestamp is what answers "was this built from what
is on disk now".

**The animation control** needs no copy of anything either.
`WIRECHECK_STOP_THE_CLOCK=1` leaves the rebind alone and simply stops ticking
from the rebind onward, so the tree is kept, the counter still counts, and the
animation assertion has to fail on its own:

```sh
WIRECHECK_STOP_THE_CLOCK=1 ./target/debug/wirecheck   # must fail
```

Expect `` `spin` went 80 -> 80 across a rebind and a frame the clock was
deliberately stopped for``, with `the counter reads 6` on the line above it — the
counter passing is half the point, since it is what shows the two instruments are
independent. The message is a different one from the unticked case's on purpose:
saying "across a ticked frame" under a control whose whole content is that the
frame was *not* ticked is the harness lying about its own run.

**The resize control** needs no copy of anything, because what it inverts is a
choice and not a line of C++. `WIRECHECK_REBUILD_ON_RESIZE=1` answers the resize
the way `render_on_gpu` used to — a new scene on the new buffer, the old one
freed after it exists — instead of calling `solium_qml_scene_rebind`:

```sh
WIRECHECK_REBUILD_ON_RESIZE=1 ./target/debug/wirecheck   # must fail
```

Expect `the QML tree was rebuilt by a resize: frames went 5 -> 1`. Run it at more
than one scale; it has been checked at 1, 2 and 1.25.

It is a knob here rather than a revert of `surface.rs` because nothing in this
binary links the compositor crate: putting `build(...)` back in `render_on_gpu`
changes nothing this runs. What `surface.rs` still owns is which of the two to
call, and that is one line under a size comparison.

## Knobs

| | |
|---|---|
| `argv[1]` | render node, default `/dev/dri/renderD128`; honoured by the independent readback too, which used to hardcode it |
| `WIRECHECK_SCALE`, `WIRECHECK_LOGICAL` | the scale and logical size to run at |
| `WIRECHECK_FRAMES` | how many frames after the first, default 3 |
| `WIRECHECK_QML` | the scene to render, default `quadrants.qml` beside this file |
| `WIRECHECK_HOST_CPP` | a different `host.cpp`, for the controls above; `host-control-clock.cpp` beside this file is the appear case's |
| `WIRECHECK_REBUILD_ON_RESIZE` | rebuild the scene on a resize instead of rebinding it — the resize control above |
| `WIRECHECK_STOP_THE_CLOCK` | stop ticking from the rebind onward — the animation control above |
| `WIRECHECK_KEEP_RESIZED_SCENE` | do not free the resized scene, so the teardown control reaches C-1 |
| `WIRECHECK_RESTORE_EARLY=0` | skip the restore after `scene_new_gpu` |
| `WIRECHECK_NO_RESTORE` | skip *every* restore of the compositor's context, not only the early one |
| `WIRECHECK_LATE_RENDERER`, `WIRECHECK_SEPARATE_GBM` | build the renderer after Qt, or on its own device |
| `WIRECHECK_RENDERER_NODE` | the node the renderer's own device comes from, with `WIRECHECK_SEPARATE_GBM`; defaults to `argv[1]` |

The last four exist because each was once a hypothesis for a failure that
turned out to be something else, and re-testing them is cheaper than arguing
about them again.
