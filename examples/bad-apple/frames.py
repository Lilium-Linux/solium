#!/usr/bin/env python3
"""Turn a black and white video into the frames `bad-apple` plays.

    frames.py <video> <film.frames> [--grid 48x36] [--fps 30] [--most 64] [--dark]

Each frame is cut down to a grid of cells, each lit or dark, and the lit
cells are covered with rectangles, the largest first, `--most` of them at
most. A rectangle is a window on screen, so `--most` is how many windows
the film is played with, and a finer grid or more of them is a truer
picture and more for the compositor to move. `--dark` lights the dark
parts instead.

The film is a text file: `grid`, `fps`, `most` and `frames` lines, then a
line a frame, its rectangles as `x y w h` separated by `;`.

Needs Python 3 and ffmpeg, and nothing else.
"""

import argparse
import subprocess
import sys


def largest(lit, columns, rows):
    """The largest rectangle of lit cells: (area, x, y, w, h), by the
    histogram of how many lit cells stand above each cell of a row."""
    best = (0, 0, 0, 0, 0)
    heights = [0] * columns
    for y in range(rows):
        row = lit[y]
        for x in range(columns):
            heights[x] = heights[x] + 1 if row[x] else 0
        # The widest rectangle under the histogram, with a stack of rising bars.
        stack = []
        for x in range(columns + 1):
            height = heights[x] if x < columns else 0
            start = x
            while stack and stack[-1][1] >= height:
                start, top = stack.pop()
                area = top * (x - start)
                if area > best[0]:
                    best = (area, start, y - top + 1, x - start, top)
            stack.append((start, height))
    return best


def rectangles(lit, columns, rows, most):
    """The lit cells as rectangles, the largest first, `most` at most."""
    lit = [list(row) for row in lit]
    out = []
    while len(out) < most:
        area, x, y, w, h = largest(lit, columns, rows)
        if area == 0:
            break
        out.append((x, y, w, h))
        for row in lit[y:y + h]:
            row[x:x + w] = [False] * w
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("video")
    parser.add_argument("film")
    parser.add_argument("--grid", default="48x36", help="cells across and down; 48x36")
    parser.add_argument("--fps", type=float, default=30, help="frames a second; 30")
    parser.add_argument("--most", type=int, default=64, help="rectangles a frame at most; 64")
    parser.add_argument("--dark", action="store_true", help="light the dark parts instead")
    arguments = parser.parse_args()
    columns, rows = (int(part) for part in arguments.grid.lower().split("x"))

    decode = subprocess.Popen(
        ["ffmpeg", "-v", "error", "-i", arguments.video,
         "-vf", f"fps={arguments.fps},scale={columns}:{rows}:flags=area,format=gray",
         "-f", "rawvideo", "-pix_fmt", "gray", "-"],
        stdout=subprocess.PIPE)
    frames = []
    used = 0
    size = columns * rows
    while True:
        raw = decode.stdout.read(size)
        if len(raw) < size:
            break
        lit = [[(raw[y * columns + x] >= 128) != arguments.dark for x in range(columns)]
               for y in range(rows)]
        found = rectangles(lit, columns, rows, arguments.most)
        used = max(used, len(found))
        frames.append(found)
    if decode.wait() != 0 or not frames:
        sys.exit(f"frames.py: ffmpeg read no frames from {arguments.video}")

    with open(arguments.film, "w", encoding="utf-8") as film:
        film.write(f"grid {columns} {rows}\n")
        film.write(f"fps {arguments.fps:g}\n")
        # The most any frame used: how many windows the player opens.
        film.write(f"most {max(used, 1)}\n")
        film.write(f"frames {len(frames)}\n")
        for found in frames:
            film.write(";".join(f"{x} {y} {w} {h}" for x, y, w, h in found) + "\n")
    print(f"{arguments.film}: {len(frames)} frames of {columns}x{rows} at {arguments.fps:g} a second, "
          f"{used} windows at most")


if __name__ == "__main__":
    main()
