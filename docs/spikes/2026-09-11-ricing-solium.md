# Ricing Solium, from the outside

**Date:** 2026-09-11
**Status:** field report, after building a complete rice. Branch: `rice/prism`.
Findings filed as [#88](https://github.com/Lilium-Linux/solium/issues/88).

## Why

Everything written down about ricing this compositor was written by the people
who built it, which is the one perspective that cannot see what is missing.
`docs/ricing.md` says a rice is files you write and no Rust; `docs/architecture.md`
says that if a new mode needs new Rust, the transform layer is missing
something, "and that is the test".

So the test was run, adversarially and for real: a whole desktop — a bar, a
dock, a window frame, a wallpaper and a *new mode* — written only in QML and
Lua, by someone who had not seen the codebase before that morning. The mode
was chosen to be awkward on purpose. A cover flow needs perspective, which a
grid of thumbnails does not, so it exercises the part of the transform layer
that overview never touches.

The result is `rice/prism`: about 1,700 lines, nothing outside `rice/`
changed, and no Rust file opened except to read it.

## The short version

The ceiling is exceptional and the floor is rough.

**What the compositor can be made to do is close to best in class.** What it
does when you get something wrong is close to worst. Roughly as much time went
into finding four silent failures as into writing the rice, and every one of
them would have been a ten-second fix if the software had said anything at
all.

That is not four problems. It is one problem with four faces, and it has a
one-call fix. See "The root cause" below.

## What worked

**The central claim holds.** `sol.present` already took `rotate_y` and
`perspective`; the animation clock already interpolated toward whatever target
was set. The deck is about 130 lines of Lua and needed nothing added to the
compositor. The bet in `architecture.md` is not a claim that survived contact
with a friendly example — it survived one picked to break it.

**Hit-testing following the transform is the detail that gives the design
away.** A card turned fifty-two degrees, scaled down and half transparent is
clickable where it is *drawn*, and `deck.lua` never learns that is what it is
looking at. The alternative — every mode hit-testing against its own idea of
where things are — is the kind of duplication that works for two modes and
drifts by the fourth.

**Module shadowing is at the right granularity, and this is the best thing
that happened all day.** The bar and the dock needed to be kept clear of, and a
scripted surface reserves nothing from the work area. Replacing one 111-line
file — `monitors.lua`, where every mode reads its area — made tiling,
scrolling *and the shipped overview* inset correctly. Behaviour was inherited
from code that was written before the feature existed and was never touched.
Insetting in each layout instead would have been the same ten lines four times,
and the fifth layout would have got it wrong.

**`sol.surface` is one primitive doing five jobs** — wallpaper, bar, dock,
deck scrim, deck caption, all the same call with a different layer. The
compositor genuinely does not know what a wallpaper is, and that is the right
answer.

**Hot reload works, including the part that is easy to get wrong.** `super+shift+r`
picks up QML and Lua together. The cache-clearing note in `host.cpp` shows the
subtle version of this bug — where reloading *looks* like it worked — was
already found and fixed.

**The comments are the best documentation in the repository.** `wallpaper.lua`
explaining that this used to be a `Command::Wallpaper` and a hundred lines of
Rust teaches the design philosophy faster than the architecture document does,
because it shows the alternative being rejected.

## What did not

### The root cause: nothing is ever said out loud

There is no `qInstallMessageHandler` in `host.cpp`, so Qt's entire diagnostic
channel — QML binding errors, type warnings, `console.log`, `console.warn` —
goes nowhere. The compositor's log stays clean while the desktop is white.

Everything below inherits this. Three of the four present as "white screen" or
"nothing is there", and none of them logs a line:

| what is wrong | what you see |
|---|---|
| user QML directory searched last | theme ignored, properties undefined, undefined assigned to a colour is transparent — **white screen** |
| `Canvas` never paints | an unpainted canvas is **white**, not empty |
| `toplevels` is `{ values: [...] }` | a `Repeater` bound at it gets a map: not empty, not iterable, **silently no items** |
| `SOLIUM_SHELL_SCENE` unset | window list empty everywhere, **no warning** |

With no console, the only way to ask the scene a question is to make it paint
the answer. Diagnosing `Canvas` meant painting six coloured bands, one after
each canvas call, and counting which appeared — about twenty-five seconds per
hypothesis, for something a warning would have answered instantly.

### The documented override path is the broken one

`qml.rs` builds `user:own:compat`; `host.cpp` walks it calling
`QQmlEngine::addImportPath`, which *prepends*. The list ends up reversed, so a
`Solium/Theme.qml` in `~/.config/solium/qml` loses to the shipped one —
exactly the opposite of what `ricing.md` promises, and the single most
advertised customisation in it.

### `sol.present` is missing half of what the architecture document says it has

`architecture.md` says every window carries "a target rect, opacity, corner
radius and z-order". Corner radius and z-order appear nowhere in the source.

The z-order half is not cosmetic. Depth ordering is what makes a cover flow a
deck rather than a fan, and without it the cards had to be spaced so they never
overlap — designing around the API instead of saying what was meant. Any
app-switcher, card stack or peek hits this on the first afternoon. `sol.focus`
restacks, but using focus to express depth means moving the keyboard to say
something about drawing.

The document should either stop describing these as present, or they should be.

### Scripted surfaces are write-once

`properties` is set when the scene is built, so a script can never tell a
running surface anything; changing one means destroying and rebuilding it. The
deck's caption works only because the selection could be smuggled through focus
and read back out of the window list. That was a pleasant thing to discover and
a bad thing to have to rely on. `sol.surface_set(name, key, value)` would close
it.

### An interactive surface swallows every press inside its rectangle

`surface.rs` returns `true` for any point within the surface, whether or not
the QML under the pointer wanted it. The scene already knows whether a
`MouseArea` accepted; the answer is simply not propagated. The consequence is
that the dock has to be sized to its contents and re-placed on every window
open and close, because a full-width strip would look identical and make the
bottom of the screen unclickable.

### A decoration can ask for exactly two things

`Action::parse` takes `close` and `maximize`. There is no minimise, and no way
to name anything of one's own. A dock and the `genie` transform that already
exists are an obvious pairing, and a frame has no way to ask for it.

### The build does not link on Fedora

`--as-needed` drops Qt's shared libraries before the static QML shim's
undefined symbols are seen, because `build.rs` emits the pkg-config `-l` flags
before `cc` emits the archive. This matters more than an ordinary packaging
nit: `dev/Containerfile` builds on Fedora, and its own comment says matching
the host distribution is deliberate. It is the documented build path.

## What to fix, in order

1. **Route Qt's message handler into `tracing`.** One call in `host.cpp`. It
   would have saved most of a day here and it saves every future ricer the same
   day. Nothing else on this list comes close for leverage.
2. **Walk the import path in reverse.** One line, and it unbreaks the
   customisation the ricing guide leads with.
3. **Expose `z` in `sol.present`.** It unblocks a class of modes rather than
   one mode.
4. **Let `surface.pointer` report whether the scene consumed the press.**

## Verdict

What is on offer here is different in kind from the neighbours, not in degree.
In Hyprland or niri one can set `blur = true`; one cannot invent cover flow.
Here a mode was invented and the compositor never learned it exists. That is
the thing worth protecting, and the architecture that makes it possible is the
part that was hard and is already right.

The price today is that ricing means debugging an embedded QML engine with no
console. The distance between "excellent if you are stubborn" and "the most
powerful ricing surface on Wayland" is smaller than it looks from inside the
white screen. Most of it is one message handler.
