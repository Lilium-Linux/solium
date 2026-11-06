#!/usr/bin/env python3
"""Record Solium's showcase: every picture and video the README and the book's
Showcase page show, from the shots in dev/showcase/shots/.

    dev/showcase/record.py                 # every shot
    dev/showcase/record.py alt-tab glass   # these
    dev/showcase/record.py --list

Each shot is a TOML file: what to put on top of the shipped default
configuration, which applications to open, a timeline of keys, pointer
journeys and other events, the stretch of time to capture and what to make
of it. This script runs Solium nested with all of it, captures the burst
(`SOLIUM_CAPTURE_FRAMES`), and writes the results to docs/media/: an MP4 at
true speed, an animated WebP for the README, a poster, a slow copy, a
filmstrip, as the shot asks. Every frame is the compositor's own; nothing is
drawn on one afterwards, and a grid of variants carries its labels in a
margin of its own.

What a run checks, and says at the end: frames that came late (a burst's
frames are as far apart as the passes that drew them, and each keeps its
time, so a video plays at true speed), frames that are black, a loop that
does not end where it began, and which shots now differ from the media in
the repository. dev/showcase/README.md has the shot format.

Needs a host Wayland session (WAYLAND_DISPLAY), a release build
(`cargo build --release`), ffmpeg with libx264 and libwebp, Python's Pillow,
dbus-python, kitty and Firefox. One run at a time.
"""

import argparse
import dataclasses
import math
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
import tomllib
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFont, ImageStat

ROOT = Path(__file__).resolve().parents[2]
# The home this was started from: podman keeps its images under it, so the
# build image `dev/foot` runs foot in is found only from there.
REAL_HOME = os.environ.get("HOME", str(Path.home()))
# And the session bus it was started on: podman asks systemd through it.
REAL_BUS = os.environ.get("DBUS_SESSION_BUS_ADDRESS", "")
HERE = ROOT / "dev" / "showcase"
SHOTS = HERE / "shots"
STAGE = HERE / "stage"
MEDIA = ROOT / "docs" / "media"
WORK = ROOT / "target" / "showcase"
DEFAULT_CONFIG = ROOT / "crates" / "solium" / "default-config"

# Media no page shows (a grid's cells, a still a grid is made from) is
# written here, not into the repository.
CELLS = WORK / "cells"

FRAME_MS = 16
# The compositor's clock runs at this rate during a shot (SOLIUM_CLOCK_RATE):
# a frame is captured every FRAME_MS of the clock, which at a quarter is 64 ms
# of real time, room for a pass that captures however heavy it is. The
# timeline is on the clock; what this script does in real time is scaled.
RATE = 0.25
# A frame more than this far after the one before it, on the clock, is
# reported: at a quarter a host frame is about 4 ms of the clock, so a frame
# is 16 to 21 ms after the last, and one 26 ms after means several passes
# went by without one.
LATE_MS = 26
# How different two frames may be, as an RMS over a small grey copy, and
# still count as the same picture: a loop's two ends, or a poster against
# the committed one. The bar's clock and a terminal's cursor move a little.
SAME_RMS = 3.0

# Keys for characters, as SOLIUM_KEY_AT names them, on a `us` layout.
SHIFTED = dict(zip('~!@#$%^&*()_+{}|:"<>?', [
    "grave", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "minus",
    "equal", "bracketleft", "bracketright", "backslash", "semicolon",
    "apostrophe", "comma", "period", "slash"]))
PLAIN = {"`": "grave", "-": "minus", "=": "equal", "[": "bracketleft",
         "]": "bracketright", "\\": "backslash", ";": "semicolon",
         "'": "apostrophe", ",": "comma", ".": "period", "/": "slash",
         " ": "space", "\n": "return", "\t": "tab"}


def key_for(char):
    if char in PLAIN:
        return PLAIN[char]
    if char in SHIFTED:
        return "shift+" + SHIFTED[char]
    if char.isalpha() and char.isupper():
        return "shift+" + char.lower()
    if char.isalnum():
        return char
    raise SystemExit(f"no key types {char!r} on a us layout")


def point(value):
    """`"x,y"` or `[x, y]` as a pair of floats."""
    if isinstance(value, str):
        x, y = value.split(",")
        return float(x), float(y)
    return float(value[0]), float(value[1])


def fmt(p):
    return f"{p[0]:g},{p[1]:g}"


def in_out_cubic(t):
    # crates/animation's Curve::InOutCubic, which synth::Journey follows.
    return 4 * t * t * t if t < 0.5 else 1 - (-2 * t + 2) ** 3 / 2


@dataclasses.dataclass
class Journey:
    start: float
    over: float
    origin: tuple | None
    to: tuple


