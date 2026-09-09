# QML GPU Render Target Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Qt render QML into a GPU buffer the compositor can sample, instead of rasterising on the CPU.

**Architecture:** Qt keeps its own EGL context — adopting ours is impossible on this platform plugin. Instead both sides share a *buffer*: the compositor allocates it through GBM, Qt binds it as an `EGLImage`-backed texture and renders into it via `QQuickRenderTarget::fromOpenGLTexture`, and the compositor imports the same dmabuf with the `import_dmabuf` path it already uses for every client buffer. A fence orders the two.

**Tech Stack:** Rust, Smithay 0.7, Qt 6 Quick (C++), GBM, EGL, `EGL_ANDROID_native_fence_sync`, `EGL_EXT_image_dma_buf_import`.

**Spec:** `docs/superpowers/specs/2026-09-08-pane-styles-design.md` (section *Prerequisite: the GPU render target*)

## Global Constraints

- Rust edition 2024. Workspace lints **deny** `unwrap_used`, `expect_used`, `panic`, `todo`. `unsafe_code` is `warn` — every `unsafe` block needs `#[expect(unsafe_code, reason = "…")]`.
- **All builds run in podman**, never on the host: `dev/gate.sh` wraps it. The host has no Qt 6 development files.
- `QQuickRenderControl::initialize()` **must not be called** under the software adaptation, and **must** be called for the RHI path. The two paths differ here and the difference is load-bearing.
- Qt's `QQuickRenderControl::initialize()` makes its own context current on the calling thread. **The compositor's context must be restored after every Qt render**, or the next `eglMakeCurrent` fails with `BAD_ACCESS`.
- The software path stays working and stays the fallback. A machine where this fails must still run.
- Never launch `claude-desktop` inside Solium while testing.


## Where this stands — 2026-09-09

Tasks 1–6 are committed on `qml-gpu-render-target`. **Task 6's review has not
been dispatched.** The SDD ledger is git-ignored and does not travel with this
branch, so what it holds that matters for resuming is below.

### Next action

Package `ba05a15..HEAD` and review Task 6 (`ce66cd6`). Four deviations, all
flagged by the implementer rather than found later:

1. **Task 6's Step 6 verification could not fail as written.** `dev/wirecheck`
   does not link the compositor crate — it compiles `host.cpp` from source and
   drives raw FFI — so `render_on_gpu` is not in the binary and reverting the
   change left every run byte-identical. Replaced with a permanent knob,
   `WIRECHECK_REBUILD_ON_RESIZE`, at the level the harness measures.
2. **`QQuickRenderControl::initialize()` replaced by a new `take_the_thread`.**
   Re-calling `initialize()` re-runs the render context's `initialize()` and
   re-emits `sceneGraphInitialized` under a live item tree. Whether the old
   path made anything current at all rests on `initRhi`'s early return, which
   could not be read (no Qt sources on that box). **Unverified — settle it at
   review.** It is the same shape as the stale-thread-local class below.
3. **`Scene::rebind_sized` added**, mirroring the existing `gpu`/`gpu_sized`
   pair, because the plan's Step 5 does not compile — `qml::allocator` and
   `qml::target` are private. Better than widening visibility and putting GBM
   in `surface.rs`.
4. **`surface.rs`'s half has no automated check.** Nothing in the gate runs
   `render_on_gpu`. Reviewed by reading; Task 8 is where it meets hardware.

A bug in the plan's own C++ was caught during implementation: it set
`scene->width`/`height` *after* `import_dmabuf_texture`, which reads exactly
those for `EGL_WIDTH`/`EGL_HEIGHT`, so it would have imported every resized
buffer at the old size, silently. Fixed in `ce66cd6`.

### The thing to be most suspicious of

**`dev/wirecheck` has been in a state where it could not detect its own target
bug five separate times.** Textures asserted when Qt's deferred releases were
buffers; structurally unable to produce the unsafe free ordering; a new
build-then-free case clearing Qt's thread-local and blinding the C-1 case; a
static reference against an unchanging fixture so "Qt did not write" and "Qt
wrote the same thing" read identically; and now Task 6's case tripping the
teardown control earlier so it no longer reaches C-1's guard textures. Every
time it reported success while measuring nothing.

The pattern is structural, not five accidents: **cases sharing one process keep
silently changing each other's preconditions.** Treat it as a whole-branch
review item. The two negative controls — the free path and the render path in
`clear_stale_current_context` — are documented in `dev/wirecheck/README.md`
with their expected output, and both must bite, and bite *orthogonally*.

### Invariants established by Tasks 5 and 6

- `solium_qml_scene_new_gpu` and `solium_qml_scene_free` both leave **nothing
  current** on the thread. `ShellSurface::new` has no renderer to restore to,
  so the obligation lives in the host rather than at call sites. Safe because
  every `GlesRenderer` entry point re-binds its own context.
- **No GPU-scene entry point may run while a `GlesFrame` is alive.** There are
  two `with_context` methods in smithay: `GlesRenderer::with_context` re-binds,
  `GlesFrame::with_context` does not, and the latter is the one Solium calls.
  Enforced by `qml::no_frame_in_flight`, backed by RAII guards at five sites.
  **Task 7 is where this stops being latent** — a decoration that builds or
  renders its scene lazily from inside `RenderElement::draw` trips it.
- **This failure class is not silent from Qt's side.** With the scene buffer
  wiped, the broken render path emits `Framebuffer incomplete: 0x8cd6`,
  `Failed to build texture render target for QQuickRenderTarget` and
  `QQuickWindow: No render target`. Nothing was looking. Task 8 greps for them.

### Two open items on `main`, neither started

- **Animated decorations render zero frames.** `render.rs:182` reads
  `decoration.animating()` *after* `decoration.frame()` rendered, and `frame()`
  clears Qt's dirty flag — so `state.redraw` is never set and the next frame
  never comes. Measured at zero frames in 60 s, confirmed three ways. Same
  class as the bug fixed in `9eeb13b`.
- **The performance figure justifying this plan is mis-cited.** "About a tenth
  of a core" is real — commit `14e49d2`, `pulse 1.32s -> 0.78s per 8s` — but it
  is a whole-compositor number of which ~90% is not QML, it describes a
  full-width gradient no shipped decoration produces, and the pane-styles spec
  attributes it to `docs/spikes/2026-09-04-qml-in-compositor.md`, which has
  never contained it. Measured at decoration geometry it is ~0.74% of a core.
  **The work is still justified — by the pane-styles load, not by today's.**
  Three layers per window maximised on a 2560×1440 260 Hz panel costs Qt
  1.94 ms plus 2.58 ms of serial main-thread memcpy against a 3.846 ms vblank
  budget, and 41 MB/frame. Rewrite the justification to say that.

