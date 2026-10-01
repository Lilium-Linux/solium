#!/usr/bin/env bash
#
# Build a Fedora package of this checkout's commit (#66): a development
# snapshot, target/rpm/RPMS/<arch>/solium-0.0.0~git<date>.<commit>-1.fc<N>.<arch>.rpm.
#
# The release binary is built in the build container, through the shared lock
# and capped, exactly as dev/install.sh builds it (dev/build-release.sh).
# rpmbuild, which the container does not have, then runs here on the host,
# `--with prebuilt` (dev/rpm/solium.spec), into a _topdir of its own under
# target/rpm and never ~/rpmbuild. Then the package is checked without
# installing it: unpacked into a scratch root, its solium --check has to pass
# and take its QML and Lua from that root's share/solium. It never uses sudo:
# it prints the dnf line that installs the package.
#
#   dev/rpm.sh [--jobs N] [--image IMAGE]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    cat <<EOF
usage: dev/rpm.sh [--jobs N] [--image IMAGE]

  --jobs N            cargo jobs for the build (default: 2)
  --image IMAGE       build container (default: localhost/solium-build:fc44)
  -h, --help          this text

Builds target/rpm/RPMS/<arch>/solium-*.rpm from the commit checked out, and
prints the sudo dnf line that installs it.
EOF
}

die() { echo "rpm.sh: $*" >&2; exit 1; }

image="localhost/solium-build:fc44"
build_args=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --jobs) build_args+=(--jobs "${2:?--jobs needs a number}"); shift 2 ;;
        --jobs=*) build_args+=(--jobs "${1#*=}"); shift ;;
        --image) image="${2:?--image needs a name}"; shift 2 ;;
        --image=*) image="${1#*=}"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
done
build_args+=(--image "$image")

for tool in rpmbuild rpm2cpio cpio git podman; do
    command -v "$tool" >/dev/null || die "$tool is not installed (rpmbuild is in rpm-build)"
done

# The package is named after a commit, and its source tarball is that commit,
# so the binary in it has to be built from that commit too.
git -C "$root" diff --quiet HEAD -- \
    || die "this checkout has uncommitted changes. The package is named after the \
commit it is built from, so commit or stash them first"

# The binary links the build image's libraries, and the package is labelled
# with this machine's release, so the two have to be the same Fedora.
host_fedora="$(rpm --eval '%{?fedora}')"
[[ -n "$host_fedora" ]] || die "this is not Fedora (rpm --eval %fedora says nothing)"
podman image exists "$image" \
    || die "the build image $image does not exist. Build it once with:
  podman build -t ${image#localhost/} -f dev/Containerfile dev/"
image_fedora="$(podman run --rm "$image" rpm --eval '%{?fedora}' 2>/dev/null || true)"
[[ "$image_fedora" == "$host_fedora" ]] || die "the build image $image is Fedora \
${image_fedora:-unknown} and this machine is Fedora $host_fedora, so the package would \
name the wrong release. Build an image for Fedora $host_fedora:
  podman build --build-arg FEDORA_VERSION=$host_fedora -t solium-build:fc$host_fedora -f dev/Containerfile dev/
and pass it with --image localhost/solium-build:fc$host_fedora"

"$root/dev/build-release.sh" "${build_args[@]}"
built="$root/target/install/release/solium"

commit="$(git -C "$root" rev-parse --short=7 HEAD)"
commitdate="$(TZ=UTC git -C "$root" show -s --format=%cd --date=format-local:%Y%m%d HEAD)"
version="0.0.0~git$commitdate.$commit"

topdir="$root/target/rpm"
[[ ! -L "$topdir" ]] || die "$topdir is a link, and this script empties it: remove the link first"
rm -rf "$topdir"
mkdir -p "$topdir/SOURCES"
git -C "$root" archive --format=tar.gz --prefix="solium-$version/" \
    -o "$topdir/SOURCES/solium-$version.tar.gz" HEAD
install -m755 "$built" "$topdir/SOURCES/solium"

log="$topdir/rpmbuild.log"
echo "packaging solium $version with rpmbuild (log: $log)..."
if ! rpmbuild -bb --with prebuilt \
    --define "_topdir $topdir" \
    --define "commit $commit" \
    --define "commitdate $commitdate" \
    "$root/dev/rpm/solium.spec" >"$log" 2>&1; then
    tail -30 "$log" >&2
    die "rpmbuild failed; the whole log is $log"
fi
shopt -s nullglob
packages=("$topdir"/RPMS/*/solium-"$version"-*.rpm)
shopt -u nullglob
[[ ${#packages[@]} -eq 1 ]] || die "rpmbuild left ${#packages[@]} packages for $version in $topdir/RPMS"
package="${packages[0]}"

# The package as another machine gets it, without installing it: unpacked into
# a scratch root, with an empty configuration directory, its binary has to pass
# --check and take its QML and Lua from the share/solium beside it, which is
# what it does once installed under /usr.
unpacked="$topdir/unpacked"
mkdir -p "$unpacked" "$topdir/check-config"
(cd "$unpacked" && rpm2cpio "$package" | cpio -idm --quiet)
check_log="$topdir/check.log"
if ! XDG_CONFIG_HOME="$topdir/check-config" RUST_LOG="info,solium::assets=debug" \
    "$unpacked/usr/bin/solium" --check >"$check_log" 2>&1; then
    cat "$check_log" >&2
    die "the packaged solium --check failed, unpacked in $unpacked. Do not install $package"
fi
chosen="$(sed -n 's/.*shipped assets root=\([^ ]*\).*/\1/p' "$check_log" | head -1)"
expected="$(realpath "$unpacked/usr/share/solium")"
if [[ -z "$chosen" || "$(realpath -m "$chosen")" != "$expected" ]]; then
    cat "$check_log" >&2
    die "the packaged solium took its QML and Lua from '${chosen:-nowhere}', not \
$expected. Do not install $package"
fi
checked="$(grep -m1 '^  ok:' "$check_log" || true)"

# An install from the checkout beside the package: two Solium sessions at the
# login screen, and units in ~/.config that come before the package's.
leftovers=()
for path in /usr/local/share/wayland-sessions/solium.desktop \
    "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/solium-session.target" \
    "$HOME/.local/bin/solium"; do
    if [[ -e "$path" ]]; then
        leftovers+=("$path")
    fi
done
leftover_note=""
if [[ ${#leftovers[@]} -gt 0 ]]; then
    leftover_note="
warning: an install from a checkout is here too:
$(printf '  %s\n' "${leftovers[@]}")
The login screen would offer two Solium sessions, and the units in ~/.config
come before the package's. Take it out first with dev/install.sh --uninstall
(and the sudo line it prints).
"
fi

cat <<EOF

Built $package ($(du -h "$package" | cut -f1))
  version   $version-1.fc$host_fedora, a development snapshot of commit $commit
  for       Fedora $host_fedora, $(rpm --eval '%{_arch}'): another Fedora release needs a package built for it
  checked   unpacked in $unpacked, solium --check passed${checked:+ (${checked#  })},
            using $chosen
$leftover_note
To install it here, run this one line. It needs root, so this script does not
run it:

  sudo dnf install $package

On another Fedora $host_fedora machine, copy the file there and run the same line
with its path, such as \`sudo dnf install ./${package##*/}\`. dnf installs
the Qt, Xwayland and portals it needs; no Rust or build image is needed there.
Then log out and pick Solium from the session list, or log in on a text
console (Ctrl+Alt+F3) and run \`solium --tty\`. \`solium --probe\`, safe inside
a running desktop, says first what the hardware offers.

To remove it: sudo dnf remove solium
EOF