class Shot:
    """One file of dev/showcase/shots/, read and checked."""

    def __init__(self, path):
        self.path = path
        self.name = path.stem
        with path.open("rb") as file:
            data = tomllib.load(file)
        self.data = data
        self.title = data.get("title", self.name)
        self.stage = data.get("stage", {})
        self.capture = data.get("capture", {})
        self.out = data.get("out", {})
        self.events = sorted(data.get("at", []), key=lambda e: e["ms"])
        self.grid = data.get("grid")
        if not self.grid and not self.capture:
            raise SystemExit(f"{path}: a shot needs [capture] (or [grid])")

    # ---- the timeline, as Solium's knobs and this script's own events ----

    def knobs(self):
        """The nested knobs this timeline sets, and the journeys the pointer
        takes (for a crop that follows it)."""
        keys, triggers, moves, drags, outputs, loading = [], [], [], [], [], []
        journeys = []
        for event in self.events:
            ms = int(event["ms"])
            if "key" in event:
                every = int(event.get("every", 70))
                for index in range(int(event.get("times", 1))):
                    keys.append(f"{ms + index * every}:{event['key']}")
            if "type" in event:
                every = int(event.get("every", 70))
                for index, char in enumerate(event["type"]):
                    keys.append(f"{ms + index * every}:{key_for(char)}")
            if "trigger" in event:
                triggers.append(f"{ms}:{event['trigger']}")
            for kind, out in (("move", moves), ("drag", drags), ("click", drags)):
                if kind not in event:
                    continue
                to = point(event[kind])
                origin = point(event["from"]) if "from" in event else None
                if kind == "click":
                    origin, over = to, 0
                else:
                    over = int(event.get("over", 0))
                held = f"{event['hold']}@" if "hold" in event else ""
                start = fmt(origin) if origin else ""
                tail = f"~{over}" if over else ""
                if "button" in event:
                    tail += f"#{event['button']}"
                out.append(f"{ms}:{held}{start}>{fmt(to)}{tail}")
                journeys.append(Journey(ms, over, origin, to))
            if "outputs" in event:
                outputs.append(f"{ms}:{int(event['outputs'])}")
            if "loading" in event:
                loading.append(f"{ms}:{event['loading']}")
        env = {}
        for name, entries, separator in (
            ("SOLIUM_KEY_AT", keys, ","),
            ("SOLIUM_TRIGGER_AT", triggers, ","),
            ("SOLIUM_MOVE_AT", moves, ";"),
            ("SOLIUM_DRAG_AT", drags, ";"),
            ("SOLIUM_OUTPUTS_AT", outputs, ","),
            ("SOLIUM_LOADING_AT", loading, ","),
        ):
            if entries:
                env[name] = separator.join(entries)
        return env, journeys

    def pointer_at(self, journeys, ms, start=(0.0, 0.0)):
        """Where the pointer is at `ms`, from the journeys before it."""
        at = start
        for journey in journeys:
            if journey.start > ms:
                break
            origin = journey.origin or at
            if journey.over <= 0:
                at = journey.to
                continue
            t = min(1.0, (ms - journey.start) / journey.over)
            eased = in_out_cubic(t)
            at = (origin[0] + (journey.to[0] - origin[0]) * eased,
                  origin[1] + (journey.to[1] - origin[1]) * eased)
        return at


# ---- the stage: configuration, home, bus, applications ----


def stage_config(shot, work):
    """The shipped default configuration, untouched, in the scratch home, and
    the shot's own Lua as an init file that runs before the shipped
    `init.lua`: `user.lua` is wrapped as it is required, never edited, so a
    window showing it shows the file that ships."""
    # The scratch home's own .config, so a path the configuration writes as
    # "~/.config/solium/..." (the pointer's scene) is this one too.
    config = work / "home" / ".config"
    solium = config / "solium"
    shutil.copytree(DEFAULT_CONFIG, solium)
    for entry in shot.stage.get("files", []):
        source, dest = entry.split(":", 1)
        source = ROOT / source
        target = solium / dest
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.is_dir():
            shutil.copytree(source, target, dirs_exist_ok=True)
        else:
            shutil.copy2(source, target)
    if shot.stage.get("bare"):
        shutil.rmtree(solium)
        solium.mkdir()

    patch = shot.stage.get("patch", "")
    scale = shot.stage.get("scale")
    if scale:
        patch = f"config.monitors = {{ {{ name = \"winit-1\", scale = {scale} }} }}\n" + patch
    lines = ["-- One showcase shot's changes (dev/showcase/record.py), on top of the",
             "-- shipped init.lua and the configuration, which are left as they ship."]
    if patch:
        user = solium / "user.lua"
        lines += ["package.preload[\"user\"] = function()",
                  f"    local config = {('dofile(' + repr(str(user)).replace(chr(39), chr(34)) + ')') if user.exists() else '{}'}",
                  *("    " + line for line in patch.rstrip().splitlines()),
                  "    return config",
                  "end"]
    places = shot.stage.get("places", {})
    if places:
        # After `open` has been heard by everyone, floating.lua's own
        # placement included, so this one is the last word; and a window's
        # app id can be empty until its application has said it, so look
        # again each frame for two seconds.
        lines += ["local places = {",
                  *(f'    ["{app}"] = {{ x = {x}, y = {y}, w = {w}, h = {h}, tile = false }},'
                    for app, (x, y, w, h) in places.items()),
                  "}",
                  "local function place(id, tries)",
                  "    for _, w in ipairs(sol.windows()) do",
                  "        if w.id == id then",
                  "            local rect = places[w.app_id] or places[w.title]",
                  "            if rect then return sol.place(id, rect) end",
                  "            if w.app_id == \"\" and tries > 0 then",
                  "                sol.after(16, function() place(id, tries - 1) end)",
                  "            end",
                  "        end",
                  "    end",
                  "end",
                  "sol.on(\"open\", function(id) sol.after(0, function() place(id, 120) end) end)"]
    shipped = ROOT / "crates" / "solium" / "lua" / "init.lua"
    lines.append(f'dofile("{shipped}")')
    # In a directory of its own: Solium watches the directory of the file it
    # was given to load and reloads when anything there changes, and the
    # shot's logs and frames are written beside the rest of `work`.
    init = work / "init" / "showcase-init.lua"
    init.parent.mkdir()
    init.write_text("\n".join(lines) + "\n")
    return config, init


