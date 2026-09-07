#!/usr/bin/env bash
#
# Copy and paste across the X11 boundary, all four ways.
#
#   clipboard  wayland -> x11        clipboard  x11 -> wayland
#   primary    wayland -> x11        primary    x11 -> wayland
#
# Run it more than once. The bug this was written for failed about one time in
# three: `X11Wm::new_selection` does not flush, so whether the X server had
# heard that the compositor owned the selection depended on whether anything
# else happened to be talking to X. A single green run proved nothing.
#
# The X11 half runs in a container because the host has no xclip and cannot
# install one. Built on first use.
set -uo pipefail

[[ -n "${WAYLAND_DISPLAY:-}" ]] || { echo "refusing to start: WAYLAND_DISPLAY is empty." >&2; exit 1; }
command -v podman >/dev/null || { echo "clipboard: SKIP — no podman for the X11 half"; exit 0; }
command -v wl-copy >/dev/null || { echo "clipboard: SKIP — wl-clipboard is not installed"; exit 0; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
[[ -x "$binary" ]] || { echo "not built: $binary" >&2; exit 1; }

if ! podman image exists localhost/solium-xtest 2>/dev/null; then
    echo "clipboard: building the X11 test image (once)…"
    printf 'FROM fedora:44\nRUN dnf -y install xclip && dnf clean all\n' \
        | podman build -q -t localhost/solium-xtest -f - . >/dev/null 2>&1 \
        || { echo "clipboard: SKIP — could not build the X11 test image"; exit 0; }
fi

out="$(mktemp -d)"
RUST_LOG=solium=info "$binary" >"$out/comp.log" 2>&1 &
solium=$!
trap 'kill $solium 2>/dev/null; wait $solium 2>/dev/null; rm -rf "$out"' EXIT

for _ in $(seq 1 80); do
    sock="$(grep -oE 'socket=wayland-[0-9]+' "$out/comp.log" | tail -1 | cut -d= -f2)"
    disp="$(grep -oE 'XWayland is up display=[0-9]+' "$out/comp.log" | tail -1 | cut -d= -f2)"
    [[ -n "$sock" && -n "$disp" ]] && break
    kill -0 $solium 2>/dev/null || break
    sleep 0.2
done
[[ -n "${sock:-}" ]] || { echo "clipboard: FAIL — no wayland socket"; exit 1; }
[[ -n "${disp:-}" ]] || { echo "clipboard: FAIL — XWayland never came up, so there is no X11 side"; exit 1; }

X() { podman run --rm --userns=keep-id --security-opt label=disable \
        -v /tmp/.X11-unix:/tmp/.X11-unix -e DISPLAY=":$disp" \
        localhost/solium-xtest "$@" 2>/dev/null; }
W() { env -u DISPLAY WAYLAND_DISPLAY="$sock" "$@"; }
xhold() { podman run --rm -d --userns=keep-id --security-opt label=disable \
        -v /tmp/.X11-unix:/tmp/.X11-unix -e DISPLAY=":$disp" localhost/solium-xtest \
        sh -c "printf '%s' '$2' | xclip -selection $1 -i && sleep 25" >/dev/null 2>&1; }

status=0
check() {
    if [[ "$2" == "$3" ]]; then printf '  PASS  %s\n' "$1"
    else printf "  FAIL  %s (wanted '%s', got '%s')\n" "$1" "$3" "$2"; status=1; fi
}

token="$$"
W wl-copy "wl-clip-$token"; sleep 1
check "clipboard  wayland -> x11" "$(X xclip -o -selection clipboard)" "wl-clip-$token"
W wl-copy --primary "wl-prim-$token"; sleep 1
check "primary    wayland -> x11" "$(X xclip -o -selection primary)" "wl-prim-$token"

xhold clipboard "x-clip-$token"; sleep 3
check "clipboard  x11 -> wayland" "$(W timeout 8 wl-paste)" "x-clip-$token"
xhold primary "x-prim-$token"; sleep 3
check "primary    x11 -> wayland" "$(W timeout 8 wl-paste --primary)" "x-prim-$token"

exit $status
