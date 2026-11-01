# Desktop homescreen

This directory is part of the personal Solium configuration, not another shell
process. `user.lua` loads `desktop.lua`; the stock preview desktop is disabled.
The existing wallpaper, taskbar, recents, workspace stack and window modes remain
the desktop's surrounding UI.

## Using it

- Real files from the XDG desktop directory supply the file and app icons.
- Click to select, double-click to open. Desktop Settings can select single-click.
- Drag an icon to move it. Right-click empty space for Edit Desktop, Add Widget,
  Add Application, Arrange Items, Open Desktop Folder and Desktop Settings.
- In edit mode, widget headers are drag handles; their bottom controls select
  supported sizes. The × removes that instance; Undo restores it.
- Grid: On snaps into cells and shifts occupied neighbors. Grid: Off permits
  free placement and overlap. Each mode remembers its own coordinates.
- Hide Icon removes a file's icon from this canvas without changing the file.
  Desktop Settings restores hidden icons. The normal file manager handles file
  deletion, renaming and clipboard operations.
- Add Application copies the selected installed launcher into Desktop, using a
  collision-safe filename. Existing shortcuts and applications are preserved.
- Keyboard: arrows navigate, Enter opens, Delete hides selected icons/removes
  widgets, Escape cancels a drag or exits the current panel/edit mode.

Widgets are Clock & Date, Calendar, Now Playing and System. A new layout starts
with one clock. Calendar is a month view; it has no calendar-account integration.
Now Playing reads Solium's media service. System reads actual procfs counters.
The clock's ⋯ menu switches between 12- and 24-hour time. Widget menus also
move an instance to a different output.

Appearance settings include grid size, margins, icon size, labels, text scale,
light/dark appearance, translucency, reduced motion and animation timing.
Saved UI settings take precedence over the defaults in `Layout.js`; Reset
Appearance and Spacing returns to those defaults.

## State and outputs

State lives at `${XDG_STATE_HOME:-~/.local/state}/solium/desktop/layout.json`.
It is data-only JSON, schema version 1, written on interaction commits. The
previous valid version is `.bak`; a malformed source is preserved as `.invalid`.
A failed save leaves the layout in memory and shows Retry. `sol.keep` also
retains state across configuration reloads.

Each entry has a connector name and independent grid/free positions. The canvas
is shared across workspaces. An absent output's entries are displayed temporarily
on the primary output; their saved output placement remains intact.

QML sends a base revision with every mutation. Lua rejects a stale revision and
republishes current state instead of overwriting another output's newer edit.
Drag previews are temporary copies; dropping commits once and Escape discards
the complete preview, including displaced neighbors.

## Opening applications

The desktop records the clicked icon's real scene rectangle and connector.
`desktop/launch.lua` serializes command issue/acknowledgment and associates each
origin with the stable loading-pane ID. Several flights may run concurrently.
`open.lua` delegates those launches and preserves normal opening for other
windows. Normal window placement and minimize-to-taskbar continue to own their
existing geometry and behavior.

The rounded-violet pane contains the traveling icon. Windows using client-side
decorations (`style=none`) have no such pane layer; they grow/fade from the
origin without that icon-art layer. Existing-window launch activations use the
departing loading pane only when the application's identity matches; ambiguous
or invalid origins retain the ordinary opening behavior. These are public-API
limits, not a reason to substitute the mobile fullscreen policy.

## Verification

Run `bash tools/check-desktop.sh` from this configuration. It uses Node and the
installed Lua 5.4 shared library for policy/layout/state tests, then Solium's
offscreen `--check` to construct five panel states, all four widgets, icons,
pane layers and effects in an isolated XDG copy. It never starts the live shell.

For real nested testing, run `nohup bash tools/test-desktop-nested.sh >
/tmp/desktop-nested-launch.log 2>&1 &`, then read the log and the run directory
it reports. The script takes the shared nested lock and busy guard, isolates
config/state/runtime, scripts edits and monitor hotplug, captures real frames,
and stops the compositor with a 70-second timeout. It exits 75 when held; do not
retry automatically. Inspect every capture before accepting the visual result.

Offscreen checks do not validate drag input, animation quality, shader GPU
compilation, multi-output behavior or 60-second stability. See `PROGRESS.md`
for the current verification status. The live configuration must not be replaced
until the nested checks have passed.
