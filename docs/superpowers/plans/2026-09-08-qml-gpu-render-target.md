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


## Where this stands — 2026-09-09 (evening)

**Tasks 1–7 are complete and reviewed. Task 8 is the only one left, and it needs
a person at a free TTY — it is not dispatchable.** The SDD ledger is git-ignored
and does not travel with this branch, so what matters for resuming is here.

`dev/gate.sh` passes. All five of `dev/wirecheck`'s negative controls bite with
their documented output. Nothing is pushed past `origin/qml-gpu-render-target`
and nothing is merged.

Three things landed after that review, all of them so the hardware session is
not wasted. Every `warn!`/`error!` inside `Gpu::sample` is now latched one line
per scene, which matters because the `error!` among them could have flooded the
journal and taken Step 2's evidence with it. `--check-qml` now starts a software
host explicitly rather than reading `SOLIUM_QML_GPU`, so it no longer reports
good QML as broken when run from the session's own shell. And Task 8 below
carries the thirteen extra steps that had only ever existed in the ledger.

**Since then: the multi-output rebind churn is fixed, and Step 5 is now a
verification rather than an expectation.** A window whose slot straddles a bezel
is drawn on both outputs at both scales, so `Gpu::render`'s `self.bound != size`
was true on every call and the scene rebound twice a frame for as long as the
window sat there. `Gpu` now keeps one imported picture per `(pixel size, scale)`,
capped at two — `Kept` in `crates/solium/src/qml/paint.rs`, which is the
pointer's own cache from Task 7, lifted out of `cursor.rs` and generalised over
its key and its cap rather than copied. The pointer keeps its own cache in front
of it, because that one short-circuits the readback as well and so is not made
redundant. Decorations, pane loading scenes and scripted shell surfaces all
reach `Gpu` and all inherit the fix. Coverage is in `qml::paint::tests`, since
`dev/wirecheck` cannot see `paint.rs` at all.

### Before running Task 8, know these

**The cursor does not take the GPU path, and that is deliberate.** A
GPU-rendered pointer cannot reach the DRM hardware cursor plane: smithay's
`UnderlyingStorage` has only `Wayland` and `Memory` variants, the `Wayland` arm
is shm-only, `try_assign_cursor_plane` has no direct-scanout branch, and
`renderer_pixman` is not in our feature list so the pixman fallback is not
compiled. Qt still draws the pointer — a GPU host refuses software scenes — and
the result is read back into a `MemoryRenderBuffer`. That readback costs
**≈65 µs fixed plus ≈3.7 ns/px**, so ~67 µs at cursor sizes, cached per device
size — in `Cursor`'s own `Kept`, which sits in front of `Gpu`'s and is what
keeps the readback itself off the per-frame path. The fixed cost dominates *at
cursor sizes only*; at 384×384 the pixel term alone is ~540 µs. Do not
generalise it.

**A failed rebind freezes, stretched, and retries.** It does not go invisible.
That was decided deliberately: insets are read once at build time and stay
reserved, so an absent decoration is a window with a hole above it, and an
absent pointer is indistinguishable from dead input. Stretched chrome is
visibly wrong and gets reported; absent chrome reads as a crash.

With one limit, which Task 8 Step 7 goes and looks at: it freezes on the last
frame it *has*. A rebind that fails on a scene's very first frame has no frame
to freeze on and draws nothing — and every `ShellSurface` starts there, built at
1×1 with its size recorded as `(0, 0)`. So the guarantee covers a scene that has
drawn at least once, which is every scene on a working desktop and not every
scene at start-up.

**The frame invariant is enforced.** No GPU-scene entry point may run while a
`GlesFrame` is alive — `GlesRenderer::with_context` re-binds, `GlesFrame::with_context`
does not, and the latter is what Solium calls. Backed by RAII guards at five
sites and asserted at four entry points. If it fires on hardware, that is an
ordering bug, not a false positive.