def stage_home(work):
    home = work / "home"
    shutil.copytree(STAGE / "home", home)
    # Where an install puts Solium's data, which a scene may look for: the
    # default configuration's lock finds the wallpaper there.
    data = home / ".local" / "share" / "solium" / "qml" / "wallpaper"
    data.mkdir(parents=True)
    shutil.copy2(ROOT / "crates" / "solium" / "qml" / "wallpaper" / "solium.png", data)
    # A window's icon is looked up by its app id in the icon theme: each
    # kitty of the stage has kitty's own, under its own id.
    kitty = Path("/usr/share/icons/hicolor/scalable/apps/kitty.svg")
    if kitty.exists():
        icons = home / ".local" / "share" / "icons" / "hicolor" / "scalable" / "apps"
        icons.mkdir(parents=True)
        for app_id, _, _ in KITTY.values():
            shutil.copy2(kitty, icons / f"{app_id}.svg")
    profile = home / "firefox"
    profile.mkdir()
    shutil.copy2(STAGE / "firefox" / "user.js", profile / "user.js")
    return home


def start_bus(work):
    """A private session bus: notifications, the tray, media players and
    the stand-in logind all live on it, and nothing reaches the host's.
    Nothing is activated on it either (stage/bus.conf says why)."""
    process = subprocess.Popen(
        ["dbus-daemon", f"--config-file={STAGE / 'bus.conf'}", "--nofork", "--print-address=1"],
        stdout=subprocess.PIPE, stderr=(work / "bus.log").open("w"), text=True)
    address = process.stdout.readline().strip()
    if not address:
        raise SystemExit("the private bus did not start")
    return process, address


# The applications a shot opens: kitty's with a class and a title of their
# own (a shot with `one_group = true` gives them all kitty's own class, so
# Atrium keeps them on the stage together, and places them by title), and
# Firefox.
KITTY = {
    # The configuration being changed: the default user.lua, in nano.
    "notes": ("showcase-notes", "user.lua",
              lambda env: ["sh", "-c", f"cd '{env['XDG_CONFIG_HOME']}/solium' && exec nano user.lua"]),
    # Solium's own history as merged (origin/stage when there is one, not
    # the branch being recorded): a terminal with colour and nothing private.
    "log": ("showcase-log", "git log",
            lambda env: ["sh", "-c",
                         f"git -C {ROOT} log $(git -C {ROOT} rev-parse -q --verify origin/stage || echo HEAD) "
                         "--graph --color=always --date=short "
                         "--format='%C(#b392ff)%h%C(reset) %C(#c9c6d1)%s%C(reset) "
                         "%C(#5c5866)%ad%C(reset)' -60 | head -60; exec sleep 3600"]),
    # What the machine is (stage/home/.config/fastfetch), drawn again
    # whenever the window changes size, so a tile or a glide never leaves it
    # cut short.
    "shell": ("showcase-shell", "fastfetch",
              lambda env: ["bash", "-c", "trap 'clear; fastfetch' WINCH; clear; fastfetch; "
                           "while :; do sleep 1 & wait $!; done"]),
    "term": ("showcase-term", "~", lambda env: ["bash"]),
    # See-through, so a backdrop effect shows through the terminal.
    "clear": ("showcase-clear", "git log",
              lambda env: ["sh", "-c",
                           f"git -C {ROOT} log $(git -C {ROOT} rev-parse -q --verify origin/stage || echo HEAD) "
                           "--graph --color=always --date=short "
                           "--format='%C(#b392ff)%h%C(reset) %C(#e9e8ec)%s%C(reset)' -40 "
                           "| head -40; exec sleep 3600"]),
}
# kitty's own settings for an application, beside its theme's.
KITTY_OPTIONS = {"clear": ["-o", "background_opacity=0.35"]}


def start_app(spec, env, log, one_group=False):
    if isinstance(spec, str):
        name, size = spec, None
    else:
        name, size = spec["app"], spec.get("size")
    if name in KITTY:
        app_id, title, command = KITTY[name]
        argv = ["kitty", "--class", "kitty" if one_group else app_id, "--title", title,
                *KITTY_OPTIONS.get(name, [])]
        if size:
            argv += ["-o", f"initial_window_width={size[0]}",
                     "-o", f"initial_window_height={size[1]}", "-o", "remember_window_size=no"]
        argv += command(env)
    elif name == "docs":
        argv = ["firefox", "--no-remote", "--profile", f"{env['HOME']}/firefox",
                "--name", "org.mozilla.firefox", "--new-window",
                "https://lilium-linux.github.io/solium/"]
    elif name == "foot":
        # foot runs in the build image, where its home is not this one: its
        # settings are named, from the stage's home, which is mounted.
        # A shell with the stage's prompt and no rc file: foot's home here is
        # the real one, whose .bashrc is not the stage's.
        argv = [str(ROOT / "dev" / "foot"), "-c", f"{env['HOME']}/.config/foot/foot.ini",
                "sh", "-c", "PS1='❯ ' exec bash --norc --noprofile"]
        env = dict(env, HOME=REAL_HOME, DBUS_SESSION_BUS_ADDRESS=REAL_BUS)
    else:
        argv = ["sh", "-c", name]
    return subprocess.Popen(argv, env=env, stdout=log, stderr=log,
                            cwd=env["HOME"], start_new_session=True)


def stop(process):
    """By its own process group, which it was started in: never by name."""
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            return
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)


# ---- a run ----


