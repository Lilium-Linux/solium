#!/usr/bin/env python3
"""Turn a black and white video into the film `bad-apple` plays.

    frames.py <video> <film.frames> [--grid 32x24] [--fps 24]

Each frame is cut down to a grid of cells, each light or dark, and each
colour's cells are covered with rectangles. A rectangle is a terminal on
screen, so a rectangle lives from frame to frame as a window does: it
keeps what is still its colour, grows into what has become so, joins a
neighbour it makes a rectangle with, and is gone only when nothing of it is
left. What no rectangle reaches is covered with new ones, the largest
first, and a rectangle takes in the smaller ones along its edges a line a
frame, so the pieces a moving shape leaves behind become a few large
windows again. So most of a frame is the windows of the frame before, a few
of them moved or resized.

The film is a text file: `grid`, `fps`, `light`, `dark` and `frames` lines,
then a line a frame: its light rectangles, a `|`, and its dark ones, each
`n x y w h` and separated by `;`. `n` is which terminal of its colour the
rectangle is, the same one for as long as it lives; `light` and `dark` say
how many of each the film needs at most.

Needs Python 3 and ffmpeg, and nothing else.
"""

import argparse
import subprocess
import sys

# A cell turns light above the second and dark below the first: between
# them it stays as it was, so the soft edge of a shape does not flicker.
DARK_BELOW, LIGHT_FROM = 112, 144


def largest(lit, x0, y0, x1, y1):
    """The largest rectangle of lit cells inside the given part of the grid:
    (area, x, y, w, h), by the histogram of how many lit cells stand above
    each cell of a row."""
    best = (0, 0, 0, 0, 0)
    width = x1 - x0
    heights = [0] * width
    for y in range(y0, y1):
        row = lit[y]
        for i in range(width):
            heights[i] = heights[i] + 1 if row[x0 + i] else 0
        # The widest rectangle under the histogram, with a stack of rising bars.
        stack = []
        for i in range(width + 1):
            height = heights[i] if i < width else 0
            start = i
            while stack and stack[-1][1] >= height:
                start, top = stack.pop()
                area = top * (i - start)
                if area > best[0]:
                    best = (area, x0 + start, y - top + 1, i - start, top)
            stack.append((start, height))
    return best


def area(rectangle):
    return rectangle[2] * rectangle[3]