Every measurement above was taken **nested in winit, never on DRM**, and three
adversarial passes rated the numbers *shaky*. Task 8's TTY run settles both the
hardware validation and this.

---

### Task 1: Detect whether this machine can do the dmabuf round-trip

The whole plan rests on an untested assumption. This task answers it in isolation, before anything depends on it.

**Files:**
- Modify: `dev/qtprobe/probe.cpp`
- Modify: `dev/qtprobe/README.md`

**Interfaces:**
- Consumes: nothing.
- Produces: a runnable probe printing `dmabuf: ok` or a named failure. No API.

- [ ] **Step 1: Extend the probe to allocate a GBM buffer and import it into Qt's context**

Add to `dev/qtprobe/probe.cpp`, after the existing context check:

```cpp
#include <gbm.h>
#include <fcntl.h>
#include <unistd.h>
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>

static bool probe_dmabuf(EGLDisplay display)
{
    int drm = open("/dev/dri/renderD128", O_RDWR | O_CLOEXEC);
    if (drm < 0) { printf("dmabuf: no render node\n"); return false; }

    gbm_device *gbm = gbm_create_device(drm);
    if (!gbm) { printf("dmabuf: no gbm device\n"); close(drm); return false; }

    gbm_bo *bo = gbm_bo_create(gbm, 256, 256, GBM_FORMAT_ARGB8888,
                               GBM_BO_USE_RENDERING);
    if (!bo) { printf("dmabuf: gbm_bo_create failed\n"); return false; }

    int fd = gbm_bo_get_fd(bo);
    const int stride = static_cast<int>(gbm_bo_get_stride(bo));
    const uint64_t modifier = gbm_bo_get_modifier(bo);

    EGLint attribs[] = {
        EGL_WIDTH, 256, EGL_HEIGHT, 256,
        EGL_LINUX_DRM_FOURCC_EXT, GBM_FORMAT_ARGB8888,
        EGL_DMA_BUF_PLANE0_FD_EXT, fd,
        EGL_DMA_BUF_PLANE0_OFFSET_EXT, 0,
        EGL_DMA_BUF_PLANE0_PITCH_EXT, stride,
        EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, static_cast<EGLint>(modifier & 0xffffffff),
        EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, static_cast<EGLint>(modifier >> 32),
        EGL_NONE,
    };
    EGLImageKHR image = eglCreateImageKHR(display, EGL_NO_CONTEXT,
                                          EGL_LINUX_DMA_BUF_EXT, nullptr, attribs);
    if (image == EGL_NO_IMAGE_KHR) {
        printf("dmabuf: eglCreateImageKHR failed 0x%x\n", eglGetError());
        return false;
    }

    GLuint texture = 0;
    glGenTextures(1, &texture);
    glBindTexture(GL_TEXTURE_2D, texture);
    glEGLImageTargetTexture2DOES(GL_TEXTURE_2D, image);
    const GLenum error = glGetError();
    if (error != GL_NO_ERROR) {
        printf("dmabuf: glEGLImageTargetTexture2DOES failed 0x%x\n", error);
        return false;
    }

    printf("dmabuf: ok  texture=%u stride=%d modifier=0x%llx\n",
           texture, stride, static_cast<unsigned long long>(modifier));
    return true;
}
```

Call `probe_dmabuf(display)` from `main` after the context report, and print
whether `EGL_ANDROID_native_fence_sync` is in
`eglQueryString(display, EGL_EXTENSIONS)`.

- [ ] **Step 2: Build and run the probe**

```bash
cd /home/kotoxik/personal_projects/solium/dev/qtprobe
podman run --rm --userns=keep-id --security-opt label=disable -v "$HOME:$HOME" \
  -w "$PWD" localhost/solium-build:fc44 \
  sh -c 'g++ -fPIC probe.cpp -o probe $(pkg-config --cflags --libs Qt6Gui gbm egl glesv2) -lEGL'
QT_QPA_PLATFORM=offscreen ./probe
```

Expected: `dmabuf: ok  texture=… stride=… modifier=0x…` and the fence extension listed.

- [ ] **Step 3: Record the answer**

Append the finding to `dev/qtprobe/README.md` under a new `## dmabuf round-trip` heading — the values printed, the date, and the Qt and driver versions from `rpm -q qt6-qtbase` and `glxinfo -B | head -3`.

**Also update the build command already in that README**: it does not link gbm,
egl or glesv2, so after this change it no longer builds the probe. A recorded
finding beside a command that does not work is worse than no record.

**If this task fails**, stop the plan and report. Everything below assumes it passed, and the spec records what to do instead: bound the ambition to one animated layer with modest bleed on the software path.

- [ ] **Step 4: Commit**

```bash
git add dev/qtprobe/probe.cpp dev/qtprobe/README.md
git commit -m "dev: probe whether Qt can render into a dmabuf we allocated"
```

---

### Task 2: Give the host a GPU scene constructor

**Files:**
- Modify: `crates/solium/qml/host.h`
- Modify: `crates/solium/qml/host.cpp`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `int solium_qml_start_gpu(const char *import_path);` — returns 1 on success, 0 if the GPU path is unavailable.
  - `SoliumQmlScene *solium_qml_scene_new_gpu(const char *qml_path, int width, int height, int dmabuf_fd, int stride, unsigned long long modifier, unsigned int fourcc, const char *initial_json);`
  - `int solium_qml_scene_render_gpu(SoliumQmlScene *scene, int *fence_fd);` — renders and returns a fence fd through the out-parameter; caller owns and closes it. Returns 1 rendered, `SOLIUM_QML_UNCHANGED`, or 0 on failure.

- [ ] **Step 1: Declare the three entry points in `host.h`**

```c
/* Start the host on the RHI (OpenGL) scene graph rather than the software one.
 *
 * Returns 1 when Qt came up on the GPU, 0 when it did not — in which case the
 * caller must fall back to solium_qml_start(). Only one of the two may be
 * called in a process: the scene graph backend is chosen once. */
int solium_qml_start_gpu(const char *import_path);

/* A scene that renders into a buffer we allocated.
 *
 * `dmabuf_fd` is borrowed for the call; the host dups it if it needs to keep
 * it. `modifier` is the DRM format modifier, `fourcc` the DRM fourcc. */
SoliumQmlScene *solium_qml_scene_new_gpu(const char *qml_path, int width, int height,
                                         int dmabuf_fd, int stride,
                                         unsigned long long modifier,
                                         unsigned int fourcc,
                                         const char *initial_json);

/* Render, and hand back a fence that signals when the GPU is done.
 *
 * `*fence_fd` is set to -1 when the driver gave no fence, which the caller must
 * treat as "finished" only after glFinish. Ownership passes to the caller. */
int solium_qml_scene_render_gpu(SoliumQmlScene *scene, int *fence_fd);
```

