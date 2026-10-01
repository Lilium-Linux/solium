#!/usr/bin/env bash
#
# Run Solium nested with a shell hosted in it, reloading as you edit.
#
#   dev/run-shell.sh <shell-dir>                  # its shell.qml
#   dev/run-shell.sh <shell-dir> dock/Dock.qml    # one file of it
#   SOLIUM_SHELL=<shell-dir> dev/run-shell.sh [scene]
#
# <shell-dir> is the shell's own checkout, and it is required: this script
# knows no shell of its own. The checkout is only ever read. A scene is a
# file, or a path inside the checkout; the default is the checkout's own
# `shell.qml`. Editing anything under the checkout reloads the scene within
# half a second — no restart, no rebuild.
#
# The scene is handed over as SOLIUM_SHELL_SCENE, the per-run override of
# `shell.scene`, so the configuration you normally run is left alone.
set -uo pipefail

usage() {
    echo "usage: dev/run-shell.sh <shell-dir> [scene]" >&2
    echo "       SOLIUM_SHELL=<shell-dir> dev/run-shell.sh [scene]" >&2
}

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    echo "an empty value resolves to the default socket, not to 'no display'." >&2
    exit 1
fi

shell="${SOLIUM_SHELL:-}"
if [[ $# -gt 0 && -d "$1" ]]; then
    shell="$1"
    shift
fi
if [[ -z "$shell" ]]; then
    usage
    exit 2
fi
[[ -d "$shell" ]] || { echo "no such shell directory: $shell" >&2; exit 1; }
shell="$(cd "$shell" && pwd)"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

scene="${1:-$shell/shell.qml}"
[[ -f "$scene" ]] || scene="$shell/$scene"
[[ -f "$scene" ]] || { echo "no such scene: ${1:-$shell/shell.qml}" >&2; exit 1; }
scene="$(cd "$(dirname "$scene")" && pwd)/$(basename "$scene")"

echo "shell:  $shell"
echo "scene:  ${scene#"$shell"/}"
echo "edit anything under the shell and it reloads within half a second"
echo

SOLIUM_QML_PATH="$root/crates/solium/qml" \
SOLIUM_SHELL_SCENE="$scene" \
SOLIUM_SHELL_WATCH="$shell" \
    "$root/dev/run-nested.sh"
