#!/usr/bin/env bash
# Render-node-only protocol checks. Never opens a primary node or takes a VT.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
node="${SOLIUM_TEST_RENDER_NODE:-}"
if [[ -z "$node" ]]; then
    for candidate in /dev/dri/renderD*; do
        [[ -e "$candidate" ]] && { node="$candidate"; break; }
    done
fi
[[ "${node##*/}" == renderD* && -c "$node" ]] || {
    echo 'needs a DRM render node; set SOLIUM_TEST_RENDER_NODE' >&2; exit 1;
}
cd "$root"
test_binary="${SOLIUM_TEST_BINARY:-}"
[[ -n "$test_binary" && -x "$test_binary" ]] || {
    echo 'set SOLIUM_TEST_BINARY to the freshly built solium test executable (cargo test -p solium --no-run)' >&2
    exit 2
}
SOLIUM_TEST_RENDER_NODE="$node" timeout -k 5 -s TERM 90 "$test_binary" explicit_sync --include-ignored --test-threads=1
