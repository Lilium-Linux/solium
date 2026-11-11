# Bad Apple, played by a window manager

A window manager for Solium, written in C#, that plays a black and white film
with real terminals. The film is cut down to a grid of cells. In every cell
stand two terminals, one with a light theme and one with a dark one, each with
a shell and its prompt, and for each frame the window manager has the
compositor draw the one whose colour the cell needs. Nothing is drawn by this
program: every cell on screen is a terminal window, and which windows are
drawn is the whole of the picture.

It is here to show that a window manager is an ordinary program in any
language. It links nothing of Solium's. It reads the protocol's WIT files,
writes its own C# types from them, and talks to the compositor over the socket
it is started with. [Writing a window manager](../../docs/window-managers.md)
describes that conversation; this is one, start to finish, in about nine
hundred lines of C#, six hundred of them the window manager itself.

The name is the tradition it joins: the shadow-art film for *Bad Apple!!* has
been played on oscilloscopes, in spreadsheets and in file managers. The film
itself is not in this repository. Bring your own copy, or any other black and
white video.

## What you need

- A build of Solium: `cargo build`.
- The .NET SDK, 8 or later.
- [kitty](https://sw.kovidgoyal.net/kitty/), for the terminals. One kitty
  process holds all the windows of a theme, so a few hundred of them fit in a
  few gigabytes. To use another terminal, change `terminals.sh`.
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
| `--grid 16x12` | Cells across and down. Each cell is two terminals, so 16x12 is 384 windows. A finer grid is a truer picture, smaller terminals and more for the compositor to hold. | `16x12` |
| `--fps 24` | Frames a second. | `24` |

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

The terminals open and glide into a chequerboard, each in a plain border.
Once the shell in every one of them has printed its prompt the film begins;
when it ends, the chequerboard comes back, and the film starts again.

| Key | What it does |
|---|---|
| `space` | Pause, and go on. |
| `r` | Back to the chequerboard, and the film from the beginning. |
| `escape` | End the session. |

A panel in the corner says how it is going:

| Row | What it is |
|---|---|
| `FPS` | Frames of the film shown in the last half second, as a rate. |
| `Terminals` | How many terminal windows are open. |
| `Changes` | Cells that changed colour, a second. |
| `WM CPU` | What this program uses of one processor. |

## How it works

| File | What is in it |
|---|---|
| `src/Program.cs` | The window manager: the handshake, the batches it answers, and the frames it sends. |
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
- **`listen`.** It listens to `opened`, `closed` and `monitors-changed`.
  Beyond those it is woken only for what it asked for itself, its keys and
  its timer, however much happens on the desktop.
- **`configure`.** One setting: the session is not locked for want of a key
  while a film plays.
- **`decoration`.** Every window wears a plain border, from a style the
  compositor ships.
- **`run-detached`.** It asks the compositor to start the terminals. A window
  manager starts nothing itself.
- **`place`.** Each terminal is put in its cell in the answer to its `opened`
  event, once. It is never moved or resized again: a real application cannot
  be resized twenty-four times a second, and need not be.
- **`present`.** Whether a terminal is drawn. The one whose theme its cell
  does not need stays open, in its place, with no opacity; its application is
  told nothing and does no work.
- **`timer`.** What changed on the desktop comes only with a batch, and a
  batch only with an event. While the terminals start, a timer on the
  compositor's clock is that event, four times a second: each brings the
  window titles that changed, and a terminal's title is how this program
  knows its shell has printed its prompt. The timer is cancelled when the
  film begins.
- **`submit`.** Frames are sent on this program's own clock, with no event
  asking for them. In a whole session it answers one batch for each terminal
  that opens, each tick of that timer, and its keys; every frame of the film
  is a `submit`.
- **`surface`.** The panel is a QML scene of its own, which it hands new
  numbers twice a second.