- [ ] **Step 2: Implement `solium_qml_start_gpu`**

In `host.cpp`, beside the existing `solium_qml_start`:

```cpp
extern "C" int solium_qml_start_gpu(const char *import_path)
{
    if (started) {
        return gpu_mode ? 1 : 0;
    }
    // The RHI path is selected by *not* naming the software backend, and by
    // asking for OpenGL explicitly. Both matter: the default backend is
    // chosen from the platform and is not OpenGL everywhere.
    QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
    if (!start_common(import_path)) {
        return 0;
    }
    gpu_mode = true;
    return 1;
}
```

Extract the body `solium_qml_start` shares — `QGuiApplication` creation, import
path, engine setup — into `static bool start_common(const char *import_path)`
and have both call it. Add `static bool gpu_mode = false;` beside `started`.

- [ ] **Step 3: Implement the GPU scene constructor**

```cpp
extern "C" SoliumQmlScene *solium_qml_scene_new_gpu(const char *qml_path, int width,
                                                    int height, int dmabuf_fd, int stride,
                                                    unsigned long long modifier,
                                                    unsigned int fourcc,
                                                    const char *initial_json)
{
    if (!started || !gpu_mode) {
        qWarning("scene_new_gpu called without a GPU host");
        return nullptr;
    }

    auto *scene = new SoliumQmlScene;
    scene->control = new QQuickRenderControl;
    scene->window = new QQuickWindow(scene->control);

    // The RHI path *requires* initialize(); the software path forbids it.
    if (!scene->control->initialize()) {
        qWarning("QQuickRenderControl::initialize failed");
        delete scene;
        return nullptr;
    }

    // Qt is now current on its own context. Import our buffer into it.
    scene->texture = import_dmabuf_texture(dmabuf_fd, width, height, stride,
                                           modifier, fourcc);
    if (scene->texture == 0) {
        delete scene;
        return nullptr;
    }
    scene->window->setRenderTarget(
        QQuickRenderTarget::fromOpenGLTexture(scene->texture, QSize(width, height)));

    if (!load_component(scene, qml_path, width, height, initial_json)) {
        delete scene;
        return nullptr;
    }
    return scene;
}
```

`import_dmabuf_texture` is the body of `probe_dmabuf` from Task 1 with the
allocation removed — it takes an fd and returns a `GLuint`. Put it in
`host.cpp` as a `static GLuint`. `load_component` is the existing component
loading from `solium_qml_scene_new_with`, extracted so both constructors share
it.

- [ ] **Step 4: Implement the fenced render**

```cpp
extern "C" int solium_qml_scene_render_gpu(SoliumQmlScene *scene, int *fence_fd)
{
    if (!scene || !fence_fd) {
        return 0;
    }
    *fence_fd = -1;
    if (!scene->control->sync() && !scene->dirty) {
        return SOLIUM_QML_UNCHANGED;
    }
    scene->control->render();

    // A fence, so our context knows when Qt's work has actually landed.
    // Without it this is a race that shows as intermittent garbage, which is
    // the worst way to find out.
    EGLDisplay display = eglGetCurrentDisplay();
    EGLSyncKHR sync = eglCreateSyncKHR(display, EGL_SYNC_NATIVE_FENCE_ANDROID, nullptr);
    if (sync != EGL_NO_SYNC_KHR) {
        glFlush();
        *fence_fd = eglDupNativeFenceFDANDROID(display, sync);
        eglDestroySyncKHR(display, sync);
    } else {
        // No fence available: the only correct fallback is to wait here.
        glFinish();
    }
    scene->dirty = false;
    return 1;
}
```

- [ ] **Step 5: Build**

```bash
cd /home/kotoxik/personal_projects/solium
podman run --rm --userns=keep-id --security-opt label=disable -v "$HOME:$HOME" \
  -e CARGO_HOME="$HOME/.cargo" -e PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin" \
  -w "$PWD" localhost/solium-build:fc44 sh -c 'cargo build -p solium -j2'
```

Expected: compiles. Nothing calls the new functions yet.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/qml/host.h crates/solium/qml/host.cpp
git commit -m "qml: a scene that renders into a buffer we allocated"
```

---

### Task 3: Allocate the buffer and hold it, Rust side

**Files:**
- Create: `crates/solium/src/qml/target.rs`
- Modify: `crates/solium/src/qml.rs`

**Interfaces:**
- Consumes: the FFI from Task 2.
- Produces:
  - `pub(crate) struct Target { pub(crate) dmabuf: Dmabuf, width: i32, height: i32 }`
  - `pub(crate) fn allocate(gbm: &GbmDevice<DrmDeviceFd>, width: i32, height: i32) -> Result<Target>`
  - `impl Target { pub(crate) fn as_ffi(&self) -> Result<(RawFd, i32, u64, u32)> }` — fd, stride, modifier, fourcc, for `solium_qml_scene_new_gpu`.

- [ ] **Step 1: Write the failing test**

`crates/solium/src/qml/target.rs`, at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::size_is_sane;

    /// A zero or negative size reaches GBM as a huge unsigned number and
    /// allocates something enormous, or fails in a way that names neither the
    /// scene nor the size.
    #[test]
    fn a_zero_sized_target_is_refused() {
        assert!(!size_is_sane(0, 100));
        assert!(!size_is_sane(100, 0));
        assert!(!size_is_sane(-4, 100));
    }

    /// The cap exists because `bleed` is author-controlled: a style asking for
    /// 20000 pixels of bleed must be told no, not allocate 1.6GB.
    #[test]
    fn an_absurd_size_is_refused() {
        assert!(!size_is_sane(20_000, 20_000));
        assert!(size_is_sane(2560, 1440));
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
podman run --rm --userns=keep-id --security-opt label=disable -v "$HOME:$HOME" \
  -e CARGO_HOME="$HOME/.cargo" -e PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin" \
  -w "$PWD" localhost/solium-build:fc44 \
  sh -c 'cargo test -p solium -j2 target::tests'
```

Expected: FAIL — `cannot find function size_is_sane`.

- [ ] **Step 3: Write the module**