def record(shot, binary):
    work = WORK / shot.name
    shutil.rmtree(work, ignore_errors=True)
    frames = work / "frames"
    frames.mkdir(parents=True)
    home = stage_home(work)
    config, init = stage_config(shot, work)
    (work / "state").mkdir()

    # The burst lands in memory, not on disk: writing a frame is part of the
    # pass that captured it, and a disk write there made one frame in two
    # late. A thread moves each finished frame to disk while Solium runs.
    burst = Path("/dev/shm") / f"solium-showcase-{os.getpid()}"
    shutil.rmtree(burst, ignore_errors=True)
    burst.mkdir()
    capture_from = int(shot.capture["from"])
    capture_to = int(shot.capture["to"])
    count = max(1, round((capture_to - capture_from) / FRAME_MS))
    knobs, _ = shot.knobs()
    bus, address = start_bus(work)
    log = (work / "solium.log").open("w")
    apps_log = (work / "apps.log").open("w")
    children = []
    try:
        logind = [sys.executable, str(STAGE / "logind.py"), "--bus", address]
        rate = float(shot.stage.get("rate", RATE))
        for event in shot.events:
            for kind in ("lock", "unlock"):
                if event.get(kind):
                    logind += [f"--{kind}", str(event["ms"] / 1000 / rate)]
        env = dict(os.environ)
        if not env.get("WAYLAND_DISPLAY"):
            raise SystemExit("WAYLAND_DISPLAY is empty: it would mean the default "
                             "socket, which is the real desktop")
        for drop in ("DISPLAY", "XDG_SESSION_ID", "SOLIUM_PANE"):
            env.pop(drop, None)
        env.update({
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(config),
            "XDG_STATE_HOME": str(work / "state"),
            "XDG_CACHE_HOME": str(work / "cache"),
            "DBUS_SESSION_BUS_ADDRESS": address,
            "SOLIUM_SESSION_BUS": address,
            "SOLIUM_LOGIND_BUS": address,
            "SOLIUM_TERMINAL": "kitty",
            "SOLIUM_LUA_INIT": str(init),
            "SOLIUM_CAPTURE": str(burst / "f.ppm"),
            "SOLIUM_CAPTURE_AT": str(capture_from),
            "SOLIUM_CAPTURE_FRAMES": str(count),
            "SOLIUM_CAPTURE_INTERVAL": str(FRAME_MS),
            "SOLIUM_CLOCK_RATE": str(rate),
            "RUST_LOG": "info",
            **knobs,
        })
        children.append(subprocess.Popen(logind, env=env, stdout=apps_log, stderr=apps_log,
                                         start_new_session=True))
        lasts = (capture_to + 1500) / 1000 / rate
        started = time.monotonic()
        # The lock screen shows the user's name, read from the password
        # database: Solium sees one whose entry for this user is "Solium",
        # through bubblewrap, and nothing else of the machine is changed.
        passwd = work / "passwd"
        uid = str(os.getuid())
        with open("/etc/passwd") as source:
            rows = [line.rstrip("\n").split(":") for line in source if line.strip()]
        for row in rows:
            if len(row) == 7 and row[2] == uid:
                row[0], row[4], row[5] = "solium", "Solium", str(home)
        passwd.write_text("\n".join(":".join(row) for row in rows) + "\n")
        solium = subprocess.Popen(["bwrap", "--dev-bind", "/", "/", "--ro-bind", str(passwd),
                                   "/etc/passwd", "--die-with-parent", "--",
                                   "timeout", "-s", "TERM", f"{lasts:.1f}", str(binary)],
                                  env=env, stdout=log, stderr=log, cwd=ROOT,
                                  start_new_session=True)
        children.append(solium)
        socket = wait_for_socket(work / "solium.log", solium)
        # What a Solium session tells its applications (session::desktop).
        client = dict(env, WAYLAND_DISPLAY=socket, XDG_CURRENT_DESKTOP="Lilium")
        for key in [k for k in client if k.startswith("SOLIUM_")]:
            client.pop(key)
        timers = []
        one_group = bool(shot.stage.get("one_group"))
        for index, spec in enumerate(shot.stage.get("apps", [])):
            # A second apart, in the order the shot lists them: what is
            # stacked on top and the taskbar's order follow the order the
            # windows open in, and a shot that clicks a button relies on it.
            if index:
                time.sleep(1.0)
            children.append(start_app(spec, client, apps_log, one_group))
        for event in shot.events:
            if "run" in event or "app" in event or "notify" in event:
                timers.append(threading.Timer(
                    max(0.0, event["ms"] / 1000 / rate - (time.monotonic() - started)),
                    run_event, (event, client, apps_log, children)))
        for timer in timers:
            timer.start()
        mover = Mover(burst, frames)
        mover.start()
        solium.wait()
        mover.finish()
        for timer in timers:
            timer.cancel()
    finally:
        shutil.rmtree(burst, ignore_errors=True)
        for child in reversed(children):
            stop(child)
        bus.terminate()
        log.close()
        apps_log.close()
    return work


class Mover(threading.Thread):
    """Moves each finished frame of a burst out of memory onto disk: a frame
    is finished once the one after it exists, and the last once Solium has
    gone."""

    def __init__(self, source, target):
        super().__init__(daemon=True)
        self.source, self.target = source, target
        self.done = threading.Event()

    def move(self, everything):
        found = sorted(self.source.glob("f-*.ppm"))
        for ppm in found if everything else found[:-1]:
            shutil.move(ppm, self.target / ppm.name)

    def run(self):
        while not self.done.wait(0.05):
            self.move(False)

    def finish(self):
        self.done.set()
        self.join()
        self.move(True)


