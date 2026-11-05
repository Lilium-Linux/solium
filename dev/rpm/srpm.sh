#!/usr/bin/env bash
#
# Make the source package COPR builds (#486): stage, solium-stage at a
# snapshot of HEAD, or release, solium at Cargo.toml's version.
#
#   dev/rpm/srpm.sh stage|release [OUTDIR]
#
# COPR rebuilds the source package with none of the definitions dev/rpm.sh
# passes on rpmbuild's command line, and with no network. So the spec in it
# is dev/rpm/solium.spec with the commit, its date, the commit count and the
# version written in as %global lines, and the variant chosen, and next to the
# source tarball is every crate Cargo.lock names, from `cargo vendor`, which
# the spec unpacks `--with vendored`. Prints the path of the .src.rpm, and
# writes OUTDIR/package.env: the name, version, base, count and commit.
#
# Needs git, cargo and rpmbuild (rpm-build), and network for `cargo vendor`;
# no build container, since nothing is compiled. .github/workflows/package.yml
# runs it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
die() { echo "srpm.sh: $*" >&2; exit 1; }

kind="${1:-}"
case "$kind" in
    stage) stage=1 ;;
    release) stage=0 ;;
    *) die "usage: dev/rpm/srpm.sh stage|release [OUTDIR]" ;;
esac
out="${2:-$root/target/srpm}"

for tool in git cargo rpmbuild; do
    command -v "$tool" >/dev/null || die "$tool is not installed (rpmbuild is in rpm-build)"
done
git -C "$root" diff --quiet HEAD -- \
    || die "this checkout has uncommitted changes, and the package is named after its commit"

commit="$(git -C "$root" rev-parse --short=7 HEAD)"
commitdate="$(TZ=UTC git -C "$root" show -s --format=%cd --date=format-local:%Y%m%d HEAD)"
count="$(git -C "$root" rev-list --count HEAD)"
# The workspace's own version: the first `version =` in Cargo.toml, as
# release.yml bumps it.
base="$(sed -n '0,/^version *= *"\(.*\)"/s//\1/p' "$root/Cargo.toml" | tail -1)"
[[ "$base" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "Cargo.toml's version is '$base', not x.y.z"
if [[ $stage -eq 1 ]]; then
    name=solium-stage
    version="$base^$commitdate.$count.git$commit"
else
    name=solium
    version="$base"
fi

topdir="$out/rpmbuild"
rm -rf "$topdir"
mkdir -p "$topdir/SOURCES" "$topdir/SPECS"
git -C "$root" archive --format=tar.gz --prefix="solium-$version/" \
    -o "$topdir/SOURCES/solium-$version.tar.gz" HEAD

# The crates, and the lines that make cargo read them instead of crates.io,
# which the spec appends to .cargo/config.toml.
vendor="$out/vendor-work"
rm -rf "$vendor"
mkdir -p "$vendor"
# Not --quiet: that drops the configuration from stdout as well as the
# progress from stderr, and the package then looks for crates.io offline.
(cd "$root" && cargo vendor --locked "$vendor/vendor") >"$vendor/vendor-config.toml" \
    2>"$out/vendor.log" || { cat "$out/vendor.log" >&2; die "cargo vendor failed"; }
grep -q '^replace-with = "vendored-sources"$' "$vendor/vendor-config.toml" \
    || die "cargo vendor printed no source replacement (see $out/vendor.log)"
# It names the directory it filled, an absolute path on this machine; the spec
# unpacks it as vendor/ in the source tree.
sed -i 's|^directory = .*$|directory = "vendor"|' "$vendor/vendor-config.toml"
grep -q '^directory = "vendor"$' "$vendor/vendor-config.toml" \
    || die "the vendor configuration names no directory"
tar -C "$vendor" -czf "$topdir/SOURCES/solium-vendor.tar.gz" vendor vendor-config.toml
rm -rf "$vendor"

spec="$topdir/SPECS/solium.spec"
{
    echo "%global commit $commit"
    echo "%global commitdate $commitdate"
    echo "%global count $count"
    echo "%global base $base"
    sed -e "s/^%bcond stage 1$/%bcond stage $stage/" \
        -e 's/^%bcond vendored 0$/%bcond vendored 1/' \
        "$root/dev/rpm/solium.spec"
} >"$spec"
grep -q "^%bcond stage $stage$" "$spec" && grep -q '^%bcond vendored 1$' "$spec" \
    || die "could not choose the variant in the spec: its %bcond lines changed"

rpmbuild -bs --define "_topdir $topdir" "$spec" >"$out/srpm.log" 2>&1 \
    || { cat "$out/srpm.log" >&2; die "rpmbuild -bs failed"; }
srpm="$(ls "$topdir"/SRPMS/"$name"-*.src.rpm)"
mv "$srpm" "$out/"
# What was packaged, for the workflow that names a release after it (#491).
printf '%s\n' "name=$name" "version=$version" "base=$base" "count=$count" \
    "commit=$commit" >"$out/package.env"
echo "$out/${srpm##*/}"