```rust
//! The buffer Qt renders a scene into.
//!
//! Allocated by us through GBM and handed to Qt as a texture, because Qt
//! cannot be made to use our EGL context — see
//! `docs/spikes/2026-09-04-qml-in-compositor.md`. Sharing the buffer is the
//! route that remains, and it is how two processes would do it anyway.

use anyhow::{Context, Result, anyhow};
use smithay::backend::allocator::{
    Buffer as _, Fourcc, Modifier,
    dmabuf::{AsDmabuf, Dmabuf},
    gbm::{GbmDevice, GbmBufferFlags},
};
use smithay::backend::drm::DrmDeviceFd;
use std::os::fd::{AsRawFd, RawFd};

/// The largest scene we will allocate, per side.
///
/// A style's `bleed` is author-controlled, so this is the difference between
/// a typo and 1.6 GB of video memory.
const MAX_SIDE: i32 = 8192;

fn size_is_sane(width: i32, height: i32) -> bool {
    width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE
}

pub(crate) struct Target {
    pub(crate) dmabuf: Dmabuf,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

/// Allocate a buffer both sides can use.
pub(crate) fn allocate(
    gbm: &GbmDevice<DrmDeviceFd>,
    width: i32,
    height: i32,
) -> Result<Target> {
    if !size_is_sane(width, height) {
        return Err(anyhow!("a scene of {width}x{height} is not a size"));
    }
    let buffer = gbm
        .create_buffer_object::<()>(
            u32::try_from(width)?,
            u32::try_from(height)?,
            Fourcc::Argb8888,
            GbmBufferFlags::RENDERING,
        )
        .context("allocating a scene buffer")?;
    let dmabuf = buffer.export().context("exporting the scene buffer")?;
    Ok(Target { dmabuf, width, height })
}

impl Target {
    /// What the QML host needs to import this: fd, stride, modifier, fourcc.
    pub(crate) fn as_ffi(&self) -> Result<(RawFd, i32, u64, u32)> {
        let fd = self
            .dmabuf
            .handles()
            .next()
            .ok_or_else(|| anyhow!("the scene buffer has no plane"))?
            .as_raw_fd();
        let stride = self
            .dmabuf
            .strides()
            .next()
            .ok_or_else(|| anyhow!("the scene buffer has no stride"))?;
        let modifier: Modifier = self.dmabuf.format().modifier;
        Ok((
            fd,
            i32::try_from(stride)?,
            u64::from(modifier),
            self.dmabuf.format().code as u32,
        ))
    }
}
```

Add `mod target;` to `crates/solium/src/qml.rs`.

- [ ] **Step 4: Run the tests**

Same command as Step 2. Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/solium/src/qml/target.rs crates/solium/src/qml.rs
git commit -m "qml: allocate the buffer Qt renders a scene into"
```

---

### Task 4: A GPU scene, end to end, behind an environment variable

> **Corrections from Tasks 1–3.** All of the following were measured on this
> machine after this plan was written. Where they contradict the steps below,
> these win.
>
> 1. **Qt must be pointed away from the card node, or it may take DRM master.**
>    `eglfs_kms` needs no master and cannot take one by asking — but the kernel
>    grants master *implicitly* to whoever opens the node when nothing holds
>    it. `start_scripts` runs before `open_gpu`, and `Command::Spawn` reaches
>    `qml::start()`, so a user config with a top-level `sol.spawn(…)` starts Qt
>    first. Qt's fd becomes master, logind's `SetMaster` then returns `EBUSY`,
>    and the only symptom is Smithay's `unable to become drm master` warning —
>    which `tty.rs` documents as benign noise. A black screen on a TTY with
>    nothing to read.
>
>    Before `QGuiApplication`, write a KMS config and set three variables:
>
>    ```json
>    { "device": "<Solium's own render node>", "headless": "64x64" }
>    ```
>    ```
>    QT_QPA_EGLFS_KMS_CONFIG=<that file>
>    QT_QPA_EGLFS_DISABLE_INPUT=1
>    QT_QPA_EGLFS_KMS_NO_EVENT_READER_THREAD=1
>    ```
>
>    Both JSON keys are required: `device` alone fails with
>    `drmModeGetResources failed (Permission denied)`; `headless` alone still
>    opens `card1`. Measured with this config: zero `card1` opens, zero
>    ioctls, `initialize()` true, and real QML rendered into the dmabuf.
>    `QT_QPA_EGLFS_DEVICE` does not exist in this Qt build. Pass the render
>    node in — `open_gpu` already computes it at `tty.rs:1110-1113` — rather
>    than hard-coding it, which also handles a second GPU.
>
> 2. **The host does not dup the fd.** The text below and Step 2's SAFETY
>    comment both say it does. It keeps nothing; EGL takes its own reference
>    during `eglCreateImageKHR`. Verified by closing the fd immediately after
>    import and rendering correctly anyway. Do not "fix" the Rust side on the
>    old premise.
>
> 3. **The `Scene` must keep its `Target` alive.** Qt does not need it after
>    the constructor, but Task 5 re-imports the same dmabuf to sample it, and
>    nothing in the struct below holds a reference. Store the `Target` in the
>    `Scene`, or pair them.
>
> 4. **Do not create a texture.** `import_dmabuf_texture` already does
>    `glGenTextures`/`glBindTexture` and hands the name to Qt. Rust supplies
>    only fd, stride, modifier and fourcc.
>
> 5. **Restore the compositor's context after *both* rendering and freeing.**
>    `initialize()` leaves Qt's context current on return from the
>    constructor, and Qt's teardown leaves *no* context current on return from
>    `scene_free` (`eglGetCurrentContext()` is NULL, measured in both
>    orderings). The global constraint only mentions rendering; it is wider
>    than that.
>
> 6. **`start_gpu` cannot fall back.** Qt calls `qFatal` on a platform plugin
>    it cannot load — verified, SIGABRT, exit 134 — so the process dies rather
>    than returning 0. Any availability decision must be made *before*
>    calling it. And `start_gpu` returning 1 is not evidence the path works:
>    a scene-level failure leaves `g_gpu_mode` true with no in-process
>    fallback, so with `SOLIUM_QML_GPU` set every scene would fail hard.
>    Decide that deliberately rather than by omission.
>
> 7. **Stale build artefacts give false passes.** Task 3's commit added a
>    second `target/debug/build/solium-*/out/` directory; a glob over that path
>    links whichever it finds first, silently. Pin with `ls -t | head -1`.

**Files:**
- Modify: `crates/solium/src/qml.rs`
- Modify: `crates/solium/src/surface.rs`
- Modify: `crates/solium/src/dev.rs`

**Interfaces:**
- Consumes: `target::allocate`, `Target::as_ffi`, the FFI from Task 2.
- Produces:
  - `Scene::gpu(qml_path: &Path, width: i32, height: i32, target: &Target, initial: Option<&str>) -> Result<Self>`
  - `Scene::render_gpu(&mut self) -> Result<Option<Option<OwnedFd>>>` — outer `None` when the scene was unchanged; inner `None` when the driver gave no fence and the host waited with `glFinish` instead.
  - `dev::qml_gpu() -> bool`, reading `SOLIUM_QML_GPU`.

- [ ] **Step 1: Add the dev knob**

In `crates/solium/src/dev.rs`:

```rust
/// Render QML on the GPU rather than the CPU.
///
/// Off by default while the dmabuf path proves itself. The software path is
/// the fallback and must keep working: a machine where this fails still has
/// to run a desktop.
pub(crate) fn qml_gpu() -> bool {
    std::env::var_os("SOLIUM_QML_GPU").is_some()
}
```

- [ ] **Step 2: Add the FFI declarations and the two `Scene` methods**

In the `ffi` block in `qml.rs`:

```rust
unsafe extern "C" {
    fn solium_qml_start_gpu(import_path: *const c_char) -> c_int;
    fn solium_qml_scene_new_gpu(
        qml_path: *const c_char,
        width: c_int,
        height: c_int,
        dmabuf_fd: c_int,
        stride: c_int,
        modifier: c_ulonglong,
        fourcc: c_uint,
        initial_json: *const c_char,
    ) -> *mut SoliumQmlScene;
    fn solium_qml_scene_render_gpu(scene: *mut SoliumQmlScene, fence_fd: *mut c_int) -> c_int;
}
```

And on `Scene`:

```rust
    /// A scene that renders into `target` on the GPU.
    pub(crate) fn gpu(
        qml_path: &Path,
        width: i32,
        height: i32,
        target: &target::Target,
        initial: Option<&str>,
    ) -> Result<Self> {
        let (fd, stride, modifier, fourcc) = target.as_ffi()?;
        let path = CString::new(qml_path.as_os_str().as_encoded_bytes())?;
        let initial = initial.map(CString::new).transpose()?;
        // SAFETY: every pointer outlives the call; the host copies what it
        // keeps, and dups the fd if it holds one.
        #[expect(unsafe_code, reason = "calling into the Qt host")]
        let scene = unsafe {
            solium_qml_scene_new_gpu(
                path.as_ptr(),
                width,
                height,
                fd,
                stride,
                modifier,
                fourcc,
                initial.as_ref().map_or(std::ptr::null(), |it| it.as_ptr()),
            )
        };
        if scene.is_null() {
            return Err(anyhow!("the GPU scene would not load: {}", qml_path.display()));
        }
        Ok(Self { scene, width, height })
    }

    /// Render, returning a fence that signals when Qt's work has landed.
    ///
    /// `Ok(None)` means the scene was already up to date. A `None` fence
    /// inside `Some` means the driver gave none and the host waited instead.
    pub(crate) fn render_gpu(&mut self) -> Result<Option<Option<OwnedFd>>> {
        let mut fence: c_int = -1;
        // SAFETY: the scene is ours and alive; `fence` is a live local.
        #[expect(unsafe_code, reason = "calling into the Qt host")]
        let result = unsafe { solium_qml_scene_render_gpu(self.scene, &raw mut fence) };
        match result {
            0 => Err(anyhow!("the GPU render failed")),
            UNCHANGED => Ok(None),
            _ if fence < 0 => Ok(Some(None)),
            // SAFETY: the host handed ownership of this fd to us.
            #[expect(unsafe_code, reason = "taking ownership of a fence fd")]
            _ => Ok(Some(Some(unsafe { OwnedFd::from_raw_fd(fence) }))),
        }
    }
