# Desktop modes

A mode is a Lua file. Tiling is one, scrolling is one, overview is one, and so
is whatever you write. The compositor holds no opinion about any of them.

That is not a boast about extensibility — it is the architecture's test. **If a
new mode needs new Rust, the transform layer is missing something**, and that
missing thing is the bug rather than your mode. Overview is ninety lines of Lua
for exactly this reason: it was written to find out whether the claim was true.

![Every mode, frame by frame, twenty milliseconds apart](modes-frame-by-frame.png)

Every mode above is captured by the compositor reading back its own
framebuffer, twenty milliseconds apart. One engine drew all four rows, which is
the whole argument on one page: a window opening, overview entering, and two
layouts arranging are the same interpolation with different targets.

## The two ways to move a window

Everything a mode does comes down to one of these, and picking the wrong one is
the most common mistake.

```lua
sol.place(id, { x = 0, y = 0, w = 960, h = 1080 })   -- where it LIVES
sol.present(id, { rect = { x = 40, y = 40, w = 320, h = 180 } })  -- where it is DRAWN
```

`place` is the layout's authority. The window is really that size; the client is
told, and asked to redraw. Use it for arrangements — tiling, scrolling, a
window snapping back after a drag.

`present` is a transform. The window still lives where it lived and the client
never learns anything happened; it is simply drawn somewhere else. Use it for
anything temporary — overview, an app switcher, a peek, a genie. Then:

```lua
sol.present_clear(id)   -- animate back to real geometry and stop transforming
```

The distinction is why leaving overview is exact rather than approximate. The
layout was never disturbed, so there is nothing to restore.

`sol.present_from(id, rect)` is the third: draw the window at `rect` and animate
it to where it lives. That is every "appears from somewhere" animation — a
window opening, or growing out of a dock icon.

### Depth and pivot

```lua
sol.present(id, { z = 2 })                       -- drawn in front
sol.present(id, { rotate_y = 20, pivot_x = 0 })  -- turns about its left edge
```

`z` is draw order and nothing else. Equal values keep the order the stack gave
them, so the default costs nothing — and a window raised above its neighbour is
still **clicked where the layout put it**, because `rect` stays the truth for
input.

`pivot` is a fraction of the window, not pixels: `(0.5, 0.5)` is the centre and
is the default, `(0, 0)` the top-left corner. **Each axis defaults on its own** —
`pivot_x = 0` means the left edge and says nothing about the vertical. Outside
`0..1` means what it says rather than being clamped: `pivot_x = 2` hinges the
window about a line off to its right, which is a door on a frame beside it.

A number that cannot be drawn with is dropped rather than drawn: a NaN depth
cannot be ordered against anything, a non-finite pivot puts every corner of the
window at NaN, and both fall back to the default with a line in the log. An
infinite `z` is kept — `math.huge` is a legible "above everything".

Both belong to a window, so both are `sol.present` keys. `sol.present_group`
takes neither, and that is deliberate: a pivot on a selection would have to
overrule each member's own — a member is drawn through one matrix, turning about
one point — and a depth would raise a desk's windows above the desk next door
while leaving its own wallpaper behind, because depth orders the windows and
scripted surfaces are drawn in fixed layers. Turning a whole desk as one shape
needs a rectangle for the selection, which no group has yet.

## Moving more than a window

A transform names a **selection**, and a selection can hold things that are not
windows at all:

```lua
sol.group("desk-2", {
    windows  = { 3, 7 },          -- by id
    surfaces = { "wallpaper-2" }, -- anything sol.surface declared
    monitors = { "DP-1" },        -- everything drawn on a screen
    monitor  = "DP-1",            -- which instance of each surface above
})
sol.present_group("desk-2", { x = -2560 }, { duration = 300 })
sol.present_group_clear("desk-2")
sol.group("desk-2", false)        -- take the name away
```

One call, one animation, one target. The wallpaper travels because it is *in*
the selection — the compositor has no idea what a wallpaper is, and does not
need one.

Three things to know before using it:

**`x` and `y` are a displacement, not a destination.** `sol.present` takes the
rectangle a window is drawn at; a selection has no rectangle of its own, so
`{ x = -2560 }` means "a screen to the left of wherever each member already is".

