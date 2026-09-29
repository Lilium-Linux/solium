#!/usr/bin/env bash
#
# Install Solium from this checkout, so the login screen can start it.
#
# Builds a release binary in the build container, copies it and the shipped
# QML and Lua into a prefix (~/.local by default), and writes a session file
# for the display manager. It never uses sudo: the session file has to go
# somewhere only root can write, so the script prints the one line that puts it
# there. `--uninstall` takes all of it back out. `dev/install-check.sh` runs an
# install and an uninstall into /tmp and checks every step.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    cat <<EOF
usage: dev/install.sh [options]
       dev/install.sh --uninstall [--prefix DIR] [--session-dir DIR]

  --prefix DIR        where to install (default: \$HOME/.local)
                        DIR/bin/solium, DIR/share/solium/{qml,lua,solium.desktop}
  --session-dir DIR   where the display manager reads Wayland sessions
                        (default: /usr/local/share/wayland-sessions); the
                        printed sudo line installs the session file there
  --no-build          install the binary already built in target/install
  --jobs N            cargo jobs for the build (default: 2)
  --image IMAGE       build container (default: localhost/solium-build:fc44)
  --uninstall         remove what an install with the same options put in place
  -h, --help          this text

environment:
  DESTDIR               stage everything under the prefix into DESTDIR instead
                          (the session file's Exec still names the prefix);
                          --session-dir is used as given
  SOLIUM_BUILD_LOCK     lock the build takes (default: \$XDG_CACHE_HOME/solium-build.lock)
  SOLIUM_BUILD_MEMORY   memory cap of the build container (default: 6g)
EOF
}

die() { echo "install.sh: $*" >&2; exit 1; }

default_prefix="$HOME/.local"
default_session_dir="/usr/local/share/wayland-sessions"
prefix="$default_prefix"
session_dir="$default_session_dir"
build=1
uninstall=0
jobs=2
image="localhost/solium-build:fc44"
lock="${SOLIUM_BUILD_LOCK:-${XDG_CACHE_HOME:-$HOME/.cache}/solium-build.lock}"
memory="${SOLIUM_BUILD_MEMORY:-6g}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --prefix=*) prefix="${1#*=}"; shift ;;
        --session-dir) session_dir="${2:?--session-dir needs a directory}"; shift 2 ;;
        --session-dir=*) session_dir="${1#*=}"; shift ;;
        --no-build) build=0; shift ;;
        --jobs) jobs="${2:?--jobs needs a number}"; shift 2 ;;
        --jobs=*) jobs="${1#*=}"; shift ;;
        --image) image="${2:?--image needs a name}"; shift 2 ;;
        --image=*) image="${1#*=}"; shift ;;
        --uninstall) uninstall=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
done