```

- [ ] **Step 3: Choose the backend at startup**

In `qml::start()`, before the existing `solium_qml_start` call:

```rust
    if crate::dev::qml_gpu() {
        // SAFETY: the path outlives the call.
        #[expect(unsafe_code, reason = "calling into the Qt host")]
        let ok = unsafe { solium_qml_start_gpu(path.as_ptr()) };
        if ok == 1 {
            tracing::info!("QML on the GPU");
            STARTED.store(true, Ordering::Release);
            return Ok(());
        }
        // Not fatal. The software path is why this is a fallback and not a
        // requirement: a machine where the dmabuf round-trip fails still has
        // to run a desktop.
        tracing::warn!("SOLIUM_QML_GPU is set and the GPU path would not start; using software");
    }
```

- [ ] **Step 4: Build and run nested with the knob off, to prove nothing regressed**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh
./target/debug/solium
```

Expected: gate passes; the compositor comes up exactly as before, with a
wallpaper and window frames. `SOLIUM_QML_GPU` unset means nothing changed.

- [ ] **Step 5: Commit**

```bash
git add crates/solium/src/qml.rs crates/solium/src/surface.rs crates/solium/src/dev.rs
git commit -m "qml: a GPU scene behind SOLIUM_QML_GPU, software still the default"
```

---

### Task 5: Import the rendered buffer and draw it

**Files:**
- Modify: `crates/solium/src/surface.rs`
- Modify: `crates/solium/src/tty.rs`

**Interfaces:**
- Consumes: `Scene::gpu`, `Scene::render_gpu`, `Target`.
- Uses from Smithay: `EGLFence::import(display, OwnedFd)`, `SyncPoint::from(EGLFence)`, `Renderer::wait(&SyncPoint)`, `GlesRenderer::egl_context()`, `EGLContext::make_current()` (unsafe).
- Produces: `ShellSurface::element` unchanged in signature, now returning a texture element when the surface is GPU-backed.

- [ ] **Step 1: Hold a GBM device where scenes can reach it**

`Target::allocate` needs the GBM device, which today lives in `tty::State::gbm`
and does not exist at all in the nested backend. Add to `qml.rs`:

```rust
/// The allocator scenes are rendered through, set once by the backend.
///
/// A global because a `Scene` is created from wherever a frame is first
/// needed — a decoration, a wallpaper, a panel — and threading an allocator
/// through every one of those call sites would put GBM in the signature of
/// things that have no business knowing what GBM is.
static ALLOCATOR: OnceLock<GbmDevice<DrmDeviceFd>> = OnceLock::new();

pub(crate) fn set_allocator(gbm: GbmDevice<DrmDeviceFd>) {
    let _ = ALLOCATOR.set(gbm);
}

pub(crate) fn allocator() -> Option<&'static GbmDevice<DrmDeviceFd>> {
    ALLOCATOR.get()
}
```

Call `crate::qml::set_allocator(gbm.clone());` in `tty::State::open_gpu`,
immediately after the `GbmDevice` is created.

The nested backend has no GBM device, so `allocator()` is `None` there and
`ShellSurface` stays on the software path. Say so in the log once, at startup:
`tracing::info!("nested: QML stays on the software path, no GBM device")`.

- [ ] **Step 2: Give `ShellSurface` a GPU variant**

```rust
enum Backing {
    /// Rasterised on the CPU into a shared-memory buffer.
    Memory(Option<MemoryRenderBuffer>),
    /// Rendered by Qt into a dmabuf we allocated.
    Gpu {
        target: crate::qml::target::Target,
        /// The imported texture, and the fence from the render that filled it.
        texture: Option<GlesTexture>,
    },
}
```