**It composes with each member's own transform.** A window you have also
`sol.present`ed is drawn at its own rect *plus* its selection's displacement, so
a window tilted inside a moving desk stays tilted within it. The corollary is
the one that catches people: a mode handing absolute rectangles to a window in a
carried selection is placing them **in that selection's space**. That is why
`overview.lua` shows the desk in front of you rather than every desk at once.

**Membership changes are animated.** A window that leaves one selection for
another would otherwise have the difference between the two displacements land
on it in a single frame; instead it is held where it was drawn and animates from
there, the same rule `sol.present` follows for a window entering a mode. The
duration is whatever `sol.animate` last set.

A selection naming a window that has closed, or a surface nothing declared,
simply does not contain it. There is nothing to clean up.

## What a mode is told

```lua
sol.on("open",   function(id) end)                 -- a window's life began
sol.on("closing", function(id) end)                -- a close was asked for
sol.on("refused", function(id) end)                -- ...and declined: it is back
sol.on("close",  function(id) end)                 -- it is gone
sol.on("focus",  function(id) end)                 -- the keyboard moved
sol.on("drop",   function(id, x, y) end)           -- a drag finished
sol.on("resize", function(id, edge_x, edge_y, horizontal_side, vertical_side) end)
sol.on("scroll", function(dx, dy) end)             -- a modified wheel turn
sol.on("click",  function(x, y) end)               -- only while grabbing input
sol.on("layout", function() end)                   -- the room windows get changed
sol.on("monitors", function() end)                 -- the screens are not the screens you knew
sol.on("restore",  function() end)                 -- you have replaced a running session
```

Six of these are worth reading twice.

**`open` fires when the window opens, which is before its application exists.**
A window's life begins when the user asks for the program. Your mode is told
then, gets to place the window then, and the application appears inside it
later. Nothing special is required of you for that to work — but it is why
`open` is the event that puts a window into your arrangement, and `layout` is
not. A layout keeps its own structure and adds to it on `open`; `layout` only
means "re-run what you already hold".

**A close is three events, because a close is a request.** The compositor
cannot take a window away; it can only ask the application to go, and an
application with unsaved work puts up a dialog and stays. So:

| event | sent | means |
|---|---|---|
| `closing(id)` | the moment a close is asked for -- the frame's button, `sol.close`, `super+q` -- as the window starts fading | reflow now, if you want to |
| `refused(id)` | when the application declines -- it said nothing for a second, or answered with a dialog -- and the window is back on screen | put it back, if you took it out |
| `close(id)` | when the window is gone | forget it |

In that order. After `closing` exactly one of the other two follows, and
**`refused` never follows `close`**: once a window has gone, nothing brings it
back. A refused window is an ordinary window again, and closing it a second
time starts over with a second `closing`. A window whose application quit on
its own was never asked, so it gets `close` and nothing before it.

`close` still means *gone*, and nothing else. Anything a script keeps about a
window -- `dialogs.forget`, `tiling.exiled` -- is dropped there and not at
`closing`, because a window that was only asked may be about to come back.
`refused` is not `open`: the window never went, and nothing about an arrival
should run again.

While a window is between `closing` and whichever comes next, its row in
`sol.windows()` says `leaving = true` (and so does its row in `close`'s own
snapshot). After `close` it is in no snapshot but that one. It is fading where
it stood whatever you do: the compositor pins the rectangle it is drawn at, so
placing it moves nothing you can see, and it stays in front of any window you
move into its space -- as does a refused window while it fades back in.

The shipped layouts close up at `closing`, while they are the layout in
charge, and put the window back at `refused`; `reflow_on_close = "when_gone"`
in their section of `config.lua` makes both events do nothing and leaves the
reflow to `close`, which is how every close worked before #128. Closing up at
once has one cost worth knowing: the compositor waits a second for an
application to go before it takes the silence for a refusal, so one slower
than that to quit is refused and then closes, and the layout moves three times
-- at the press, at `refused`, and at `close`.

A layout that listens for neither event hears exactly what it always heard --
`close`, once the window is gone -- so a mode written before these two existed
keeps working unchanged, and keeps its slot for a closing window until the
application has gone. To close up at once, handle `closing`, leave `leaving`
windows out of any arrangement you build from `sol.windows()`, and put the
window back on `refused`.