class Colour:
    """The rectangles of one colour, each with the number it keeps."""

    def __init__(self, columns, rows):
        self.columns, self.rows = columns, rows
        self.live = {}
        # The numbers no rectangle has now, and how many numbers there have
        # been: how many terminals the film needs of this colour.
        self.spare = []
        self.most = 0

    def step(self, cells):
        """The rectangles for the next frame, whose cells of this colour are
        `cells`: those of the last frame, carried over."""
        free = [list(row) for row in cells]
        # Each keeps the largest part of itself that is still its colour.
        for n in self.largest_first():
            x, y, w, h = self.live[n]
            kept, x, y, w, h = largest(cells, x, y, x + w, y + h)
            if kept == 0:
                self.let_go(n)
                continue
            self.live[n] = (x, y, w, h)
            for row in free[y:y + h]:
                row[x:x + w] = [False] * w
        self.grow(free)
        self.join()
        # What is left is covered afresh, the largest rectangle first.
        while True:
            found, x, y, w, h = largest(free, 0, 0, self.columns, self.rows)
            if found == 0:
                break
            self.live[self.number()] = (x, y, w, h)
            for row in free[y:y + h]:
                row[x:x + w] = [False] * w
        self.join()
        self.absorb()
        self.join()
        return sorted((n, *rectangle) for n, rectangle in self.live.items())

    def largest_first(self):
        return sorted(self.live, key=lambda n: -area(self.live[n]))

    def number(self):
        """The lowest number no rectangle has, so the film needs as few
        terminals as it can, or a new one."""
        if self.spare:
            self.spare.sort()
            return self.spare.pop(0)
        self.most += 1
        return self.most - 1

    def let_go(self, n):
        """A rectangle is gone, and its number is another's to take."""
        del self.live[n]
        self.spare.append(n)

    def grow(self, free):
        """Each takes the free cells beside it, a whole edge at a time, the
        largest rectangle first, until none can."""
        grew = True
        while grew:
            grew = False
            for n in self.largest_first():
                x, y, w, h = self.live[n]
                while x > 0 and all(free[row][x - 1] for row in range(y, y + h)):
                    x, w = x - 1, w + 1
                    for row in range(y, y + h):
                        free[row][x] = False
                while x + w < self.columns and all(free[row][x + w] for row in range(y, y + h)):
                    for row in range(y, y + h):
                        free[row][x + w] = False
                    w += 1
                while y > 0 and all(free[y - 1][x:x + w]):
                    y, h = y - 1, h + 1
                    free[y][x:x + w] = [False] * w
                while y + h < self.rows and all(free[y + h][x:x + w]):
                    free[y + h][x:x + w] = [False] * w
                    h += 1
                if (x, y, w, h) != self.live[n]:
                    self.live[n] = (x, y, w, h)
                    grew = True

    def absorb(self):
        """Each takes one line of cells along each of its edges from the
        smaller rectangles there, the largest first, when every one of them
        lies wholly along that edge: they give up a line and stay
        rectangles. A line a frame, so a window is taken in by its
        neighbour over several frames rather than gone in one."""
        owner = [[None] * self.columns for _ in range(self.rows)]
        for n, (x, y, w, h) in self.live.items():
            for row in owner[y:y + h]:
                row[x:x + w] = [n] * w
        for n in self.largest_first():
            for edge in ("right", "left", "bottom", "top"):
                if n not in self.live:
                    break
                x, y, w, h = self.live[n]
                # The line of cells just past the edge.
                if edge in ("right", "left"):
                    column = x + w if edge == "right" else x - 1
                    line = [(column, row) for row in range(y, y + h)] if 0 <= column < self.columns else []
                else:
                    row = y + h if edge == "bottom" else y - 1
                    line = [(column, row) for column in range(x, x + w)] if 0 <= row < self.rows else []
                others = {owner[row][column] for column, row in line}
                if not others or None in others or not all(
                        self.gives(other, edge, (x, y, w, h)) for other in others):
                    continue
                for other in others:
                    ox, oy, ow, oh = self.live[other]
                    if edge == "right":
                        ox, ow = ox + 1, ow - 1
                    elif edge == "left":
                        ow -= 1
                    elif edge == "bottom":
                        oy, oh = oy + 1, oh - 1
                    else:
                        oh -= 1
                    if ow and oh:
                        self.live[other] = (ox, oy, ow, oh)
                    else:
                        self.let_go(other)
                if edge == "right":
                    w += 1
                elif edge == "left":
                    x, w = x - 1, w + 1
                elif edge == "bottom":
                    h += 1
                else:
                    y, h = y - 1, h + 1
                self.live[n] = (x, y, w, h)
                for column, row in line:
                    owner[row][column] = n

    def gives(self, other, edge, to):
        """Whether a rectangle gives the one at `to` a line along `edge` of
        it: it is the smaller, and no part of it lies past the edge's ends."""
        ox, oy, ow, oh = self.live[other]
        x, y, w, h = to
        if ow * oh >= w * h:
            return False
        if edge in ("right", "left"):
            return y <= oy and oy + oh <= y + h
        return x <= ox and ox + ow <= x + w

    def join(self):
        """Two that share a whole edge are one rectangle: the larger takes
        the smaller in."""
        joined = True
        while joined:
            joined = False
            for a in self.largest_first():
                if a not in self.live:
                    continue
                for b in list(self.live):
                    if b == a or a not in self.live:
                        continue
                    ax, ay, aw, ah = self.live[a]
                    bx, by, bw, bh = self.live[b]
                    if ax == bx and aw == bw and (ay + ah == by or by + bh == ay):
                        self.live[a] = (ax, min(ay, by), aw, ah + bh)
                    elif ay == by and ah == bh and (ax + aw == bx or bx + bw == ax):
                        self.live[a] = (min(ax, bx), ay, aw + bw, ah)
                    else:
                        continue
                    self.let_go(b)
                    joined = True


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("video")
    parser.add_argument("film")
    parser.add_argument("--grid", default="32x24", help="cells across and down; 32x24")
    parser.add_argument("--fps", type=float, default=24, help="frames a second; 24")
    arguments = parser.parse_args()
    columns, rows = (int(part) for part in arguments.grid.lower().split("x"))

    decode = subprocess.Popen(
        ["ffmpeg", "-v", "error", "-i", arguments.video,
         "-vf", f"fps={arguments.fps},scale={columns}:{rows}:flags=area,format=gray",
         "-f", "rawvideo", "-pix_fmt", "gray", "-"],
        stdout=subprocess.PIPE)
    light, dark = Colour(columns, rows), Colour(columns, rows)
    lines = []
    size = columns * rows
    lit = [False] * size
    while True:
        raw = decode.stdout.read(size)
        if len(raw) < size:
            break
        lit = [value >= (DARK_BELOW if was else LIGHT_FROM) for value, was in zip(raw, lit)]
        cells = [lit[y * columns:(y + 1) * columns] for y in range(rows)]
        lines.append("|".join(
            ";".join(" ".join(str(number) for number in rectangle) for rectangle in colour)
            for colour in (light.step(cells), dark.step([[not cell for cell in row] for row in cells]))))
    if decode.wait() != 0 or not lines:
        sys.exit(f"frames.py: ffmpeg read no frames from {arguments.video}")

    with open(arguments.film, "w", encoding="utf-8") as film:
        film.write(f"grid {columns} {rows}\n")
        film.write(f"fps {arguments.fps:g}\n")
        film.write(f"light {light.most}\n")
        film.write(f"dark {dark.most}\n")
        film.write(f"frames {len(lines)}\n")
        film.write("\n".join(lines) + "\n")
    print(f"{arguments.film}: {len(lines)} frames of {columns}x{rows} at {arguments.fps:g} a second, "
          f"{light.most} light terminals and {dark.most} dark")


if __name__ == "__main__":
    main()