**This failure class is not silent from Qt's side.** A scene rendering into the
wrong context emits `Framebuffer incomplete: 0x8cd6`, `Failed to build texture
render target for QQuickRenderTarget` and `QQuickWindow: No render target`.
Nothing was looking for years. Task 8 greps for them.

**Everything so far was measured nested, never on DRM.** Task 8 is the first
real hardware evidence for the whole path.

### The harness is the most-corrected thing here

`dev/wirecheck` has been in a state where it could not see its own target bug,
or documented an expectation it does not produce, **eight** separate times —
covering the assertion, the fixture, the preconditions, the documentation, and
the build graph underneath all of it. Each was found by review, never by the
harness. Read `dev/wirecheck/README.md` fully before trusting or changing it,
and prefer running a control to reasoning about one.

### Known and deliberately not fixed

- `client_size()` mixes device and logical pixels (`decoration.rs`), pre-existing.
- `buffer_size` diverges from `Gpu::size` while a decoration is frozen.
- `contentWidth`/`contentHeight` go stale across a pure resize.
- The cursor cache's theme-change invalidation has no trigger today — `Theme.qml`
  is all `readonly` literals. The wiring is right; the coverage is not end-to-end.
- Insets are read from a 1×1 scene on the GPU path and a client-sized one on the
  software path. Benign while every shipped decoration declares them as literals.
- **The software path still has the multi-output fan-out.** A straddling window
  makes `Decoration::in_memory` see a different `size` per output, so `resized`
  is true on every call and its `MemoryRenderBuffer` is reallocated once per
  output per frame — a `QImage` realloc rather than a GBM round trip, and this
  is the default path. The `Gpu` cache does not reach it. The same shape is in
  `ShellSurface::in_memory`. Measure it on hardware before deciding it is worth
  a second cache; the whole argument for the GPU path is that this is what it
  replaces.
- **The cap is two, so three straddled outputs at three distinct scales thrash
  it** and behave as the branch did before the cache. Deliberate: an entry is a
  whole window-sized GBM buffer and there is one decoration per window. See
  `KEPT` in `qml/paint.rs` for the arithmetic.

### Two open items on `main`, neither started

- **Animated decorations render zero frames.** `render.rs:182` reads
  `decoration.animating()` *after* `decoration.frame()` rendered, and `frame()`
  clears Qt's dirty flag — so `state.redraw` is never set. Measured at zero
  frames in 60 s, confirmed three ways.
- **The performance figure justifying this plan is mis-cited.** "About a tenth of
  a core" is real (commit `14e49d2`) but it is a whole-compositor number of which
  ~90% is not QML, describing a load no shipped decoration produces; at decoration
  geometry it is ~0.74%. The work is still justified — by the pane-styles load, not
  today's: three layers per window maximised on a 2560×1440 260 Hz panel costs Qt
  1.94 ms plus 2.58 ms of serial main-thread memcpy against a 3.846 ms vblank budget.
  Rewrite the justification to say that.

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

**An empty result is not evidence until Step 6 says it is.** If anything on this
path is failing per frame, journald is dropping lines faster than it is keeping
them, and the lines it drops are these. Do not conclude anything from a clean
grep here without running Step 6.

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

---

The thirteen steps below accumulated across seven task reviews and, until this
commit, existed only in the git-ignored SDD ledger — so the one person who has
to run them had never seen them. They are in priority order.

**Steps 5 to 8 are the ones not to skip.** Between them they are the only
coverage this branch has for the multi-output rebind cache on real hardware, for
whether the log can be believed at all, for a failure path the plan asserts the
behaviour of and has never once executed, and for the 1×1 build that every
window frame and the pointer now start from. The rest are worth whatever time
the session has left, in the order written.

- [ ] **Step 5: Two monitors at different scales, one window dragged across the bezel and left there**

**This step changed: it used to be "expect the churn and record the numbers",
and the churn is now fixed. It is a verification that it has gone.**

