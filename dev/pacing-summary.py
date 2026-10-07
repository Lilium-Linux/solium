#!/usr/bin/env python3
"""Summarise a SOLIUM_TRACE file: what Phase 0's runs are judged by.

    dev/pacing-summary.py trace.jsonl [--from NS] [--to NS]
                          [--expect-captures N] [--totals stderr.log]

Reads the pass and flip records inside [--from, --to] (CLOCK_MONOTONIC
nanoseconds; dev/README.md, "Measuring frame pacing on a TTY", lists the
fields) and prints, for the passes, their number and rate, the spread of each
time, the misses, the GPU's status and clocks, the captures and the effect
chain runs per pass and Qt's costliest scenes; for the flips, the vblanks each
monitor's flips were late by. Standard library only.

Exit status: 0; 2 when fewer than half the passes captured --expect-captures
windows (the scene was not the one asked for); 3 when there were no passes.
"""
import argparse
import json
import re
import sys
from collections import Counter, defaultdict

TIMES = ["total_us", "prep_us", "qml_us", "elements_us", "gles_us", "commit_us", "gpu_us", "gpu_prep_us", "gpu_effects_us"]


def spread(values):
    if not values:
        return "-"
    ordered = sorted(values)

    def at(quantile):
        return ordered[min(len(ordered) - 1, int(quantile * len(ordered)))]

    return f"p50={at(0.50)} p90={at(0.90)} p99={at(0.99)} max={ordered[-1]}"


def main():
    parser = argparse.ArgumentParser(description="Summarise a SOLIUM_TRACE file.")
    parser.add_argument("trace")
    parser.add_argument("--from", dest="start", type=int, default=0)
    parser.add_argument("--to", dest="end", type=int, default=None)
    parser.add_argument("--expect-captures", type=int, default=None)
    parser.add_argument("--totals", default=None, help="a log holding the session's 'pacing:' totals line")
    args = parser.parse_args()
    end = args.end if args.end is not None else 2**63

    passes, flips, broken = [], [], 0
    with open(args.trace, encoding="utf-8") as trace:
        for line in trace:
            try:
                record = json.loads(line)
            except ValueError:
                broken += 1
                continue
            if "flip" in record:
                if args.start <= record.get("at_ns", 0) <= end:
                    flips.append(record)
            elif "pass" in record and args.start <= record.get("t_ns", 0) <= end:
                passes.append(record)

    if not passes:
        print(f"no passes between {args.start} and {end} ({broken} unreadable lines)")
        return 3
    seconds = (end - args.start) / 1e9 if args.end is not None else None
    rate = f"{len(passes) / seconds:.1f}/s" if seconds else "-"
    print(f"passes {len(passes)}  rate {rate}  unreadable lines {broken}")
    timed = [one for one in passes if one.get("gpu") == "ok"]
    for field in TIMES:
        pool = timed if field.startswith("gpu") else passes
        if pool and not any(field in one for one in pool):
            continue
        print(f"  {field:<12} {spread([one.get(field, 0) for one in pool])}")
    print(f"missed {sum(1 for one in passes if one.get('missed'))} of {len(passes)} passes")
    late, flipped = defaultdict(int), Counter()
    for flip in flips:
        late[flip.get("monitor", "?")] += flip.get("late", 0)
        flipped[flip.get("monitor", "?")] += 1
    for monitor in sorted(flipped):
        print(f"late   {late[monitor]} vblanks over {flipped[monitor]} flips on {monitor}")
    statuses = Counter(one.get("gpu", "?") for one in passes)
    print("gpu    " + ", ".join(f"{name}={count}" for name, count in statuses.most_common()))
    captures = Counter(one.get("captures", 0) for one in passes)
    print("captures per pass  " + ", ".join(f"{count} in {seen}" for count, seen in sorted(captures.items())))
    runs = sorted(one.get("effect_runs", 0) for one in passes)
    print(f"effect_runs per pass  median {runs[len(runs) // 2]}  total {sum(runs)}")
    states = Counter(one.get("pstate", -1) for one in passes)
    print(f"clocks {passes[0].get('clocks', '?')}  pstates " + ", ".join(f"P{state}:{seen}" for state, seen in sorted(states.items())))
    print(f"  gpu_mhz {spread([one.get('gpu_mhz', 0) for one in passes])}")
    print(f"  mem_mhz {spread([one.get('mem_mhz', 0) for one in passes])}")
    scenes = defaultdict(list)
    for one in passes:
        for label, micros in one.get("qml", {}).items():
            scenes[label].append(micros)
    for label, values in sorted(scenes.items(), key=lambda item: -sum(item[1]))[:5]:
        print(f"  qml {label:<28} in {len(values):>6} passes  {spread(values)}")
    if args.totals:
        with open(args.totals, encoding="utf-8", errors="replace") as log:
            for line in log:
                if "pacing:" in line and "passes=" in line:
                    print("totals " + " ".join(re.findall(r"\b(?:passes|missed|late)=\d+", line)))
    if args.expect_captures is not None and captures.get(args.expect_captures, 0) * 2 < len(passes):
        print(f"WRONG SCENE: {captures.get(args.expect_captures, 0)} of {len(passes)} passes captured {args.expect_captures} windows")
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