**`resize` gives you where the dragged edge should go, not where the pointer
is and not a delta.** `edge_x` and `edge_y` are in the same coordinates
`tree:layout` returns and `sol.place` takes — one per axis, so a corner drag's
two seams each get their own.

A position rather than a delta, deliberately: a delta would be measured against
a layout your own last response just changed, and the windows shake for as long
as the button is held. But a position *of the edge* rather than of the pointer,
also deliberately, and that is #124. A seam set from the cursor lands under the
cursor, so a drag begun anywhere except exactly on the edge threw that edge
across to the cursor on its first frame — half a border's width for a border
drag, and most of a window for `super`+right-button, which starts a resize from
wherever inside the window you happened to press. Handed the edge instead, the
same arithmetic makes the gesture relative: the edge starts where it already is
and moves as far as the pointer moves.

"Where it already is" means *where you put it* — the rect your last
`sol.place` for that window carried — and not where the client drew itself. The
two differ whenever a client commits a size other than the one it was asked
for, which a terminal on a cell grid does every time. Hand that back on a first
frame and your seam moves by the client's rounding before the pointer has
travelled a pixel, so the compositor sends your own number back to you.

On an axis this drag does not move there is no dragged edge, and the pointer's
own coordinate comes through there instead. Check the side before using the
coordinate — which is what the side is for — and you will never see it.

**`horizontal_side` and `vertical_side` are the *sides* being dragged**, not the axes:
`"left"` or `"right"`, `"top"` or `"bottom"`, and `nil` for an axis this drag
does not move. A corner drag fills both, because a corner drag moves one seam
per axis. They were booleans until #120, and a boolean cannot choose a seam: a
window sitting on the right of a vertical split has that split's seam on its
*left*, so "the horizontal axis is in play" was true whichever edge the hand
was on, and dragging the right edge moved the left one instead. Hand the side
straight to `tree:drag_seam`.

**`click` only arrives while you hold input.** `sol.grab_input(true)` takes keys
and clicks away from clients, which is what a mode needs while it owns the
screen. Release it when you leave, or nothing will ever reach a window again.

**`restore` fires after a reload and never at startup.** That asymmetry is the
event. See below.

## What survives `super+shift+r`

A reload throws the whole Lua state away and reads your files again. The
session does not go with it: the windows, the monitors, the workspace in view
and the layout in charge are all still exactly what they were. So your mode
comes back as a stranger to a desktop it was running a moment ago, and it is
entitled to exactly two things.

**Whatever you handed to `sol.keep`.**

```lua
local state = sol.keep("my-mode", { showing = 1 })
```

You get the table the last load left under that name, or the defaults the first
time. Mutate it in place; the host takes a copy when the reload happens. It
holds plain data only — numbers, strings, booleans and tables of those. A
function in there cannot cross and is named in the log rather than dropped in
silence.

Keep as little as you can. Anything you can work out again from `sol.windows()`
and `sol.monitors()` should be worked out again, because a keep is a claim about
the past that nothing checks.

**And the world, re-announced:** `restore`, then `monitors`, then `layout`.

`restore` is the moment every script has loaded, which is the earliest you can
touch another module's registrations — `lua/modes.lua` uses it to make the
remembered layout active again, which it cannot do at its own top level because
`lua/tiling.lua` has not registered itself yet. `monitors` then says the screens
are what they are, and `layout` asks you to arrange.

Without this, a reload was a quiet way to lose the session: on workspace 3 it
came back believing it was on workspace 1, put every window on desk 1, and drew
the desk two screen-widths off-stage with no key that brought it back. That is
issue #116, and both halves above are what it cost.

## What a mode can ask

```lua
sol.windows()          -- every window: id, rect, drawn, title, focused, monitor,
                       --                modal, parent, leaving
sol.monitors()         -- every monitor: name, x, y, w, h, whole, scale,
                       --                 transform, focused, primary
sol.monitor()          -- the active monitor's work area
sol.monitor(id)        -- the work area of the monitor that window is on
sol.cursor()           -- { x, y }
sol.window_at(x, y, skip)  -- the id under a point, optionally skipping one
```

