# Decorations

A window frame is a QML file. There is nothing to compile and no compositor
code to touch: write a file, name it in your configuration, and it draws every
window.

    -- ~/.config/solium/user.lua
    return { decoration = "left" }

A name is one of the files here, or one of your own in
`~/.config/solium/qml/decorations/` -- yours shadows a shipped one of the same
name. A path is anywhere. `SOLIUM_DECORATION=left` does the same thing for one
run, which is the quicker way to try one.

Press **super+shift+r** and the running session picks up the change: the
configuration is read again, the QML cache is dropped, and every frame is
rebuilt. Windows keep their slots, and each client is resized to whatever the
new decoration left it.

Everything the frames draw with -- colours, fonts, spacing -- comes from
`Solium.Theme`. Drop your own `Solium/Theme.qml` into `~/.config/solium/qml/`
and every frame and every shell surface follows it, without touching anything
that ships.

## The contract

The root is an `Item` covering the window's **whole outer rect** — frame and
client together. Whatever it does not paint stays transparent and the client
shows through, which is why a bar can be on any side, or there can be no bar
and only a border, or a bar that floats over the window and reserves nothing.

Declare what it reserves. These are read once, when the frame is built, and
the client is placed inside what is left:

    property int insetTop: 32
    property int insetRight: 0
    property int insetBottom: 0
    property int insetLeft: 0

Reserve nothing and the decoration becomes an overlay: it draws on top of the
client and never moves it.

Set by the compositor, every frame:

| property | |
|---|---|
| `title` | the window's title |
| `focused` | whether it has the keyboard |
| `pointerInside` | whether the pointer is anywhere over the window |
| `contentWidth`, `contentHeight` | the client's size, inside the insets |

Read by the compositor:

| property | |
|---|---|
| `action` | set to `"close"` or `"maximize"` to ask for it; cleared once taken |
| `hovered` | the name of the button under the pointer, or `""` |

`hovered` decides whether a press starts a window drag, so a decoration whose
buttons do not set it will have its buttons dragging the window instead.

The pointer arrives as ordinary mouse events, in the frame's own coordinates,
while it is anywhere over the window — including over the client, which is how
a border can follow the cursor. When it leaves, the frame is told with a
position outside itself, so `MouseArea.containsMouse` goes false on its own.

## What is here

| file | |
|---|---|
| `top.qml` | the default: a titlebar above the window |
| `left.qml` | a vertical titlebar down the left side |
| `bottom.qml` | a titlebar underneath |
| `border.qml` | no bar at all, just a frame around the window |
| `reactive.qml` | a border that lights up where the cursor is, with a bar |
| `proximity.qml` | a border that answers the pointer entering and leaving |
| `reveal.qml` | a bar that slides out of the window's edge on approach |
| `pulse.qml` | a bar with an animation running in it |
