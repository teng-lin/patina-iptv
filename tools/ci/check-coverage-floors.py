#!/usr/bin/env python3
"""Check line coverage floors for each library crate."""
from collections import defaultdict
from pathlib import Path
import re
import sys

FLOORS = {
    "iptv-core": 94.0,
    "iptv-nav": 97.0,
    "iptv-settings": 97.0,
    "iptv-epg": 93.0,
    "iptv-providers": 93.0,
    "iptv-guide": 89.0,
    "iptv-import": 88.0,
    "iptv-net": 90.0,
    "iptv-store-sqlite": 92.0,
    "iptv-search": 94.0,
    "iptv-player-mpv": 94.0,
    "iptv-library": 91.0,
    "iptv-runtime": 49.5,
}


def parse_summary(summary_text):
    totals = defaultdict(lambda: [0, 0])
    for line in summary_text.splitlines():
        m = re.search(r"crates/([a-z0-9_-]+)/", line)
        if not m:
            continue
        crate = m.group(1)
        if crate not in FLOORS:
            continue
        parts = line.split()
        if len(parts) >= 10:
            try:
                lines_total = int(parts[-6])
                lines_missed = int(parts[-5])
                totals[crate][0] += lines_total
                totals[crate][1] += lines_missed
            except ValueError:
                pass
    return totals


def evaluate(totals, floors=FLOORS):
    results = {}
    failed = []
    for crate, floor in sorted(floors.items()):
        tot, miss = totals.get(crate, [0, 0])
        if tot == 0:
            cov = 0.0
            failed.append((crate, cov, floor))
            results[crate] = (cov, floor, False, 0, 0)
        else:
            cov = 100.0 * (tot - miss) / tot
            passed = cov >= floor
            if not passed:
                failed.append((crate, cov, floor))
            results[crate] = (cov, floor, passed, tot - miss, tot)
    return results, failed


def main(summary_path="target/coverage/summary.txt"):
    text = Path(summary_path).read_text(encoding="utf-8")
    totals = parse_summary(text)
    results, failed = evaluate(totals)
    print(f"{'Crate':<20} {'Covered':<10} {'Floor':<10} {'Status'}")
    print("-" * 50)
    for crate, (cov, floor, passed, covered, total) in sorted(results.items()):
        status = "OK" if passed else "FAILED"
        print(f"{crate:<20} {cov:5.2f}%    {floor:5.1f}%     {status} ({covered}/{total} lines)")
    if failed:
        print("\nCoverage floor check failed for:", file=sys.stderr)
        for crate, cov, floor in failed:
            print(f"  {crate}: {cov:.2f}% < floor {floor:.1f}%", file=sys.stderr)
        sys.exit(1)
    print("\nAll library crates met or exceeded their coverage floors.")


if __name__ == "__main__":
    path = sys.argv[1] if len(sys.argv) > 1 else "target/coverage/summary.txt"
    main(path)