def wait_for_socket(path, process):
    for _ in range(300):
        if process.poll() is not None:
            raise SystemExit(f"solium exited early; see {path}")
        found = re.findall(r"socket=(wayland-\d+)", path.read_text(errors="replace"))
        if found:
            return found[-1]
        time.sleep(0.05)
    raise SystemExit(f"solium never reported a socket; see {path}")


def run_event(event, env, log, children):
    if "notify" in event:
        title, body = event["notify"], event.get("body", "")
        argv = ["notify-send", "-a", event.get("from_app", "Solium"), title, body]
    elif "app" in event:
        children.append(start_app(event["app"], env, log, event.get("one_group", False)))
        return
    else:
        argv = ["sh", "-c", event["run"]]
    children.append(subprocess.Popen(argv, env=env, stdout=log, stderr=log,
                                     start_new_session=True))


# ---- the frames ----


def frames_of(work):
    """Each captured frame as PNG, with its moment in ms after the first."""
    times = {}
    for line in (work / "solium.log").read_text(errors="replace").splitlines():
        found = re.search(r"captured a frame path=(\S+) at_us=(\d+)", line)
        if found:
            times[Path(found.group(1)).name] = int(found.group(2)) / 1000
    frames = []
    for ppm in sorted((work / "frames").glob("f-*.ppm")):
        png = ppm.with_suffix(".png")
        Image.open(ppm).save(png, compress_level=1)
        ppm.unlink()
        frames.append([png, times.get(ppm.name)])
    if not frames:
        raise SystemExit(f"no frames captured; see {work / 'solium.log'}")
    first = next((at for _, at in frames if at is not None), 0.0)
    for index, frame in enumerate(frames):
        frame[1] = (frame[1] - first) if frame[1] is not None else index * FRAME_MS
    return frames


def crop_frames(shot, frames, work):
    """The shot's crop, fixed or following the pointer, as new frames."""
    crop = shot.out.get("crop")
    follow = shot.out.get("follow")
    if not crop and not follow:
        return frames
    with Image.open(frames[0][0]) as image:
        width, height = image.size
    scale = float(shot.stage.get("scale", 1))
    _, journeys = shot.knobs()
    capture_from = int(shot.capture["from"])
    cropped = work / "cropped"
    cropped.mkdir(exist_ok=True)
    out = []
    for png, at in frames:
        if crop:
            x, y, w, h = (round(v * scale) for v in crop)
        else:
            w, h = (round(v * scale) for v in follow["size"])
            px, py = shot.pointer_at(journeys, capture_from + at)
            x = round(px * scale - w / 2 + follow.get("dx", 0) * scale)
            y = round(py * scale - h / 2 + follow.get("dy", 0) * scale)
        x = min(max(0, x), width - w)
        y = min(max(0, y), height - h)
        target = cropped / png.name
        with Image.open(png) as image:
            image.crop((x, y, x + w, y + h)).save(target, compress_level=1)
        out.append([target, at])
    return out


def published():
    """The shots whose media a page shows: the Showcase page's, and the
    README's (`docs/media/<shot>.`)."""
    with (HERE / "page.toml").open("rb") as file:
        index = tomllib.load(file)
    names = {name for section in index["section"] for name in section["shots"]}
    names |= set(re.findall(r"docs/media/([a-z0-9-]+)\.", (ROOT / "README.md").read_text()))
    return names


def media_dir(shot):
    """Where a shot's media goes: the repository's docs/media for a page's,
    target/showcase/cells for the rest."""
    folder = MEDIA if shot.name in published() else CELLS
    folder.mkdir(parents=True, exist_ok=True)
    return folder


def cell_source(name, suffix):
    for folder in (CELLS, MEDIA):
        if (folder / f"{name}{suffix}").exists():
            return folder / f"{name}{suffix}"
    return None


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *args], check=True)


