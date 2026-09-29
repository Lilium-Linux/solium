#!/usr/bin/env bash
#
# fmt, clippy, tests, build, the Lua check and the QML GPU check. Exits
# non-zero if any of them complains.
#
# It exists because a clippy failure went into a commit three times in a row,
# each time because the test line was read and the one above it was not. A
# gate whose output has to be read is not a gate.
#
# Runs cargo on this machine by default, which needs Rust and the development
# packages listed in the README's "Building" section. Everything else is
# opt-in, through the environment:
#
#   SOLIUM_GATE_IMAGE=<image>      build in this podman image instead, e.g.
#                                  localhost/solium-build:fc44 from
#                                  dev/Containerfile. Only the checkout,
#                                  CARGO_HOME and RUSTUP_HOME are mounted.
#   SOLIUM_GATE_PODMAN_ARGS=<args> extra arguments for `podman run`, split on
#                                  spaces, e.g. "--memory=6g --memory-swap=6g".
#   SOLIUM_GATE_JOBS=<n>           cargo -j<n>. Unset, cargo uses every CPU.
#   SOLIUM_GATE_CPUS=<list>        pin the build to these CPUs (a taskset
#                                  list, e.g. 14,15).
#   SOLIUM_GATE_NO_GPU=1           skip running the QML GPU check.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${SOLIUM_GATE_IMAGE:-}"

# Politeness first, so a gate running in the background does not make the
# machine it runs on unusable. The pin comes last so it applies to cargo.
polite="nice -n 19"
if [[ -n "$image" ]] || command -v ionice >/dev/null 2>&1; then
    polite="$polite ionice -c 3"
fi
if [[ -n "${SOLIUM_GATE_CPUS:-}" ]]; then
    polite="$polite taskset -c ${SOLIUM_GATE_CPUS}"
fi
jobs=""
if [[ -n "${SOLIUM_GATE_JOBS:-}" ]]; then
    jobs="-j${SOLIUM_GATE_JOBS}"
fi

if [[ -n "$image" ]]; then
    if ! podman image exists "$image"; then
        echo "GATE FAILED: no image $image. Build it with:" >&2
        echo "  podman build -t ${image#localhost/} -f $root/dev/Containerfile $root/dev/" >&2
        exit 1
    fi
    # Rust comes from the user's rustup install: the proxies in CARGO_HOME/bin
    # and the toolchains under RUSTUP_HOME. The image supplies only the C
    # toolchain and the system libraries.
    cargo_home="${CARGO_HOME:-$HOME/.cargo}"
    rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
    mounts=(-v "$root:$root" -v "$cargo_home:$cargo_home")
    if [[ -d "$rustup_home" ]]; then
        mounts+=(-v "$rustup_home:$rustup_home")
    fi
    # Word-split on purpose: this is a list of arguments.
    # shellcheck disable=SC2206
    extra=(${SOLIUM_GATE_PODMAN_ARGS:-})
fi

# One step, in the container or here. `$1` is a shell command line.
run() {
    if [[ -n "$image" ]]; then
        # keep-id so what is built belongs to whoever ran the gate; label=disable
        # because relabelling CARGO_HOME and the checkout for SELinux would
        # change them for everything else that uses them. HOME is the
        # container's own /tmp, so what the tests' QML caches, and any
        # configuration they might look for, stays inside the container
        # instead of landing in the checkout (podman's HOME for an unmounted
        # home directory is the working directory).
        podman run --rm --userns=keep-id --security-opt label=disable \
            "${extra[@]}" "${mounts[@]}" \
            -e HOME=/tmp \
            -e CARGO_HOME="$cargo_home" -e RUSTUP_HOME="$rustup_home" \
            -e PATH="$cargo_home/bin:/usr/local/bin:/usr/bin:/bin" \
            -w "$root" "$image" \
            sh -c "$1"
    else
        (cd "$root" && sh -c "$1")
    fi
}

echo "fmt..."
run "$polite cargo fmt --all"
echo "clippy..."
run "$polite cargo clippy --all-targets $jobs -- -D warnings" \
    || { echo "GATE FAILED: clippy" >&2; exit 1; }
echo "tests..."
run "$polite cargo test $jobs" \
    || { echo "GATE FAILED: tests" >&2; exit 1; }
echo "build..."
run "$polite cargo build $jobs" \
    || { echo "GATE FAILED: build" >&2; exit 1; }

# The configuration is Lua, and nothing above this line reads Lua. A syntax
# error in config.lua compiles, tests and clippies perfectly cleanly, and then
# the compositor starts with no scripts at all -- no layouts, no bindings, no
# decorations. That shipped past a green gate once; it is one line to stop.
# Run here rather than in the container, because it is the built binary rather
# than the build.
echo "scripts..."
"$root/target/debug/solium" --check >/dev/null \
    || { echo "GATE FAILED: scripts ($root/target/debug/solium --check)" >&2; exit 1; }

# The QML GPU path, against a real GLES renderer and the real host.cpp.
#
# Nothing above this line touches it: `cargo test` cannot, because it needs a
# GPU and a Qt installation at run time, and a build container has neither a
# render node nor a display. So it is built like everything else and *run*
# here, the same split as the script check above.
#
# It earns a place in the gate because every defect this path has produced was
# silent -- a frame drawn into the wrong context, a teardown deleting the
# compositor's GL objects, a buffer stored upside down -- and each of them
# returned success from every call involved. There is nothing to notice by
# looking. See dev/wirecheck/README.md.
echo "wirecheck..."
# A source tarball leaves the harness out; the checks above are the whole gate
# there.
if [[ ! -d "$root/dev/wirecheck" ]]; then
    echo "  skipped: dev/wirecheck is not in this tree"
    echo "gate passed"
    exit 0
fi
run "cd dev/wirecheck && $polite cargo build $jobs" \
    || { echo "GATE FAILED: wirecheck did not build" >&2; exit 1; }
# Whichever render node this machine has, rather than renderD128 by name: a
# machine that enumerates differently would otherwise skip this silently
# forever.
node="$(ls -1 /dev/dri/renderD* 2>/dev/null | head -1 || true)"
log="$root/dev/wirecheck/target/wirecheck.log"
if [[ -z "$node" ]]; then
    echo "  skipped: no render node on this machine"
elif [[ -n "${SOLIUM_GATE_NO_GPU:-}" ]]; then
    echo "  skipped: SOLIUM_GATE_NO_GPU is set"
else
    "$root/dev/wirecheck/target/debug/wirecheck" "$node" >"$log" 2>&1 \
        || { echo "GATE FAILED: wirecheck on $node (see $log)" >&2
             tail -25 "$log" >&2; exit 1; }
fi

echo "gate passed"
