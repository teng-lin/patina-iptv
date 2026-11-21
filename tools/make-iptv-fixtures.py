#!/usr/bin/env python3
"""Synthetic IPTV fixtures: an M3U playlist and a matching XMLTV guide.

Everything is invented (names, titles, descriptions) and points at the
local mock server. By default the files go to fixtures/hls/iptv/, which
tools/mock-server.py serves as /iptv/ (it serves fixtures/hls as its root):

  python3 -I tools/make-iptv-fixtures.py            # 5000 channels
  tools/mock-server.py 8089 &                       # serves /chN, /archive, /iptv
  # emulator: http://10.0.2.2:8089/iptv/playlist.m3u
  # desktop:  python3 -I tools/make-iptv-fixtures.py --host 127.0.0.1:8089

Writes playlist.m3u (url-tvg -> guide.xml.gz), guide.xml.gz (now-7d to
now+2d, variable programme lengths) and logos/gNN.png (one per group).
Streams are /ch{N%3}/index.m3u8 from tools/make-hls-fixtures.sh; every
--catchup-every'th channel declares catchup="default" with an /archive/
template (the mock server maps /archive/* to /*). Output is deterministic
for a given --seed and --now.
"""

import argparse
import gzip
import os
import random
import struct
import sys
import time
import zlib
from xml.sax.saxutils import escape, quoteattr

GROUPS = [
    "News", "Sports", "Movies", "Series", "Kids", "Music", "Documentary",
    "Science", "Nature", "Travel", "Cooking", "Comedy", "Drama", "Classics",
    "Regional", "Weather", "Business", "Gaming", "Lifestyle", "Education",
    "Animation", "Talk", "Radio", "Shopping",
]
ADJECTIVES = [
    "Quiet", "Bright", "Hidden", "Northern", "Golden", "Late", "Early",
    "Silver", "Wild", "Little", "Grand", "Distant", "Open", "Secret",
    "Rapid", "Gentle", "Midnight", "Summer", "Winter", "Coastal",
]
NOUNS = [
    "Harbour", "Orbit", "Kitchen", "Valley", "Workshop", "Signal", "Garden",
    "Circuit", "Lantern", "Meadow", "Atlas", "Engine", "Compass", "Archive",
    "Studio", "Horizon", "Bridge", "Market", "Canyon", "Library",
]
VERBS = ["explores", "follows", "revisits", "builds", "tests", "maps", "repairs", "visits"]
THINGS = [
    "an old lighthouse", "a mountain railway", "three invented recipes",
    "a tiny robot", "the night sky", "a forgotten garden", "a model city",
    "a river delta", "a vintage radio", "a desert festival",
]
# Minutes, weighted towards half-hours and hours (mean about 60).
LENGTHS = [15, 20, 30, 30, 45, 60, 60, 60, 90, 120]


def png(rgb, size=64):
    """A solid-colour RGB PNG (stdlib only)."""
    row = b"\x00" + bytes(rgb) * size
    raw = zlib.compress(row * size, 9)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", raw) + chunk(b"IEND", b"")


def xmltv_time(t):
    return time.strftime("%Y%m%d%H%M%S +0000", time.gmtime(t))


def group_name(gi):
    return GROUPS[gi % len(GROUPS)] + ("" if gi < len(GROUPS) else f" {gi}")


def channel_name(n, group):
    return f"Synth {group} {n:04d}"


def write_playlist(path, args, base):
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(f'#EXTM3U url-tvg="{base}/iptv/guide.xml.gz"\n')
        for n in range(args.channels):
            gi = n % args.groups
            group = group_name(gi)
            name = channel_name(n, group)
            stream = f"{base}/ch{n % 3}/index.m3u8"
            attrs = [
                f'tvg-id="ch{n}.synthetic.example"',
                f"tvg-name={quoteattr(name)}",
                f'tvg-logo="{base}/iptv/logos/g{gi:02d}.png"',
                f'tvg-chno="{n + 1}"',
                f"group-title={quoteattr(group)}",
            ]
            if args.catchup_every and n % args.catchup_every == 0:
                attrs += [
                    'catchup="default"',
                    'catchup-days="3"',
                    f'catchup-source="{base}/archive/ch{n % 3}/index.m3u8'
                    '?utc={utc}&duration={duration}"',
                ]
            f.write(f"#EXTINF:-1 {' '.join(attrs)},{name}\n{stream}\n")


def write_guide(path, args, now):
    start = (now - args.days_back * 86400) // 3600 * 3600
    end = now + args.days_ahead * 86400
    programs = 0
    with gzip.open(path, "wt", encoding="utf-8", compresslevel=6, newline="\n") as f:
        f.write('<?xml version="1.0" encoding="UTF-8"?>\n')
        f.write('<tv generator-info-name="iptv-rs synthetic fixtures">\n')
        for n in range(args.channels):
            group = group_name(n % args.groups)
            f.write(
                f'<channel id="ch{n}.synthetic.example">'
                f"<display-name>{escape(channel_name(n, group))}</display-name></channel>\n"
            )
        for n in range(args.channels):
            rng = random.Random(args.seed * 1_000_003 + n)
            cid = f"ch{n}.synthetic.example"
            t = start - rng.choice(LENGTHS) * 60 // 2
            episode = rng.randrange(1, 40)
            while t < end:
                length = rng.choice(LENGTHS) * 60
                title = f"{rng.choice(ADJECTIVES)} {rng.choice(NOUNS)}"
                desc = f"{title} {rng.choice(VERBS)} {rng.choice(THINGS)}. Episode {episode}."
                f.write(
                    f'<programme start="{xmltv_time(t)}" stop="{xmltv_time(t + length)}" '
                    f'channel="{cid}"><title>{escape(title)}</title>'
                    f"<desc>{escape(desc)}</desc>"
                    f'<episode-num system="onscreen">E{episode}</episode-num></programme>\n'
                )
                t += length
                episode += 1
                programs += 1
        f.write("</tv>\n")
    return programs


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--channels", type=int, default=5000)
    p.add_argument("--groups", type=int, default=24)
    p.add_argument("--host", default="10.0.2.2:8089", help="host:port the player uses for the mock server")
    p.add_argument("--out", default=os.path.join(root, "fixtures", "hls", "iptv"))
    p.add_argument("--days-back", type=int, default=7)
    p.add_argument("--days-ahead", type=int, default=2)
    p.add_argument("--catchup-every", type=int, default=5, help="every Nth channel has catch-up (0 = none)")
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--now", type=int, default=None, help="unix time to centre the guide on (default: now)")
    args = p.parse_args()
    if args.channels < 1 or args.groups < 1:
        p.error("--channels and --groups must be positive")

    now = int(time.time()) if args.now is None else args.now
    base = f"http://{args.host}"
    os.makedirs(os.path.join(args.out, "logos"), exist_ok=True)
    for gi in range(args.groups):
        rng = random.Random(args.seed + gi)
        color = (rng.randrange(40, 230), rng.randrange(40, 230), rng.randrange(40, 230))
        with open(os.path.join(args.out, "logos", f"g{gi:02d}.png"), "wb") as f:
            f.write(png(color))
    playlist = os.path.join(args.out, "playlist.m3u")
    guide = os.path.join(args.out, "guide.xml.gz")
    write_playlist(playlist, args, base)
    programs = write_guide(guide, args, now)
    print(
        f"{playlist}: {args.channels} channels in {args.groups} groups\n"
        f"{guide}: {programs} programmes, {os.path.getsize(guide) // 1024} KiB\n"
        f"playlist URL: {base}/iptv/playlist.m3u",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
