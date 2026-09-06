#!/usr/bin/env bash
#
# Run Solium with a piece of the Lilium shell in it, reloading as you edit.
#
#   dev/run-shell.sh                          # the shell's own dock
#   dev/run-shell.sh path/to/Thing.qml        # any one file
#
# The staged tree is rebuilt each run, so the shell's repository is only ever
# read. Editing anything under it reloads the scene within half a second —
# no restart, no rebuild.
set -uo pipefail

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    echo "an empty value resolves to the default socket, not to 'no display'." >&2
    exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
shell="${SOLIUM_SHELL:-$HOME/personal_projects/lilium-shell}"
staged="$root/build/staged"

"$root/dev/stage-shell.sh" "$shell" "$staged" >/dev/null || exit 1

scene="${1:-$staged/qs/dock/Dock.qml}"
[[ -f "$scene" ]] || { echo "no such scene: $scene" >&2; exit 1; }

echo "shell:  $shell"
echo "scene:  ${scene#$staged/}"
echo "edit anything under the shell and it reloads within half a second"
echo

# The staged tree is what the engine reads, so restage on every change too.
( while sleep 1; do "$root/dev/stage-shell.sh" "$shell" "$staged" >/dev/null 2>&1; done ) &
restager=$!
trap 'kill "$restager" 2>/dev/null' EXIT

SOLIUM_QML_PATH="$root/crates/solium/qml:$root/crates/solium/qml/compat:$staged" \
SOLIUM_SHELL_SCENE="$scene" \
SOLIUM_SHELL_WATCH="$shell" \
SOLIUM_SHELL_DIR="$shell" \
    exec "$root/dev/run-nested.sh"
