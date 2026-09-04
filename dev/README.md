# Development knobs

Environment variables Solium reads. All of them exist so the compositor can be
exercised and photographed without a human at the keyboard — that is what turns
a demo into a regression test.

| Variable | Effect |
|---|---|
| `SOLIUM_CAPTURE=<path>` | Write one rendered frame to `<path>` as a binary PPM. |
| `SOLIUM_CAPTURE_AT=<ms>` | Capture at this moment instead of "once a window has settled". Naming a moment is what makes capturing an *animation* possible. |
| `SOLIUM_OVERVIEW_AT=<ms>` | Toggle overview at this moment, for exercising the transform with no keyboard. |
| `SOLIUM_FORM_FACTOR=` | `desktop` (default), `laptop`, `tablet`, `phone`. Selects the input profile. |
| `SOLIUM_DRAG_MODIFIER=` | `logo` (default) or `alt`. Held to drag a window from anywhere in it. |

## Capturing a frame

```sh
SOLIUM_CAPTURE=/tmp/frame.ppm dev/run-nested.sh
```

A compositor cannot be verified by looking at it: a desktop screenshot tool
captures the *host* session, which proves nothing about what Solium composited,
and using a shell's own capture to test that shell is circular. So Solium reads
its own framebuffer back.

The capture waits until a window has been mapped for half a second, because a
capture of an empty compositor is exactly the misleading result the mechanism
exists to avoid. A captured frame is not presented — reading the framebuffer
back invalidates the bind, and the following `submit` would fail to reallocate
its EGL surface.

## Photographing the transform

Three passes produce `docs/overview-transform.png`:

```sh
SOLIUM_CAPTURE=/tmp/normal.ppm SOLIUM_CAPTURE_AT=4500 dev/run-nested.sh
SOLIUM_CAPTURE=/tmp/mid.ppm  SOLIUM_CAPTURE_AT=4590 SOLIUM_OVERVIEW_AT=4500 dev/run-nested.sh
SOLIUM_CAPTURE=/tmp/over.ppm SOLIUM_CAPTURE_AT=4900 SOLIUM_OVERVIEW_AT=4500 dev/run-nested.sh
```

## Bindings

| Input | Effect |
|---|---|
| `Super` + `Space` | Overview on/off |
| `Super` + drag | Move a window from anywhere in it |
| Titlebar drag | Move a window (the client asks, via `xdg_toplevel.move`) |
| Click | Focus and raise; in overview, focus that window and leave |