In `ShellSurface::element`, when `Backing::Gpu`:

```rust
            Backing::Gpu { target, texture } => {
                let rendered = self.scene.render_gpu()?;
                if let Some(fence) = rendered {
                    // Wait for Qt before sampling. Without this the texture is
                    // read while it is still being written, which appears as
                    // intermittent garbage on one frame in a few hundred —
                    // the hardest possible thing to attribute.
                    //
                    // `Renderer::wait` takes a `SyncPoint`, not a raw fd, so
                    // the fence is imported first. `EGLFence::import` is the
                    // only constructor that takes a native fence fd.
                    if let Some(fd) = fence {
                        let display = renderer.egl_context().display();
                        let egl_fence = EGLFence::import(display, fd)
                            .context("importing Qt's fence")?;
                        renderer.wait(&SyncPoint::from(egl_fence))?;
                    }
                    *texture = Some(renderer.import_dmabuf(&target.dmabuf, None)?);
                }
                let texture = texture.as_ref()?;
                Some(TextureRenderElement::from_static_texture(
                    Id::from(self.id),
                    renderer.id(),
                    area.loc.to_physical_precise_round(scale),
                    texture.clone(),
                    scale as i32,
                    Transform::Normal,
                    Some(1.0),
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                ))
            }
```

`Element` needs the texture variant; `Element::Screen` is already
`TextureRenderElement<GlesTexture>` and can be reused rather than adding a
sixth arm.

- [ ] **Step 3: Restore our EGL context after every Qt render**

In `render_gpu`'s Rust caller, immediately after the FFI returns:

```rust
        // Qt's `initialize()` made its own context current on this thread and
        // `render()` leaves it that way. Ours has to go back before anything
        // else touches GL, or the next `eglMakeCurrent` fails with BAD_ACCESS
        // and the failure surfaces somewhere unrelated — in whichever draw
        // call happens to run next, not here.
        //
        // SAFETY: called on the thread that owns this context, with no other
        // context in use on it.
        #[expect(unsafe_code, reason = "restoring our EGL context after Qt")]
        unsafe {
            renderer.egl_context().make_current()?;
        }
```

`EGLContext::make_current` is the API; there is no `bind_context`. It is
`unsafe` because Qt could have destroyed the context, which it does not.

- [ ] **Step 4: Build and check nothing regressed with the knob off**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh
```

Expected: gate passes.

- [ ] **Step 5: Commit**

```bash
git add crates/solium/src/surface.rs crates/solium/src/tty.rs crates/solium/src/qml.rs
git commit -m "qml: sample the buffer Qt rendered, fenced, with our context restored"
```

---

### Task 6: Resize a GPU scene without rebuilding it

**Files:**
- Modify: `crates/solium/qml/host.cpp`
- Modify: `crates/solium/qml/host.h`
- Modify: `crates/solium/src/qml.rs`
- Modify: `crates/solium/src/surface.rs`
- Modify: `dev/wirecheck/main.rs`

**Interfaces:**
- Consumes: `Target`, `target::allocate`, `Scene::gpu`, `scene_context_is_current`.
- Produces: `Scene::rebind(&mut self, target: Target, width: i32, height: i32, scale: f64) -> Result<()>`, and `solium_qml_scene_rebind` on the C side.

`render_on_gpu` currently answers a size change by calling `build(...)` — a
fresh `QQuickRenderControl`, a fresh `QOpenGLContext`, a fresh `QRhi`, a fresh
GBM allocation and a recompiled QML tree. `render.rs` sizes a pane's scene from
an *animating* rect, so that whole stack is constructed and destroyed at frame
rate for the length of every window animation.

The cost is the smaller half. The rebuilt QML tree is a new object tree, so
every animation, transition and stored property inside the scene restarts from
zero on every frame it is resized. A scene that animates while its window
animates does not run slowly — it never advances. That is a correctness
problem, not a performance one, and it is why this task comes before the
decoration conversion rather than after it: decorations are the scenes that
resize.

Only the *buffer* genuinely cannot be resized. Everything above it can stay.

- [ ] **Step 1: Write the failing check**

In `dev/wirecheck/main.rs`, add a case that proves the object tree survives a
resize. The QML holds a counter that only a fresh tree resets:

```rust
    // A scene that counts its own frames. A rebuild produces a new object tree
    // and the count restarts; a true in-place resize carries it across.
    let scene = build_gpu_scene(FIXTURE_COUNTER, 256, 256)?;
    for _ in 0..5 {
        scene.render_gpu()?;
    }
    let before = scene.property_int("frames")?;
    scene.rebind(target::allocate(gbm, 384, 384)?, 384, 384, 1.0)?;
    scene.render_gpu()?;
    let after = scene.property_int("frames")?;
    ensure!(
        after > before,
        "the QML tree was rebuilt by a resize: frames went {before} -> {after}, \
         so every animation in a resizing scene restarts every frame"
    );
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh wirecheck
```

Expected: FAIL — `rebind` does not exist yet. Add it as `todo!()` only if the
build needs it to get to a running failure, and never leave a `todo!()` behind:
the workspace lints deny it.

- [ ] **Step 3: Add `solium_qml_scene_rebind` to the host**

The scale-only branch of `solium_qml_scene_resize` already does everything a
full resize needs except swapping the texture underneath it. Take that shape and
give it a new buffer.

In `host.h`, beside the other GPU entry points:

```c
/* Point an existing GPU scene at a different buffer.
 *
 * Everything above the buffer — the render control, the RHI, the QML object
 * tree and its animation state — is kept. Only the EGLImage and the texture
 * are replaced, which is the whole of what a dmabuf's fixed size forces.
 *
 * `width` and `height` are device pixels and must match the new buffer.
 * Returns false and leaves the scene on its previous buffer on failure, so a
 * surface whose resize failed keeps drawing last frame's picture. */
bool solium_qml_scene_rebind(SoliumQmlScene *scene, int dmabuf_fd, int stride,
                             unsigned long long modifier, unsigned int fourcc,
                             int width, int height, double scale);