What it was. `render.rs:477` decides by the window's **slot**, so a window
straddling the bezel is deliberately drawn on *both* outputs — the comment above
that line says so and argues for it. `Decoration::frame` therefore runs once per
output per frame, each time with that output's scale, and `Gpu::render`'s
`if self.bound != size` flipped between the two sizes on every call. Every one
of them took the rebind branch: a fresh GBM allocation, a dmabuf export, an
`eglCreateImageKHR`, a render-target swap, a full Qt render and an
`import_dmabuf` — twice per frame, for as long as the window sat there — with a
`damage.reset()` each time, so both outputs reported the window's whole area as
damaged every frame as well.

What it is now. `Gpu` keeps one imported picture per `(pixel size, scale)` it
has been asked for, capped at two — `Kept` and `KEPT` in `crates/solium/src/qml/paint.rs`,
the same container the pointer has used since Task 7 and now shared with it
rather than copied. A straddling window pays two renders in total instead of two
per frame, and an idle one pays none. The pane loading scenes and the scripted
shell surfaces go through the same `Gpu` and inherit it.

Set the two scales in the configuration and reload with `super+shift+r`:

```lua
    monitors = {
        { name = "DP-1",  scale = 1 },
        { name = "HDMI-A-1", scale = 2, beside = "DP-1" },
    },
```

Expected: dragging a window across the bezel and **leaving it there** costs
nothing measurable once it has settled. Watch frame pacing on **both** outputs,
not the one with the window's title on it — a drop that only shows on the 1x
screen is still this.

Three things worth knowing before reading the result:

- **An animating decoration still renders once per output per frame**, and that
  is correct rather than a residue of the bug. Qt genuinely has a new picture,
  and each output genuinely needs it at its own resolution; the cache
  invalidates on Qt's dirty flag precisely so that it does. What is fixed is the
  *idle* case, which is the one that lasted for ever. Judge the step with the
  window settled, not mid-animation.
- **Three monitors at three distinct scales, one window straddling all three,
  thrashes the two-entry cache and behaves exactly as the branch did before.**
  That is the documented cost of the cap and the arithmetic for raising it is at
  `KEPT`. If the hardware session has three screens, this is worth ten seconds
  of confirming.
- **The software path was not changed and still has this shape**, in its own
  cheaper form: `Decoration::in_memory` reallocates its `MemoryRenderBuffer`
  once per output per frame for a straddling window, because `buffer_size`
  is one size and alternates. A `QImage` realloc rather than a GBM round trip,
  and the default path. Not fixed here because the fix lives in `Gpu`; worth
  recording if it shows on the 1x screen.

None of this is visible to `dev/wirecheck` — it does not link the compositor
crate, which is why the defect survived seven task reviews. The in-tree coverage
is `qml::paint::tests`, and it bites: with `Kept::current` made never to report
a hit, `a_window_across_a_bezel_is_drawn_twice_and_then_not_again` reads
`left: 200, right: 2`.

- [ ] **Step 6: Before believing any empty journal grep, check for suppression**

```bash
journalctl --user -b 0 | grep -c 'Suppressed'
```

Expected: `0`. Anything else means the journal from this run is incomplete and
Step 2's clean grep proves nothing.

Why it can happen: everything in `Gpu::sample` runs once per scene per output
per frame, so a failure that does not heal is a four-figure-per-second log — a
two-monitor desktop with a handful of scenes at 60 Hz is order 1200 lines a
second, and the EGL-restore site is an `error!`. journald's shipped defaults are
`RateLimitBurst=10000` per `RateLimitIntervalSec=30s` (this box has no
`/etc/systemd/journald.conf` and no drop-ins, so those are what is in force), so
dropping starts within seconds. What gets dropped is *everything else being
said at the time* — including the Qt diagnostics Step 2 is looking for.

All four of those sites are latched as of this branch — one line per scene until
it works again — which is what should keep this from happening. This step is how
you find out whether that held, and it costs one command.

