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

Do not read them as an expectation. Whether a guard is visibly clobbered depends
on Qt's deferred deletes colliding with the integers the compositor's own
textures happen to have been given, and that is not stable run to run. Measured
in the configuration where the teardown control reaches C-1
(`WIRECHECK_KEEP_RESIZED_SCENE=1`, below): **0 of 6 runs** clobbered any guard,
while all 6 failed on the census with the identical five objects. So a control
run showing `0 of 8` guards touched is not a control that has stopped working —
the census is what says so, and it said so every time. The guards are an
illustration of what the damage means; the census is the proof that it happened.

The free-path case also asserts its own precondition, through
`wirecheck_belief_names_scene`: Qt's thread-local must name *this* scene's
context and EGL must disagree. Neither half alone is enough. A null belief is
safe with or without the fix, and so is a belief naming a different live scene,
because `ensureContext()` compares it against its own `ctx` and corrects. The
first version of that guard asked only whether *some* belief existed and passed
with the fix reverted.

## Negative controls

A harness that passes both ways proves nothing, and this one has been in that
state four times. `clear_stale_current_context` has two call sites and each
guards a different defect, so there are two controls and both are worth running
after any change here.

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
('f', 1), ('r', 1), ('b', 2), ('f', 2), ('r', 2), ('b', 3)]`, of which buffers
1, 2 and 3 appear in the pre-Qt census. Measured identical in 6 of 6 runs.

That is the **resize case** failing, not C-1: the resized scene is freed in
exactly C-1's ordering, so it reaches the same defect first and the run stops
there — C-1 was not reached in any of those 6 runs. Run the control a second
time with the free skipped, so C-1 executes its own instrument:

```sh
WIRECHECK_KEEP_RESIZED_SCENE=1 ./target-control/debug/wirecheck   # must fail
```

Expect `DESTROYED by Qt's teardown, in our context: [('b', 1), ('f', 1),
('r', 1), ('b', 2), ('f', 2)]`, `...of which existed before Qt was started, so
are certainly ours: [('b', 1), ('b', 2)]` — measured identical in 6 of 6 runs,
with **0 of 8 guard textures clobbered in any of them**. Both runs matter and
they check different things: the first that the defect is caught, the second
that C-1's own census, precondition assertion and guards still execute at all.
Without the knob they only ever run in the passing state, where a regression in
the instrument would be invisible.

Two things this measurement settled, against what was written here first:

* C-1 does **not** need the resize case's census to protect its baseline. With
  that free skipped, C-1's diff is the full five objects, not a clean one. The
  census in the resize case earns its place by catching the damage where it
  happens; the earlier claim that C-1 would otherwise diff clean was reasoned,
  not run, and is wrong.
* "Most of the guard textures clobbered outright" was also reasoned. See the
  paragraph on the guards above: it reproduces sometimes and not here.

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

That control only works *because* of the wipe. `quadrants.qml` paints an
unchanging picture, so before the wipe existed the dmabuf still held frame 1's
identical pixels and the comparison read zero: "Qt did not write" and "Qt wrote
the same thing" were the same measurement, and this control exited 0 with the
defect present. Do not remove the wipe, and do not make the probe scene static
in a way that survives it.

Verify the substitution took, rather than assuming the rebuild noticed:

```sh
diff ../../crates/solium/qml/host.cpp host-control.cpp   # exactly one line
```

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
| `WIRECHECK_HOST_CPP` | a different `host.cpp`, for the controls above |
| `WIRECHECK_REBUILD_ON_RESIZE` | rebuild the scene on a resize instead of rebinding it — the resize control above |
| `WIRECHECK_KEEP_RESIZED_SCENE` | do not free the resized scene, so the teardown control reaches C-1 |
| `WIRECHECK_RESTORE_EARLY=0` | skip the restore after `scene_new_gpu` |
| `WIRECHECK_LATE_RENDERER`, `WIRECHECK_SEPARATE_GBM` | build the renderer after Qt, or on its own device |

The last three exist because each was once a hypothesis for a failure that
turned out to be something else, and re-testing them is cheaper than arguing
about them again.