`sol.windows()` is a snapshot taken fresh for your handler, never a live view.
A window closing while you hold its id is ordinary: `place` and `present` on an
id that no longer exists do nothing rather than failing.

`rect` is where the window lives; `drawn` is where it is being drawn right now,
which in a mode is somewhere else. Read `drawn` when you care what the user is
looking at, `rect` when you care what the layout thinks.

`modal` is a window that says it is a modal dialog — a save prompt, a
permissions box — and `parent` is the window it belongs to. A layout should
leave a modal out of its arrangement and put it over its parent; `dialogs.lua`
is that policy, shared by `tiling.lua` and `scrolling.lua` so the two cannot
disagree. Onto its *parent's* monitor, which is not always its own: a window is
mapped at the origin and belongs to whichever screen that lands on, so a dialog
for a window on the second screen arrives on the first one and has to be moved
off it. `dialogs.place` is where that is decided, from the rect it is centred on
rather than from the screen it was mapped on.

Dragging one is allowed and sticks: `dialogs.dropped` records where it was
dropped as an offset from its parent, so the prompt you pushed off the sentence
it was covering stays off it, and still follows the document when the layout
moves it. Staying *above* that document is not the layout's business at all —
the compositor keeps a modal over its parent in the stack, whatever raises what
(`Solium::map_stacked`).

`parent` has three values and it is worth knowing why. It is the parent's id
when there is a window to point at, `false` when the client named a parent that
is not on screen — not mapped yet, not ours, or closed while its dialog was
still up — and absent when no parent was ever named. So `if window.parent then`
is the right question for "have I got something to centre on", and
`window.parent == false` is how you tell a lost parent from no parent at all.

`skip` on `window_at` exists because of one specific bug: a new window is
already mapped and under the pointer, so asking "what am I pointing at" without
skipping it names the window as its own split target.

## A layout runs per monitor

There is **one global coordinate space** and every monitor is a rectangle in
it. That is the whole mechanism: a window is on the second screen because its
`x` lands there. There is no per-screen coordinate system and nothing to
convert.

The consequence is the thing to get right. `sol.monitor()` answers *one*
screen, so a mode that lays every window out against it piles both monitors'
worth of windows onto whichever one the pointer happens to be on. A layout is
per monitor:

```lua
local monitors = require("monitors")

for _, each in ipairs(monitors.each()) do
    -- each.monitor is the rect; each.windows are the windows on it
end
```

`monitors.each()` always lists every monitor, including one with nothing on it
— a layout has to hear about an empty screen, because that is what tells it the
last window left.

Anything stateful is keyed by monitor as well as workspace. `tiling.lua` keeps
a dwindle tree per pair and `scrolling.lua` a strip, via `monitors.key`:

```lua
local function tree_for(workspace, monitor)
    local key = monitors.key(workspace, monitor)
    ...
end
```

Two reasons, and the second is the one that bites. The screens are different
sizes, so a split or a column width that reads well on one is wrong on the
other. And a window moved across has to *leave* one arrangement and join the
other: a window in two trees is a window given two slots, and it ends up in
whichever was laid out last. `adopt` is where that settles — missing from its
new screen's tree, still in its old one's, both halves fixed in one pass.

**One `sol.animate` for every screen.** Two monitors rearranging at once is one
movement; see [animation.md](animation.md) on why the feel is set per batch and
not per window.

### A window is drawn only on the monitors it lives on

Its **slot** decides that, not its transform. A transform can move a window
around its own monitors and off them; it cannot put it on somebody else's.

This is worth knowing before you write a mode that moves a window a long way,
because it is what makes such a mode work on more than one screen. Workspaces
are the example. A workspace switch does not move windows — it draws the ones
belonging to other workspaces a screen away, and with one monitor "a screen
away" is off the desktop, which is how they are hidden. With two side by side,
one screen away is *the other monitor*: switching the left screen's workspace
threw its windows onto the right screen, on top of what was already there.

The intent was always containment; one screen just made moving and hiding the
same thing. So the rule is enforced where it belongs, and the slide stays one
screen long — which is what makes it read as a slide. Offsetting by the whole
desk instead would hide them correctly and look wrong: the window would leave
the screen halfway through and the next arrive halfway through, with empty
screen in between.

