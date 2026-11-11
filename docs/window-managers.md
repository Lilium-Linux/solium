# Writing a window manager

Solium is a display server and an engine: it speaks Wayland, draws every
frame, reads the input devices, runs the animations, the effects and the QML
scenes, and keeps the session's services. It does not decide where a window
goes or what a key does. A **window manager** decides that, and it is a
separate program, as it is under an X server.

The window manager Solium ships is `solium-lua`, which runs the Lua
configuration. Any program that speaks the same protocol can take its place,
in any language, and nobody rebuilds Solium to change it.

This page says how such a program is started, how the conversation with the
compositor goes, what happens when it is slow or stops, and how each value is
written on the socket. Every message and type is listed, with what it means,
in [the protocol reference](generated/reference/protocol.md).

Use this guide by task:

- [Run another window manager](#choosing-the-window-manager).
- [Follow the conversation](#the-conversation), from `hello` to `goodbye`.
- [Know how long the compositor waits](#how-long-the-compositor-waits) and
  [what it does when a window manager stops](#when-a-window-manager-stops).
- [Encode and decode the messages](#the-wire-format) without generated code.
- Start from an example, [in Rust](#a-window-manager-in-rust) or
  [in C#](#a-window-manager-in-another-language).

## What each side does

| Solium | The window manager |
|---|---|
| Wayland and X11 clients, their surfaces and buffers | Where each window lives (`place`), and where it is drawn (`present`) |
| Rendering, frame timing, every frame of every animation | How a motion moves: its duration and easing, or a spring |
| Keyboard, pointer and touch devices; which window has the keyboard | Which keys are its own (`declare`), and what each does |
| QML scenes: it loads and draws them | Which scenes are on the desktop (`surface`), and their properties |
| Effects, run in their sandbox | Which effect applies to what (the `effects` settings) |
| Starting programs, timers, reading and writing files | Asking for each (`spawn`, `run`, `timer`, `file-read`) |
| Notifications, audio, the tray, media players | Showing them, and deciding a notification's fate |

A window manager never draws and never keeps time. It says where something
goes and how it gets there, and the compositor moves it: a window follows the
pointer in the compositor, at the display's refresh rate, whatever the window
manager is doing.

## Choosing the window manager

`~/.config/solium/wm.toml` says which program is started. It is the one file
of the configuration that Solium reads for itself, because it is read before
any window manager runs.

```toml
# ~/.config/solium/wm.toml
command = ["my-wm", "--rules", "~/.config/my-wm/rules"]
cpu_weight = 1000
```

| Key | What it is |
|---|---|
| `command` | The program and its arguments, run exactly as written. A program with no directory in its name is looked for on `PATH`, and a leading `~/` in the program or an argument is your home directory. Without this key, the shipped `solium-lua` runs. |
| `cpu_weight` | The CPU weight of the systemd scope the window manager is put in, from 1 to 10000, where 100 is what an ordinary program has. 1000 unless you say. |

The program is started with the socket to the compositor as its **standard
input**, as Solium starts every program: with the environment Solium itself
was started with, in a session of its own, and holding no file descriptor but
its standard input, output and error. It gets no arguments but the ones you
wrote. What it writes to its standard error goes to the compositor's log
(`~/.local/state/solium/session.log`).

- The file is read each time a window manager is started. After you change
  `command`, a reload starts the new program; if it will not start, the one
  that was running goes on.
- A key the file gets wrong is shown on the desktop's problems overlay and
  fails `solium --check`, which also names the program it would start.
- A program that cannot be started, or that says `goodbye` instead of
  answering `hello`, is replaced by the shipped configuration, with the reason
  on the overlay. See [when a window manager stops](#when-a-window-manager-stops).

When Solium is the session (started by its session file), it asks systemd to
put the window manager's process in a scope of its own, `solium-wm-<pid>.scope`
in `session.slice`, at `cpu_weight`. A window manager answers in microseconds
and then sleeps, so the weight costs nothing and keeps its answers inside a
frame while every core is busy. Started nested or by hand, Solium asks systemd
for nothing and the window manager stays a plain child process.

## The conversation

Everything is a **message**, one per frame on the socket. The names below are
the protocol's own; each is described in [the reference](generated/reference/protocol.md).

### 1. Hello

The compositor speaks first:

- `hello` carries the protocol's `version`, whether the session is in debug
  mode, and `keeps`: what the window manager this one takes the place of asked
  the compositor to hold (nothing, at the start of a session).

The window manager answers with one of:

- `hello-answer`: the version it was built for, and the commands that set the
  desktop up before its first frame. These usually include `declare` (its key
  bindings), `listen` (the events it wants) and `configure` (settings).
- `goodbye`, with why it cannot run. The compositor then runs the shipped
  configuration in its place.

Two versions speak to each other when their major numbers are equal. Before
1.0, the minor numbers must be equal too: `0.1.3` speaks to `0.1.0` and not to
`0.2.0`. A window manager refuses a compositor of another version in its
answer to `hello`, and the compositor refuses a `hello-answer` of another
version.

The compositor waits `wm.load_ms` (10 seconds) for the answer to `hello`.

### 2. Batches and answers

From then on the compositor sends a `batch` whenever something the window
manager listens to happens. A batch has:

- a `serial`;
- `changes`: what changed on the desktop since the last batch. A window or a
  monitor is sent whole when any field of it changed. Apply these first, to
  your own copy of the desktop, so each event is handled with the desktop as
  it was when it happened;
- `events`: what happened, in order.

The window manager answers **every batch once, in the order they came**, with
an `answer` naming the batch's serial:

- `handled`: whether the batch's key, press or scroll was the window
  manager's. One it did not take goes to the window under it. For an `opened`
  event, `handled` with a `place` command means the window manager placed the
  window; otherwise the compositor places it, centred and cascaded.
- `commands`: applied in order, all at once, on the next frame at the latest.
- `motion`: for a `fullscreen` or `maximize` event, how the window's picture
  moves to its new place.
- `decision`: for a `notification` event, what becomes of it.

The first batch is `startup`, sent once `hello-answer`'s commands are applied:
its changes are the whole desktop, every window and monitor there is.

A window manager's picture of the desktop is as fresh as the last batch. What
changed comes only with a batch, and a batch is sent only for an event it
listens to, so one that wants to know of every window listens to `opened` and
`closed`.

### 3. Listening

A window manager is sent only the events it named in its last `listen`
command, which lists all of them each time. Until it has sent one, it listens
to none. An event nobody listens to is not sent at all: the compositor treats
it as one nobody handled and does its own thing.

Eight events need no listening, because each answers something the window
manager itself asked for, or is the moment it starts: `startup`, `key`,
`timers`, `runs`, `files`, `attempts-settled`, `notify-action` and
`notify-forget`.

Listen to what you use. A window manager that listens to `dragged` is woken
for every motion of a dragged window and must answer each one.

### 4. Key bindings

The key combinations that are the window manager's are **declared**, in the
`declare` command, so the compositor answers a key without asking: one that is
declared is the window manager's, and any other goes to the focused window.

A combination is any of the modifiers `ctrl`, `alt`, `shift` and `super` and
one key by its XKB name, joined by `+`, in any order and either case:
`super+shift+e`. When it is pressed, the window manager is sent `key` with the
combination spelled as it declared it. The compositor does the matching, so a
binding works whichever keyboard layout is live.

A binding can also be `repeating` (fired again while held), `locked` (fired at
the lock screen) and `inhibited` (fired over a window that holds the
compositor's shortcuts, as a virtual machine does). `declare` is sent whole
each time it changes.

### 5. Commands of its own

A window manager can send `submit`, a list of commands, at any time, with no
batch asking for them: what a timer or a socket of its own led to. They are
applied in the order they come.

For timing, prefer the compositor's clock: the `timer` command sets one, and
the `timers` event says when it falls due. That clock is the one animations
run on.

### 6. Questions

In the middle of a batch, a window manager can ask the compositor something it
cannot know from its own copy of the desktop, with `query`, and gets a `reply`
with the same serial:

- `window-at`: the topmost window drawn at a point;
- `store`: a saved document, read from its file;
- `styles`: the window frame styles this machine has.

The compositor may already have sent the next batch when it reads the
question, so other messages can arrive before the reply. Keep them, and handle
them after the batch you are in.

### 7. Reload

`reload` asks the window manager to load its configuration again: the user
pressed the reload key, or a file under `~/.config/solium` changed. It answers
`reloaded`:

- `loaded`, with an answer whose commands set the desktop up as
  `hello-answer`'s did. What the window manager declared and listened to
  before is forgotten, so say both again;
- `failed`, with why. Nothing changes, and the reason is shown on the overlay.

A window manager with no configuration of its own to read answers `loaded`
with what it said at the start.

### 8. Goodbye

Either side may say `goodbye`, with why. The compositor says it as the session
ends, waits a second for the window manager to exit, and then stops it.

## How long the compositor waits

A window manager that is slow never holds a frame.

- The compositor waits **one frame** for an answer: `wm.ask_ms`, 4 ms, which
  is a frame at 260 Hz. The socket itself takes about 20 microseconds there
  and back, so how long an answer takes is how long the window manager works
  on it, and one that is late is a window manager that is busy or stuck.
- When the answer is late the compositor goes on, and does what it would do
  with no window manager:

  | The batch was | The compositor |
  |---|---|
  | a bound key | does not type it into the focused window |
  | a window opening | places it plainly, centred and cascaded |
  | a notification | shows it |
  | a fullscreen or maximise | moves the window with its default motion |
  | a wheel turn, or a window's own resize | does as it did for the last one |
  | anything else | nothing |

- The late answer is **applied when it comes**. A window the compositor placed
  plainly then moves to where the window manager wanted it.
- While an answer is owed, **no later batch is waited for at all**. So a slow
  window manager costs one frame once, not one frame for each event. Once it
  has answered everything, the next batch is waited for again.

The window manager sets these with the `wm` settings section
(`configure("wm", …)`):

| Key | Default | What it is |
|---|---|---|
| `ask_ms` | 4 | How long an answer is waited for. |
| `hard_ms` | 1000 | How long without a word, with a batch unanswered, counts once against the window manager. |
| `strikes` | 3 | How many of those in a row mean it has stopped. |
| `load_ms` | 10000 | How long `hello` and `reload` are waited for. |
| `restarts` | 3 | How many times it may stop inside `restart_window_s` before the shipped configuration takes its place. |
| `restart_window_s` | 60 | The window those are counted in. |

## When a window manager stops

A window manager has stopped when its socket closes, a read or a write on it
fails, it says `goodbye`, or it has sent nothing at all for `strikes` ×
`hard_ms` (3 seconds) while a batch waits for its answer. Any byte it sends,
a question included, counts as a word.

Then:

1. **Every window stays where it is.** The compositor goes on drawing, and
   clients are told nothing.
2. The same program is **started again**, about a tenth of a second later,
   and a problem on the overlay says what happened.
3. It is handed, in `hello`, **what the last one kept**: every document it
   sent with `keep`. A window manager that keeps its layout this way comes
   back with every window where it was.
4. It is told the desktop from the start (every window and monitor, as
   changes), and sent `restored`.

The timers, runs and file watches of the one that stopped are gone with it.

A window manager that stops `wm.restarts` times inside `wm.restart_window_s`
is not started a fourth time. The **shipped configuration** runs in its place,
reading nothing of yours, and the overlay says so. A reload tries yours again.
If the shipped one stops as often, none runs.

The same happens at the start of a session: a window manager that will not
start gives way to the shipped configuration, so there is always a key that
opens a terminal.

### Keeping state

`keep(name, document)` asks the compositor to hold a document under a name.
Send it whenever the state changes, or when the compositor sends
`save-stores`, which it does every `files.store_interval` (a second) to a
window manager that listens to it. The compositor holds the latest of each
name and hands them all back in the next `hello`.

A reload forgets them: the window manager is the same process and still has
its state.

## Settings

Every setting of the compositor is set with `configure(section, document)`:
the section's name (`input`, `idle`, `monitors`, `effects`, `lock` and the
rest) and a document of its keys. The sections and their keys are the ones in
[the configuration reference](generated/reference/configuration.md), which
writes each as the Lua table the shipped window manager sends.

A **document** is a tree of tables, numbers, text and booleans, flattened into
a list because WIT has no recursive types: the first node is the root, and a
table names its children by their index in the list. A table has a list part
(the values at 1, 2, 3…), named fields, and numbered fields.

A section the compositor cannot read is left as it was, and the mistake is a
problem on the overlay with the key named.

## The wire format

The socket is a Unix stream socket: the window manager's standard input. Both
directions use it.

A **frame** is a `u32` byte count, little-endian, followed by that many bytes:
one `message`. A frame of more than 16 MiB closes the connection.

A value is encoded by its WIT type:

| WIT type | Bytes |
|---|---|
| `bool` | One byte, 0 or 1. |
| `u8` … `u64` | LEB128: seven bits a byte, low bits first, the top bit set on every byte but the last. |
| `s8` … `s64` | Zigzag (`(n << 1) ^ (n >> 63)`), then LEB128. |
| `f32`, `f64` | IEEE 754, little-endian. |
| `char` | Its scalar value, as a `u32`. |
| `string` | Its length in bytes, as a `u32`, then its UTF-8. |
| `list<T>` | Its length, as a `u32`, then each item. |
| `option<T>` | 0 for none; 1, then the value. |
| `tuple`, `record` | Each element or field, in order. |
| `variant`, `enum` | The case's index in the order the WIT declares the cases, as a `u32`, then the case's payload if it has one. |

Lengths and case indices are LEB128 like any `u32`.

For example, this answer:

```text
answer { serial: 300, handled: true, commands: [unplace(5), quit], motion: none, decision: none }
```

is these ten bytes, and the frame is `0a 00 00 00` and then them:

| Bytes | What |
|---|---|
| `03` | `message`'s fourth case, `answer` |
| `ac 02` | `serial`, 300 |
| `01` | `handled`, true |
| `02` | two commands |
| `0d 05` | `command`'s fourteenth case, `unplace`, and its window, 5 |
| `34` | `command`'s fifty-third case, `quit` |
| `00` | no `motion` |
| `00` | no `decision` |

Because a case is its position, the protocol's version says which positions
are meant: build a window manager against the WIT of the version the
compositor reports in `hello`.

## A window manager in Rust

The crate `solium-wm-protocol` (`crates/wm-protocol`) is the WIT as Rust
types, with their encoding, generated from the same files. It depends on
nothing of the compositor's. Beside the types it has:

- `client::Client`: the conversation. `hello`, `ready`, `next`, `reply`,
  `submit`, `query`, `reloaded`;
- `mirror::Mirror`: a copy of the desktop, kept by applying each batch's
  changes.

[`crates/wm-protocol/examples/tiny_wm.rs`](../crates/wm-protocol/examples/tiny_wm.rs)
is a whole window manager in one file of about two hundred lines: it tiles
windows into equal columns and binds five keys. Its loop is the whole shape of
one:

```rust
client.hello()?;
client.ready(setup())?;            // declare the bindings, listen to three events
loop {
    match client.next()? {
        Heard::Batch(batch) => {
            mirror.apply_all(batch.changes);
            let (handled, commands) = handle(batch.events);
            client.reply(batch.serial, handled, commands)?;
        }
        Heard::Reload(_) => client.reloaded(/* what `setup()` said */)?,
        Heard::Goodbye(_) => return Ok(()),
    }
}
```

Build it and name it in `wm.toml`:

```sh
cargo build -p solium-wm-protocol --example tiny_wm
```

```toml
command = ["/path/to/solium/target/debug/examples/tiny_wm"]
```

## A window manager in another language

Nothing above needs Rust. A window manager in another language reads the
three WIT files, represents each type in its own terms, and encodes and
decodes them as [the wire format](#the-wire-format) says.

[`examples/bad-apple`](../examples/bad-apple/README.md) is one in C#. It
writes its C# types from the WIT with a script of about three hundred lines of
Python, so nothing the WIT says is written a second time by hand, and the
rest is about seven hundred lines of C#: it plays a black and white
film by telling the compositor where to draw a few dozen plain windows, thirty
times a second. Its README lists which parts of the protocol it uses and why.

A case of a variant is its position in the WIT, counted from 0. The generated
Rust types keep that order, and `solium_wm_protocol::Event::CASES` and
`Command::CASES` list the names in it.

## Checking and debugging

- `solium --check` starts the window manager `wm.toml` names, lists the
  bindings it declared, and exits 1 if it would not start or the file has a
  mistake. It needs no display.
- To try a window manager without touching your session, run Solium nested
  with a configuration directory of its own:

  ```sh
  XDG_CONFIG_HOME=/tmp/try ./target/debug/solium
  ```

  with `/tmp/try/solium/wm.toml` naming it.
- Every session's log ends with one line on how the window manager did: how
  many batches it answered, the time half of the answers and 99 in 100 came
  within, the slowest, how many came after the compositor had gone on, and
  which events there were most of. A window manager that handles a batch in
  tens of microseconds costs a frame nothing.
- `RUST_LOG=info,solium::wm=debug` logs each answer that came late and how
  long it took.
