#!/usr/bin/env bash
#
# Run one client inside a nested Solium and report whether it worked.
#
#   dev/app-check.sh konsole
#   dev/app-check.sh firefox --new-instance
#
# "Worked" is four things, and a compositor can fail any one of them while
# looking fine on the others:
#
#   * the client is still alive when we look (it did not abort on a missing
#     global, or on a protocol error we sent it),
#   * the compositor logged no protocol error against it,
#   * something was actually drawn (a client can live and map nothing),
#   * the compositor is still alive itself.
#
# Prints one PASS/FAIL line per client, and leaves the capture and both logs
# behind for anything that needs looking at.
set -uo pipefail

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "refusing to start: WAYLAND_DISPLAY is empty or unset." >&2
    exit 1
fi
[[ $# -gt 0 ]] || { echo "usage: $0 <program> [args...]" >&2; exit 2; }

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$root/target/debug/solium"
[[ -x "$binary" ]] || { echo "not built: $binary" >&2; exit 1; }

# Named after the last argument that looks like an app rather than the
# wrapper: `flatpak run ... com.slack.Slack --flag` is a Slack test.
name="$1"
for arg in "$@"; do
    [[ "$arg" == -* ]] || name="$arg"
done
name="$(basename "$name")"
out="${SOLIUM_CHECK_DIR:-$(mktemp -d -t solium-check-XXXXXX)}"
mkdir -p "$out"
comp_log="$out/$name.compositor.log"
app_log="$out/$name.client.log"
shot="$out/$name.ppm"
settle="${SOLIUM_CHECK_SETTLE:-7000}"

SOLIUM_CAPTURE="$shot" SOLIUM_CAPTURE_AT="$settle" "$binary" >"$comp_log" 2>&1 &
solium=$!
cleanup() {
    [[ -n "${client:-}" ]] && kill "$client" 2>/dev/null
    kill "$solium" 2>/dev/null
    wait "$solium" 2>/dev/null
}
trap cleanup EXIT

socket=""
for _ in $(seq 1 150); do
    kill -0 "$solium" 2>/dev/null || { echo "$name: FAIL — solium exited before the client started"; tail -3 "$comp_log"; exit 1; }
    socket="$(grep -oE 'socket=wayland-[0-9]+' "$comp_log" | tail -1 | cut -d= -f2)"
    [[ -n "$socket" ]] && break
    sleep 0.1
done
[[ -n "$socket" ]] || { echo "$name: FAIL — solium never reported a socket"; exit 1; }

# A sandboxed client gets no environment from us, so the socket has to go in
# its own argument list: write @SOCKET@ where it belongs and it is substituted.
#   dev/app-check.sh flatpak run --env=WAYLAND_DISPLAY=@SOCKET@ com.slack.Slack
args=()
for arg in "$@"; do args+=("${arg//@SOCKET@/$socket}"); done

env -u DISPLAY WAYLAND_DISPLAY="$socket" QT_QPA_PLATFORM=wayland GDK_BACKEND=wayland \
    "${args[@]}" >"$app_log" 2>&1 &
client=$!

deadline=$(( $(date +%s%3N) + settle + 3000 ))
while (( $(date +%s%3N) < deadline )); do
    kill -0 "$solium" 2>/dev/null || break
    sleep 0.2
done

alive_client=no; kill -0 "$client" 2>/dev/null && alive_client=yes
alive_comp=no;   kill -0 "$solium" 2>/dev/null && alive_comp=yes
# Protocol errors are the interesting ones: a client killed for sending
# something we rejected, or for asking for something we never advertised.
errors="$(grep -icE "protocol error|invalid |wl_display@1: error|no such global" "$comp_log")"
drawn=unknown
if [[ -s "$shot" ]]; then
    drawn="$(env -u LD_LIBRARY_PATH python3 - "$shot" <<'PY'
import pathlib, sys
raw = pathlib.Path(sys.argv[1]).read_bytes(); parts, idx = [], 0
while len(parts) < 4:
    while raw[idx:idx+1].isspace(): idx += 1
    st = idx
    while not raw[idx:idx+1].isspace(): idx += 1
    parts.append(raw[st:idx])
idx += 1
w, h, px = int(parts[1]), int(parts[2]), raw[idx:]
at = lambda x, y: px[((y*w+x)*3):((y*w+x)*3)+3]
bg = at(4, h-4)
lit = sum(1 for y in range(0, h, 4) for x in range(0, w, 4) if at(x, y) != bg)
print("yes" if lit > 400 else "no")
PY
)"
fi

verdict=PASS
[[ "$alive_client" == yes && "$alive_comp" == yes && "$errors" == 0 && "$drawn" == yes ]] || verdict=FAIL
printf '%-22s %s  client=%s compositor=%s protocol_errors=%s drawn=%s\n' \
    "$name" "$verdict" "$alive_client" "$alive_comp" "$errors" "$drawn"
echo "                       logs: $comp_log"
[[ "$verdict" == PASS ]]
