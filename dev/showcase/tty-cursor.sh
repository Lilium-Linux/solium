#!/usr/bin/env bash
#
# The showcase's one capture made on the hardware: the default
# configuration's pointer with its glow, which is a QML MultiEffect and is
# drawn only on the GPU path, so a nested capture (record.py) leaves it out.
#
# Run it from a text console (Ctrl+Alt+F3), from the checkout, after
# `cargo build --release`:
#
#   dev/showcase/tty-cursor.sh
#
# It starts Solium on this console with the shipped default configuration in
# a scratch directory and two windows in it, and records the screen for
# twenty seconds with wf-recorder through Solium's own screencopy. Move the
# pointer while it records: over the terminal's text, a link in the browser,
# a window's edges and corners, and quickly across the screen for the glow.
# Then Solium is ended, and the video is target/showcase/tty-cursor.mp4.
#
# wf-recorder comes from a small image built on the build image the first
# time (`podman build`, a minute), so nothing is installed on this machine.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
binary="$root/target/release/solium"
seconds="${SECONDS_TO_RECORD:-20}"
out="$root/target/showcase"

if [[ -n "${WAYLAND_DISPLAY:-}" || -n "${DISPLAY:-}" ]]; then
    echo "run this from a text console, not inside a desktop session" >&2
    exit 1
fi
[[ -x "$binary" ]] || { echo "not built: $binary (cargo build --release)" >&2; exit 1; }

image=localhost/solium-showcase-recorder:fc44
if ! podman image exists "$image"; then
    echo "building $image (wf-recorder on the build image), once"
    printf 'FROM localhost/solium-build:fc44\nRUN dnf install -y --setopt=install_weak_deps=False wf-recorder && dnf clean all\n' \
        | podman build -t "$image" -f - "$out" >/dev/null
fi

work="$(mktemp -d -t solium-tty-cursor-XXXXXX)"
mkdir -p "$work/home/.config" "$work/state" "$out"
cp -a "$root/crates/solium/default-config" "$work/home/.config/solium"
cp -a "$root/dev/showcase/stage/home/." "$work/home/"
log="$work/solium.log"

echo "starting Solium on this console; it records for ${seconds}s once two windows are up"
HOME="$work/home" XDG_CONFIG_HOME="$work/home/.config" XDG_STATE_HOME="$work/state" \
    "$binary" --tty >"$log" 2>&1 &
solium=$!
trap 'kill "$solium" 2>/dev/null || true' EXIT

socket=""
for _ in $(seq 1 200); do
    kill -0 "$solium" 2>/dev/null || { echo "Solium ended early; see $log" >&2; exit 1; }
    socket="$(grep -oE 'socket=wayland-[0-9]+' "$log" | tail -1 | cut -d= -f2)" && [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "Solium never reported a socket; see $log" >&2; exit 1; }

run() { WAYLAND_DISPLAY="$socket" HOME="$work/home" XDG_CONFIG_HOME="$work/home/.config" setsid "$@" >/dev/null 2>&1 & }
run kitty --title "git log" sh -c "git -C '$root' log --graph --color=always --oneline -60; exec sleep 3600"
mkdir -p "$work/home/firefox" && cp "$root/dev/showcase/stage/firefox/user.js" "$work/home/firefox/"
run firefox --no-remote --profile "$work/home/firefox" --new-window https://lilium-linux.github.io/solium/
sleep 4

echo "recording ${seconds}s: move the pointer now"
podman run --rm --userns=keep-id --security-opt label=disable \
    -v "$XDG_RUNTIME_DIR:$XDG_RUNTIME_DIR" -v "$out:$out" \
    -e "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" -e "WAYLAND_DISPLAY=$socket" \
    "$image" timeout -s INT "$seconds" wf-recorder -y -c libx264 -p crf=16 \
    -f "$out/tty-cursor.mp4" || true

kill "$solium" 2>/dev/null || true
wait "$solium" 2>/dev/null || true
echo "done: $out/tty-cursor.mp4"