- [ ] **Step 7: Make a rebind fail on purpose and watch it recover**

Nothing in the harness, the test suite or any run so far exercises the
freeze-stretched-and-retry path in `Gpu::render` (`paint.rs:697-761`). The plan
asserts its behaviour — see *A failed rebind freezes, stretched, and retries*
above — on reasoning alone. This is the only opportunity to execute it.

The cache in front of it does not soften this and does not need a separate look.
Every way a rebind can fail leaves nothing in `Gpu`'s `kept`, so a failing scene
misses on every output on every frame and reaches the retry exactly as often as
it did before — which is also why the latches above it still matter.

The cheapest lever is a scene wider than `MAX_SIDE` (8192, `qml/target.rs:21`),
because `target::allocate` refuses it before it touches GBM: the failure is
exact, reproducible and nothing to do with the driver. A scripted surface takes
a rect directly, so put one in `config.lua` at a size that is fine, and then
raise the monitor's scale until `logical × scale` crosses 8192:

```lua
    -- 3000 logical: fine at scale 1 (3000 px), refused at scale 3 (9000 px).
    sol.surface("toobig", {
        scene = "wallpaper.qml",
        on = { x = 0, y = 0, w = 3000, h = 200 },
    })
```

Expected, when the scale goes up: the surface visibly **stretched** — its last
good 3000-pixel picture drawn into the geometry it should have had — and exactly
one line per scene:

```
a GPU scene did not render; drawing the last frame it managed. Said once per scene until it renders again
```

Put the scale back and it should heal on the next frame, with the warning
becoming news again if it recurs.

**Check the other half too, because the plan's claim is narrower than it
reads.** "A failed rebind freezes; it does not go invisible" is true only when
there is an earlier frame to freeze on. A rebind that fails on a scene's *first*
frame leaves `shown` as `None` and the surface draws nothing at all — which is
exactly the state a `ShellSurface` starts in (`surface.rs:99` builds at 1×1 with
`Gpu::new((0, 0))`, so its first frame always takes the rebind branch). Declare
the surface at `w = 9000` from the start to see it, and record which of the two
outcomes the operator actually gets.

- [ ] **Step 8: A decoration and a cursor built at 1×1, and then rebound**

`dev/wirecheck` builds those two QML files at their full size — 640×480 and
64×64. The compositor does not: `decoration.rs:254` and `surface.rs:99` build at
1×1 on the GPU path and rebind on the first frame. So a `Text` or a `Shape` that
does not survive a 1×1 layout, or an inset bound to `width`, is invisible to
every check on this branch and shows up here for the first time.

Read the insets rather than eyeballing the bar: `insetTop` and `insetLeft` are
read once, from that 1×1 scene, and the space they reserve does not go away if
the picture does. The observable is geometric — the client's top edge sits
exactly under the titlebar, with no strip of wallpaper between them and no bar
drawn over the client's first rows.

The sharpest version is the same window twice, because the two paths read the
insets from *different* scenes and would give two different wrong answers:

```bash
SOLIUM_QML_GPU=1 ./target/debug/solium --tty   # insets from a 1x1 scene
./target/debug/solium --tty                    # insets from a client-sized one
```

Expected: identical geometry. A divergence is the *Known and deliberately not
fixed* item about insets stopping being benign, which needs reporting rather
than tolerating.

- [ ] **Step 9: Cursor-plane measurement at 1x, 2x *and* 3x**

The readback in `cursor.rs` exists for one reason: to keep the pointer on the
DRM hardware cursor plane. Whether it does is not observable anywhere but here.

Measure all three scales, because the answer changes between them.
`try_assign_cursor_plane` refuses any element bigger than the plane — commonly
64×64 — at `drm/compositor/mod.rs:3043`, and the pointer is 24 *logical* pixels.
So it is expected to **lose** the plane above about 2.67x: fine at 1x (24) and
2x (48), refused at 3x (72). Measure it rather than assuming it; the plane size
is the device's to report.

