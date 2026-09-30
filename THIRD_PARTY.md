# Third-party code, behaviour and dependencies

Solium is published under the [GNU General Public License, version 3
only](LICENSE). This file lists what in it comes from somewhere else: code
adapted from another project, behaviour modelled on another project without
its code, and the licences of what Solium links.

## Code adapted from other projects

| Where | From | Licence |
|---|---|---|
| `fit` in `crates/layout/src/scroller.rs` | niri's `compute_new_view_offset`, `src/layout/scrolling.rs` at [niri-wm/niri@97c96a1](https://github.com/niri-wm/niri/blob/97c96a13829ec74c83e976c2ea34e7cc717b02ec/src/layout/scrolling.rs), © the niri contributors | GPL-3.0-or-later |

GPL-3.0-or-later code may be used in a GPL-3.0-only work, and the file's own
header says which passage it is.

## Behaviour modelled on other projects, with no code from them

These reimplement how something behaves. The behaviour is the specification;
no code was copied beyond the passage listed above, and so no licence of theirs
applies here. They are credited because the ideas are theirs.

| Where | Behaviour of | Their licence |
|---|---|---|
| `crates/solium/lua/scrolling.lua`, and the rest of `crates/layout/src/scroller.rs` | [niri](https://github.com/niri-wm/niri)'s scrolling layout: columns, and a view anchored to the focused one | GPL-3.0-or-later |
| `crates/solium/lua/tiling.lua`, `crates/layout/src/tree.rs` | [Hyprland](https://github.com/hyprwm/Hyprland)'s dwindle layout (`CDwindleAlgorithm`) | BSD-3-Clause |

Other mentions of niri, Hyprland, Quickshell and sway in the source are
comparisons ("what niri does") or links to how they behave, not code.

## Rust dependencies

Everything in `Cargo.lock`, summarised by licence. The compositor's own
foundation is [Smithay](https://github.com/Smithay/smithay) (MIT), and the
Wayland, event-loop and hardware crates it builds on (the `wayland-*` crates,
`calloop`, `drm`, `gbm`, `input`, `libseat`, `udev`, `xkbcommon`, `xcursor`)
are all MIT too. Lua comes from `mlua`
(MIT), which compiles the Lua sources vendored by `lua-src` (MIT). The session
bus is spoken through `zbus` and the `zvariant` crates (MIT), on the `async-io`
family of crates (MIT OR Apache-2.0). The signals that end a session are caught
through `signal-hook-registry` (MIT OR Apache-2.0), which that family already
uses.

| Licence | Crates |
|---|---|
| MIT OR Apache-2.0 (in any of its spellings) | 161 |
| MIT | 92 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT (`rustix`, `linux-raw-sys`, `io-lifetimes`, `wasip2`, `wit-bindgen`) | 8 |
| Apache-2.0 (`winit`, `cgmath`, `approx`, `gl_generator`, `khronos_api`, `gethostname`) | 6 |
| Other choices that include MIT: with Unlicense, Zlib, BSD-2-Clause or BSD-3-Clause | 15 |
| ISC (`libloading`) | 1 |
| `(Apache-2.0 OR MIT) AND BSD-3-Clause` (`encoding_rs`) | 1 |
| `(MIT OR Apache-2.0) AND Unicode-3.0` (`unicode-ident`) | 1 |
| `Apache-2.0 AND MIT` (`dpi`) | 1 |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later (`r-efi`, two versions) | 2 |

That is all 288 crates the lock file names. None is GPL- or AGPL-only, and
none has an unknown licence. The one entry that mentions a GPL-family licence,
`r-efi`, offers MIT and Apache-2.0 as alternatives, and is a dependency of
`getrandom` only on UEFI targets, so no Linux build compiles it.

`dev/wirecheck` has its own lock file. Its 109 crates are a subset of the same
families, with the same single `r-efi` entry.

To check this again after a dependency change:

```sh
cargo metadata --format-version 1 --locked \
  | jq -r '.packages[] | select(.source != null) | .license // "UNKNOWN"' \
  | sort | uniq -c | sort -rn
```

## System libraries

Linked at build time and found on the system, never bundled: Qt 6 (Core, Gui,
Qml, Quick; LGPL-3.0, or GPL), Wayland (MIT), libinput (MIT), libseat (MIT),
libxkbcommon (MIT), Mesa's GBM and EGL (MIT), libdrm (MIT) and systemd's
libudev (LGPL-2.1-or-later). Each is used through its
public interface and is compatible with GPL-3.0.
