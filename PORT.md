# Porting the Lilium shell — kept off `main`

This branch and worktree exist because the shell port does not belong in the
compositor's history until it works. It changes how QML is hosted, what a
shell surface is, and what `work_area` reserves — and each of those can break
tiling, scrolling and workspaces, which are finished and in use.

    ~/personal_projects/solium              main         the compositor
    ~/personal_projects/solium-shell-port   shell-port   this

One repository, two working trees, so nothing here is ever a checkout away
from the compositor's tree.

## Where it got to

The shell's own `Dock.qml` resolves every type, instantiates with real
`screenInfo`, receives real pointer input, and reads Solium's real window
list. It renders a surface in the right place and paints almost nothing on it.

Three data sources short of pixels:

* `Hyprland.monitorFor(screenInfo)` answers with nothing, so the dock's
  geometry is undefined and it stays collapsed. Solium has one output with a
  name, a position and a mode, which is all the shell asks for.
* The apps list and icon services answer with nothing, so there is nothing to
  draw even once it has a size.
* The placeholder dock — `shell.rs`, `qml/dock.qml` — was mine, not the
  shell's, and was never asked for. It is scaffolding and should go once the
  real dock draws.

## How to check a file without running a compositor

    ./target/debug/solium --check-qml <file.qml>        # 70ms
    dev/stage-shell.sh ~/personal_projects/lilium-shell build/staged

Read Qt's errors from the **bottom**. It prints one line per file in a module
that failed to build, and only the last is a cause; the rest are siblings
listed alphabetically. Reading from the top invents dependency chains that do
not exist.
