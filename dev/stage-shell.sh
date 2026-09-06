#!/usr/bin/env bash
#
# Stage the Lilium shell's QML so Solium's QML engine can import it.
#
# Quickshell synthesises a `qmldir` for every directory of the shell at load
# time, which is why the shell ships none and why a plain QQmlEngine cannot
# import `qs.services`. This does the same thing ahead of time, into a tree of
# symlinks, so the shell's own repository is left untouched.
#
#   dev/stage-shell.sh ~/personal_projects/lilium-shell  build/shell
#
# Then put the staged tree's *parent* on SOLIUM_QML_PATH, so `import qs.dock`
# resolves to <staged>/qs/dock.
set -euo pipefail

source_root="${1:?usage: stage-shell.sh <shell-root> <staging-dir>}"
staging="${2:?usage: stage-shell.sh <shell-root> <staging-dir>}"
source_root="$(cd "$source_root" && pwd)"

rm -rf "$staging"
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

    module="qs${relative:+.${relative//\//.}}"
    {
        echo "module $module"
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
