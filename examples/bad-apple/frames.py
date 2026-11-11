#!/usr/bin/env python3
"""Turn a black and white video into the film `bad-apple` plays.

    frames.py <video> <film.frames> [--grid 16x12] [--fps 24]

Each frame is cut down to a grid of cells, each light or dark. A cell is a
terminal on screen, so the grid is how many terminals the film is played
with: 16x12 is 192 cells, and a finer grid is a truer picture and more
windows for the compositor to hold.

The film is a text file: `grid`, `fps` and `frames` lines, then a line a
frame, a `1` for each light cell and a `0` for each dark one, row by row.

Needs Python 3 and ffmpeg, and nothing else.
"""

import argparse
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("video")
    parser.add_argument("film")
    parser.add_argument("--grid", default="16x12", help="cells across and down; 16x12")
    parser.add_argument("--fps", type=float, default=24, help="frames a second; 24")
    arguments = parser.parse_args()
    columns, rows = (int(part) for part in arguments.grid.lower().split("x"))

    decode = subprocess.Popen(
        ["ffmpeg", "-v", "error", "-i", arguments.video,
         "-vf", f"fps={arguments.fps},scale={columns}:{rows}:flags=area,format=gray",
         "-f", "rawvideo", "-pix_fmt", "gray", "-"],
        stdout=subprocess.PIPE)
    frames = []
    size = columns * rows
    while True:
        raw = decode.stdout.read(size)
        if len(raw) < size:
            break
        frames.append("".join("1" if value >= 128 else "0" for value in raw))
    if decode.wait() != 0 or not frames:
        sys.exit(f"frames.py: ffmpeg read no frames from {arguments.video}")

    with open(arguments.film, "w", encoding="utf-8") as film:
        film.write(f"grid {columns} {rows}\n")
        film.write(f"fps {arguments.fps:g}\n")
        film.write(f"frames {len(frames)}\n")
        film.write("\n".join(frames) + "\n")
    print(f"{arguments.film}: {len(frames)} frames of {columns}x{rows} at {arguments.fps:g} a second, "
          f"{size} cells")


if __name__ == "__main__":
    main()
