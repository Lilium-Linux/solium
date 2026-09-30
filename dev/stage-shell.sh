#!/usr/bin/env bash
#
# Stage a Quickshell shell's QML so Solium's QML engine can import it.
#
# Quickshell synthesises a `qmldir` for every directory of the shell at load
# time, which is why a shell written for it ships none and why a plain
# QQmlEngine cannot import `qs.services`. This does the same thing ahead of
# time, into a copy, so the shell's own repository is left untouched.
#
#   dev/stage-shell.sh <shell-root> <staging-dir>
#   SOLIUM_SHELL=<shell-root> dev/stage-shell.sh <staging-dir>
#
# The copy is <staging-dir>/qs, and only that is replaced on each run, so the
# staging directory can be one that holds other things -- your own
# ~/.config/solium/qml, which is on the QML search path already, so that
# `import qs.dock` resolves to <staging-dir>/qs/dock with nothing else set.
set -euo pipefail

usage="usage: stage-shell.sh <shell-root> <staging-dir>, or SOLIUM_SHELL=<shell-root> stage-shell.sh <staging-dir>"
if [[ $# -ge 2 ]]; then
    source_root="$1"
    staging="$2"
elif [[ $# -eq 1 && -n "${SOLIUM_SHELL:-}" ]]; then
    source_root="$SOLIUM_SHELL"
    staging="$1"
else
    echo "$usage" >&2
    exit 2
fi
[[ -d "$source_root" ]] || { echo "no such shell directory: $source_root" >&2; exit 1; }
source_root="$(realpath "$source_root")"
staging="$(realpath -m "$staging")"

# Staging inside the shell would copy the copy on the next run.
case "$staging/" in
    "$source_root"/*)
        echo "refusing to stage inside the shell itself: $staging" >&2
        exit 1
        ;;
esac
# And a shell checked out at or under <staging-dir>/qs is what the next lines
# delete, uncommitted work and all.
case "$source_root/" in
    "$staging/qs/"*)
        echo "refusing to stage over the shell's own checkout: $source_root is under $staging/qs" >&2
        exit 1
        ;;
esac

mkdir -p "$staging"
rm -rf "$staging/qs"
mkdir -p "$staging/qs"

# Every directory holding QML becomes a module. A file with `pragma Singleton`
# is registered as one -- getting that wrong is the difference between a
# working singleton and "is not a type".
find "$source_root" -type d -not -path '*/.git*' -not -path '*/build*' | while read -r directory; do
    relative="${directory#"$source_root"}"
    relative="${relative#/}"
    target="$staging/qs${relative:+/$relative}"
    mkdir -p "$target"

    shopt -s nullglob
    files=("$directory"/*.qml)
    shopt -u nullglob
    [[ ${#files[@]} -eq 0 ]] && continue

    # Deliberately no `module` line.
    #
    # A qmldir that declares a module turns the directory into one, and QML's
    # implicit directory import — where a file sees its siblings without
    # importing anything — is switched off for it. The shell's files rely on
    # that: none of the twenty configuration files imports `qs.config`, they
    # just name each other. Declaring the module made every one of them unable
    # to see the next, which reads as a dependency chain and is nothing of the
    # kind.
    #
    # Without the line the directory still answers to `import qs.config`,
    # because that resolves by path, and siblings resolve as they always did.
    {
        for file in "${files[@]}"; do
            name="$(basename "$file" .qml)"
            # A type name must start with a capital; anything else is a plain
            # component file the shell loads by path, not a module type.
            [[ "$name" =~ ^[A-Z] ]] || continue
            if grep -q '^pragma Singleton' "$file"; then
                echo "singleton $name 1.0 $name.qml"
            else
                echo "$name 1.0 $name.qml"
            fi
            # Copied, not linked. Qt canonicalises paths, so a symlinked file
            # resolves its own `import qs.config` relative to where it really
            # lives -- back in the shell's tree, where there is no qmldir and
            # every type is therefore unavailable.
            cp -f "$file" "$target/$name.qml"
        done
    } > "$target/qmldir"
done

echo "staged $(find "$staging" -name qmldir | wc -l) modules under $staging"
