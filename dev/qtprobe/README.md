# Does this Qt adopt a foreign EGL context?

The whole GPU path for in-compositor QML turned on one call. If Qt can adopt
the compositor's EGL context, it can render into a texture we sample directly.
If it cannot, Qt renders on a context of its own and the only way across is a
shared buffer.

**Since then (2026-09-30).** The answer was no: `QEGLContext::fromNative`
returns null under both `offscreen` and `eglfs` (see
[the QML spike](../../docs/spikes/2026-09-04-qml-in-compositor.md)). So the GPU
path was built on a shared buffer (#94), and it has been the default since
#147. Whether that path works on a given machine is now answered by
`solium --probe-qml-gpu`, which the compositor also runs in a child process at
every start on the hardware under the default `auto` renderer
([dev/README.md, *QML on the GPU*](../README.md#qml-on-the-gpu)). This probe is
for the question in its title: re-run it after a Qt update and read the
`fromNative:` line. The exit status does not report that line; it is 0 when
the GBM-paired import below worked, 2 or 3 when EGL or a context could not be
set up, and 4 when the import failed. The probe opens `/dev/dri/renderD128` itself rather
than looking the render node up, so on a machine with two GPUs it may test the
wrong one.

From `dev/qtprobe`:

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
a texture, provided the importing `EGLDisplay` is a GBM-platform display on the
same GPU as the buffer.** That pairing matters: it is not the default
`EGLDisplay`, and getting it wrong looks exactly like the feature being
unsupported. Read this whole section before re-deriving it — the pairing is the
part that is easy to get wrong quietly.

The probe pairs the display with the buffer's own `gbm_device`, the strictest
form of that rule. The first real use of the import, the same evening, measured
the looser form Solium depends on: Qt's eglfs display, opened on the card node,
accepted a buffer allocated through the render node, so the pairing is per GPU,
not per `gbm_device` object or per DRM node (the comment above
`import_dmabuf_texture` in `crates/solium/qml/host.cpp`). It also means a
second GPU would break it.

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

That run: Qt `qt6-qtbase-6.11.1-1.fc44.x86_64` at run time (what the probe
binary loads when run natively; it was built in the container against Qt6Gui
6.11.2 via pkg-config, the same 6.11 ABI line), on an NVIDIA GPU with NVIDIA's
proprietary driver.

Not tested by this probe: rendering into the bound texture through an FBO
from a second, Qt-owned context on the same device, and fencing the hand-off
to the compositor's context — the harder half the spike named ("NVIDIA's
driver is where dmabuf round-trips and cross-context fences are least
forgiving"). This result is the allocate/import/bind step only, and it
works; the render-and-fence pipeline built on it is exercised end to end by
`dev/wirecheck` (see its README). The two `dmabuf[...]` lines quoted above are
the probe's own output, from that run.