```

In `host.cpp`:

```cpp
extern "C" bool solium_qml_scene_rebind(SoliumQmlScene *scene, int dmabuf_fd, int stride,
                                        unsigned long long modifier, unsigned int fourcc,
                                        int width, int height, double scale)
{
    if (scene == nullptr || !scene->gpu || width <= 0 || height <= 0 || dmabuf_fd < 0) {
        return false;
    }
    if (scale <= 0.0) {
        scale = 1.0;
    }

    // Qt's thread-local may name Qt's context while the compositor's is the one
    // actually current — see clear_stale_current_context. Everything below
    // issues GL, so the thread has to be honestly Qt's first.
    clear_stale_current_context(scene);
    if (!scene->control->initialize()) {
        qWarning("solium_qml_scene_rebind: could not make the scene's context current");
        return false;
    }

    // Import first, release second. An import that fails leaves the scene whole
    // and still drawing, which is the difference between a dropped frame and a
    // black window.
    const EGLImageKHR previous_image = scene->egl_image;
    const GLuint previous_texture = scene->texture;
    scene->egl_image = EGL_NO_IMAGE_KHR;
    scene->texture = 0;

    if (!import_dmabuf_texture(scene, dmabuf_fd, stride, modifier, fourcc)) {
        scene->egl_image = previous_image;
        scene->texture = previous_texture;
        qWarning("solium_qml_scene_rebind: the new buffer would not import, "
                 "staying on the old one");
        return false;
    }

    if (previous_image != EGL_NO_IMAGE_KHR && scene->egl_display != EGL_NO_DISPLAY) {
        static PFNEGLDESTROYIMAGEKHRPROC destroy_image =
            reinterpret_cast<PFNEGLDESTROYIMAGEKHRPROC>(
                eglGetProcAddress("eglDestroyImageKHR"));
        if (destroy_image != nullptr) {
            destroy_image(scene->egl_display, previous_image);
        }
    }
    if (previous_texture != 0 && scene_context_is_current(scene)) {
        QOpenGLContext *context = QOpenGLContext::currentContext();
        if (context != nullptr) {
            context->functions()->glDeleteTextures(1, &previous_texture);
        }
    }

    scene->width = width;
    scene->height = height;
    scene->scale = scale;

    const int logical_width = qMax(1, qRound(width / scale));
    const int logical_height = qMax(1, qRound(height / scale));
    scene->window->setGeometry(0, 0, logical_width, logical_height);
    if (scene->root != nullptr) {
        scene->root->setWidth(logical_width);
        scene->root->setHeight(logical_height);
    }

    QQuickRenderTarget target =
        QQuickRenderTarget::fromOpenGLTexture(scene->texture, QSize(width, height));
    target.setDevicePixelRatio(scale);
    mirror_for_the_compositor(&target);
    scene->window->setRenderTarget(target);
    scene->dirty = true;
    return true;
}
```

Then replace the refusal in `solium_qml_scene_resize` — the `qWarning` about a
GPU scene not being resizable in place — with a pointer to this function, since
it is now false as written:

```cpp
        if (width != scene->width || height != scene->height) {
            qWarning("solium_qml_scene_resize cannot change a GPU scene's pixel "
                     "size (%dx%d to %dx%d): it has no buffer to change it to. "
                     "Use solium_qml_scene_rebind with one.",
                     scene->width, scene->height, width, height);
            return;
        }
```

- [ ] **Step 4: Add `Scene::rebind`**

In `qml.rs`, beside `gpu`:

```rust
    /// Move this scene onto a different buffer, keeping everything above it.
    ///
    /// The QML object tree survives, which is the point: rebuilding it restarts
    /// every animation inside the scene, and a pane's scene is resized on every
    /// frame of a window animation.
    pub(crate) fn rebind(
        &mut self,
        target: target::Target,
        width: i32,
        height: i32,
        scale: f64,
    ) -> Result<()> {
        let plane = target.as_ffi()?;
        // SAFETY: `scene` is ours and live, and `plane` borrows a buffer that
        // outlives this call.
        #[expect(unsafe_code, reason = "handing Qt a buffer we allocated")]
        let ok = unsafe {
            ffi::solium_qml_scene_rebind(
                self.scene,
                plane.fd,
                plane.stride,
                plane.modifier,
                plane.fourcc,
                width,
                height,
                scale,
            )
        };
        if !ok {
            bail!("Qt would not rebind the scene onto a {width}x{height} buffer");
        }
        // Held only after the host has taken its own reference, so a failed
        // rebind leaves the previous target in place and still being drawn.
        self.target = Some(target);
        Ok(())
    }
```

- [ ] **Step 5: Use it from `render_on_gpu`**

In `surface.rs`, replace the rebuild:

```rust
        if self.size != size {
            // Only the buffer's size is fixed. Rebuilding the scene around a
            // new one would restart every animation in it, once per frame, for
            // as long as the window is animating.
            let gbm = qml::allocator()
                .context("this backend has no GBM device, so it cannot resize a GPU scene")?;
            let target = qml::target::allocate(gbm, size.0.max(1), size.1.max(1))?;
            self.scene.rebind(target, size.0.max(1), size.1.max(1), scale)?;
            self.size = size;
            self.backing = Backing::Gpu(None);
            self.damage.reset();
        } else {
            self.scene.resize(size.0, size.1, scale);
        }
```

`Backing::Gpu(None)` still clears the imported texture: the compositor's side
of the dmabuf is a different import and genuinely is a new buffer.

- [ ] **Step 6: Run the check**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh wirecheck
```

Expected: PASS, and the frame counter carries across the resize.

Then confirm it is a real check by reverting Step 5 to `build(...)` and running
again. Expected: FAIL. Report both runs. A check that passes either way proves
nothing.

- [ ] **Step 7: Commit**

```bash
git add crates/solium/qml/host.cpp crates/solium/qml/host.h \
        crates/solium/src/qml.rs crates/solium/src/surface.rs dev/wirecheck/main.rs
git commit -m "qml: resize a GPU scene onto a new buffer instead of rebuilding it"
```

---

### Task 7: Put the cursor and the decorations on the GPU path

**Files:**
- Modify: `crates/solium/src/cursor.rs:58`
- Modify: `crates/solium/src/decoration.rs:197`
- Modify: `crates/solium/src/main.rs:150`

**Interfaces:**
- Consumes: `Scene::gpu_sized`, `Scene::rebind`, `qml::on_gpu`, `surface::build`.
- Produces: nothing new. This is the task that makes `SOLIUM_QML_GPU=1` a
  desktop rather than a wallpaper.

Task 5 converted `surface.rs`. Two other places build scenes and neither was
converted, so on a GPU host they hit the refusal at `host.cpp:435` — "this
process came up on the GPU scene graph and a software scene cannot" — and
return `nullptr`.

The result today, with the knob on: no window frames and no cursor. Not a
degraded desktop, an absent one. The hardware task below cannot observe what it
is written to observe until this lands.

- [ ] **Step 1: Write the failing check**

In `dev/wirecheck/main.rs`, assert that a GPU host builds every kind of scene
the compositor actually builds:

```rust
    // Every scene the compositor builds, on a GPU host. Before this task the
    // cursor and the decoration went down solium_qml_scene_new, which a GPU
    // host refuses outright — so the knob produced a desktop with no frames
    // and no pointer, and nothing in the log said which scenes were missing.
    for (what, path, w, h) in [
        ("cursor", CURSOR_QML, 64, 64),
        ("decoration", DECORATION_QML, 640, 480),
    ] {
        let scene = build_like_the_compositor(path, w, h)
            .with_context(|| format!("a GPU host could not build the {what} scene"))?;
        scene.render_gpu()?;
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh wirecheck
```

