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

Do not read them as an expectation in either direction. Whether a guard is
visibly clobbered depends on Qt's deferred deletes landing on the integers the
compositor's own textures happen to have been given, and that collision is not
stable run to run. The census is: it has been identical in every run of every
configuration anyone has logged here.

The frequency, since a number is what stops the guessing. Across configurations
the guards fire in a small minority of runs — measured 2 of 24 — and when they
do fire it is **6 of 8**, not one or two. In the single configuration this file
gives an `Expect` for (`WIRECHECK_KEEP_RESIZED_SCENE=1`, below) they have not
fired at all: 0 of 8 clobbered in 30 of 30 runs on this machine, while all 30
failed on the census with the identical five objects.

So neither reading is a finding. A control run reporting `8 of 8 compositor
textures survived` has **not** stopped working, and one reporting `6 of 8
visibly clobbered` has **not** found a new bug — it is the same defect, caught
by an instrument that only sometimes has anything to catch it with. Check the
census. The guards illustrate what the damage means; the census is the proof
that it happened.

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
('f', 1), ('r', 1), ('b', 2), ('f', 2), ('r', 2), ('b', 3)]`, of which buffers
**1 and 2** appear in the pre-Qt census. Measured identical in 6 of 6 runs.

Two of them and not three, and that distinction is the whole value of the line.
The pre-Qt census is `[('b', 1), ('b', 2), ('p', 3), ('p', 4), ('p', 5),
('p', 6), ('p', 7), ('p', 8), ('p', 9)]` — invariant across 25 logged runs of
every configuration. `('p', 3)` is a *program* named 3, and programs and buffers
are separate GL namespaces, so there is no buffer 3 for Qt to have destroyed:
`('b', 3)` is Qt's own. Do not hand-count this at all. The harness prints
`...of which existed before Qt was started, so are certainly ours`, that line is
the claim, and it reads `[('b', 1), ('b', 2)]`.

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
this machine. The guard line is **not** part of the expectation: it has read
`8 of 8 compositor textures survived` in all 30, and see the guards paragraph
above for why that is neither surprising nor a problem.

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
* "Most of the guard textures clobbered outright" was right about the magnitude
  and wrong about the reliability. When the guards fire it really is most of
  them — 6 of 8 — but they fire in a small minority of runs and in none of the
  30 measured here.

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
| `WIRECHECK_NO_RESTORE` | skip *every* restore of the compositor's context, not only the early one |
| `WIRECHECK_LATE_RENDERER`, `WIRECHECK_SEPARATE_GBM` | build the renderer after Qt, or on its own device |
| `WIRECHECK_RENDERER_NODE` | the node the renderer's own device comes from, with `WIRECHECK_SEPARATE_GBM`; defaults to `argv[1]` |

The last four exist because each was once a hypothesis for a failure that
turned out to be something else, and re-testing them is cheaper than arguing
about them again.
