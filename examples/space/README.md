# Space, a desktop in three dimensions

A window manager for Solium, written in C#, with three desktops in three
dimensions over the same windows:

- **The cube** (`super+1`). Four desks on the faces of a cube.
  `super+left` and `super+right` turn it to the next desk; the cube backs
  away as it turns and comes back. `super+shift+left` and `super+shift+right`
  take the focused window to the next desk and turn after it.
- **The carousel** (`super+2`). Every window a card on a ring standing on its
  edge. The card in front is flat and has the keyboard; `super+left` and
  `super+right` turn the ring, and a window that opens is turned to the front.
- **Flying** (`super+3`). The windows stand about in rows going away from you,
  and you fly among them: `w` `a` `s` `d` (or the up and down arrows), `r` and
  `f` to rise and sink, left and right to turn. `return` docks in the window
  most nearly ahead: the camera comes to where that window is drawn flat and at
  its own size, and it takes the keyboard. `super+escape` takes off again.

`super+return` opens a terminal (`$TERMINAL`, or foot), `super+q` closes the
focused window and `super+shift+e` ends the session.

## How it works

This program draws nothing and keeps no time. It says where each window lives,
on the screen, and how groups of windows are posed in three dimensions: each
desk is a group turned a quarter more than the last about the cube's middle,
with the cube as their parent, turning them all and seen in perspective; each
card is a group turned a share of the ring; while flying, each window's group
puts it in its spot, a `move` group takes the room the other way from the
camera, and an `eye` group turns the room about your eye. The compositor
turns the poses into pixels on every frame, with springs that keep their speed
when a key changes the target, orders what it draws by how near it comes, and
hides a window turned away, which is how only the front face of the cube is
drawn, and clicked, at rest. A key held down while flying retargets the camera
at the key's repeat rate, and the compositor glides between.

It links nothing of Solium's: its types are the ones protobuf's generator for
C# writes from the protocol's `.proto` file, through the package in
[`sdk/csharp`](../../sdk/csharp/README.md), and [Writing a window
manager](../../docs/window-managers.md), "Groups in three dimensions", says
what a pose is.

## Build it, and run it

```sh
cd examples/space
dotnet build -c Release -o bin
```

and name it in `~/.config/solium/wm.toml`:

```toml
command = ["dotnet", "/path/to/examples/space/bin/space-wm.dll"]
```

The backdrop, a field of stars behind everything, is `qml/Backdrop.qml`, built
into `bin/qml/`.

## What it does not do yet

- Clicks go to where a window lives, not where a pose draws it: a window is
  clicked as it is drawn only once it is at rest, flat, which the front face,
  the front card and a docked window are. While flying, the pointer is held
  by the window manager.
- A scene, such as the backdrop, moves and fades with a group but does not
  turn with it, so a face of the cube is its windows and no wallpaper.
