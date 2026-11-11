# Bad Apple, played by a window manager

A window manager for Solium, written in C#, that plays a black and white film
by managing windows. Each frame of the film is cut into rectangles, the light
parts into some and the dark parts into others, and every rectangle is a real
terminal with a shell and its prompt, light-themed or dark-themed. A rectangle
lives from frame to frame as a window does, so playing the film is moving and
resizing terminals: twenty-four times a second the window manager places the
ones whose rectangles changed. Nothing is drawn by this program: every part of
the picture is a terminal window, put there by Solium.

It is here to show that a window manager is an ordinary program in any
language. It links nothing of Solium's. It reads the protocol's WIT files,
writes its own C# types from them, and talks to the compositor over the socket
it is started with. [Writing a window manager](../../docs/window-managers.md)
describes that conversation; this is one, start to finish, in about a
thousand lines of C#, seven hundred of them the window manager itself.

The name is the tradition it joins: the shadow-art film for *Bad Apple!!* has
been played on oscilloscopes, in spreadsheets and in file managers. The film
itself is not in this repository. Bring your own copy, or any other black and
white video.

## What you need

- A build of Solium: `cargo build`.
- The .NET SDK, 8 or later.
- [foot](https://codeberg.org/dnkl/foot), for the terminals. One foot server
  holds all the windows of a theme. To use another terminal, change
  `terminals.sh`; it has to be one that draws a new size as soon as it is
  given one. kitty waits for a resize to be over before it draws, and so is a
  size behind for the whole film.
- Python 3 and ffmpeg, to turn a video into a film.

## Build it

```sh
cd examples/bad-apple
dotnet build -c Release -o bin
```

The build first runs `gen/wit2cs.py`, which writes `obj/Protocol.g.cs` from
`crates/wm-protocol/wit/`: every record, variant and enum of the protocol as a
C# type that can read and write itself.

## Make a film

```sh
python3 frames.py ~/Videos/bad-apple.mp4 bad-apple.frames
```

| Option | What it does | Default |
|---|---|---|
| `--grid 32x24` | Cells across and down. A finer grid is a truer picture, smaller terminals and more of them. The smallest terminal is one cell, which at 32x24 on a 1600x900 screen still holds a prompt. | `32x24` |
| `--fps 24` | Frames a second. | `24` |

Each frame is cut down to the grid, and each colour's cells are covered with
rectangles. A rectangle is carried from one frame to the next: it keeps what
is still its colour, grows into what has become so, joins a neighbour it
makes a rectangle with, takes in the smaller ones along its edges a line a
frame, and is gone only when nothing of it is left. So most of a frame is the
windows of the frame before, a few of them moved or resized. `frames.py` says
how many terminals of each colour the film needs: for the whole of
*Bad Apple!!* at 32x24, 69 light and 73 dark, about 43 of them on screen at a
time.

## Play it

Name the program in a `wm.toml`. To try it in a window, without touching your
own session, give a nested Solium a configuration directory of its own. Write
this as `/tmp/apple/solium/wm.toml`, with the three paths written out in full:

```toml
command = [
    "dotnet", "/path/to/solium/examples/bad-apple/bin/bad-apple.dll",
    "/path/to/solium/examples/bad-apple/bad-apple.frames",
    "/path/to/solium/examples/bad-apple/terminals.sh",
]
```

and start Solium on it:

```sh
XDG_CONFIG_HOME=/tmp/apple ./target/debug/solium
```

Nothing else is read: with `wm.toml` naming this program, the Lua window
manager and its configuration are not started at all.

The terminals open and glide into rows, light and dark by turns, each in a
plain border. Once the shell in every one of them has printed its prompt, and
they have stood so for five seconds, the film begins: the terminals a frame
needs take their rectangles, and the rest are not drawn. When it ends the rows
come back, and the film starts again.

| Key | What it does |
|---|---|
| `space` | Pause, and go on. |
| `r` | Back to the rows, and the film from the beginning. |
| `escape` | End the session. |

A panel in the corner says how it is going:

| Row | What it is |
|---|---|
| `FPS` | Frames of the film shown, a second: fewer than the film has when the compositor took longer than a frame over one. |
| `Terminals` | How many terminal windows are open. |
| `Moves` | Terminals moved or resized, a second. |
| `WM CPU` | What this program uses of one processor. |

## How it works

| File | What is in it |
|---|---|
| `src/Program.cs` | The window manager: the handshake, and the batches it answers, a frame of the film among them twenty-four times a second. |
| `src/Connection.cs` | The socket and the frames on it: a length, then a message. |
| `src/Wire.cs` | The encoding of each kind of value. |
| `src/Film.cs` | Reading a film. |
| `Stats.qml` | The panel: a scene the compositor draws, with the rows it is handed. |
| `terminals.sh` | Opens the terminals of one theme. |
| `gen/wit2cs.py` | Writes the protocol's C# types from the WIT. |
| `frames.py` | Turns a video into a film. |

What it uses of the protocol:

- **The handshake.** It reads `hello`, refuses a compositor of another
  version, and answers with what it wants before the first frame.
- **`declare`.** Its three keys are declared, so the compositor knows they are
  its own without asking, and nothing under them is typed into.
- **`listen`.** It listens to `opened`, `closed`, `monitors-changed` and
  `renamed`. Beyond those it is woken only for what it asked for itself, its
  keys and its timers, however much happens on the desktop. `renamed` is how
  it knows a terminal is ready: the shell's prompt titles its window `ready`
  as it is printed.
- **`configure`.** One setting: the session is not locked for want of a key
  while a film plays.
- **`decoration`.** Every window wears a plain border, from a style the
  compositor ships.
- **`run-detached`.** It asks the compositor to start the terminals. A window
  manager starts nothing itself.
- **`timer`.** It keeps no clock of its own: the compositor's timers tick on
  the clock its animations run on, and this program sets two. One falls due
  for each frame of the film while it plays, and its answer to that is the
  frame. The other falls due twice a second, always, for the panel. Every
  batch says what the compositor's clock read, so the panel's rates are
  worked out by that clock too.
- **`place`.** Each terminal is put in its row in the answer to its `opened`
  event. Then, for each frame, every terminal whose rectangle changed is
  placed where it is now, with no motion. The compositor draws it there from
  the next frame, whole, from the picture the terminal last drew, and keeps
  it so while the terminal draws the new size.
- **`present` and `clear`.** Whether a terminal is drawn. One no rectangle
  needs stays open, where it was, with no opacity, and `clear` takes that
  away when a rectangle needs it again.
- **`surface`.** The panel is a QML scene of its own, which it hands new
  numbers twice a second.

It never sends a `submit`: everything it does is its answer to something the
compositor told it. That is also what lets a capture slow the film down with
everything else: `SOLIUM_CLOCK_RATE` ([dev/README.md](../../dev/README.md),
"The knobs") slows the compositor's clock, and the film is on it.