Direct evidence, at `trace!` on that one target so the synchronous session log
does not swallow the run:

```bash
RUST_LOG=warn,smithay::backend::drm::compositor=trace \
  SOLIUM_QML_GPU=1 ./target/debug/solium --tty
```

The three lines that matter, all from `drm/compositor/mod.rs`:

```
element ... too big for cursor plane(s), skipping                     # the size refusal, 3044
failed to copy element to cursor bo, skipping element on cursor plane # the copy refusal, 3252
skipping element ... on cursor plane(s), element kind not cursor      # a Kind::Cursor regression, 3033
```

Do **not** grep for `Can't obtain cursor's underlying storage`. That line is
real but it is inside the `#[cfg(feature = "renderer_pixman")]` arm at
`mod.rs:3269`, and `renderer_pixman` is not in our feature list — it cannot be
printed by this binary, so its absence says nothing.

Then compare pointer-motion cost against the software path on an otherwise still
desktop. Losing the plane turns every pointer motion into a full composite and
page flip of the whole output, which is the cost the whole readback exists to
avoid.

- [ ] **Step 10: A rotated output**

```lua
    monitors = { { name = "DP-1", transform = 90 } },
```

A rotated monitor loses the cursor plane on **both** paths:
`copy_element_to_cursor_bo` gives up unless `output_transform == Transform::Normal`
(`drm/compositor/mod.rs:4202`). Put one in the pass so that is recorded as
inherent to smithay rather than misread later as a GPU-path regression.

It is also the only chance to meet a non-normal output at all: `paint.rs:672`
hardcodes `Transform::Normal` in the element every GPU scene is drawn through,
and nothing has ever handed it anything else. Look at whether the chrome is
oriented correctly on the rotated screen, not only at whether the pointer moved
off the plane.

- [ ] **Step 11: Ten window opens, ten closes, then a style swap**

Volume, on the ordering the C-1 control exists for. Every close frees a GPU
scene with another scene's belief possibly still on the thread, and until now
that ordering has only ever been produced one scene at a time by a harness.

Open ten windows, close all ten, then swap the decoration style — `--debug-mode`
gives the Developer Tweaks panel, which is one keypress per decoration
(`lua/tweaks.lua:100` calls `sol.decoration(name)`), or put `sol.decoration("border")`
in the configuration and reload.

Re-run Step 2's grep after each phase rather than once at the end, so a hit can
be attributed to opening, to closing or to the swap.

- [ ] **Step 12: A window frame at a non-1.0 output scale**

The one combination where all three sizes differ: the rebind size is device
pixels, the element's `src` rect is the buffer in its own pixels, and `dst` is
logical. Any two of them can agree while the third is wrong.

```lua
    monitors = { { name = "DP-1", scale = 1.5 } },
```

Expected: the titlebar text is *crisp*, not half-resolution and stretched back
up, and the buttons at the right-hand end of the bar are where they belong — the
right end is where a `src`/`dst` mismatch shows first and worst.

- [ ] **Step 13: An animation inside a titlebar during a slow drag-resize**

`SOLIUM_DECORATION=pulse`, then grab a window edge and resize it slowly. The
animation must keep **advancing**, not restart from its `from:` on every frame.
That is the exact property Task 6 exists for, asserted by `dev/wirecheck` on
`quadrants.qml`'s `spin`, and a decoration is the first scene to exercise it
where a person can see it.

The drag is not incidental. There is a standing open item on `main` — animated
decorations render zero frames, because `render.rs:182` reads
`decoration.animating()` *after* `frame()` has cleared Qt's dirty flag — so an
animating decoration asks for no frames of its own. A drag-resize is what keeps
frames coming, which is what makes this observable at all.

- [ ] **Step 14: Warp and overview, in and out, and a screencopy of a GPU-backed frame**

`super+space` toggles overview and `escape` leaves it (`lua/overview.lua:90-91`).
Then take a screenshot with any wlr-screencopy client.