The slot and not the drawn rect, so a window straddling the bezel — dragged
between screens, where the slot itself is on both — is still drawn on both.

### Workspaces

`config.workspaces.per_monitor` decides whether each screen has its own
workspace in view. On by default: `super+2` switches the monitor the pointer is
on and leaves the other showing what it was. Off, one switch moves every
screen.

Both are real desktops, and the difference is what you take a workspace to
*be* — a screenful, or a whole desk. `workspaces.lua` shares every line
between them: which workspace a monitor shows is looked up by monitor either
way, and with the setting off every monitor looks up the same entry.

A workspace is a **selection**, one per monitor per workspace, named
`desk-<n>@<connector>`. `workspaces.lua` declares it from the windows on that
desk plus that desk's background, and carries it with a single
`sol.present_group` — which is why a per-workspace wallpaper travels with the
switch. Set `config.wallpaper` to a list of images to get one; a single image
belongs to the monitor and stays put, because every desk sharing one picture
makes a wallpaper that slides indistinguishable from one that does not.

If you keep per-workspace state of your own, key it by monitor as well.
`monitors.key(workspaces.on(name), name)` is the string `tiling.lua` and
`scrolling.lua` both use, and `tree_for` takes only the monitor — the workspace
that screen is showing is something `workspaces` knows, and threading it
through every call site is how one of them ends up asking for the wrong
screen's.

`monitors.active()` is the monitor the pointer is on — where a new window goes,
and what a binding pressed with no particular window in mind is about. It is
the pointer and not the focused window on purpose: look at the second screen,
click the empty desktop, press the key for a terminal, and a focus-based rule
would open it on the screen you just looked away from.

The `primary` flag is a different question and answers a different one: it is
where things belonging to *one* screen go — a dock, a bar, a layer surface that
named no output. It does not move, which is the whole point of it. See
[shell-boundary.md](shell-boundary.md).

`x`, `y`, `w`, `h` are the **work area** — the monitor less whatever bars have
reserved — and `whole` is the monitor itself, which is what a wallpaper or a
fullscreen window covers. Both are in the global space, so either can be handed
straight to `sol.place`. Copy the rect before adding keys to it; the one you
were given belongs to the snapshot.

## The arrangements that ship

You do not have to compute geometry yourself.

```lua
local slots = sol.layout.grid(windows, area)          -- overview's grid
local slots = sol.layout.master_stack(count, area)    -- one big, the rest beside
local slots = sol.layout.strip(columns, area)         -- a row, with an offset
```

These are pure: windows in, rectangles out. Two are not, and cannot be:

```lua
local tree = sol.layout.tree()        -- dwindle
local scroller = sol.layout.scroller()  -- niri's model
```

A dwindle tree is stateful because the arrangement is. Where a window lands
depends on which window was split and where the pointer was, and no function of
"how many windows are there" can recover that afterwards. Same for a scroller:
which column is active and where the view sits relative to it are not in the
window list.

They are held by the script that made one, not by the compositor:

```lua
tree:insert(id, target, x, y, options)
tree:remove(id)
tree:contains(id)
tree:windows()
tree:layout(options)      -- the slots, to hand to sol.place
tree:resize(id, "width", share)                        -- keyboard: an axis
tree:drag_seam(id, "right", edge_x, edge_y, options)   -- pointer: a side
```

The two resize calls take different things on purpose. A drag names a side —
the hand is on one specific edge, and which seam moves follows from that. A
keypress names only an axis: `super+equal` means "wider" and says nothing about
which neighbour gives up the room, so `resize` prefers the seam on the right or
below and falls back to the other, with positive `share` always growing the
window. Either way, a window flush against its container on that side has no
seam there and nothing happens — the screen edge is not a seam.

