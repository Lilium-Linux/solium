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
        -v /home/kotoxik:/home/kotoxik \
        -e CARGO_HOME=/home/kotoxik/.cargo -e CARGO_BUILD_JOBS=2 \
        -e PATH=/home/kotoxik/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
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
echo "gate passed"