# Both end up unquoted: the prefix in the session file's Exec line, where a
# space or a `%` would need the escaping the Desktop Entry spec asks for, and
# both in the sudo line printed for pasting. Refused rather than escaped;
# dev/install-check.sh asserts the refusal.
for name in prefix session_dir; do
    value="${!name}"
    [[ "$value" == /* ]] || die "--${name//_/-} must be an absolute path: $value"
    [[ "$value" =~ ^[A-Za-z0-9._/+@-]+$ ]] \
        || die "--${name//_/-} may only contain letters, digits and ._/+@-: $value"
done
prefix="${prefix%/}"
session_dir="${session_dir%/}"
destdir="${DESTDIR:-}"
destdir="${destdir%/}"
[[ -z "$destdir" || "$destdir" == /* ]] || die "DESTDIR must be an absolute path: $destdir"
[[ "$jobs" =~ ^[1-9][0-9]*$ ]] || die "--jobs must be a positive number: $jobs"

dest="$destdir$prefix"
bin="$dest/bin/solium"
share="$dest/share/solium"
staged_session="$share/solium.desktop"
session_file="$session_dir/solium.desktop"

# Where the container sees this checkout. assets.rs looks in the build tree
# (`CARGO_MANIFEST_DIR`) *before* `<binary>/../share/solium`, so a binary
# compiled at the checkout's own path would go on reading the checkout's QML
# and Lua after it was installed. Compiled here, a path the host does not have,
# the installed binary falls through to the copy beside it. The check after
# installing asserts exactly that, and dev/install-check.sh asserts it for a
# staged install.
mount="/solium-src"
built="$root/target/install/release/solium"

# Processes of this user running the binary at $1. /proc/<pid>/exe is how
# fuser answers the same question; a binary replaced while it runs reads
# "<path> (deleted)", which still counts. dev/install-check.sh starts a process
# from the staged path and asserts both install and uninstall refuse.
running_from() {
    local target link exe
    target="$(realpath -m "$1")"
    for link in /proc/[0-9]*/exe; do
        exe="$(readlink "$link" 2>/dev/null)" || continue
        exe="${exe% (deleted)}"
        if [[ "$exe" == "$target" ]]; then
            link="${link#/proc/}"
            echo "${link%/exe}"
        fi
    done
}

refuse_if_running() {
    local pids
    pids="$(running_from "$bin" | tr '\n' ' ')"
    [[ -z "$pids" ]] && return 0
    die "Solium is running from $bin (pid ${pids% }). End that session first; \
this script replaces nothing a running session is using, and stops nothing."
}

# ---------------------------------------------------------------- uninstall --

if [[ $uninstall -eq 1 ]]; then
    refuse_if_running
    removed=()
    if [[ -e "$bin" ]]; then
        rm -f "$bin"
        removed+=("$bin")
    fi
    for path in "$share/qml" "$share/lua" "$staged_session"; do
        if [[ -e "$path" ]]; then
            rm -rf "$path"
            removed+=("$path")
        fi
    done
    if [[ -d "$share" ]] && ! rmdir "$share" 2>/dev/null; then
        echo "left in place, because it holds files this script did not install: $share"
    fi

    if [[ ${#removed[@]} -eq 0 ]]; then
        echo "nothing installed under $dest"
    else
        echo "removed:"
        printf '  %s\n' "${removed[@]}"
    fi
    if [[ -e "$session_file" ]]; then
        echo
        echo "The login screen still offers Solium. Removing that needs root, so run:"
        echo
        echo "  sudo rm -f $session_file"
    fi
    echo
    echo "Kept, because they are yours rather than the installation's:"
    echo "  configuration  ${XDG_CONFIG_HOME:-$HOME/.config}/solium"
    echo "  session log    ${XDG_STATE_HOME:-$HOME/.local/state}/solium/session.log"
    exit 0
fi

# ------------------------------------------------------------------ install --

refuse_if_running

source_session="$root/dev/session/solium.desktop"
[[ -f "$source_session" ]] || die "missing $source_session"
[[ $(grep -c '^Exec=' "$source_session") -eq 1 ]] \
    || die "$source_session must have exactly one Exec line"
grep -q '^Exec=solium\( \|$\)' "$source_session" \
    || die "$source_session's Exec no longer starts with 'solium'; update dev/install.sh"

if [[ $build -eq 1 ]]; then
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
fi
[[ -x "$built" ]] || die "no release build at $built; run without --no-build"

mkdir -p "$dest/bin" "$share" 2>/dev/null \
    || die "cannot write to $dest. This script never uses sudo: choose a --prefix you own"

install -m755 "$built" "$bin"
# Copied, not linked, so a later `git checkout` cannot change a running
# session. dev/install-check.sh diffs the copy against the checkout and asserts
# there are no symlinks in it.
rm -rf "$share/qml" "$share/lua"
cp -RL "$root/crates/solium/qml" "$root/crates/solium/lua" "$share/"

# The Exec names the binary by its full path. Plasma Login starts a session
# with PATH=/usr/local/bin:/usr/bin:/bin (DefaultPath in /etc/plasmalogin.conf),
# and whether ~/.local/bin is added after that is up to the user's shell
# profile. dev/install-check.sh asserts the Exec is absolute.
sed "s|^Exec=solium|Exec=$prefix/bin/solium|" "$source_session" > "$staged_session"
chmod 644 "$staged_session"

# The gate's scripts check, run from the installed copy, with the asset root
# read from the line assets.rs logs when it resolves one (`shipped assets`,
# `root=`). A copy that passes by reading some other tree proves nothing.
# dev/install-check.sh asserts the same root from the staged copy.
echo "checking $bin --check ..."
check_log="$(mktemp -t solium-install-check-XXXXXX.log)"
trap 'rm -f "$check_log"' EXIT
if ! RUST_LOG="info,solium::assets=debug" "$bin" --check >"$check_log" 2>&1; then
    cat "$check_log" >&2
    die "$bin --check failed"
fi
chosen="$(sed -n 's/.*shipped assets root=\([^ ]*\).*/\1/p' "$check_log" | head -1)"
expected="$(realpath "$share")"
if [[ "$chosen" != "$expected" ]]; then
    cat "$check_log" >&2
    die "the installed binary took its QML and Lua from '${chosen:-nowhere}', not $expected"
fi
checked="$(grep -m1 '^  ok:' "$check_log" || true)"

if [[ -z "$destdir" ]]; then
    on_path="$(command -v solium || true)"
    if [[ -z "$on_path" ]]; then
        path_note="warning: solium is not on your PATH. The login screen does not need it \
(the session file names $bin), but a shell will not find \`solium --check\`."
    elif [[ "$(realpath "$on_path")" != "$(realpath "$bin")" ]]; then
        path_note="warning: the solium on your PATH is $on_path, not the one just installed ($bin)."
    else
        path_note="solium on your PATH is the one just installed."
    fi
else
    path_note="PATH not checked: DESTDIR is set, so nothing here is live yet."
fi

uninstall_cmd="dev/install.sh --uninstall"
[[ "$prefix" == "$default_prefix" ]] || uninstall_cmd+=" --prefix $prefix"
[[ "$session_dir" == "$default_session_dir" ]] || uninstall_cmd+=" --session-dir $session_dir"
[[ -z "$destdir" ]] || uninstall_cmd="DESTDIR=$destdir $uninstall_cmd"

cat <<EOF

Installed Solium into $dest
  binary        $bin
  QML and Lua   $share/qml, $share/lua
                (copies: rebuilding or checking out another branch does not change them)
  session file  $staged_session
                ($(grep -m1 '^Exec=' "$staged_session"))
  check         $bin --check passed${checked:+ (${checked#  })}, using $chosen
  $path_note

To offer Solium at the login screen, run this one line. It needs root, so this
script does not run it:

  sudo install -Dm644 $staged_session $session_file

Then log out and pick Solium from the session list. Plasma Login, Fedora 44
KDE's display manager, reads /usr/local/share/wayland-sessions and
/usr/share/wayland-sessions when its greeter starts.

The session logs to ${XDG_STATE_HOME:-$HOME/.local/state}/solium/session.log, appended
to by every session. Plasma Login also writes the session's stderr, which carries
the same lines, to ~/.local/share/plasmalogin/wayland-session.log, overwritten at
each login.

To uninstall, which prints the matching sudo line for the session file:

  $uninstall_cmd
EOF