`offscreen::capture` (`offscreen.rs:32`) and `screencopy.rs` both bind
framebuffers around scene work, and neither has run against a real KMS target.
The failure to look for is not a wrong picture — it is a bind left behind, which
shows up as the *next* frame being wrong rather than this one.

- [ ] **Step 15: Clean teardown, and does the VT come back**

`Ctrl+Alt+Backspace` (`input/mod.rs:198-212`), then check that the terminal is
usable: a shell prompt, echoing keys, in text mode.

`qml.rs:505-524` argues that `QT_QPA_NO_SIGNAL_HANDLER` is what stops Qt
`_exit(1)`ing straight past every Rust destructor — the libseat session, the DRM
master release, the VT restore. That argument was verified by reading a
disassembly of `libQt6EglFSDeviceIntegration.so.6.11.1` and has never been
verified by exiting. A compositor that dies without putting the VT back is how a
TTY session ends in a reboot, so this is worth doing deliberately rather than
finding out at the end.

- [ ] **Step 16: Live QML reload — read this before testing it**

`super+shift+r` reloads. `state.rs:2876-2877` implements the rebuild as:

```rust
                self.decorations.set_style(None);
                self.decorations.set_style(style);
```

and `set_style` returns `false` without doing anything when the style it is
given is the one already in place (`decoration.rs:655`). So when `style` is
`None`, **both** calls are no-ops and no frame is rebuilt — the QML cache is
cleared and every window keeps its old scene.

Whether that bites depends on the configuration, so check before concluding.
The shipped `config.lua` sets `decoration = "top"` and `init.lua:11` passes it
through unconditionally, so on a stock session the style is `Some("top")` and
reload does rebuild. It is a no-op exactly when the running configuration never
named a decoration — the `decoration` key absent or `nil`.

And `SOLIUM_DECORATION` does not rescue it. That variable is read inside
`qml_path` and `bare` only (`decoration.rs:779`, `:821`); it never sets `style`,
so it cannot make the pair fire. A session started with `SOLIUM_DECORATION=pulse`
and a configuration that names no decoration will not pick up edits to
`pulse.qml`, and that will read exactly like a GPU-path caching bug.

Pre-existing, not this branch, and not to be fixed here — but it will be blamed
on this branch if it is met without warning.

- [ ] **Step 17: `--check-qml` is safe from this session's shell — and was not**

```bash
SOLIUM_QML_GPU=1 ./target/debug/solium --check-qml qml/decorations/top.qml
```

Expected: `ok`.

It used to call `qml::start()`, which honours `SOLIUM_QML_GPU` — so run from the
shell this session is driven from, with the knob exported, it brought up a *GPU*
host, and `Scene::software` was then refused by `host.cpp:449`. A perfectly
valid QML file came back reported as broken, from the one entry point whose
whole job is to answer that question accurately. It now calls
`qml::start_software()` (`qml.rs:160`), which takes the decision rather than
reading it.

If the answer here is not `ok`, check that the binary is this branch's before
believing it about anything else.

---

- [ ] **Step 18: Record the finding and decide the default**

Append to `dev/README.md` under `## QML on the GPU`: whether it worked, the
frame-pacing numbers from the log with and without, whether tearing was found,
and whether anything from Step 2 appeared.

Record Steps 5 to 17 there too, and record the ones that were *not* reached as
not reached rather than leaving them out — a step nobody ran and a step nobody
wrote down are indistinguishable afterwards, and this list exists because that
already happened once.

If it is clean, flip the default: `qml_gpu()` returns `true` unless
`SOLIUM_QML_SOFTWARE` is set, and say so in the same paragraph. If it is not
clean, leave the knob off and record what was seen — the plan below it does not
depend on the default, only on the path existing.

- [ ] **Step 19: Commit**

```bash
git add dev/README.md crates/solium/src/dev.rs
git commit -m "qml: what the GPU path does on real hardware"
```
