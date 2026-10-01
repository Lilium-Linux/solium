#!/usr/bin/env bash
#
# Install Solium from this checkout, so the login screen can start it.
#
# Builds a release binary in the build container, copies it, the
# solium-session script the login screen starts, and the shipped QML and Lua
# into a prefix (~/.local by default), and writes a session file for the
# display manager. The systemd user units Solium starts and its portal choice
# (#146) go into the user's configuration directory. It never uses sudo:
# the session file has to go somewhere only root can write, so the script
# prints the one line that puts it there. `--uninstall` takes all of it back
# out. `dev/install-check.sh` runs an install and an uninstall into /tmp and
# checks every step.
#
# A prefix of /usr or /usr/local is a system installation, which is what the
# Fedora package's %install runs (dev/rpm/solium.spec): everything goes under
# the prefix, the units in lib/systemd/user, the portal choice in
# share/xdg-desktop-portal and the session file in share/wayland-sessions,
# where systemd, xdg-desktop-portal and the display manager read system files,
# and nothing goes into the user's configuration. dev/install-check.sh's
# "a system prefix" checks that layout, and that it is the spec's file list.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    cat <<EOF
usage: dev/install.sh [options]
       dev/install.sh --uninstall [--prefix DIR] [--session-dir DIR]

  --prefix DIR        where to install (default: \$HOME/.local)
                        DIR/bin/{solium,solium-session},
                        DIR/share/solium/{qml,lua,solium.desktop}
                        and into \$XDG_CONFIG_HOME (default: \$HOME/.config):
                        systemd/user/solium-{session,autostart}.target
                        and xdg-desktop-portal/lilium-portals.conf
                      /usr and /usr/local are system prefixes, all of it
                        under DIR: bin/{solium,solium-session},
                        share/solium/{qml,lua},
                        share/wayland-sessions/solium.desktop,
                        lib/systemd/user/solium-{session,autostart}.target
                        and share/xdg-desktop-portal/lilium-portals.conf
  --session-dir DIR   where the display manager reads Wayland sessions
                        (default: /usr/local/share/wayland-sessions); the
                        printed sudo line installs the session file there.
                        Not with a system prefix
  --no-build          install the binary already built in target/install
  --jobs N            cargo jobs for the build (default: 2)
  --image IMAGE       build container (default: localhost/solium-build:fc44)
  --uninstall         remove what an install with the same options put in place
  -h, --help          this text

environment:
  DESTDIR               stage everything under the prefix and \$XDG_CONFIG_HOME
                          into DESTDIR instead (the session file's Exec still
                          names the prefix); --session-dir is used as given
  XDG_CONFIG_HOME       where the units and the portal configuration go,
                          unless the prefix is a system one
  SOLIUM_BUILD_LOCK     lock the build takes (default: \$XDG_CACHE_HOME/solium-build.lock)
  SOLIUM_BUILD_MEMORY   memory cap of the build container (default: 6g)
EOF
}

die() { echo "install.sh: $*" >&2; exit 1; }

default_prefix="$HOME/.local"
default_session_dir="/usr/local/share/wayland-sessions"
default_config_home="$HOME/.config"
config_home="${XDG_CONFIG_HOME:-$default_config_home}"
prefix="$default_prefix"
session_dir="$default_session_dir"
session_dir_given=0
build=1
uninstall=0
jobs=2
image="localhost/solium-build:fc44"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift 2 ;;
        --prefix=*) prefix="${1#*=}"; shift ;;
        --session-dir) session_dir="${2:?--session-dir needs a directory}"; session_dir_given=1; shift 2 ;;
        --session-dir=*) session_dir="${1#*=}"; session_dir_given=1; shift ;;
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