Expected: FAIL, with the `qWarning` from `host.cpp:435` in the output.

- [ ] **Step 3: Route both through the path-aware constructor**

`surface.rs`'s `build` already reads which path Qt came up on. It is the only
correct way to construct a scene and it should not be private to one module.
Move it to `qml.rs` as `Scene::for_host(source, width, height, properties)`,
leave `surface::build` delegating to it, and use it from both sites.

`cursor.rs:58` becomes:

```rust
        // `SIZE` square, and the cursor is the one scene that is never resized
        // — so on the GPU path its buffer is allocated once and kept.
        let scene = qml::Scene::for_host(&qml_path(), SIZE, SIZE, None)?;
```

`decoration.rs:197` becomes:

```rust
        let mut scene = qml::Scene::for_host(path, width.max(1), height.max(1), None)?;
```

`main.rs:150` validates a QML file for `--check` and runs before any host is
started, so it stays on the software constructor. Say so where it sits:

```rust
    // `--check` never starts a GPU host, so this is a software scene by
    // construction rather than by preference.
```

- [ ] **Step 4: Give the decoration's resize the same treatment as Task 6**

A decoration resizes with its window, which is the case Task 6 exists for.
Find the size-change path in `decoration.rs` and route it through
`Scene::rebind` when `qml::on_gpu()`, exactly as `surface.rs` does. If it
currently calls `Scene::resize` unconditionally, that call now warns on every
window resize instead of working — the host refuses a pixel-size change and
says so — so this step is not optional.

- [ ] **Step 5: Run the check and the gate**

```bash
cd /home/kotoxik/personal_projects/solium && dev/gate.sh
```

Expected: the whole gate passes, wirecheck included.

- [ ] **Step 6: Commit**

```bash
git add crates/solium/src/cursor.rs crates/solium/src/decoration.rs \
        crates/solium/src/main.rs crates/solium/src/qml.rs crates/solium/src/surface.rs \
        dev/wirecheck/main.rs
git commit -m "qml: build the cursor and the decorations on whichever path Qt came up on"
```

---

### Task 8: Prove it on hardware

**Files:**
- Modify: `dev/README.md`
- Modify: `crates/solium/src/dev.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: a recorded finding, and the knob's default decided.

This is the only task that needs a person at a free TTY. Everything it looks
for is something no harness can assert from inside a nested session.

- [ ] **Step 1: Run on a TTY with the knob on**

From a free TTY (`Ctrl+Alt+F3`), not from inside a session:

```bash
SOLIUM_QML_GPU=1 ./target/debug/solium --tty
```

Expected: the desktop comes up. The wallpaper draws, window frames draw, and
the cursor is a cursor. All three only became possible in Task 7 — before it a
GPU host refused every software scene, so the frames and the pointer were
simply absent.

Use a **debug** build. There is no `[profile.release]` in `Cargo.toml`, so
`debug-assertions` is off in release and `qml::no_frame_in_flight`'s
`debug_assert_eq!` is compiled out — a release TTY run gets only the latched
log line, which says something happened once and then never again.

- [ ] **Step 2: Read Qt's own diagnostics, not only the pixels**

Task 5 established that this failure is not silent from Qt's side. With the
buffer wiped, a scene rendering into the wrong context emits:

```
Framebuffer incomplete: 0x8cd6
Failed to build texture render target for QQuickRenderTarget
QQuickWindow: No render target
```

Nothing was looking for those. Look now:

```bash
journalctl --user -b 0 -o cat | grep -iE 'QQuick|Framebuffer|render target|GlesFrame|no_frame_in_flight'
```

Expected: nothing. Any hit names the defect directly and is worth more than
any amount of pixel comparison.

- [ ] **Step 3: Look for the failure this is most likely to have**

A missing or wrong fence shows as intermittent corruption rather than a crash,
so it must be looked for deliberately rather than waited for. With a terminal
open and a decoration that animates:

```bash
SOLIUM_QML_GPU=1 SOLIUM_DECORATION=pulse SOLIUM_CAPTURE=/tmp/gpu \
  SOLIUM_CAPTURE_FRAMES=200 SOLIUM_CAPTURE_INTERVAL=16 ./target/debug/solium --tty
```

Then compare every captured frame against its neighbours: a frame that differs
from both the one before and the one after by a large, non-contiguous region is
a torn read.

```bash
python3 - <<'EOF'
import glob
frames = sorted(glob.glob('/tmp/gpu-*'))
def load(p):
    d = open(p,'rb').read(); parts = d.split(b'\n',3)
    w,h = map(int, parts[1].split()); return w,h,parts[3]
for i in range(1, len(frames)-1):
    _,_,a = load(frames[i-1]); w,h,b = load(frames[i]); _,_,c = load(frames[i+1])
    differs = sum(1 for j in range(0, w*h*3, 331) if b[j] != a[j] and b[j] != c[j])
    if differs > 50:
        print(f"{frames[i]}: {differs} samples differ from BOTH neighbours")
EOF
```

Expected: no lines printed. Any output is a torn read and the fence is wrong.

Compare neighbours rather than a stored golden frame. The shell draws a clock,
so a golden frame reports a false regression on the next calendar day — a
pre-task commit was measured disagreeing with *itself* by 1122 bytes across a
day boundary.

- [ ] **Step 4: Confirm the gate ran rather than skipped**

`dev/gate.sh` runs `dev/wirecheck` on the host and skips *visibly* when there
is no render node or `SOLIUM_GATE_NO_GPU` is set — but a skip still leaves the
gate green.

```bash
dev/gate.sh 2>&1 | grep -iE 'wirecheck|skipped'
```

Expected: the run, not `skipped: no /dev/dri/renderD128`. This is the first
box where `crates/solium/src/tty.rs`'s frame-in-flight mark runs for real.

- [ ] **Step 5: Record the finding and decide the default**

Append to `dev/README.md` under `## QML on the GPU`: whether it worked, the
frame-pacing numbers from the log with and without, whether tearing was found,
and whether anything from Step 2 appeared.

If it is clean, flip the default: `qml_gpu()` returns `true` unless
`SOLIUM_QML_SOFTWARE` is set, and say so in the same paragraph. If it is not
clean, leave the knob off and record what was seen — the plan below it does not
depend on the default, only on the path existing.

- [ ] **Step 6: Commit**

```bash
git add dev/README.md crates/solium/src/dev.rs
git commit -m "qml: what the GPU path does on real hardware"
```
