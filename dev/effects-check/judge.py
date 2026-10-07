#!/usr/bin/env python3
"""Pixel judgements on SOLIUM_CAPTURE frames (binary PPM), for effects-check.

  judge.py differs A B FX FY FW FH        exit 0 if any pixel in the region differs
  judge.py same A B FX FY FW FH [TOL]     exit 0 if every channel is within TOL (0)
  judge.py edges A FX FY FW FH            print the region's edge energy: the sum of
                                          |difference| between horizontal neighbours
  judge.py trace FILE FIELD               print the number of pass records in a
                                          SOLIUM_TRACE file whose FIELD is non-zero,
                                          and the field's total
  judge.py quiet FILE FIELD MS            exit 0 if no pass record within MS ms of
                                          the trace's last one (by t_ns) has FIELD
                                          non-zero

A region number is a fraction of the frame (0..1), so a check does not depend
on the nested window's size, or pixels when it ends in `px` (`408px`), for a
check against a window placed at a known rectangle.
"""
import json
import sys


def read(path):
    with open(path, "rb") as f:
        data = f.read()
    parts, at = [], 0
    while len(parts) < 4:
        while data[at:at + 1].isspace():
            at += 1
        if data[at:at + 1] == b"#":
            at = data.index(b"\n", at)
            continue
        end = at
        while not data[end:end + 1].isspace():
            end += 1
        parts.append(data[at:end])
        at = end
    assert parts[0] == b"P6", f"{path}: not a binary PPM"
    width, height = int(parts[1]), int(parts[2])
    return width, height, data[at + 1:]


def along(value, side):
    """One region number: pixels when it ends in `px`, else a fraction of `side`."""
    if value.endswith("px"):
        return int(value[:-2])
    return int(float(value) * side)


def region(width, height, fx, fy, fw, fh):
    x0, y0 = along(fx, width), along(fy, height)
    return x0, y0, max(1, along(fw, width)), max(1, along(fh, height))


def passes(path):
    """The pass records of a SOLIUM_TRACE file, in order: its lines that have a
    `pass`, leaving out each flip's line and a line cut short by a stop."""
    records = []
    with open(path) as f:
        for line in f:
            try:
                record = json.loads(line)
            except ValueError:
                continue
            if "pass" in record:
                records.append(record)
    return records


def pixels(frame, box):
    width, _height, data = frame
    x0, y0, w, h = box
    for y in range(y0, y0 + h):
        row = (y * width + x0) * 3
        yield data[row:row + w * 3]


def main(argv):
    what = argv[1] if len(argv) > 1 else ""
    if what in ("differs", "same"):
        a, b = read(argv[2]), read(argv[3])
        box = region(a[0], a[1], *argv[4:8])
        tolerance = int(argv[8]) if what == "same" and len(argv) > 8 else 0
        worst = max(abs(p - q) for ra, rb in zip(pixels(a, box), pixels(b, box)) for p, q in zip(ra, rb))
        print(f"largest channel difference {worst}")
        if what == "differs":
            return 0 if worst > 0 else 1
        return 0 if worst <= tolerance else 1
    if what == "edges":
        a = read(argv[2])
        box = region(a[0], a[1], *argv[3:7])
        print(sum(abs(row[i] - row[i + 3]) for row in pixels(a, box) for i in range(len(row) - 3)))
        return 0
    if what == "trace":
        values = [record.get(argv[3], 0) for record in passes(argv[2])]
        print(sum(1 for value in values if value), sum(values))
        return 0
    if what == "quiet":
        records = passes(argv[2])
        if not records:
            print("no pass records")
            return 1
        since = records[-1]["t_ns"] - int(argv[4]) * 1_000_000
        loud = [record for record in records if record["t_ns"] >= since and record.get(argv[3], 0)]
        print(f"{len(loud)} pass(es) with {argv[3]} in the last {argv[4]} ms")
        return 1 if loud else 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
