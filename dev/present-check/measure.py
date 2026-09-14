#!/usr/bin/env python3
"""Measure a frame Solium captured.

A compositor cannot be tested by looking at it, and "that looks rotated" is not
a claim a program can check. `SOLIUM_CAPTURE` writes a binary PPM; this reads
one and answers in numbers.

    at     FILE X Y [X Y ...]              the colour at each point
    span   FILE R G B [TOL]                bounding box and centroid of a colour
    quad   FILE R G B [TOL]                its four corners
    corner FILE WR WG WB MR MG MB [TOL]    the window's *top-left* corner
    hist   FILE [N]                        the N commonest colours

`corner` is the one that earns its place. A rotated window's top-left is not
the top-left of anything in the image -- it is one particular vertex of a quad,
and which one cannot be read off the picture. So the client prints a marker
block of a second colour at its home position: the marker travels with the
corner, and the quad vertex nearest the marker's centroid is the corner,
whatever the matrix did.
"""

import sys
from collections import Counter


def read_ppm(path):
    with open(path, "rb") as handle:
        data = handle.read()
    if not data.startswith(b"P6"):
        raise SystemExit(f"{path}: not a binary PPM")
    fields = []
    i = 2
    while len(fields) < 3:
        while i < len(data) and data[i : i + 1].isspace():
            i += 1
        if data[i : i + 1] == b"#":
            while data[i : i + 1] not in (b"\n", b""):
                i += 1
            continue
        start = i
        while i < len(data) and not data[i : i + 1].isspace():
            i += 1
        fields.append(int(data[start:i]))
    i += 1
    width, height, _ = fields
    return width, height, data[i : i + width * height * 3]


def at(pixels, width, x, y):
    off = (y * width + x) * 3
    return pixels[off], pixels[off + 1], pixels[off + 2]


def mask(pixels, width, height, colour, tol):
    out = []
    for y in range(height):
        row = y * width * 3
        for x in range(width):
            off = row + x * 3
            if (
                abs(pixels[off] - colour[0]) <= tol
                and abs(pixels[off + 1] - colour[1]) <= tol
                and abs(pixels[off + 2] - colour[2]) <= tol
            ):
                out.append((x, y))
    return out


def hull(points):
    """Monotone chain, counter-clockwise in image coordinates."""
    points = sorted(set(points))
    if len(points) <= 2:
        return points

    def cross(o, a, b):
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])

    lower = []
    for p in points:
        while len(lower) >= 2 and cross(lower[-2], lower[-1], p) <= 0:
            lower.pop()
        lower.append(p)
    upper = []
    for p in reversed(points):
        while len(upper) >= 2 and cross(upper[-2], upper[-1], p) <= 0:
            upper.pop()
        upper.append(p)
    return lower[:-1] + upper[:-1]


def area_of(points):
    ordered = hull(points)
    total = 0.0
    for i in range(len(ordered)):
        x1, y1 = ordered[i]
        x2, y2 = ordered[(i + 1) % len(ordered)]
        total += x1 * y2 - x2 * y1
    return abs(total) / 2


def corners(points, k=4):
    """The k hull vertices that enclose the most area.

    A rotated rectangle's outline is a staircase, so its convex hull carries
    hundreds of near-collinear vertices and the four real corners have to be
    picked out of them. Greedily -- but with each trial set's area computed
    from that set's *own* hull, not from an angular sort around a centroid.
    The angular sort put three points in the wrong cyclic order for a twenty
    degree rotation and dropped the one corner the pivot check is about, which
    is the sort of failure that reads as a compositor bug.
    """
    h = hull(points)
    if len(h) <= k:
        return h
    cx = sum(p[0] for p in h) / len(h)
    cy = sum(p[1] for p in h) / len(h)
    chosen = [max(h, key=lambda p: (p[0] - cx) ** 2 + (p[1] - cy) ** 2)]
    chosen.append(
        max(h, key=lambda p: (p[0] - chosen[0][0]) ** 2 + (p[1] - chosen[0][1]) ** 2)
    )
    while len(chosen) < k:
        best, best_area = None, -1.0
        for p in h:
            if p in chosen:
                continue
            trial = area_of(chosen + [p])
            if trial > best_area:
                best, best_area = p, trial
        chosen.append(best)
    return hull(chosen)


def top_left(path, width, height, pixels, window, marker, tol):
    win = mask(pixels, width, height, window, tol)
    mark = mask(pixels, width, height, marker, tol)
    if not win or not mark:
        print(f"{path}: window {len(win)} px, marker {len(mark)} px -- nothing to measure")
        raise SystemExit(1)
    mx = sum(p[0] for p in mark) / len(mark)
    my = sum(p[1] for p in mark) / len(mark)
    quad = corners(win + mark)
    best = min(quad, key=lambda p: (p[0] - mx) ** 2 + (p[1] - my) ** 2)
    print(f"{path}  window {len(win)} px  marker centroid ({mx:.1f},{my:.1f})")
    print("  quad: " + "  ".join(f"({x},{y})" for x, y in quad))
    print(f"  TOP-LEFT = ({best[0]},{best[1]})")


def main():
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    command, path = sys.argv[1], sys.argv[2]
    width, height, pixels = read_ppm(path)

    if command == "at":
        nums = [int(v) for v in sys.argv[3:]]
        for x, y in zip(nums[0::2], nums[1::2]):
            print(f"{path} ({x},{y}) = rgb{at(pixels, width, x, y)}")
        return

    if command == "hist":
        count = int(sys.argv[3]) if len(sys.argv) > 3 else 8
        counts = Counter(
            (pixels[i], pixels[i + 1], pixels[i + 2]) for i in range(0, len(pixels), 3)
        )
        print(f"{path} {width}x{height}")
        for colour, seen in counts.most_common(count):
            print(f"  rgb{colour}  {seen}")
        return

    if command == "corner":
        window = tuple(int(v) for v in sys.argv[3:6])
        marker = tuple(int(v) for v in sys.argv[6:9])
        tol = int(sys.argv[9]) if len(sys.argv) > 9 else 12
        top_left(path, width, height, pixels, window, marker, tol)
        return

    colour = tuple(int(v) for v in sys.argv[3:6])
    tol = int(sys.argv[6]) if len(sys.argv) > 6 else 12
    points = mask(pixels, width, height, colour, tol)
    if not points:
        print(f"{path}: nothing within {tol} of rgb{colour}")
        raise SystemExit(1)

    if command == "span":
        xs = [p[0] for p in points]
        ys = [p[1] for p in points]
        print(
            f"{path} rgb{colour} n={len(points)} "
            f"x {min(xs)}..{max(xs)}  y {min(ys)}..{max(ys)}  "
            f"centroid ({sum(xs) / len(xs):.1f},{sum(ys) / len(ys):.1f})"
        )
        return

    if command == "quad":
        print(f"{path} rgb{colour} n={len(points)}")
        print("  quad: " + "  ".join(f"({x},{y})" for x, y in corners(points)))
        return

    raise SystemExit(__doc__)


main()
