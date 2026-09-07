#!/usr/bin/env bash
#
# fmt, clippy, tests. Exits non-zero if any of them complains.
#
# This exists because I committed over a clippy failure three times in one
# session, each time by reading the test line and not the one above it. A gate
# whose output has to be read is not a gate.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run() {
    podman run --rm --userns=keep-id --security-opt label=disable \
        -v "$HOME:$HOME" \
        -e CARGO_HOME="$HOME/.cargo" -e CARGO_BUILD_JOBS=2 \
        -e PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin" \
        -w "$root" localhost/solium-build:fc44 \
        sh -c "$1"
}

echo "fmt..."
run 'nice -n 19 cargo fmt --all'
echo "clippy..."
run 'nice -n 19 ionice -c 3 taskset -c 14,15 cargo clippy --all-targets -j2 -- -D warnings' \
    || { echo "GATE FAILED: clippy" >&2; exit 1; }
echo "tests..."
run 'nice -n 19 ionice -c 3 taskset -c 14,15 cargo test -j2' \
    || { echo "GATE FAILED: tests" >&2; exit 1; }
echo "build..."
run 'nice -n 19 ionice -c 3 taskset -c 14,15 cargo build -j2' \
    || { echo "GATE FAILED: build" >&2; exit 1; }

# The configuration is Lua, and nothing above this line reads Lua. A syntax
# error in config.lua compiles, tests and clippies perfectly cleanly, and then
# the compositor starts with no scripts at all -- no layouts, no bindings, no
# decorations. That shipped past a green gate once; it is one line to stop.
# Run on the host, because it is the built binary rather than the build.
echo "scripts..."
"$root/target/debug/solium" --check >/dev/null \
    || { echo "GATE FAILED: scripts ($root/target/debug/solium --check)" >&2; exit 1; }

echo "gate passed"
