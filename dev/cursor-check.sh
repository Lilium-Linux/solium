#!/usr/bin/env bash
#
# Is the pointer visible?
#
# This exists because it was not, for the whole life of the project, and
# nothing noticed. The cursor is a fixed-size QML scene, and a scene that is
# never resized was never given a render target -- so Qt drew into nothing and
# the buffer stayed exactly as transparent as it was filled. Every other scene
# is resized to its area each frame and so worked by accident.
#
# It could not be seen while developing because a nested compositor sits inside
# a session that draws its own cursor over the top. On the hardware there is
# nothing else to draw it, and an invisible pointer is indistinguishable from
# input being dead.
#
# So: move the pointer somewhere with no window under it, take the frame, and
# count what was drawn there.
set -uo pipefail

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
[[ -x "$binary" ]] || { echo "not built: $binary" >&2; exit 1; }

out="${SOLIUM_CHECK_DIR:-$(mktemp -d -t solium-cursor-XXXXXX)}"
mkdir -p "$out"
shot="$out/cursor.ppm"
log="$out/cursor.log"

# Over empty desktop, deliberately: a client sets its own cursor while the
# pointer is over its surface, so testing there would pass on the client's
# cursor and prove nothing about ours.
SOLIUM_DRAG_AT="2000:900,500>900,500" \
SOLIUM_CAPTURE="$shot" SOLIUM_CAPTURE_AT=3000 \
    "$binary" >"$log" 2>&1 &
solium=$!
trap 'kill "$solium" 2>/dev/null; wait "$solium" 2>/dev/null' EXIT
for _ in $(seq 1 80); do
    [[ -s "$shot" ]] && break
    kill -0 "$solium" 2>/dev/null || break
    sleep 0.1
done
[[ -s "$shot" ]] || { echo "cursor: FAIL — no frame was captured"; tail -5 "$log"; exit 1; }

env -u LD_LIBRARY_PATH python3 - "$shot" <<'PY'
import pathlib, sys
raw = pathlib.Path(sys.argv[1]).read_bytes(); parts, i = [], 0
while len(parts) < 4:
    while raw[i:i+1].isspace(): i += 1
    st = i
    while not raw[i:i+1].isspace(): i += 1
    parts.append(raw[st:i])
i += 1
w, h, px = int(parts[1]), int(parts[2]), raw[i:]
at = lambda x, y: tuple(px[((y*w+x)*3):((y*w+x)*3)+3])
# Far from the pointer, and not the top-left, which is where a pointer that
# was never moved would be sitting.
bg = at(w - 4, h - 4)
drawn = sum(1 for y in range(500, 526) for x in range(900, 926) if at(x, y) != bg)
verdict = "PASS" if drawn >= 40 else "FAIL"
print(f"cursor                 {verdict}  pixels drawn at the pointer: {drawn} (want >= 40)")
sys.exit(0 if verdict == "PASS" else 1)
PY