`drag_seam` puts the named side of that window at `edge_x` (for `"left"` and
`"right"`) or `edge_y` (for `"top"` and `"bottom"`), reading only the one its
side names — which is what lets a corner drag call it twice with one pair and
have each axis take its own. Hand it the `edge_x`, `edge_y` a `resize` gave you
and the gesture is relative; hand it the pointer and you have rebuilt #124.
The ratio it computes is separately clamped to `0.05..0.95`, so a window shoved
hard against a seam stops there rather than vanishing. That clamp is the only
bound on a tiled drag: the compositor sends an unfloored edge, deliberately,
because a floor measured in a window's pixels cannot bound a seam's position —
see `Tiling::drag_seam`.

`options` is a monitor's work area with `gap` and `split` added. Passing the
monitor in rather than the tree asking for it is what lets one tree per
workspace *per monitor* exist without any of them knowing about either. Copy
the rect before adding keys — the one from `sol.monitors()` belongs to the
snapshot.

## A whole mode

![Overview entering and leaving, and the desktop coming back unchanged](overview-in-lua.png)

Above: three windows at rest with their QML frames, `super+space` into overview,
and `super+space` again. The last frame differs from the first by zero pixels,
which is the property that matters — a mode that cannot put the desktop back
exactly is a mode nobody will use twice. All of it is `lua/overview.lua`.


This is real and it works. Paste it into `~/.config/solium/mymode.lua` and
`require("mymode")` from your `init.lua`.

```lua
-- Two columns, newest window on the right, everything else stacked on the left.
local modes = require("modes")
local mine = { active = false }

local function arrange()
    if not mine.active then return end
    local windows = sol.windows()
    if #windows == 0 then return end

    local area = sol.monitor()
    local gap = 12
    local half = (area.w - gap * 3) / 2

    sol.animate({ duration = 220, easing = "outCubic" })

    -- The newest window takes the right half.
    local newest = windows[1]
    sol.place(newest.id, {
        x = area.x + gap * 2 + half, y = area.y + gap,
        w = half, h = area.h - gap * 2,
    })

    -- The rest share the left half, top to bottom.
    local rest = #windows - 1
    if rest > 0 then
        local each = (area.h - gap * (rest + 1)) / rest
        for index = 2, #windows do
            sol.place(windows[index].id, {
                x = area.x + gap, y = area.y + gap + (index - 2) * (each + gap),
                w = half, h = each,
            })
        end
    end
end

sol.on("open",   arrange)
sol.on("close",  arrange)
sol.on("layout", arrange)

function mine.started() arrange() end
function mine.toggle() modes.use("mine") end

modes.register("mine", mine)
sol.bind("super+y", mine.toggle)

return mine
```

Four things in there are the conventions rather than the content.

**Register with `modes`, do not keep your own on/off flag.** A layout is a
choice of one. Two layouts both placing every window means the second one to run
wins and the arrangement looks like whichever that happened to be — and
switching away from one leaves its windows where it put them. `modes.use(name)`
turns the others off, calls your `started`, and calls their `stopped`. Calling
it with the mode already current falls back to floating, so one key toggles.

**Guard on `active`.** Your handlers stay registered when your mode is off.

**`sol.animate` before the batch, not per window.** It applies to everything
queued after it, so every window in one arrangement moves over the same interval
on the same clock. Setting it per window is how an arrangement ends up looking
like several separate animations that happen to overlap.

**Newest window first.** `sol.windows()` is topmost-first, which is the order a
hit test wants and the order "the one I just opened" is at the front of.

## Modes that are not layouts

`overview.lua` is worth reading as the other shape a mode takes: it grabs input,
transforms every window onto a grid with `present`, and clears them on the way
out. It never calls `place`, so the layout underneath is untouched and leaving
is exact.

That file is also the architecture's proof, and its comment says so — the app
switcher is that with a row instead of a grid, peek is it with one window at the
cursor, and the icon-to-window genie is it with a dock icon named as the thing
the window comes out of. If any of those ever needs new Rust, the transform
layer is missing something.

## Worth knowing

`solium --check` prints every binding it registered and will tell you an edit
dropped one. `super+shift+r` reloads while the session runs, so a mode can be
written against a desktop you are using.

A configuration that fails to load is reported with the file and the line, and
the running session keeps whatever it already had. A typo costs a line of output
rather than your windows.

See also: **[animation.md](animation.md)** for how the feel is configured,
**[ricing.md](ricing.md)** for the settings a mode should read rather than
hardcode.