# All three end up unquoted: the prefix in the session file's Exec line, where
# a space or a `%` would need the escaping the Desktop Entry spec asks for, and
# all of them in the sudo and uninstall lines printed for pasting. Refused
# rather than escaped; dev/install-check.sh asserts the refusal for a prefix and
# for a DESTDIR.
for name in prefix session_dir; do
    value="${!name}"
    [[ "$value" == /* ]] || die "--${name//_/-} must be an absolute path: $value"
    [[ "$value" =~ ^[A-Za-z0-9._/+@-]+$ ]] \
        || die "--${name//_/-} may only contain letters, digits and ._/+@-: $value"
done
# Printed the same way, in the uninstall line.
[[ "$config_home" == /* ]] || die "XDG_CONFIG_HOME must be an absolute path: $config_home"
[[ "$config_home" =~ ^[A-Za-z0-9._/+@-]+$ ]] \
    || die "XDG_CONFIG_HOME may only contain letters, digits and ._/+@-: $config_home"
config_home="${config_home%"${config_home##*[!/]}"}"
[[ -n "$config_home" ]] || die "/ is not a usable XDG_CONFIG_HOME"
destdir="${DESTDIR:-}"
[[ -z "$destdir" || "$destdir" == /* ]] || die "DESTDIR must be an absolute path: $destdir"
# And ~, which rpmbuild's buildroot has, the version being 0.0.0~git...
# DESTDIR is never in the Exec line, and in the printed uninstall line a ~ that
# does not start a word is only a ~. dev/install-check.sh's "a system prefix"
# installs into one.
[[ -z "$destdir" || "$destdir" =~ ^[A-Za-z0-9._/+@~-]+$ ]] \
    || die "DESTDIR may only contain letters, digits and ._/+@~-: $destdir"
# Every trailing slash goes, so `/` and `//` come out empty and are refused
# (as a prefix, they would put the binary at /bin/solium). dev/install-check.sh
# asserts the refusal of `--prefix /` and of `--session-dir //`.
prefix="${prefix%"${prefix##*[!/]}"}"
session_dir="${session_dir%"${session_dir##*[!/]}"}"
destdir="${destdir%"${destdir##*[!/]}"}"
[[ -n "$prefix" ]] || die "/ is not a usable --prefix"
[[ -n "$session_dir" ]] || die "/ is not a usable --session-dir"
[[ "$jobs" =~ ^[1-9][0-9]*$ ]] || die "--jobs must be a positive number: $jobs"

# The system layout, described at the top. dev/install-check.sh asserts that
# --session-dir is refused with it.
system=0
case "$prefix" in
    /usr | /usr/local) system=1 ;;
esac
if [[ $system -eq 1 ]]; then
    [[ $session_dir_given -eq 0 ]] || die "--session-dir is for an install into \
your home: with --prefix $prefix the session file goes in $prefix/share/wayland-sessions"
    session_dir="$prefix/share/wayland-sessions"
fi

dest="$destdir$prefix"
bin="$dest/bin/solium"
# What the session file starts: `solium --tty --session`, and the clean-up
# after a Solium that could not do its own (dev/session/solium-session).
wrapper="$dest/bin/solium-session"
share="$dest/share/solium"
session_file="$session_dir/solium.desktop"
if [[ $system -eq 1 ]]; then
    staged_session="$destdir$session_file"
else
    staged_session="$share/solium.desktop"
fi

# What goes into the user's configuration rather than the prefix, as
# `<file in dev/session>:<where>`: the units Solium starts (`session.rs`) and
# which portal answers what. Beside the user's own configuration, so a file
# there is only replaced or removed while it is still exactly what this script
# wrote, as `config.sha256` in share/solium records it. One edited since, or a
# link, is the user's and is kept. dev/install-check.sh plants both and asserts
# both are left alone.
#
# Under a system prefix they are the installation's, like the binary: no
# record, replaced by an install and removed by an uninstall whatever put them
# there (dev/install-check.sh's "a system prefix").
config_dest="$destdir$config_home"
units="$config_dest/systemd/user"
portals="$config_dest/xdg-desktop-portal"
manifest="$share/config.sha256"
if [[ $system -eq 1 ]]; then
    config_dest="$dest"
    units="$dest/lib/systemd/user"
    portals="$dest/share/xdg-desktop-portal"
    manifest=""
fi
config_files=(
    "solium-session.target:$units/solium-session.target"
    "solium-autostart.target:$units/solium-autostart.target"
    "lilium-portals.conf:$portals/lilium-portals.conf"
)

# Whether $1 is a file this script wrote and nobody has changed since.
ours() {
    local path="$1" recorded
    [[ -f "$path" && ! -L "$path" && -f "$manifest" ]] || return 1
    recorded="$(awk -v path="$path" '$2 == path { print $1 }' "$manifest")"
    [[ -n "$recorded" && "$(sha256sum <"$path" | cut -d' ' -f1)" == "$recorded" ]]
}

# Install replaces $share/qml and $share/lua, and uninstall removes them, with
# `rm -rf`. Through a link that deletes whatever the link leads to: a
# share/solium left linked to a checkout would lose its QML and Lua, uncommitted
# work included. So neither goes on while share/solium is a link, or while it
# resolves into this checkout by any other route. A linked <prefix>/share is
# fine -- a ~/.local/share moved to another disk is an ordinary setup -- because
# the realpath check below still catches one that leads into this checkout.
# dev/install-check.sh plants files behind each case and asserts that install
# and uninstall both refuse and the files survive.
for path in "$share"; do
    if [[ -L "$path" ]]; then
        die "$path is a symlink (to $(readlink "$path")). This script deletes what \
is under $share, so it will not go through a link: remove the link yourself first"
    fi
done
resolved_share="$(realpath -m "$share")"
case "$resolved_share/" in
    "$(realpath "$root")"/*)
        die "$share resolves to $resolved_share, inside this checkout. Installing or \
uninstalling there would delete the checkout's own files: choose a --prefix outside it" ;;
esac

# What dev/build-release.sh builds, at a path the host does not have, so that
# the installed copy reads the QML and Lua beside it rather than this
# checkout's: the check after installing asserts it.
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
    kept=()
    for entry in "${config_files[@]}"; do
        path="${entry#*:}"
        if ours "$path" || { [[ $system -eq 1 ]] && [[ -e "$path" || -L "$path" ]]; }; then
            rm -f "$path"
            removed+=("$path")
        elif [[ -e "$path" || -L "$path" ]]; then
            kept+=("$path")
        fi
    done
    if [[ -n "$manifest" && -e "$manifest" ]]; then
        rm -f "$manifest"
        removed+=("$manifest")
    fi
    for path in "$bin" "$wrapper"; do
        if [[ -e "$path" ]]; then
            rm -f "$path"
            removed+=("$path")
        fi
    done
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
    if [[ ${#kept[@]} -gt 0 ]]; then
        echo "kept, because this script did not write them as they are now:"
        printf '  %s\n' "${kept[@]}"
    fi
    if [[ $system -eq 0 && -e "$session_file" ]]; then
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
grep -qx 'Exec=solium-session' "$source_session" \
    || die "$source_session's Exec is no longer 'solium-session'; update dev/install.sh"
source_wrapper="$root/dev/session/solium-session"
[[ -f "$source_wrapper" ]] || die "missing $source_wrapper"
for entry in "${config_files[@]}"; do
    [[ -f "$root/dev/session/${entry%%:*}" ]] || die "missing $root/dev/session/${entry%%:*}"
done

if [[ $build -eq 1 ]]; then
    "$root/dev/build-release.sh" --jobs "$jobs" --image "$image" || die "the build failed"
fi
[[ -x "$built" ]] || die "no release build at $built; run without --no-build"

# Again, now that the build is over: a session may have started from the old
# install while it ran. dev/install-check.sh starts one from inside a stand-in
# build and asserts that nothing is replaced or created.
refuse_if_running

mkdir -p "$dest/bin" "$share" 2>/dev/null \
    || die "cannot write to $dest. This script never uses sudo: choose a --prefix you own"

install -m755 "$built" "$bin"
install -m755 "$source_wrapper" "$wrapper"
# Copied, not linked, so a later `git checkout` cannot change a running
# session. dev/install-check.sh diffs the copy against the checkout and asserts
# there are no symlinks in it.
rm -rf "$share/qml" "$share/lua"
cp -RL "$root/crates/solium/qml" "$root/crates/solium/lua" "$share/"

# The Exec names solium-session by its full path, and solium-session runs the
# solium beside it. Plasma Login starts a session with
# PATH=/usr/local/bin:/usr/bin:/bin (DefaultPath in /etc/plasmalogin.conf), and
# whether ~/.local/bin is added after that is up to the user's shell profile.
# dev/install-check.sh asserts the Exec is absolute.
# Written beside the target and renamed over it, never through a redirect: a
# redirect follows a link, and a staged solium.desktop linked to the checkout's
# own session file would be emptied before sed read it.
# Under a system prefix it is written where the display manager reads it.
mkdir -p "$(dirname "$staged_session")"
staged_tmp="$(mktemp "$(dirname "$staged_session")/.solium.desktop.XXXXXX")"
sed "s|^Exec=solium-session\$|Exec=$prefix/bin/solium-session|" "$source_session" > "$staged_tmp"
chmod 644 "$staged_tmp"
mv -f "$staged_tmp" "$staged_session"
grep -qx "Exec=$prefix/bin/solium-session" "$staged_session" \
    || die "the generated session file has no absolute Exec line: $staged_session"

# The units and the portal choice. Each written beside its target and renamed
# over it, like the session file, so a link is replaced rather than written
# through -- except that a link, or a file edited since this script wrote it,
# is the user's, and is kept instead.
config_written=()
config_kept=()
manifest_tmp="$(mktemp "$share/.config.sha256.XXXXXX")"
for entry in "${config_files[@]}"; do
    source="$root/dev/session/${entry%%:*}"
    path="${entry#*:}"
    if [[ $system -eq 0 ]] && { [[ -L "$path" ]] \
        || { [[ -e "$path" ]] && ! ours "$path" && ! cmp -s "$source" "$path"; }; }; then
        config_kept+=("$path")
        continue
    fi
    mkdir -p "$(dirname "$path")" 2>/dev/null \
        || die "cannot write to $(dirname "$path"): choose an XDG_CONFIG_HOME you own"
    tmp="$(mktemp "$(dirname "$path")/.${path##*/}.XXXXXX")"
    cp "$source" "$tmp"
    chmod 644 "$tmp"
    mv -f "$tmp" "$path"
    config_written+=("$path")
    echo "$(sha256sum <"$path" | cut -d' ' -f1) $path" >>"$manifest_tmp"
done
if [[ -n "$manifest" ]]; then
    chmod 644 "$manifest_tmp"
    mv -f "$manifest_tmp" "$manifest"
else
    rm -f "$manifest_tmp"
fi

uninstall_cmd="dev/install.sh --uninstall"
[[ "$prefix" == "$default_prefix" ]] || uninstall_cmd+=" --prefix $prefix"
[[ $system -eq 1 || "$session_dir" == "$default_session_dir" ]] \
    || uninstall_cmd+=" --session-dir $session_dir"
[[ -z "$destdir" ]] || uninstall_cmd="DESTDIR=$destdir $uninstall_cmd"
[[ $system -eq 1 || "$config_home" == "$default_config_home" ]] \
    || uninstall_cmd="XDG_CONFIG_HOME=$config_home $uninstall_cmd"

# The new files are already in place when the check runs, and a session file
# from an earlier install would start them at the next login, so a failure says
# so. dev/install-check.sh fails the check on purpose and asserts the message.
placed="$dest and $config_dest"
[[ $system -eq 0 ]] || placed="$dest"
not_checked="The new files are already in place under $placed. Do \
not log into Solium until this is fixed and dev/install.sh re-run, or take them \
out with:
  $uninstall_cmd"

# The gate's scripts check, run from the installed copy, with the asset root
# read from the line assets.rs logs when it resolves one (`shipped assets`,
# `root=`). A copy that passes by reading some other tree proves nothing.
# dev/install-check.sh asserts the same root from the staged copy.
#
# Under a system prefix, with an empty configuration directory: the
# installation is every user's, so what has to load is the shipped
# configuration, not this user's. dev/install-check.sh's "a system prefix"
# installs with a broken one.
echo "checking $bin --check ..."
check_log="$(mktemp -t solium-install-check-XXXXXX.log)"
check_config="$config_home"
if [[ $system -eq 1 ]]; then
    check_config="$(mktemp -d -t solium-install-config-XXXXXX)"
fi
trap 'rm -f "$check_log"; [[ "$check_config" == "$config_home" ]] || rm -rf "$check_config"' EXIT
if ! XDG_CONFIG_HOME="$check_config" RUST_LOG="info,solium::assets=debug" "$bin" --check \
    >"$check_log" 2>&1; then
    cat "$check_log" >&2
    die "$bin --check failed. $not_checked"
fi
chosen="$(sed -n 's/.*shipped assets root=\([^ ]*\).*/\1/p' "$check_log" | head -1)"
# Both sides resolved: the binary logs the path it was reached by, which goes
# through a linked <prefix>/share, while $share is spelled by its real place.
expected="$(realpath "$share")"
if [[ -z "$chosen" || "$(realpath -m "$chosen")" != "$expected" ]]; then
    cat "$check_log" >&2
    die "the installed binary took its QML and Lua from '${chosen:-nowhere}', not \
$expected. $not_checked"
fi
checked="$(grep -m1 '^  ok:' "$check_log" || true)"

config_note=""
if [[ ${#config_kept[@]} -gt 0 ]]; then
    config_note="
  kept          these, because this script did not write them as they are now
                (a link, or edited since): compare each with dev/session/
$(printf '                  %s\n' "${config_kept[@]}")"
fi
reload_note=""
if [[ -z "$destdir" && ${#config_written[@]} -gt 0 ]]; then
    reload_note="
systemd reads the units at your next login. To have it read them now, which
changes nothing that is running:

  systemctl --user daemon-reload
"
fi

# A session file from an earlier install goes on starting Solium the old way
# until the sudo line is run again, and nothing else says so.
session_note=""
if [[ $system -eq 0 && -e "$session_file" ]] && ! cmp -s "$session_file" "$staged_session"; then
    installed_exec="$(sed -n 's/^Exec=//p' "$session_file" | head -1)"
    case "$installed_exec" in
        *solium-session* | *--session*) told="" ;;
        *) told=", which tells systemd and D-Bus nothing about the session" ;;
    esac
    session_note="
warning: the login screen's session file, $session_file, is not this
install's. It starts '${installed_exec:-nothing}'$told. Run the line below
again to replace it.
"
fi

# Under a system prefix the session file is already where the display manager
# looks, so there is no sudo line (dev/install-check.sh's "a system prefix").
uninstall_note=", which prints the matching sudo line for the session file"
if [[ $system -eq 1 ]]; then
    uninstall_note=""
    login_note="The session file is in $session_dir, where the login screen looks
when its greeter starts: log out and pick Solium from the session list."
else
    login_note="To offer Solium at the login screen, run this one line. It needs root, so this
script does not run it:

  sudo install -Dm644 $staged_session $session_file

Then log out and pick Solium from the session list. Plasma Login, Fedora 44
KDE's display manager, reads /usr/local/share/wayland-sessions and
/usr/share/wayland-sessions when its greeter starts."
fi

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

cat <<EOF

Installed Solium into $dest
  binary        $bin
  started by    $wrapper
  QML and Lua   $share/qml, $share/lua
                (copies: rebuilding or checking out another branch does not change them)
  session file  $staged_session
                ($(grep -m1 '^Exec=' "$staged_session"))
  units         $units/solium-session.target
                $units/solium-autostart.target
  portals       $portals/lilium-portals.conf$config_note
  check         $bin --check passed${checked:+ (${checked#  })}, using $chosen
  $path_note
$reload_note$session_note
$login_note

Getting back: Plasma Login remembers the last session, so it offers Solium
first from then on; pick Plasma to return. Ctrl+Alt+Backspace ends a Solium
session whatever the configuration says, unless the screen is locked
(super+shift+q does too, in the shipped configuration), and Ctrl+Alt+F3
switches to a text console to log in and read the log. Solium handles both
chords itself, so neither helps if Solium itself hangs.

The session logs to ${XDG_STATE_HOME:-$HOME/.local/state}/solium/session.log, appended
to by every session. Plasma Login also writes the session's stderr, which carries
the same lines, to ~/.local/share/plasmalogin/wayland-session.log, overwritten at
each login.

To uninstall$uninstall_note:

  $uninstall_cmd
EOF
