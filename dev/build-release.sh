#!/usr/bin/env bash
#
# Build the release binary that is installed or packaged:
# target/install/release/solium.
#
# In the build container, through the shared build lock, with its memory
# capped. dev/install.sh builds through this, and so does dev/rpm.sh, so a
# package holds the same binary an install from the checkout does.
# dev/install-check.sh plays the build with a stand-in podman, and asserts
# what install.sh does when a session starts while it runs.
#
#   dev/build-release.sh [--jobs N] [--image IMAGE]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    cat <<EOF
usage: dev/build-release.sh [--jobs N] [--image IMAGE]

  --jobs N            cargo jobs for the build (default: 2)
  --image IMAGE       build container (default: localhost/solium-build:fc44)
  -h, --help          this text

environment:
  SOLIUM_BUILD_LOCK     lock the build takes (default: \$XDG_CACHE_HOME/solium-build.lock)
  SOLIUM_BUILD_MEMORY   memory cap of the build container (default: 6g)
EOF
}

die() { echo "build-release.sh: $*" >&2; exit 1; }

jobs=2
image="localhost/solium-build:fc44"
lock="${SOLIUM_BUILD_LOCK:-${XDG_CACHE_HOME:-$HOME/.cache}/solium-build.lock}"
memory="${SOLIUM_BUILD_MEMORY:-6g}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --jobs) jobs="${2:?--jobs needs a number}"; shift 2 ;;
        --jobs=*) jobs="${1#*=}"; shift ;;
        --image) image="${2:?--image needs a name}"; shift 2 ;;
        --image=*) image="${1#*=}"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
done
[[ "$jobs" =~ ^[1-9][0-9]*$ ]] || die "--jobs must be a positive number: $jobs"

# Where the container sees this checkout. assets.rs looks in the build tree
# (`CARGO_MANIFEST_DIR`) *before* `<binary>/../share/solium`, so a binary
# compiled at the checkout's own path would go on reading the checkout's QML
# and Lua after it was installed. Compiled here, a path the host does not have,
# the installed binary falls through to the copy beside it. install.sh's check
# after installing asserts exactly that, dev/install-check.sh asserts it for a
# staged install, and dev/rpm.sh for the unpacked package.
mount="/solium-src"
built="$root/target/install/release/solium"

command -v podman >/dev/null || die "podman is not installed, and the build runs in a container"
podman image exists "$image" \
    || die "the build image $image does not exist. Build it once with:
  podman build -t ${image#localhost/} -f dev/Containerfile dev/"
mkdir -p "$(dirname "$lock")"
echo "building a release binary in $image (waiting for $lock if another build holds it)..."
status=0
flock "$lock" podman run --rm --memory="$memory" --memory-swap="$memory" \
    --userns=keep-id --security-opt label=disable \
    -v "$HOME:$HOME" -v "$root:$mount" \
    -e CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" -e CARGO_BUILD_JOBS="$jobs" \
    -e CARGO_TARGET_DIR="$mount/target/install" \
    -e PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin" \
    -w "$mount" "$image" \
    sh -c "nice -n 19 ionice -c 3 cargo build --release -j$jobs -p solium" \
    || status=$?
if [[ $status -eq 137 ]]; then
    die "the build container was killed (exit 137: out of its $memory). Re-run with --jobs 1"
elif [[ $status -ne 0 ]]; then
    die "the build failed (exit $status)"
fi
[[ -x "$built" ]] || die "the build finished and left no binary at $built"
