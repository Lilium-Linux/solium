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

**2026-09-08: no.** `eglCreateImageKHR` rejects the dmabuf with `EGL_BAD_MATCH`
(0x3009), reproducibly — three runs, both `offscreen` and `eglfs`, identical
result each time. The GBM allocation itself succeeds first: a 256×256
`ARGB8888` buffer, stride 1024, modifier `0x300000000e08014`. That modifier's
vendor byte (`0x03`) is `DRM_FORMAT_MOD_VENDOR_NVIDIA`, not
`DRM_FORMAT_MOD_INVALID` — so this is NVIDIA's own GBM handing back one of its
own real tiled modifiers, and NVIDIA's own EGL then refusing to import it as a
dma-buf. The break is specifically at EGL's dma-buf import step, not at buffer
allocation, GBM device creation, or the render node.

    dmabuf: eglCreateImageKHR failed 0x3009  (format=ARGB8888 stride=1024 modifier=0x300000000e08014)

`EGL_ANDROID_native_fence_sync` is present in `eglQueryString`'s extension
list, so fencing was never reached as a second blocker — the round trip fails
before that matters.

Qt: `qt6-qtbase-6.11.1-1.fc44.x86_64` (host runtime — what the probe binary
actually loads when run natively; built in the container against Qt6Gui
6.11.2 via pkg-config, same 6.11 ABI line). Driver: NVIDIA GeForce RTX 3070,
driver 610.57.04.

    name of display: :0
    display: :0  screen: 0
    direct rendering: Yes

This is the stall point the spike named in advance: "NVIDIA's driver is where
dmabuf round-trips and cross-context fences are least forgiving." The
dmabuf-backed render target route is closed on this machine. Full probe
output, build log, and detail:
`.superpowers/sdd/2026-09-08-qml-gpu-render-target/task-1-report.md`.