def encode(shot, frames, work):
    """The media the shot asks for, and what was written."""
    out_dir = media_dir(shot)
    out = shot.out
    written = []
    listing = work / "frames.txt"
    with listing.open("w") as file:
        for (png, at), following in zip(frames, frames[1:] + [None]):
            duration = ((following[1] - at) if following else FRAME_MS) / 1000
            file.write(f"file '{png}'\nduration {max(duration, 0.001):.6f}\n")
        file.write(f"file '{frames[-1][0]}'\n")
    master = work / "master.mp4"
    ffmpeg("-f", "concat", "-safe", "0", "-i", str(listing),
           "-vf", "fps=60,format=yuv420p", "-c:v", "libx264", "-crf", "16",
           "-preset", "slow", "-movflags", "+faststart", str(master))
    # The full-speed video is what a grid is made from and what a person
    # reviews; a page shows the WebP, which plays at 60 frames a second, so
    # the video stays out of the repository.
    CELLS.mkdir(parents=True, exist_ok=True)
    if out.get("mp4", True):
        target = CELLS / f"{shot.name}.mp4"
        shutil.copy2(master, target)
        written.append(target)
    if out.get("slow"):
        target = out_dir / f"{shot.name}-x{out['slow']}.mp4"
        ffmpeg("-i", str(master), "-vf", f"setpts={out['slow']}*PTS,fps=60",
               "-c:v", "libx264", "-crf", "16", "-preset", "slow",
               "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(target))
        written.append(target)
        # And as a WebP, shown on the page under the true-speed one, for a
        # motion too quick to follow: the pointer's morphs.
        if out.get("slow_webp"):
            written.append(webp(shot, target, int(out.get("webp", 640)),
                                int(out.get("webp_kb", 3072)) * 2, f"{shot.name}-x{out['slow']}"))
    if out.get("webp"):
        written.append(webp(shot, master, int(out["webp"]), int(out.get("webp_kb", 3072))))
    poster_ms = out.get("poster")
    if poster_ms is not None or out.get("still"):
        at = float(poster_ms if poster_ms is not None else frames[-1][1])
        png = min(frames, key=lambda frame: abs(frame[1] - at))[0]
        # A still a page shows; a moving shot's poster is for review.
        target = (out_dir if out.get("still") else CELLS) / f"{shot.name}.png"
        with Image.open(png) as image:
            image.convert("RGB").save(target, optimize=True)
        written.append(target)
    if out.get("filmstrip"):
        written.append(filmstrip(shot, frames, int(out["filmstrip"])))
    return written


def webp(shot, master, width, budget_kb, name=None):
    """An animated WebP within `budget_kb`, as good as fits: one try at a
    good quality, then, if it is too large, one at the quality its size says
    should fit, and only then fewer frames a second."""
    target = media_dir(shot) / f"{name or shot.name}.webp"

    def make(fps, quality):
        ffmpeg("-i", str(master), "-vf", f"fps={fps},scale={width}:-2:flags=lanczos",
               "-c:v", "libwebp_anim", "-lossless", "0", "-q:v", str(quality),
               "-compression_level", "4", "-loop", "0", str(target))
        return target.stat().st_size / 1024

    size = make(60, 80)
    if size > budget_kb:
        # Size grows roughly linearly with quality over this range.
        size = make(60, max(40, int(80 * budget_kb / size * 0.92)))
    if size > budget_kb:
        make(30, 60)
    return target


def filmstrip(shot, frames, count):
    """`count` frames evenly through the shot, side by side with a gap."""
    picks = [frames[round(i * (len(frames) - 1) / (count - 1))][0] for i in range(count)]
    width = int(shot.out.get("filmstrip_width", 320))
    tiles = []
    for png in picks:
        with Image.open(png) as image:
            ratio = width / image.width
            tiles.append(image.convert("RGB").resize((width, round(image.height * ratio)),
                                                     Image.LANCZOS))
    gap = 6
    strip = Image.new("RGB", (count * width + (count - 1) * gap, tiles[0].height), "#0b0a0e")
    for index, tile in enumerate(tiles):
        strip.paste(tile, (index * (width + gap), 0))
    target = media_dir(shot) / f"{shot.name}-strip.png"
    strip.save(target, optimize=True)
    return target


# ---- a grid of shots, compared ----


def font(size):
    try:
        path = subprocess.run(["fc-match", "-f", "%{file}", "Inter,sans-serif:weight=600"],
                              capture_output=True, text=True, check=True).stdout
        return ImageFont.truetype(path, size)
    except (OSError, subprocess.CalledProcessError):
        return ImageFont.load_default(size)


def grid(shot):
    """Shots already recorded, side by side, each labelled in a margin under
    its own picture, never on it; started together, so the comparison is a
    fair one."""
    spec = shot.grid
    if spec.get("still"):
        return still_grid(shot)
    cells = spec["cells"]
    labels = spec.get("labels", cells)
    cols = int(spec.get("cols", 2))
    width = int(spec.get("cell_width", 800))
    margin = 44
    work = WORK / shot.name
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    inputs, filters = [], []
    for index, (cell, label) in enumerate(zip(cells, labels)):
        source = cell_source(cell, f"{spec.get('suffix', '')}.mp4")
        if source is None:
            raise SystemExit(f"{shot.name}: record {cell} first")
        caption = work / f"label-{index}.png"
        image = Image.new("RGB", (width, margin), "#0b0a0e")
        draw = ImageDraw.Draw(image)
        draw.text((width / 2, margin / 2), label, fill="#e9e8ec", font=font(20), anchor="mm")
        image.save(caption)
        inputs += ["-i", str(source), "-i", str(caption)]
        filters.append(f"[{2 * index}:v]scale={width}:-2,setsar=1[v{index}];"
                       f"[v{index}][{2 * index + 1}:v]vstack[c{index}]")
    # Every cell the same size, so a cell's place is a multiple of the first's.
    layout = "|".join(
        f"{'+'.join(['w0'] * (i % cols)) or 0}_{'+'.join(['h0'] * (i // cols)) or 0}"
        for i in range(len(cells)))
    stacked = "".join(f"[c{i}]" for i in range(len(cells)))
    graph = ";".join(filters) + f";{stacked}xstack=inputs={len(cells)}:layout={layout}:fill=#0b0a0e,format=yuv420p[out]"
    CELLS.mkdir(parents=True, exist_ok=True)
    target = CELLS / f"{shot.name}.mp4"
    ffmpeg(*inputs, "-filter_complex", graph, "-map", "[out]", "-c:v", "libx264",
           "-crf", "18", "-preset", "slow", "-movflags", "+faststart", "-shortest", str(target))
    written = [target]
    if shot.out.get("webp"):
        written.append(webp(shot, target, int(shot.out["webp"]), int(shot.out.get("webp_kb", 4096))))
    return written


def still_grid(shot):
    """Recorded stills side by side, each labelled in a margin under it."""
    spec = shot.grid
    cells, labels = spec["cells"], spec.get("labels", spec["cells"])
    cols, width, margin, gap = int(spec.get("cols", 2)), int(spec.get("cell_width", 800)), 44, 8
    tiles = []
    for cell, label in zip(cells, labels):
        source = cell_source(cell, ".png")
        if source is None:
            raise SystemExit(f"{shot.name}: record {cell} first")
        with Image.open(source) as image:
            picture = image.convert("RGB").resize(
                (width, round(image.height * width / image.width)), Image.LANCZOS)
        tile = Image.new("RGB", (width, picture.height + margin), "#0b0a0e")
        tile.paste(picture, (0, 0))
        ImageDraw.Draw(tile).text((width / 2, picture.height + margin / 2), label,
                                  fill="#e9e8ec", font=font(20), anchor="mm")
        tiles.append(tile)
    rows = math.ceil(len(tiles) / cols)
    sheet = Image.new("RGB", (cols * width + (cols - 1) * gap,
                              rows * tiles[0].height + (rows - 1) * gap), "#0b0a0e")
    for index, tile in enumerate(tiles):
        sheet.paste(tile, ((index % cols) * (width + gap), (index // cols) * (tile.height + gap)))
    target = media_dir(shot) / f"{shot.name}.png"
    sheet.save(target, optimize=True)
    return [target]


# ---- what a person looks at, and what a run checks ----


def small(png):
    with Image.open(png) as image:
        return image.convert("L").resize((160, 90))


def rms(a, b):
    """The root mean square difference of two grey pictures, 0 to 255."""
    return math.sqrt(sum(ImageStat.Stat(ImageChops.difference(a, b)).sum2)
                     / (a.width * a.height))


def jumps(frames):
    """Frames where the picture changes far more than in the frames either
    side: something that appears, vanishes or moves at once in a moment that
    is otherwise still. It does not see a skip in the middle of a motion (a
    picture's difference stops growing once a window has moved more than its
    own detail), which only looking at the frames does. A cut the shot asks
    for (a scene opening at once) is named too."""
    smalls = [small(png) for png, _ in frames]
    change = [0.0] + [ImageStat.Stat(ImageChops.difference(a, b)).mean[0]
                      for a, b in zip(smalls, smalls[1:])]
    found = []
    for i in range(1, len(change)):
        around = max(change[i - 1], change[i + 1] if i + 1 < len(change) else 0.0)
        if change[i] > 6.0 and change[i] > 3.0 * max(around, 0.5):
            found.append(i)
    return found


# What Solium's log says when something a shot names did not load: an
# effect drawn flat, a scene swapped for the shipped one, a broken
# configuration. Any of them makes the shot a picture of something else.
TROUBLE = re.compile(r"effects problem|would not load|falls? back|failed to load|"
                     r"configuration failed|not found", re.I)


def check(shot, frames, written, work=None):
    findings = []
    if work is not None:
        trouble = sorted({line.split("  ", 1)[-1][:160]
                          for line in (work / "solium.log").read_text(errors="replace").splitlines()
                          if TROUBLE.search(line) and (" WARN " in line or " ERROR " in line)})
        for line in trouble[:5]:
            findings.append(f"log: {line}")
    spikes = jumps(frames)
    if spikes:
        findings.append(f"{len(spikes)} pop(s) in a still moment: " + ", ".join(
            f"#{i} at {frames[i][1]:.0f} ms" for i in spikes[:10]))
    gaps = [(index, round(at - frames[index - 1][1]))
            for index, (_, at) in enumerate(frames) if index and at - frames[index - 1][1] > LATE_MS]
    if gaps:
        findings.append(f"{len(gaps)} late frame(s): " + ", ".join(
            f"#{index} after {gap} ms" for index, gap in gaps[:8]))
    dark = [index for index, (png, _) in enumerate(frames)
            if ImageStat.Stat(small(png)).mean[0] < 4]
    if dark:
        findings.append(f"{len(dark)} black frame(s), from #{dark[0]}")
    if shot.capture.get("loop"):
        seam = rms(small(frames[0][0]), small(frames[-1][0]))
        if seam > SAME_RMS:
            findings.append(f"the loop's ends differ (rms {seam:.1f})")
    for path in written:
        if path.suffix == ".png":
            committed = subprocess.run(["git", "-C", str(ROOT), "show", f"HEAD:{path.relative_to(ROOT)}"],
                                       capture_output=True)
            if committed.returncode == 0:
                old = work_image(committed.stdout)
                if rms(old, small(path)) > SAME_RMS:
                    findings.append(f"{path.name} differs from the committed one")
            else:
                findings.append(f"{path.name} is new")
    return findings


def work_image(data):
    from io import BytesIO
    return Image.open(BytesIO(data)).convert("L").resize((160, 90))


def contact_sheet(shot, frames, work):
    """Every frame, small and numbered with its moment: what is looked at,
    frame by frame, before a shot is kept."""
    cols, width = 8, 240
    with Image.open(frames[0][0]) as first:
        height = round(first.height * width / first.width)
    rows = math.ceil(len(frames) / cols)
    sheet = Image.new("RGB", (cols * width, rows * (height + 18)), "#0b0a0e")
    draw = ImageDraw.Draw(sheet)
    label = font(12)
    for index, (png, at) in enumerate(frames):
        with Image.open(png) as image:
            tile = image.convert("RGB").resize((width, height), Image.BILINEAR)
        x, y = (index % cols) * width, (index // cols) * (height + 18)
        sheet.paste(tile, (x, y))
        draw.text((x + 4, y + height + 2), f"#{index}  {at:.0f} ms", fill="#c9c6d1", font=label)
    review = WORK / "review"
    review.mkdir(parents=True, exist_ok=True)
    target = review / f"{shot.name}.png"
    sheet.save(target)
    return target


def code_of(spec):
    """The lines a shot's `[[code]]` names, from the file as it is now: from
    the first line containing `find` (or the top), `lines` of them (or all)."""
    path = ROOT / spec["file"]
    text = path.read_text().splitlines()
    start = 0
    if "find" in spec:
        found = [i for i, line in enumerate(text) if spec["find"] in line]
        if not found:
            raise SystemExit(f"{spec['file']} no longer has {spec['find']!r}: "
                             "the shot's [[code]] needs updating")
        start = found[0]
    lines = text[start:start + int(spec["lines"])] if "lines" in spec else text[start:]
    return "\n".join(lines).rstrip()


def page():
    """docs/showcase.md, from page.toml and the shots."""
    with (HERE / "page.toml").open("rb") as file:
        index = tomllib.load(file)
    shots = {path.stem: Shot(path) for path in SHOTS.glob("*.toml")}
    out = ["<!-- Written by dev/showcase/record.py --page from dev/showcase/page.toml",
           "     and the shots in dev/showcase/shots/: change those, not this. -->", "",
           f"# {index['title']}", "", index["intro"].strip(), ""]
    for section in index["section"]:
        out += [f"## {section['title']}", ""]
        if section.get("intro"):
            out += [section["intro"].strip(), ""]
        for name in section["shots"]:
            shot = shots[name]
            out += [f"### {shot.title}", ""]
            image = "webp" if (MEDIA / f"{name}.webp").exists() else "png"
            out.append(f"![{shot.data.get('what', shot.title)}](media/{name}.{image})")
            slow = shot.out.get("slow")
            if slow and (MEDIA / f"{name}-x{slow}.webp").exists():
                out += ["", f"![The same, {slow} times slower]"
                            f"(media/{name}-x{slow}.webp)", "", f"{slow} times slower."]
            elif slow and (MEDIA / f"{name}-x{slow}.mp4").exists():
                out += ["", f"[{slow} times slower](media/{name}-x{slow}.mp4)"]
            out.append("")
            if shot.data.get("what"):
                out += [shot.data["what"], ""]
            if shot.data.get("keys"):
                out += [f"**Keys:** {shot.data['keys']}", ""]
            if shot.data.get("how"):
                out += ["**How it is done.** " + shot.data["how"].strip(), ""]
            patch = shot.stage.get("patch", "").strip()
            if patch:
                out += ["Changed for this one, in `user.lua`:", "", "```lua", patch, "```", ""]
            for spec in shot.data.get("code", []):
                language = {"lua": "lua", "qml": "qml", "frag": "glsl", "toml": "toml"}.get(
                    spec["file"].rsplit(".", 1)[-1], "")
                link = "../" + spec["file"]
                out += [f"[`{spec['file']}`]({link})", "", f"```{language}", code_of(spec), "```", ""]
    target = ROOT / "docs" / "showcase.md"
    target.write_text("\n".join(out).rstrip() + "\n")
    return target


def interrupted(_signal, _frame):
    raise KeyboardInterrupt


def main():
    # Ended from outside, a run still stops what it started (its `finally`).
    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("shots", nargs="*", help="shot names; every shot when none")
    parser.add_argument("--list", action="store_true", help="list the shots and stop")
    parser.add_argument("--binary", default=str(ROOT / "target" / "release" / "solium"))
    parser.add_argument("--keep", action="store_true", help="keep each shot's frames")
    parser.add_argument("--page", action="store_true",
                        help="write docs/showcase.md from page.toml and stop")
    args = parser.parse_args()
    if args.page:
        print(page().relative_to(ROOT))
        return

    shots = [Shot(path) for path in sorted(SHOTS.glob("*.toml"))]
    if args.list:
        for shot in shots:
            print(f"{shot.name:28} {shot.title}")
        return
    if args.shots:
        known = {shot.name: shot for shot in shots}
        missing = [name for name in args.shots if name not in known]
        if missing:
            raise SystemExit(f"no such shot: {', '.join(missing)}")
        shots = [known[name] for name in args.shots]
    binary = Path(args.binary)
    if not binary.exists():
        raise SystemExit(f"{binary} is not built: cargo build --release")
    # Grids last: they are made from the other shots' media.
    shots.sort(key=lambda shot: bool(shot.grid))

    report = []
    for shot in shots:
        print(f"== {shot.name}: {shot.title}", flush=True)
        if shot.grid:
            written = grid(shot)
            report.append((shot.name, len(shot.grid["cells"]), [], written, None))
            continue
        work = record(shot, binary)
        frames = crop_frames(shot, frames_of(work), work)
        written = encode(shot, frames, work)
        sheet = contact_sheet(shot, frames, work)
        findings = check(shot, frames, written, work)
        report.append((shot.name, len(frames), findings, written, sheet))
        if not args.keep:
            shutil.rmtree(work / "frames", ignore_errors=True)
            shutil.rmtree(work / "cropped", ignore_errors=True)

    if not args.shots:
        page()
    print()
    for name, count, findings, written, sheet in report:
        sizes = ", ".join(f"{path.name} {path.stat().st_size / 1024:.0f} KB" for path in written)
        print(f"{name}: {count} frames; {sizes}")
        if sheet:
            print(f"    review: {sheet.relative_to(ROOT)}")
        for finding in findings:
            print(f"    ! {finding}")


if __name__ == "__main__":
    main()
