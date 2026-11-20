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
- **The room** (`super+3`). A room at night, its walls, floor and ceiling
  QML scenes, with the windows hung on its walls under picture lights, two to
  a wall, and you walk it as in a game: `w` `a` `s` `d` to walk, left and
  right to turn, up and down to look up and down, `space` to jump. `return`
  steps up to the window most nearly ahead: you come to where it is drawn flat
  and at its own size, and it takes the keyboard. `super+escape` steps back.

`super+return` opens a terminal (`$TERMINAL`, or foot), `super+q` closes the
focused window and `super+shift+e` ends the session.

## How it works

This program draws nothing and keeps no time. It says where each window lives,
on the screen, and how groups of windows and scenes are posed in three
dimensions: each desk is a group turned a quarter more than the last about the
cube's middle, with the cube as their parent, turning them all and seen in
perspective; each card is a group turned a share of the ring. In the room,
each wall, the floor and the ceiling is a scene declared as large as fits on
the screen and posed to its place at the size it stands, and each window's
group hangs it on its wall; four groups, one inside the next, carry it all:
where you stand (`move`), how high a jump has lifted you (`hop`), which way
you face (`turn`) and how far you look up (`eye`), the last seeing it in
perspective. A jump is two motions on `hop`, up as a thrown thing slows and
down as it falls, both cubic beziers, then a spring for the knees.

The compositor turns the poses into pixels on every frame, with springs that
keep their speed when a key changes the target, orders what it draws by how
near it comes, cuts away what reaches behind you, so the floor is drawn up to
your feet, and hides what faces away, which is how only the front face of the
cube is drawn, and clicked, at rest. A posed scene is drawn into a texture at
the size its pose gives it and bent like a window, so the walls are as sharp
as the windows on them. A key held down in the room retargets the camera at
the key's repeat rate, and the compositor glides between.

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

The backdrop, a field of stars behind everything, is `qml/Backdrop.qml`, and
the room is `qml/Room/`: `Wall.qml` (plaster, wainscot, a picture light over
each place a window hangs, and a window onto a city, a door, a bookcase or a
print between), `Floor.qml` and `Ceiling.qml`, all measured in metres so they
read right at any size. They are built into `bin/qml/`.

## What it does not do yet

- Clicks go to where a window lives, not where a pose draws it: a window is
  clicked as it is drawn only once it is at rest, flat, which the front face,
  the front card and a window stepped up to are. While walking, the pointer is
  held by the window manager.
- Looking around is the keys, not the mouse: the protocol tells a window
  manager about presses, not the pointer's motion.
- The room's scenes are drawn behind the windows, which is right from
  anywhere inside a room with nothing standing in it. Anything standing in
  the room would need its own order against the windows.
