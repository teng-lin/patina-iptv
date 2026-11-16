#!/usr/bin/env python3
"""Lengthen generated smoke media without encoding or copying more segments."""

import argparse
import math
from pathlib import Path
import re

MARKER = "#PATINA-SYNTHETIC-REPEAT"


def repeat_manifest(text, cycles):
    if not 1 <= cycles <= 240:
        raise ValueError("fixture cycles must be between 1 and 240")
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines or lines[0] != "#EXTM3U" or lines[-1] != "#EXT-X-ENDLIST":
        raise ValueError("expected a complete generated VOD manifest")
    if "#EXT-X-PLAYLIST-TYPE:VOD" not in lines:
        raise ValueError("expected a generated VOD playlist")
    start = next((i for i, line in enumerate(lines) if line.startswith("#EXTINF:")), None)
    if start is None:
        raise ValueError("fixture contains no segments")
    header, segments = lines[:start], lines[start:-1]
    if "#EXT-X-DISCONTINUITY" in segments:
        if MARKER not in header:
            raise ValueError("unexpected discontinuity in original fixture")
        segments = segments[:segments.index("#EXT-X-DISCONTINUITY")]
    if len(segments) % 2:
        raise ValueError("expected generated duration/segment pairs")
    for duration, uri in zip(segments[::2], segments[1::2]):
        if not duration.startswith("#EXTINF:") or not re.fullmatch(r"seg[0-9]+\.ts", uri):
            raise ValueError("expected generated local transport-stream segments")
        seconds = float(duration.removeprefix("#EXTINF:").split(",", 1)[0])
        if not math.isfinite(seconds) or seconds <= 0:
            raise ValueError("invalid generated segment duration")
    header = [line for line in header if line != MARKER]
    result = header + [MARKER]
    for cycle in range(cycles):
        if cycle:
            # Encoded timestamps restart at each reuse of the original media.
            result.append("#EXT-X-DISCONTINUITY")
        result.extend(segments)
    return "\n".join(result + ["#EXT-X-ENDLIST", ""])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--cycles", type=int, default=120)
    args = parser.parse_args()
    paths = sorted(args.directory.glob("ch*/index.m3u8"))
    if not paths:
        parser.error("no generated channel manifests found")
    for path in paths:
        path.write_text(repeat_manifest(path.read_text(), args.cycles))
    print(f"Repeated {len(paths)} generated HLS fixtures for {args.cycles} cycles")


if __name__ == "__main__":
    main()
