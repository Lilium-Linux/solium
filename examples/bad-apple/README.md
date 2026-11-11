# Bad Apple, played by a window manager

A window manager for Solium, written in C#, that plays a black and white film
by moving windows. It opens a few dozen plain white windows and, thirty times
a second, tells the compositor where to draw each one, so that together they
are the lit parts of the frame. Nothing is drawn by this program: every pixel
on screen is a window, put there by Solium.

It is here to show that a window manager is an ordinary program in any
language. It links nothing of Solium's. It reads the protocol's WIT files,
writes its own C# types from them, and talks to the compositor over the socket
it is started with. [Writing a window manager](../../docs/window-managers.md)
describes that conversation; this is one, start to finish, in about seven
hundred lines of C#, four hundred of them the window manager itself.

The name is the tradition it joins: the shadow-art film for *Bad Apple!!* has
been played on oscilloscopes, in spreadsheets and in file managers. The film
itself is not in this repository. Bring your own copy, or any other black and
white video.

## What you need

- A build of Solium, with the example client that opens the windows:

  ```sh
  cargo build -p solium -p wl-probe --examples --bins
  ```

- The .NET SDK, 8 or later.
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

Each frame is cut down to a grid of cells and its lit cells are covered with
rectangles, the largest first. A rectangle is a window, so:

| Option | What it does | Default |
|---|---|---|
| `--grid 48x36` | Cells across and down. Finer is a truer picture. | `48x36` |
| `--most 64` | The most rectangles a frame may have, which is how many windows are opened. A frame that needs more loses its smallest details. | `64` |
| `--fps 30` | Frames a second. | `30` |
| `--dark` | Light the dark parts instead of the bright ones. | off |

## Play it

Name the program in a `wm.toml`. To try it in a window, without touching your
own session, give a nested Solium a configuration directory of its own. From
`examples/bad-apple`, write this as `/tmp/apple/solium/wm.toml`, with the
three paths written out in full:

```toml
command = [
    "dotnet", "/path/to/solium/examples/bad-apple/bin/bad-apple.dll",
    "/path/to/solium/examples/bad-apple/bad-apple.frames",
    "/path/to/solium/target/debug/examples/tiles",
]
```

and start Solium on it:

```sh
XDG_CONFIG_HOME=/tmp/apple ../../target/debug/solium
```

The first argument is the film; the rest is the program that opens the
windows, which is told how many. `--glide` before the film has the compositor
glide each window to its next place instead of putting it there at once.

| Key | What it does |
|---|---|
| `space` | Pause, and go on. |
| `r` | From the beginning. |
| `escape` | End the session. |

The film starts over when it ends.

## How it works

| File | What is in it |
|---|---|
| `src/Program.cs` | The window manager: the handshake, the batches it answers, and the frames it sends. |
| `src/Connection.cs` | The socket and the frames on it: a length, then a message. |
| `src/Wire.cs` | The encoding of each kind of value. |
| `src/Film.cs` | Reading a film. |
| `gen/wit2cs.py` | Writes the protocol's C# types from the WIT. |
| `frames.py` | Turns a video into a film. |

What it uses of the protocol:

- **The handshake.** It reads `hello`, refuses a compositor of another
  version, and answers with what it wants before the first frame.
- **`declare`.** Its three keys are declared, so the compositor knows they are
  its own without asking, and nothing under them is typed into.
- **`listen`.** It listens to `opened`, `closed` and `monitors-changed`. It is
  never woken for anything else, however much happens on the desktop.
- **`decoration`.** Windows are drawn with no frame.
- **`run-detached`.** It asks the compositor to start the program that opens
  the windows. A window manager starts nothing itself.
- **`present`.** Each window is drawn at a rectangle that is not where it
  lives. Its application is told nothing and does no work: the compositor
  stretches the window's picture to the rectangle. A window the frame has no
  use for is drawn with no opacity.
- **`submit`.** Frames are sent on this program's own clock, with no event
  asking for them. In a whole session it answers a few dozen batches (one a
  window opening, and its keys); every frame of the film is a `submit`.

The log of a session ends with what the compositor made of it:

```text
window manager: 69 batches answered: half within 32 us, …; most were opened 64
(80 us each), key 3 (228 us each), monitors-changed 1, startup 1
```
