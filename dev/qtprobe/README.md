# Does this Qt adopt a foreign EGL context?

The whole GPU path for in-compositor QML turns on one call. If Qt can adopt the
compositor's EGL context, it can render into a texture we sample directly. If
it cannot, Qt renders on a context of its own and the only way across is a
shared buffer.

    podman run --rm --userns=keep-id --security-opt label=disable -v "$HOME:$HOME" -w "$PWD" \
        localhost/solium-build:fc44 \
        sh -c 'g++ -fPIC probe.cpp -o probe $(pkg-config --cflags --libs Qt6Gui gbm egl glesv2) -lEGL'
    QT_QPA_PLATFORM=offscreen ./probe
    QT_QPA_PLATFORM=eglfs ./probe

This exists because the answer has now been derived twice, and it is a
property of the installed Qt rather than of anything in this repository — so it
is worth re-running after a Qt update rather than reasoning about.

## dmabuf round-trip

**2026-09-08: yes — a GBM buffer can be imported as an EGLImage and bound to
a texture, provided the importing `EGLDisplay` is paired with the same GBM
device the buffer was allocated on.** That pairing matters: it is not the
default `EGLDisplay`, and getting it wrong looks exactly like the feature
being unsupported. Read this whole section before re-deriving it — the
device pairing is the part that is easy to get wrong quietly.

Solium's own `EGLDisplay` (`crates/solium/src/tty.rs`, `open_gpu`) is built
as `EGLDisplay::new(gbm.clone())` — a GBM-platform display on a specific
`GbmDevice` object, not `eglGetDisplay(EGL_DEFAULT_DISPLAY)`. The probe
imports the same dmabuf against both, to make the contrast impossible to
miss:

    dmabuf[default display]: eglCreateImageKHR failed 0x3009  (format=ARGB8888 stride=1024 modifier=0x300000000e08014)
    dmabuf[gbm-paired display]: ok  texture=1 stride=1024 modifier=0x300000000e08014

Same fd, same 256×256 `ARGB8888` buffer, same stride (1024), same modifier
(`0x300000000e08014` — vendor byte `0x03` is `DRM_FORMAT_MOD_VENDOR_NVIDIA`,
a real tiled modifier, not `DRM_FORMAT_MOD_INVALID`). The default display
rejects it with `EGL_BAD_MATCH` (`0x3009`); a display obtained via
`eglGetPlatformDisplayEXT(EGL_PLATFORM_GBM_KHR, gbm, nullptr)` on the
buffer's own `gbm_device*` accepts it. Reproducible across three runs, both
`offscreen` and `eglfs`, identical every time. `EGL_EXT_image_dma_buf_import`
and `EGL_EXT_image_dma_buf_import_modifiers` are both present on both
displays, and `EGL_ANDROID_native_fence_sync` is present — none of those were
the blocker.

Qt: `qt6-qtbase-6.11.1-1.fc44.x86_64` (host runtime — what the probe binary
actually loads when run natively; built in the container against Qt6Gui
6.11.2 via pkg-config, same 6.11 ABI line). Driver: NVIDIA GeForce RTX 3070,
driver 610.57.04.

    name of display: :0
    display: :0  screen: 0
    direct rendering: Yes

Not tested by this probe: rendering into the bound texture through an FBO
from a second, Qt-owned context on the same device, and fencing the hand-off
to the compositor's context — the harder half the spike named ("NVIDIA's
driver is where dmabuf round-trips and cross-context fences are least
forgiving"). This result is the allocate/import/bind step only, and it
works; the render-and-fence pipeline is what the tasks that depend on this
one build and prove. Full probe output, both rounds, and the full trail from
the wrong first answer to this one:
`.superpowers/sdd/2026-09-08-qml-gpu-render-target/task-1-report.md`.
