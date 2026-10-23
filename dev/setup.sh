#!/usr/bin/env bash
#
# Run once after cloning. It does what CONTRIBUTING.md asks before a first
# change, and says what is missing when it cannot do it itself:
#
#   dev/setup.sh              check, build the image if it is needed, install
#                             the hooks
#   dev/setup.sh --no-image   the same, but only name the image it would build
#
# 1. Rust, through rustup: the toolchain rust-toolchain.toml names, with
#    rustfmt and clippy, which dev/gate.sh runs. rustup installs them.
# 2. The system packages the README's "Building" section lists, asked of
#    pkg-config. With all of them, cargo builds on this machine and dev/gate.sh
#    runs as it is. Without them, on Fedora with podman, it builds the image
#    dev/Containerfile describes for this Fedora release, and the gate builds
#    in it. Anywhere else it says what to install: an image of Fedora builds a
#    binary that links Fedora's libraries, which does not run elsewhere.
# 3. The git hooks in dev/hooks, copied into this clone's hooks directory. A
#    hook of the same name that is already there and differs is left alone,
#    and named.
#
# It never uses sudo and changes nothing outside this checkout but the rustup
# toolchain and the podman image. It can be run again at any time, and exits
# non-zero while something is still missing.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
build_image=1
for arg in "$@"; do
    case "$arg" in
        --no-image) build_image=0 ;;
        -h | --help)
            awk 'NR > 2 && /^#/ { sub(/^# ?/, ""); print; next } NR > 2 { exit }' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        *)
            echo "setup: unknown argument $arg (see dev/setup.sh --help)" >&2
            exit 2
            ;;
    esac
done

missing=0
ok() { printf '  ok       %s\n' "$*"; }
note() { printf '  note     %s\n' "$*"; }
lack() {
    printf '  MISSING  %s\n' "$*"
    missing=1
}

echo "Rust"
rustup=0
if command -v rustup >/dev/null 2>&1; then
    rustup=1
    # With no argument, rustup installs the toolchain this checkout's
    # rust-toolchain.toml names, with its components, when it is not there.
    # An older rustup wants a name; the first cargo command then installs it.
    if (cd "$root" && rustup toolchain install >/dev/null 2>&1) || (cd "$root" && cargo --version >/dev/null 2>&1); then
        ok "$(cd "$root" && rustc --version)"
    else
        lack "rustup could not install the toolchain rust-toolchain.toml names: run 'rustup toolchain install' here to see why"
    fi
elif command -v cargo >/dev/null 2>&1; then
    note "$(rustc --version) without rustup: rust-toolchain.toml is ignored, and the build image cannot use it"
else
    lack "Rust: install rustup from https://rustup.rs, then run this again"
fi
if command -v cargo >/dev/null 2>&1; then
    for tool in fmt clippy; do
        if (cd "$root" && cargo "$tool" --version >/dev/null 2>&1); then
            ok "cargo $tool"
        else
            lack "cargo $tool (with rustup: rustup component add ${tool/fmt/rustfmt})"
        fi
    done
fi

echo "System packages"
# What the README's "Building" section installs, by pkg-config name.
modules=(wayland-server libinput libudev libseat xkbcommon gbm egl libdrm "Qt6Quick >= 6.5" Qt6Qml)
absent=()
if command -v pkg-config >/dev/null 2>&1; then
    for module in "${modules[@]}"; do
        pkg-config --exists "$module" || absent+=("$module")
    done
else
    absent=("${modules[@]}")
fi
command -v c++ >/dev/null 2>&1 || absent+=("a C++17 compiler (c++)")
# solium-auth links PAM's runtime library by name (#281), and the runtime
# package has no pkg-config file, so the dynamic linker's cache is asked.
# Read whole first: under pipefail, grep -q leaving early fails the pipe.
if ! grep -q 'libpam\.so\.0 ' <<<"$(PATH="$PATH:/sbin:/usr/sbin" ldconfig -p 2>/dev/null)"; then
    absent+=("libpam.so.0 (PAM's runtime library)")
fi
if command -v pkg-config >/dev/null 2>&1 && pkg-config --exists Qt6Gui; then
    qt_include_dir="$(pkg-config --variable=includedir Qt6Gui)"
    qt_base_version="$(pkg-config --modversion Qt6Gui)"
    if [[ ! -f "$qt_include_dir/QtGui/$qt_base_version/QtGui/private/qpointingdevice_p.h" ]]; then
        absent+=("QtBase $qt_base_version private development headers")
    fi
fi

gate="dev/gate.sh"
if [[ ${#absent[@]} -eq 0 ]]; then
    ok "every package the build links, so it builds on this machine"
else
    note "not installed here: ${absent[*]}"
    os_id=""
    os_version=""
    if [[ -r /etc/os-release ]]; then
        # shellcheck disable=SC1091
        os_id="$(. /etc/os-release && echo "${ID:-}")"
        # shellcheck disable=SC1091
        os_version="$(. /etc/os-release && echo "${VERSION_ID:-}")"
    fi
    if [[ "$os_id" == fedora ]] && command -v podman >/dev/null 2>&1; then
        image="localhost/solium-build:fc$os_version"
        gate="SOLIUM_GATE_IMAGE=$image dev/gate.sh"
        build=(podman build -t "solium-build:fc$os_version" --build-arg "FEDORA_VERSION=$os_version"
            -f "$root/dev/Containerfile" "$root/dev/")
        if podman image exists "$image"; then
            ok "the build image $image"
        elif [[ $build_image -eq 1 ]]; then
            echo "  building the image $image, which takes a few minutes the first time"
            if "${build[@]}"; then
                ok "the build image $image"
            else
                lack "the build image: '${build[*]}' failed above"
            fi
        else
            lack "the build image: ${build[*]}"
        fi
        if [[ $rustup -eq 0 ]]; then
            lack "rustup: the image takes Rust from your rustup install (https://rustup.rs)"
        fi
    else
        lack "the development packages: README.md, \"Building\", has the command for your distribution"
    fi
fi
if compgen -G "${XDG_CONFIG_HOME:-$HOME/.config}/solium/*.lua" >/dev/null; then
    note "${XDG_CONFIG_HOME:-$HOME/.config}/solium holds Lua of its own, so a native gate skips the scenario tests (dev/README.md, \"The gate\")"
fi

echo "Git hooks"
if git -C "$root" rev-parse --git-dir >/dev/null 2>&1; then
    hooks="$(git -C "$root" rev-parse --path-format=absolute --git-path hooks)"
    mkdir -p "$hooks"
    for hook in "$root"/dev/hooks/*; do
        name="$(basename "$hook")"
        if [[ -e "$hooks/$name" ]] && ! cmp -s "$hook" "$hooks/$name"; then
            note "$hooks/$name is a different hook, left alone; dev/hooks/$name is the one this project uses"
        else
            install -m 755 "$hook" "$hooks/$name"
            ok "$name"
        fi
    done
else
    note "not a git checkout, so no hooks"
fi

echo
if [[ $missing -ne 0 ]]; then
    echo "Something is missing (above). Fix it and run dev/setup.sh again."
    exit 1
fi
cat <<EOF
Ready. Next:
  check a change:  $gate
  run it nested:   ./target/debug/solium, once the gate has built it
  read:            CONTRIBUTING.md and dev/README.md, or AGENTS.md for an agent
EOF
