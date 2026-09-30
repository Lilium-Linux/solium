# The road to a public preview

**Written 2026-09-07. Revised 2026-09-18**, after the first day of using it
for real work rather than developing it, **and again 2026-09-30.** Where the
compositor stands, what has to be true before strangers run it, and the order
to do it in.

The first version gated on *protocols* and *platforms* — multi-monitor, HiDPI,
screencopy, session lock — and all of it is done. The 2026-09-18 revision found
instead that the three things Solium is actually *for* each had something in
them that made the experience worse than the architecture deserved. Most of
that is fixed now, and is listed under
[Done since this was written](#done-since-this-was-written). What blocks a
preview today is mostly where the compositor meets the rest of a user session.

## Where it stands

E1 to E5 of the [roadmap](roadmap.md) have landed, and E6 closed as "modes as
scripts": it boots on hardware, transforms and animates every window through
one engine, is scripted in Lua, has floating, tiling and scrolling layouts,
draws its own decorations from QML, and expresses every mode as a script over
the transform.

That last one was the bet. If overview had needed new Rust the architecture was
wrong and everything after would have been fighting it. It didn't — overview is
about a hundred lines of Lua.

It has run real applications: Firefox, LibreOffice, Steam, Konsole, Dolphin,
Okular, X11 clients through XWayland, with no protocol errors. Copy and paste
works in both directions across the X11 boundary. It is used to develop itself,
which is the only test that counts for a compositor.

The protocols it speaks, and every one it does not, are listed in
[gaps.md](gaps.md#what-is-there-today).

A shell — the bar, the dock, the launcher — is a project of its own. It runs
inside Solium, hosted in the compositor's QML engine as part of the
configuration, or beside it as an ordinary `wlr-layer-shell` client;
[shell-boundary.md](shell-boundary.md) draws the line.

## Call it a preview, not a beta

"Beta" tells people it is nearly ready for normal use. It is not, and a
compositor crash takes the session and whatever was unsaved in it. Spending
that trust once is enough to lose it.

A **public preview**, aimed at people who want to hack on a compositor rather
than people looking for a desktop, is honest and achievable. It also gets the
first-contact bugs sooner and smaller, from people who will not be angry about
them. Nobody but the author has run this yet, on one machine and one monitor
layout. First contact with other GPUs and drivers, laptop panels and other
people's configurations will produce a burst of bugs that no amount of local
testing predicts.

## What still blocks

**Blocking, by which of Solium's own ideas it breaks.** A preview exists to
show what a compositor is *for*. Solium's three claims are that its layouts
are scripts, that its chrome is QML and belongs to one design system, and that
you can change either while it runs. Two of the three still have something in
them that undermines the claim. The third is clear.

### The layouts are the pitch

| | why |
|---|---|
| fullscreen fires no event | a script cannot see that a window is fullscreen or maximised, so the next relayout — a window opening, a layer surface arriving — puts a fullscreen video back into its tile. No issue tracks it yet |
| [#150](https://github.com/Lilium-Linux/solium/issues/150) no keyboard navigation | no key moves focus or a window left, right, up or down, and none toggles floating or fullscreen: a tiling compositor you have to drive with the pointer |

### The chrome is the differentiator

| | why |
|---|---|
| [#102](https://github.com/Lilium-Linux/solium/issues/102) no decoration policy | a GTK dialog gets Solium's titlebar *and* keeps its own rounded body underneath. Two decorations on one window, and no way to say "this one draws its own". The cost #103 knowingly accepted, and this is what settles it |

### Changing it while it runs

Nothing blocks here any more. A reload keeps the session, `user.lua` can add a
binding, and `solium --check` says when a setting is read by nothing; see
[below](#done-since-this-was-written). "Edit a file, press a key, watch it
change" is the sentence a preview is sold on, and it is now true.

### Anywhere but here

| | why |
|---|---|
| [#146](https://github.com/Lilium-Linux/solium/issues/146) the session environment | nothing exports the display to systemd or D-Bus, so portals, XDG autostart and anything else the session starts on its own cannot find it — a shell that runs as its own program included |
| [#152](https://github.com/Lilium-Linux/solium/issues/152) D-Bus idle inhibit | the screens go off after ten minutes by default, and a browser's request to keep them on never reaches Solium, so a film can go dark |
| [#66](https://github.com/Lilium-Linux/solium/issues/66) packages | a preview nobody can install is a preview nobody tries. **Done:** an installed binary finds its own QML and Lua, and `dev/install.sh` installs from a checkout into `~/.local` on Fedora 44 and prints the one `sudo` line that puts the session file where the login screen reads it (`dev/install-check.sh` checks it). **Left:** packages — a Fedora `.spec` and COPR, an Arch `PKGBUILD` — plus `--config` ([#106](https://github.com/Lilium-Linux/solium/issues/106)) and the configuration directory's name ([#107](https://github.com/Lilium-Linux/solium/issues/107)), which are worth settling before the recipes are written |
| [#65](https://github.com/Lilium-Linux/solium/issues/65) a soak | the compositor has never been left running unattended for hours, with window churn, on a real session. `SOLIUM_SOAK_TTY=1 dev/soak.sh` can do it now; what is missing is the run. A preview that dies after six hours is worse than one that is missing a feature |
| [#64](https://github.com/Lilium-Linux/solium/issues/64) suspend and resume | never tried at all, which is the same sentence about laptops |
| screens off, on hardware | [#54](https://github.com/Lilium-Linux/solium/issues/54)'s DRM path — black, then a modeset back — was merged without a run on hardware |

### Done since this was written

| | |
|---|---|
| [#113](https://github.com/Lilium-Linux/solium/issues/113) resize | a resize is the rectangle you drag, tiled or floating: while an edge is held the pane's slot is the authority, not the client's size, so the edge you grab is the one that moves (with #120, #123 and #124) |
| [#116](https://github.com/Lilium-Linux/solium/issues/116), [#118](https://github.com/Lilium-Linux/solium/issues/118), [#129](https://github.com/Lilium-Linux/solium/issues/129) reload | `super+shift+r` keeps the session: workspaces, the mode in charge, and every tiling tree and scrolling strip, desks not in view included. See [modes.md](modes.md#what-survives-supershiftr) |
| [#117](https://github.com/Lilium-Linux/solium/issues/117) the configuration says what it does | `bindings = {}` in `user.lua` adds, replaces or removes a binding; every setting `config.lua` advertises is read; and `solium --check` names a setting nothing reads, suggests the one you meant, and exits 1 |
| [#115](https://github.com/Lilium-Linux/solium/issues/115), [#134](https://github.com/Lilium-Linux/solium/issues/134) sizes | layouts respect each application's minimum and maximum size; a tile has a configurable minimum, and a new window that does not fit goes to the next empty workspace |
| [#72](https://github.com/Lilium-Linux/solium/issues/72) modal dialogs | float over the window waiting on them, on its screen, X11 dialogs included |
| [#92](https://github.com/Lilium-Linux/solium/issues/92), [#141](https://github.com/Lilium-Linux/solium/issues/141), [#142](https://github.com/Lilium-Linux/solium/issues/142) fullscreen | leaving fullscreen puts a window back where it was, and a fullscreen window covers the bars and takes their clicks unless `fullscreen.covers = "none"` |
| [#126](https://github.com/Lilium-Linux/solium/issues/126), [#127](https://github.com/Lilium-Linux/solium/issues/127) closing | a window its own application closes fades out like one Solium closes, and a close that has started always finishes |
| [#121](https://github.com/Lilium-Linux/solium/issues/121), [#132](https://github.com/Lilium-Linux/solium/issues/132) keys | bindings fire under a non-Latin layout, and a shifted binding on a non-letter key, `super+shift+1` included, is reachable |
| [#149](https://github.com/Lilium-Linux/solium/issues/149) frame callbacks | layer-shell and lock surfaces are told when to draw again, so a bar or a locker keeps drawing |
| [#147](https://github.com/Lilium-Linux/solium/issues/147) QML on the GPU | the default on the hardware, once a trial render in a child process has passed; software otherwise, and in nested sessions |
| [#54](https://github.com/Lilium-Linux/solium/issues/54) screens off | `wlr-output-power-management`, `sol.monitor_power`, and the screens going off by themselves after `idle.screens_off_after` |
| the lock, hardened | only the lock surface can have the keyboard, and `locked` is sent only once every monitor shows the lock |
| [#53](https://github.com/Lilium-Linux/solium/issues/53) keyboard layout | in `config.keyboard`, with the repeat rate — which was the part that genuinely could not be changed. The layout could always be set through `XKB_DEFAULT_LAYOUT` |
| [#41](https://github.com/Lilium-Linux/solium/issues/41) multi-monitor | a pipeline per monitor, one global space, layouts and workspaces per screen |
| [#43](https://github.com/Lilium-Linux/solium/issues/43) hotplug | a monitor plugged in is picked up, one pulled out is let go, and its windows come back |
| [#39](https://github.com/Lilium-Linux/solium/issues/39) HiDPI | scale per monitor, chrome rasterised at it, chosen from the panel's dpi |
| [#28](https://github.com/Lilium-Linux/solium/issues/28) screen capture | `wlr-screencopy`, so grim and wf-recorder work. The portal has not been tested end to end ([#83](https://github.com/Lilium-Linux/solium/issues/83)), and cannot find the display until #146 |
| [#27](https://github.com/Lilium-Linux/solium/issues/27) session lock | `ext-session-lock-v1`, and it fails locked rather than open |
| [#33](https://github.com/Lilium-Linux/solium/issues/33) the leak | re-measured and not reproducible: `dev/leak.sh` over 40 windows in four settled cycles returns to within ±1 MB of baseline, with file descriptors two *below* it. The number in that issue's title should not be quoted until a soak on real hardware says otherwise |

**Shippable as documented gaps.** Real holes, but ones a preview can name and
survive: [#26](https://github.com/Lilium-Linux/solium/issues/26) IME,
[#52](https://github.com/Lilium-Linux/solium/issues/52) no clipboard manager,
[#83](https://github.com/Lilium-Linux/solium/issues/83) portals never tested
end to end, [#56](https://github.com/Lilium-Linux/solium/issues/56) window
rules, [#153](https://github.com/Lilium-Linux/solium/issues/153) logind lock
and sleep, [#157](https://github.com/Lilium-Linux/solium/issues/157) no
tap-to-click. Each should be named where a stranger will look before they hit
it, and [docs/gaps.md](gaps.md) is the full list.

**Not features, and do them anyway.** CI runs on Fedora 44 and gates `stage`
and `release`: fmt, clippy, the build, the tests and `solium --check`. It still
does not run the checks under `dev/`, and `dev/gate.sh` runs one CI does not,
the QML GPU check. Two gates testing different things is one gate.

## Order, and why

1. **The session: [#146](https://github.com/Lilium-Linux/solium/issues/146),
   then [#152](https://github.com/Lilium-Linux/solium/issues/152) and
   [#153](https://github.com/Lilium-Linux/solium/issues/153).** #146 first,
   because it decides whether anything the session starts on its own can find
   the display: portals, autostart, and a shell that runs as its own program.
   #152 matters as soon as the screens go off by themselves, which they now do
   by default.
2. **The layouts: the fullscreen yank, then
   [#150](https://github.com/Lilium-Linux/solium/issues/150).** The yank undoes
   the one thing somebody watching a video asked for; #150 is the difference
   between a tiling compositor and one you drive with the pointer.
3. **[#102](https://github.com/Lilium-Linux/solium/issues/102)**, the decoration
   policy.
4. **Soak and suspend on hardware,
   [#65](https://github.com/Lilium-Linux/solium/issues/65) and
   [#64](https://github.com/Lilium-Linux/solium/issues/64)**, with #54's DRM
   path tried alongside. These are the difference between working here and
   working anywhere.
5. **Packages, [#66](https://github.com/Lilium-Linux/solium/issues/66)**, once
   #106 and #107 are settled.
6. Then the P3s, after the preview is out.

Priorities on the tracker are by **whether an application can be used at all
without the thing**, not by effort. That is why `wp_alpha_modifier_v1`
([#75](https://github.com/Lilium-Linux/solium/issues/75)) is P3 — no
application stops working without it — and multi-monitor was P1.

### Notes from the work already done

- **[#41](https://github.com/Lilium-Linux/solium/issues/41) multi-monitor**
  was the biggest and the one everything else is easier after, which is why it
  went first. Left over as their own issues:
  [#42](https://github.com/Lilium-Linux/solium/issues/42) absolute devices,
  [#44](https://github.com/Lilium-Linux/solium/issues/44) matching a monitor by
  what it is rather than which port it is in,
  [#45](https://github.com/Lilium-Linux/solium/issues/45) mirroring,
  [#46](https://github.com/Lilium-Linux/solium/issues/46) 10-bit.
- **[#39](https://github.com/Lilium-Linux/solium/issues/39) HiDPI**: #41 was
  indeed most of it, because every call site already took an output or a
  screen rect. The part that was not plumbing was the QML host, which needed
  the *distinction* between logical layout and device rasterisation rather than
  a bigger canvas — a scene given the device size lays out in it and comes out
  half the size it should be.
- **[#28](https://github.com/Lilium-Linux/solium/issues/28) screencopy** was
  self-contained, as expected. Its successor, `ext-image-copy-capture-v1`, is
  [#47](https://github.com/Lilium-Linux/solium/issues/47): the
  `wayland-protocols` crate carries it, but Smithay 0.7 has no handler for it,
  so it would be written by hand, as `screencopy.rs` was.
- **[#27](https://github.com/Lilium-Linux/solium/issues/27) session lock**: the
  protocol was the easy half. The hard half was that "locked" has to mean
  something to every path that interprets input, and one of them asked none of
  the questions the others did. The resize path walked the panes itself
  rather than going through the hit test, so with the obvious guards in place a
  drag on a locked screen still resized a window nobody could see, and it was
  still that size after unlocking. Found by firing a scripted drag during a lock
  and measuring the window afterwards, which is the only reason it was found at
  all. There is one hit test now, so there is one guard.
- **[#36](https://github.com/Lilium-Linux/solium/issues/36) idle**: what the
  issue said would happen did. The visibility rule was written against where a
  window *lives*, and a workspace switch moves where it is *drawn*, so a video
  on another workspace went on holding the machine awake until a test hid one
  and waited.

The lessons that cost the most time on the way here are in
[CONTRIBUTING.md](../CONTRIBUTING.md#rules-learned-the-hard-way). The checks
and tools that exist, with what each one asserts, are in
[dev/README.md](../dev/README.md).
