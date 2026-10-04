# dev/pacing/tty-lib.sh: what every script run on a free VT shares. Sourced,
# never run: dev/pacing-tty.sh and the spike scripts set `pids` (a file of
# the PIDs they started) and then call these.
#
# Only what a script started is ever stopped, by the PIDs it wrote down:
# another session may be running on another VT.

# Refuse anything that is not a logged-in free VT.
tty_refuse() {
    [[ -z "${WAYLAND_DISPLAY:-}" ]] || { echo "refusing: run this on a free VT, not inside a session" >&2; exit 1; }
    [[ "$(tty)" == /dev/tty[0-9]* ]] || { echo "refusing: this is not a virtual terminal" >&2; exit 1; }
    [[ -n "${XDG_RUNTIME_DIR:-}" ]] || { echo "refusing: XDG_RUNTIME_DIR is not set (log in on the VT first)" >&2; exit 1; }
}

# Every PID in $pids: TERM, then, after up to ten seconds, KILL. Never by name.
tty_stop() {
    [[ -n "${pids:-}" && -f "$pids" ]] || return 0
    while read -r pid; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null; done <"$pids"
    for _ in $(seq 1 50); do
        local alive=0
        while read -r pid; do [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null && alive=1; done <"$pids"
        [[ "$alive" = 0 ]] && break
        sleep 0.2
    done
    while read -r pid; do [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null; done <"$pids"
    : >"$pids"
}

# The machine as a run finds it, appended to $1: the kernel, the load, and
# how many other compositors and builds are running (read only), then the
# driver and its clocks.
tty_meta() {
    {
        echo "kernel=$(uname -r) other_solium=$(pgrep -c -x solium) cargo=$(pgrep -c -x cargo) rustc=$(pgrep -c -x rustc)"
        echo "loadavg=$(cat /proc/loadavg)"
        nvidia-smi --query-gpu=driver_version,pstate,clocks.gr,clocks.mem --format=csv,noheader 2>/dev/null || echo "nvidia-smi: none"
    } >>"$1"
}

# The player (Ruling 6) into `player`, an array, and `playing`, its name for
# meta.txt: mpv on PACING_CLIP when both exist, ffplay's test pattern if not.
tty_player() {
    if command -v mpv >/dev/null && [[ -n "${PACING_CLIP:-}" ]]; then
        player=(mpv --no-config --loop-file=inf --no-audio --hwdec=no --vo=gpu "$PACING_CLIP")
        playing="mpv $(basename "$PACING_CLIP")"
    elif command -v ffplay >/dev/null; then
        player=(env SDL_VIDEODRIVER=wayland ffplay -loglevel error -an -loop 0 -f lavfi -i testsrc2=size=1280x720:rate=60)
        playing="ffplay testsrc2 1280x720@60"
    else
        echo "needs mpv with PACING_CLIP, or ffplay" >&2
        return 1
    fi
}

# Start one compositor on this VT: $1 the run's directory, $2 its cap in
# seconds, $3 the binary, the rest its environment. Its configuration and
# state directories are the run's own, so nothing of yours is read or
# written. With TTY_STDERR_CAP=<bytes> its log keeps only that much (a
# trace-level log at 260 Hz is many megabytes a second). Sets `solium` (the
# PID, also written to $pids) and `socket`; returns 1 if it exited or never
# said its socket.
tty_start() {
    local dir="$1" cap="$2" binary="$3"
    shift 3
    mkdir -p "$dir/config" "$dir/state"
    if [[ -n "${TTY_STDERR_CAP:-}" ]]; then
        env "$@" XDG_CONFIG_HOME="$dir/config" XDG_STATE_HOME="$dir/state" \
            timeout -k 10 -s TERM "$cap" "$binary" --tty > >(head -c "$TTY_STDERR_CAP" >"$dir/stderr.log") 2>&1 &
    else
        env "$@" XDG_CONFIG_HOME="$dir/config" XDG_STATE_HOME="$dir/state" \
            timeout -k 10 -s TERM "$cap" "$binary" --tty >"$dir/stderr.log" 2>&1 &
    fi
    solium=$!
    echo "$solium" >>"$pids"
    socket=""
    for _ in $(seq 1 300); do
        kill -0 "$solium" 2>/dev/null || { echo "solium exited early; see $dir/stderr.log" >&2; return 1; }
        socket="$(grep -oE 'socket=wayland-[0-9]+' "$dir/stderr.log" 2>/dev/null | tail -1 | cut -d= -f2)"
        [[ -n "$socket" ]] && return 0
        sleep 0.1
    done
    echo "solium never said its socket; see $dir/stderr.log" >&2
    return 1
}
