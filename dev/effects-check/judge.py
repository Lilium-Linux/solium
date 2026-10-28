#!/usr/bin/env python3
"""Pixel judgements on SOLIUM_CAPTURE frames (binary PPM), for effects-check.

  judge.py differs A B FX FY FW FH        exit 0 if any pixel in the region differs
  judge.py same A B FX FY FW FH [TOL]     exit 0 if every channel is within TOL (0)
  judge.py edges A FX FY FW FH            print the region's edge energy: the sum of
                                          |difference| between horizontal neighbours
  judge.py sharpness A FX FY FW FH        print the region's gradient energy: the sum
                                          of squared differences between horizontal
                                          and between vertical neighbours. A blur
                                          lowers it even where it only widens a step,
                                          which leaves `edges` unchanged: a step of
                                          height H spread over n pixels sums to H
                                          either way, and to H*H/n squared
  judge.py trace FILE FIELD               print the number of pass records in a
                                          SOLIUM_TRACE file whose FIELD is non-zero,
                                          and the field's total
  judge.py after FILE FIELD               print the number of pass records from the
                                          first whose FIELD is non-zero to the end,
                                          and the field's total over them (0 0 when
                                          none is)
  judge.py only FILE FIELD WITH           print the number of pass records whose FIELD
                                          is non-zero and how many of those have WITH
                                          non-zero too; exit 0 if all of them do
  judge.py since FILE FIELD MS            print the number of pass records within MS ms
                                          of the trace's last one (by t_ns), and
                                          FIELD's total over them
  judge.py quiet FILE FIELD MS            exit 0 if no pass record within MS ms of
                                          the trace's last one (by t_ns) has FIELD
                                          non-zero
  judge.py most FILE FIELD MS             print the number of pass records within MS
                                          ms of the trace's last one (by t_ns), the
                                          largest FIELD among them, and how many of
                                          them have FIELD non-zero
  judge.py first_lit DIR                  print the first capture in DIR (f-*, in
                                          order) with any channel above 31, so not
                                          only the clear colour (13, 13, 15) a held
                                          first frame draws; nothing when none is
  judge.py size A                         print the frame's width and height
  judge.py box A R G B                    print the bounding box (x y w h) of the
                                          pixels within 8 of R G B on every channel;
                                          exit 1 when there are none
  judge.py live SHARP FRAME REF           of the pixels where FRAME differs from
                                          SHARP (the wallpaper with no rule) by more
                                          than 40 on a channel, so inside a blurred
                                          window, print how many are within 6 of REF
                                          (the wallpaper blurred everywhere) on every
                                          channel; exit 0 when at least 95 % of at
                                          least 20000 are: the window shows the
                                          backdrop blurred where each pixel is

A region number is a fraction of the frame (0..1), so a check does not depend
on the nested window's size, or pixels when it ends in `px` (`408px`), for a
check against a window placed at a known rectangle.
"""
import glob
import json
import os
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
    if what == "box":
        width, height, data = read(argv[2])
        want = [int(value) for value in argv[3:6]]
        xs, ys = [], []
        for y in range(height):
            row = y * width * 3
            for x in range(width):
                at = row + x * 3
                if all(abs(data[at + i] - want[i]) <= 8 for i in range(3)):
                    xs.append(x)
                    ys.append(y)
        if not xs:
            print("none")
            return 1
        print(min(xs), min(ys), max(xs) - min(xs) + 1, max(ys) - min(ys) + 1)
        return 0
    if what == "edges":
        a = read(argv[2])
        box = region(a[0], a[1], *argv[3:7])
        print(sum(abs(row[i] - row[i + 3]) for row in pixels(a, box) for i in range(len(row) - 3)))
        return 0
    if what == "sharpness":
        a = read(argv[2])
        rows = list(pixels(a, region(a[0], a[1], *argv[3:7])))
        across = sum((row[i] - row[i + 3]) ** 2 for row in rows for i in range(len(row) - 3))
        down = sum((p - q) ** 2 for upper, lower in zip(rows, rows[1:]) for p, q in zip(upper, lower))
        print(across + down)
        return 0
    if what == "trace":
        values = [record.get(argv[3], 0) for record in passes(argv[2])]
        print(sum(1 for value in values if value), sum(values))
        return 0
    if what == "after":
        records = passes(argv[2])
        first = next((at for at, record in enumerate(records) if record.get(argv[3], 0)), len(records))
        since = records[first:]
        print(len(since), sum(record.get(argv[3], 0) for record in since))
        return 0
    if what == "only":
        loud = [record for record in passes(argv[2]) if record.get(argv[3], 0)]
        both = [record for record in loud if record.get(argv[4], 0)]
        print(len(loud), len(both))
        return 0 if len(both) == len(loud) else 1
    if what == "since":
        records = passes(argv[2])
        since = records[-1]["t_ns"] - int(argv[4]) * 1_000_000 if records else 0
        late = [record for record in records if record["t_ns"] >= since]
        print(len(late), sum(record.get(argv[3], 0) for record in late))
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
    if what == "most":
        records = passes(argv[2])
        since = records[-1]["t_ns"] - int(argv[4]) * 1_000_000 if records else 0
        late = [record.get(argv[3], 0) for record in records if record["t_ns"] >= since]
        print(len(late), max(late, default=0), sum(1 for value in late if value))
        return 0
    if what == "first_lit":
        for path in sorted(glob.glob(os.path.join(argv[2], "f-*"))):
            if max(read(path)[2]) > 31:
                print(path)
                return 0
        return 1
    if what == "live":
        sharp, frame, ref = read(argv[2]), read(argv[3]), read(argv[4])
        box = region(frame[0], frame[1], "0", "0", "1", "1")
        inside = matched = 0
        for rs, rf, rr in zip(pixels(sharp, box), pixels(frame, box), pixels(ref, box)):
            for at in range(0, len(rf), 3):
                if max(abs(rf[at + c] - rs[at + c]) for c in range(3)) > 40:
                    inside += 1
                    if max(abs(rf[at + c] - rr[at + c]) for c in range(3)) <= 6:
                        matched += 1
        print(f"{matched} of {inside} window pixels are the wallpaper blurred where they are")
        return 0 if inside >= 20000 and matched * 100 >= inside * 95 else 1
    if what == "size":
        width, height, _data = read(argv[2])
        print(width, height)
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
