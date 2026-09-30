#!/usr/bin/env bash
#
# Build the documentation site into target/book, the way CI publishes it:
#
#   dev/docs.sh            build it, and check every link inside it
#   dev/docs.sh --serve    build it, then serve it on http://localhost:3000 and
#                          rebuild as docs/ changes
#
# In order: the compositor (its `--check` lists the key bindings), the Rust
# API with rustdoc, the pages dev/docs/generate.py writes from the code, the
# book, the rustdoc copied under api/, and the link check. Fails on the first
# step that does.
#
# `--serve` is for writing: mdBook rebuilds what is in docs/ on every save, but
# not the generated pages -- run this again after changing config.lua, sol.lua,
# environment.txt or a binding -- and the Rust API is not served.
#
# mdBook is installed with `cargo install`, at the version pinned below, into
# a cache directory rather than the build image, so the image stays what
# dev/Containerfile says it is:
#
#   SOLIUM_DOCS_TOOLS=<dir>         where mdBook is installed: <dir>/bin/mdbook.
#                                   ${XDG_CACHE_HOME:-~/.cache}/solium-docs
#                                   by default.
#
# Runs on this machine by default, which needs what the README's "Building"
# section lists, and Python 3. The rest is opt-in, the same way dev/gate.sh
# does it:
#
#   SOLIUM_DOCS_IMAGE=<image>       build in this podman image instead, e.g.
#                                   localhost/solium-build:fc44 from
#                                   dev/Containerfile. SOLIUM_GATE_IMAGE is
#                                   used when this is not set.
#   SOLIUM_DOCS_PODMAN_ARGS=<args>  extra arguments for `podman run`.
#   SOLIUM_DOCS_JOBS=<n>            cargo -j<n>.
#   SOLIUM_DOCS_PORT=<port>         the port --serve listens on; 3000.
set -euo pipefail

# The mdBook this site is built with. CI runs this script, so this is the one
# place the version is written.
MDBOOK_VERSION=0.5.4

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${SOLIUM_DOCS_IMAGE:-${SOLIUM_GATE_IMAGE:-}}"
tools="${SOLIUM_DOCS_TOOLS:-${XDG_CACHE_HOME:-$HOME/.cache}/solium-docs}"
port="${SOLIUM_DOCS_PORT:-3000}"

serve=""
for argument in "$@"; do
    case "$argument" in
        --serve) serve=1 ;;
        -h|--help) sed -n '2,38p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "docs.sh: unknown argument $argument (see --help)" >&2; exit 2 ;;
    esac
done

# Into the container, once, and run this same script there natively.
if [[ -n "$image" && -z "${SOLIUM_DOCS_INSIDE:-}" ]]; then
    if ! podman image exists "$image"; then
        echo "docs.sh: no image $image. Build it with:" >&2
        echo "  podman build -t ${image#localhost/} -f $root/dev/Containerfile $root/dev/" >&2
        exit 1
    fi
    cargo_home="${CARGO_HOME:-$HOME/.cargo}"
    rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
    mkdir -p "$tools"
    mounts=(-v "$root:$root" -v "$cargo_home:$cargo_home" -v "$tools:$tools")
    if [[ -d "$rustup_home" ]]; then
        mounts+=(-v "$rustup_home:$rustup_home")
    fi
    ports=()
    if [[ -n "$serve" ]]; then
        ports=(-p "127.0.0.1:$port:$port")
    fi
    # Word-split on purpose: this is a list of arguments.
    # shellcheck disable=SC2206
    extra=(${SOLIUM_DOCS_PODMAN_ARGS:-})
    # keep-id and label=disable for the gate's reasons: what is built belongs
    # to whoever ran this, and nothing mounted is relabelled.
    exec podman run --rm --userns=keep-id --security-opt label=disable \
        "${extra[@]}" "${mounts[@]}" "${ports[@]}" \
        -e HOME=/tmp -e SOLIUM_DOCS_INSIDE=1 \
        -e SOLIUM_DOCS_TOOLS="$tools" -e SOLIUM_DOCS_PORT="$port" \
        -e SOLIUM_DOCS_JOBS="${SOLIUM_DOCS_JOBS:-}" \
        -e CARGO_HOME="$cargo_home" -e RUSTUP_HOME="$rustup_home" \
        -e PATH="$cargo_home/bin:/usr/local/bin:/usr/bin:/bin" \
        -w "$root" "$image" \
        "$root/dev/docs.sh" "$@"
fi

cd "$root"
# Politeness, as in dev/gate.sh: a docs build in the background should not
# make the machine it runs on unusable.
polite=(nice -n 19)
if command -v ionice >/dev/null 2>&1; then
    polite+=(ionice -c 3)
fi
jobs=()
if [[ -n "${SOLIUM_DOCS_JOBS:-}" ]]; then
    jobs=(-j "$SOLIUM_DOCS_JOBS")
fi
fail() { echo "DOCS FAILED: $*" >&2; exit 1; }

command -v python3 >/dev/null || fail "python3 is not installed"

echo "compositor..."
"${polite[@]}" cargo build -p solium "${jobs[@]}" || fail "cargo build -p solium"

echo "rustdoc..."
"${polite[@]}" cargo doc --workspace --no-deps --document-private-items "${jobs[@]}" \
    || fail "cargo doc"

mdbook="$tools/bin/mdbook"
if [[ "$("$mdbook" --version 2>/dev/null || true)" != "mdbook v$MDBOOK_VERSION" ]]; then
    echo "mdbook $MDBOOK_VERSION, into $tools..."
    "${polite[@]}" cargo install mdbook --locked --version "$MDBOOK_VERSION" --root "$tools" "${jobs[@]}" \
        || fail "cargo install mdbook"
fi

echo "generated pages..."
python3 dev/docs/generate.py --solium target/debug/solium || fail "dev/docs/generate.py"

if [[ -n "$serve" ]]; then
    echo "serving on http://localhost:$port"
    exec "$mdbook" serve --hostname "$([[ -n "${SOLIUM_DOCS_INSIDE:-}" ]] && echo 0.0.0.0 || echo 127.0.0.1)" \
        --port "$port"
fi

echo "book..."
"$mdbook" build || fail "mdbook build"

# After the book, because mdBook empties its build directory first.
rm -rf target/book/api
cp -R target/doc target/book/api

echo "links..."
python3 dev/docs/linkcheck.py target/book || fail "broken links"

echo "docs built: $root/target/book/index.html"
