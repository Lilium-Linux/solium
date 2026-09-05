# Where Solium ends and Lilium begins

Solium is the compositor. Lilium is the desktop — its bar, its dock, its
launcher. This is the line between them, and why it falls where it does.

## Three kinds of surface, three different relationships

**Windows** are clients over `xdg-shell`. The compositor owns their geometry;
they ask, it decides.

**Decorations, modes and animations live inside the compositor.** A titlebar has
to move in the same frame as its window or the two visibly come apart, and an
overview transform has to be applied to the same texture the renderer is about
to draw. Anything *window-coupled* is in-process — which is why the frames are
QML rendered by the compositor and not by a shell.

**The shell — bar, dock, launcher — is a separate process.** It attaches over
`wlr-layer-shell`: it anchors to an edge, says how much room it needs, and the
compositor keeps windows out of that strip. `Solium::work_area` is whatever is
left, so a dock that grows pushes windows out of its way without either side
knowing anything about the other's internals.

The compositor briefly drew its own top bar. That was wrong and has been
removed: it made the compositor own a design, a font stack and a layout it had
no business owning, and it hid the interesting question — how a *replaceable*
shell attaches — rather than answering it.

## The part `wlr-layer-shell` does not solve

The point of building a compositor rather than configuring one is animations
that cross the boundary: an icon in the dock that grows into a window, a window
that shrinks back into it. For that, the compositor has to know **where the icon
is**, and that is geometry the shell owns.

There is exactly one rule about how it arrives, and it is the rule this project
learned the hard way:

> **Geometry the compositor animates against must arrive with the frame that
> shows it. Never on a side channel.**

A dock that sends "my Files icon is at (120, 980, 48, 48)" whenever it feels
like it, applied whenever the compositor gets round to it, is a *mirror* of the
dock's state — and mirrors drift. The window then flies out of where the icon
used to be, one frame before it moved. The previous incarnation of this project
lost a week to exactly this shape of bug in a different guise.

So: icon rectangles are attached to the dock's surface and applied **atomically
with its commit**, the same way a surface's damage and buffer are. If the dock
moves an icon and redraws in one commit, the compositor sees both at once or
neither. That is a small custom protocol — `solium_shell_v1` — and it is the
only thing Solium will ask a shell to speak beyond the standard ones.

## What the compositor does with it

Nothing, by itself. It hands the rectangle to a script:

```lua
sol.on("open", function(id, window)
    local icon = sol.icon_for(window.app_id)   -- from the dock, or nil
    sol.animate({ duration = 260, easing = "spring" })
    sol.present_from(id, icon or shrunk(window))
end)
```

`present_from` puts the window at a rectangle and animates it to where it
actually lives. With an icon rectangle that is the macOS-style genie; without
one it is the ordinary open animation; with a different rectangle it is
something nobody has thought of yet. The compositor supplies the primitive and
the script decides — which is the same arrangement as overview, and the reason
adding the genie later will not need new Rust.

## Summary

| | Where it runs | How it talks to the compositor |
|---|---|---|
| Windows | Clients | `xdg-shell` |
| Window frames, modes, animations | In the compositor | — |
| Bar, dock, launcher | The shell, a separate process | `wlr-layer-shell` for space |
| Dock icon geometry | The shell | `solium_shell_v1`, atomic with its commit |
| What an animation *does* | A Lua script | `sol.present_from`, `sol.on("open")` |
