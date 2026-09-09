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

**A scene built and freed without ever rendering.** What a resize rebuild does to
the scene it replaces, and the path that reaches
`clear_stale_current_context`'s null-thread-local early return.

**A scene freed with the compositor's context current** — the ordering the rest
of the harness cannot produce, since building the replacement first leaves the
*new* scene's context current and Qt then does the right thing by accident.

That last one is checked with a census of live GL object names
(`glIsTexture`/`glIsBuffer`/`glIsFramebuffer`/`glIsProgram`/`glIsRenderbuffer`
over 1..64) in the compositor's context, taken before and after the free and
diffed. A third census is taken **before Qt is started at all**, so anything
destroyed that appears in it is unambiguously the compositor's.

The census is the assertion and it was not the first thing tried. Guard textures
drawn and compared across the free passed with the bug present *and* absent,
because Qt's deferred releases for this scene were buffers and a framebuffer,
not textures — a texture-only check cannot see a `glDeleteBuffers`. The guards
are still here, because they are the check that would catch a `glDeleteTextures`,
but they are the illustration and the census is the proof.

## Negative controls

A harness that passes both ways proves nothing. Build a deliberately broken
`host.cpp` and point the crate at it:

Keep the copy inside the repository. The build runs in the container, which
mounts `$HOME` and nothing else, so a control under `/tmp` compiles to
`fatal error: no such file`.

```sh
cd dev/wirecheck
# Drop the clear_stale_current_context call from solium_qml_scene_free only,
# leaving the one in solium_qml_scene_render_gpu, so this isolates the teardown.
awk '/^extern "C" void solium_qml_scene_free/ {f=1}
     /^extern "C" void solium_qml_scene_resize/ {f=0}
     f && /clear_stale_current_context\(scene\);/ {print "    /* control */"; next}
     {print}' ../../crates/solium/qml/host.cpp > host-control.cpp

WIRECHECK_HOST_CPP="$PWD/host-control.cpp" cargo build --target-dir target-control
./target-control/debug/wirecheck
```

`host-control.cpp` and `target-control/` are both gitignored.

Expect it to fail, naming the compositor's own GL buffers 1 and 2 as destroyed
by Qt's teardown. Removing the call in `solium_qml_scene_render_gpu` instead
gives the other one: every frame reads back as zeros while `render_gpu` returns
success and a fence that signals.

## Knobs

| | |
|---|---|
| `argv[1]` | render node, default `/dev/dri/renderD128` |
| `WIRECHECK_SCALE`, `WIRECHECK_LOGICAL` | the scale and logical size to run at |
| `WIRECHECK_FRAMES` | how many frames after the first, default 3 |
| `WIRECHECK_QML` | the scene to render, default `quadrants.qml` beside this file |
| `WIRECHECK_HOST_CPP` | a different `host.cpp`, for the controls above |
| `WIRECHECK_RESTORE_EARLY=0` | skip the restore after `scene_new_gpu` |
| `WIRECHECK_LATE_RENDERER`, `WIRECHECK_SEPARATE_GBM` | build the renderer after Qt, or on its own device |

The last three exist because each was once a hypothesis for a failure that
turned out to be something else, and re-testing them is cheaper than arguing
about them again.
