#!/usr/bin/env python3
"""Local mock IPTV server for development (synthetic data only).

Serves the synthetic HLS channels from fixtures/hls (see
make-hls-fixtures.sh) and answers catch-up requests: any path under
/archive/ is served as the same path without the prefix, whatever the
query string, so the app can play "archived" programs. Every request is
logged with its full query to stdout, so catch-up URL construction can be
checked against what the app actually sent.

Usage: tools/mock-server.py [--loop] [--window SEGMENTS] [port] [directory]
The Android emulator reaches the host at http://10.0.2.2:<port>/. Playlists
(*.m3u) name that address; they are served with it replaced by the host
the request came to, so the desktop app (127.0.0.1) plays them too.

By default each channel's index.m3u8 is served as generated: a VOD playlist
that ends (#EXT-X-ENDLIST), so a live channel built on it ends and the app
reconnects. With --loop each channel is a live stream that never ends: its
index.m3u8 is a sliding window of the newest SEGMENTS segments, without
#EXT-X-ENDLIST, whose media sequence numbers keep increasing with the
wall clock as the generated segments repeat. The app joins a live stream's
segments into one transport stream, so each repeat is served with its
timestamps (PTS, DTS, PCR) moved on by the media's length and its
continuity counters carried on: the joined stream has one timeline and no
discontinuity at the wrap. Catch-up (/archive/) stays the ending VOD.
"""

import argparse
import bisect
import http.server
import io
import itertools
import math
import os
import re
import sys
import threading
import time
import urllib.parse

TS_PACKET = 188
TICKS = 90_000  # MPEG-TS timestamps count a 90 kHz clock.
WRAP = 1 << 33  # PTS, DTS and the PCR base are 33-bit counters.
SEGMENT = re.compile(r"seg[0-9]+\.ts")
LIVE_SEGMENT = re.compile(r"live-([0-9]+)\.ts")
# PES stream IDs whose header has no PTS/DTS fields (ISO/IEC 13818-1 2.4.3.7).
NO_TIMESTAMPS = {0xBC, 0xBE, 0xBF, 0xF0, 0xF1, 0xF2, 0xF8, 0xFF}


def read_timestamp(field):
    return (
        ((field[0] >> 1) & 0x07) << 30
        | field[1] << 22
        | (field[2] >> 1) << 15
        | field[3] << 7
        | field[4] >> 1
    )


def write_timestamp(field, value):
    """Writes a 5-byte PTS/DTS field in place, keeping its prefix bits."""
    field[0] = (field[0] & 0xF0) | ((value >> 29) & 0x0E) | 0x01
    field[1] = (value >> 22) & 0xFF
    field[2] = ((value >> 14) & 0xFE) | 0x01
    field[3] = (value >> 7) & 0xFF
    field[4] = ((value << 1) & 0xFE) | 0x01


def shift_ts(data, ticks, counters):
    """Returns transport stream `data` with every PTS, DTS and PCR base moved
    on by `ticks` (90 kHz, modulo 2^33) and each PID's continuity counter
    moved on by `counters[pid]` (modulo 16)."""
    if len(data) % TS_PACKET:
        raise ValueError("expected whole transport-stream packets")
    out = bytearray(data)
    for start in range(0, len(out), TS_PACKET):
        packet = memoryview(out)[start:start + TS_PACKET]
        if packet[0] != 0x47:
            raise ValueError("lost transport-stream sync")
        pid = (packet[1] & 0x1F) << 8 | packet[2]
        control = packet[3] >> 4 & 0x03
        offset = 4
        if control & 0x01:
            packet[3] = (packet[3] & 0xF0) | ((packet[3] + counters.get(pid, 0)) & 0x0F)
        if control & 0x02:
            length = packet[4]
            if length and packet[5] & 0x10 and length >= 7:
                pcr = packet[6:12]
                base = pcr[0] << 25 | pcr[1] << 17 | pcr[2] << 9 | pcr[3] << 1 | pcr[4] >> 7
                base = (base + ticks) % WRAP
                pcr[0] = base >> 25 & 0xFF
                pcr[1] = base >> 17 & 0xFF
                pcr[2] = base >> 9 & 0xFF
                pcr[3] = base >> 1 & 0xFF
                pcr[4] = (base & 0x01) << 7 | (pcr[4] & 0x7F)
            offset += 1 + length
        if not (control & 0x01 and packet[1] & 0x40) or offset + 14 > TS_PACKET:
            continue
        pes = packet[offset:]
        if pes[:3] != b"\x00\x00\x01" or pes[3] in NO_TIMESTAMPS or pes[6] >> 6 != 0x02:
            continue
        flags = pes[7] >> 6
        if flags & 0x02:
            write_timestamp(pes[9:14], (read_timestamp(pes[9:14]) + ticks) % WRAP)
        if flags == 0x03 and offset + 19 <= TS_PACKET:
            write_timestamp(pes[14:19], (read_timestamp(pes[14:19]) + ticks) % WRAP)
    return bytes(out)


ADTS_RATES = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350]


def adts_ticks(payload):
    """How long the AAC (ADTS) frames that make up a PES payload play, or
    None when the payload is not ADTS audio."""
    ticks = 0
    at = 0
    while at + 7 <= len(payload):
        header = payload[at:at + 7]
        rate = header[2] >> 2 & 0x0F
        length = (header[3] & 0x03) << 11 | header[4] << 3 | header[5] >> 5
        if header[0] != 0xFF or header[1] & 0xF6 != 0xF0 or rate >= len(ADTS_RATES) or length < 7:
            return None
        ticks += 1024 * ((header[6] & 0x03) + 1) * TICKS / ADTS_RATES[rate]
        at += length
    return round(ticks) if at == len(payload) and ticks else None


class Media:
    """What a channel's generated segments hold, read once: each PID's
    packets that carry a payload (and so advance its continuity counter),
    and when each elementary stream starts and ends."""

    def __init__(self):
        self.counts = {}
        self.starts = {}  # pid -> PTS values of its PES packets
        self.ends = {}  # pid -> where its audio frames end, when it is audio
        self.pending = {}  # pid -> (PTS, payload so far) of an audio PES

    def add(self, data):
        for start in range(0, len(data) - TS_PACKET + 1, TS_PACKET):
            packet = data[start:start + TS_PACKET]
            if not packet[3] & 0x10:
                continue
            pid = (packet[1] & 0x1F) << 8 | packet[2]
            self.counts[pid] = self.counts.get(pid, 0) + 1
            offset = 4 + (1 + packet[4] if packet[3] & 0x20 else 0)
            payload = packet[offset:]
            if not packet[1] & 0x40:
                if pid in self.pending:
                    self.pending[pid][1].extend(payload)
                continue
            self.finish(pid)
            if (len(payload) >= 14 and payload[:3] == b"\x00\x00\x01" and payload[3] not in NO_TIMESTAMPS
                    and payload[6] >> 6 == 0x02 and payload[7] & 0x80):
                pts = read_timestamp(payload[9:14])
                self.starts.setdefault(pid, []).append(pts)
                if 0xC0 <= payload[3] <= 0xDF:  # an MPEG audio stream
                    self.pending[pid] = (pts, bytearray(payload[9 + payload[8]:]))

    def finish(self, pid):
        if pid in self.pending:
            pts, payload = self.pending.pop(pid)
            ticks = adts_ticks(payload)
            if ticks:
                self.ends[pid] = max(self.ends.get(pid, 0), pts + ticks)

    def length(self):
        """The longest stream's length, from its first presentation to the
        end of its last frame. AAC frames rarely end where video frames do,
        and each audio PES holds several of them; a video PES holds one
        frame, which lasts the smallest step between them."""
        for pid in list(self.pending):
            self.finish(pid)
        longest = 0
        for pid, values in self.starts.items():
            ordered = sorted(set(values))
            steps = [b - a for a, b in zip(ordered, ordered[1:])]
            end = self.ends.get(pid) or (ordered[-1] + min(steps) if steps else None)
            if end:
                longest = max(longest, end - ordered[0])
        return longest


class Loop:
    """One generated channel replayed as an endless live stream.

    Times are in 90 kHz ticks from the stream's start. Segment `seq` is the
    original segment `seq % n` in repeat `seq // n`; it is published (listed)
    once its end time has passed.
    """

    def __init__(self, manifest, read_segment, window):
        lines = [line.strip() for line in manifest.splitlines() if line.strip()]
        if not lines or lines[0] != "#EXTM3U" or lines[-1] != "#EXT-X-ENDLIST":
            raise ValueError("expected a complete generated VOD playlist")
        if "#EXT-X-PLAYLIST-TYPE:VOD" not in lines:
            raise ValueError("expected a generated VOD playlist")
        if window < 1:
            raise ValueError("the live window needs at least one segment")
        self.target = None
        self.durations = []
        self.names = []
        duration = None
        for line in lines[1:-1]:
            if line.startswith("#EXT-X-TARGETDURATION:"):
                self.target = int(line.split(":", 1)[1])
            elif line.startswith("#EXTINF:"):
                seconds = float(line[len("#EXTINF:"):].split(",", 1)[0])
                if not math.isfinite(seconds) or seconds <= 0:
                    raise ValueError("invalid generated segment duration")
                duration = round(seconds * TICKS)
            elif line == "#EXT-X-DISCONTINUITY":
                raise ValueError("unexpected discontinuity in a generated playlist")
            elif not line.startswith("#"):
                if duration is None or not SEGMENT.fullmatch(line):
                    raise ValueError("expected generated local transport-stream segments")
                self.durations.append(duration)
                self.names.append(line)
                duration = None
        if not self.names or not self.target:
            raise ValueError("expected generated segments and a target duration")
        media = Media()
        for name in self.names:
            media.add(read_segment(name))
        # Repeat k is moved on by k periods. A period as long as the longest
        # stream keeps every stream rising: when the audio outlasts the
        # video by part of a frame, the video has that much of a gap at the
        # wrap rather than the audio going back. The last segment lasts the
        # difference longer, so the clock and the media keep step.
        extra = max(media.length() - sum(self.durations), 0)
        self.durations[-1] += extra
        self.target = max(self.target, round(self.durations[-1] / TICKS))
        self.ends = list(itertools.accumulate(self.durations))
        self.period = self.ends[-1]
        self.counts = media.counts
        self.window = window
        self._read = read_segment

    def end(self, seq):
        """The time segment `seq` ends."""
        repeat, index = divmod(seq, len(self.names))
        return repeat * self.period + self.ends[index]

    def published(self, elapsed):
        """How many segments have ended by `elapsed`."""
        repeat, rest = divmod(max(elapsed, 0), self.period)
        return repeat * len(self.names) + bisect.bisect_right(self.ends, rest)

    def preroll(self):
        """The time by which a full window has been published, so a player
        that joins at once sees as much as one that joins later."""
        return self.end(self.window - 1)

    def playlist(self, elapsed):
        last = self.published(elapsed) - 1
        if last < 0:
            raise ValueError("no segment has been published yet")
        first = max(0, last - self.window + 1)
        lines = [
            "#EXTM3U",
            "#EXT-X-VERSION:3",
            f"#EXT-X-TARGETDURATION:{self.target}",
            f"#EXT-X-MEDIA-SEQUENCE:{first}",
        ]
        for seq in range(first, last + 1):
            seconds = self.durations[seq % len(self.names)] / TICKS
            lines += [f"#EXTINF:{seconds:.6f},", f"live-{seq}.ts"]
        return "\n".join(lines) + "\n"

    def segment(self, seq, elapsed):
        """Segment `seq`'s bytes, or None while it has not been published."""
        if seq < 0 or seq >= self.published(elapsed):
            return None
        repeat, index = divmod(seq, len(self.names))
        data = self._read(self.names[index])
        if not repeat:
            return data
        counters = {pid: repeat * count % 16 for pid, count in self.counts.items()}
        return shift_ts(data, repeat * self.period % WRAP, counters)


class Loops:
    """The looping channels under a directory, each loaded on first use and
    reloaded when its playlist is regenerated. All share one clock."""

    def __init__(self, window, clock=time.monotonic):
        self.window = window
        self.clock = clock
        self.started = clock()
        self.lock = threading.Lock()
        self.loaded = {}

    def get(self, directory):
        manifest = os.path.join(directory, "index.m3u8")
        try:
            stamp = os.stat(manifest).st_mtime_ns
        except OSError:
            return None
        with self.lock:
            cached = self.loaded.get(directory)
            if cached and cached[0] == stamp:
                return cached[1]
        try:
            with open(manifest, encoding="utf-8") as f:
                text = f.read()

            def read_segment(name):
                with open(os.path.join(directory, name), "rb") as f:
                    return f.read()

            loop = Loop(text, read_segment, self.window)
        except (OSError, ValueError, UnicodeDecodeError) as error:
            # Served as it is: a hand-made playlist, or a generated one that
            # cannot be looped, still plays (and ends).
            print(f"not looping {manifest}: {error}", flush=True)
            loop = None
        with self.lock:
            self.loaded[directory] = (stamp, loop)
        return loop

    def elapsed(self, loop):
        return round((self.clock() - self.started) * TICKS) + loop.preroll()


class Handler(http.server.SimpleHTTPRequestHandler):
    loops = None  # Loops when serving with --loop

    def translate_path(self, path):
        parsed = urllib.parse.urlsplit(path)
        p = parsed.path
        if p.startswith("/archive/"):
            p = p[len("/archive"):]
        return super().translate_path(p)

    def send_head(self):
        path = self.translate_path(self.path)
        archive = urllib.parse.urlsplit(self.path).path.startswith("/archive/")
        if self.loops and not archive:
            live = self.live(path)
            if live is not False:
                return live
        host = self.headers.get("Host")
        if not (path.endswith(".m3u") and host and os.path.isfile(path)):
            return super().send_head()
        with open(path, "rb") as f:
            body = f.read().replace(b"10.0.2.2:8089", host.encode("ascii", "replace"))
        return self.body(body, "audio/x-mpegurl", cache=True)

    def live(self, path):
        """A looping channel's playlist or segment (None once an error was
        sent), or False for any other path."""
        directory, name = os.path.split(path)
        match = LIVE_SEGMENT.fullmatch(name)
        if name != "index.m3u8" and not match:
            return False
        loop = self.loops.get(directory)
        if loop is None:
            return False
        elapsed = self.loops.elapsed(loop)
        if not match:
            return self.body(loop.playlist(elapsed).encode(), "application/vnd.apple.mpegurl")
        data = loop.segment(int(match.group(1)), elapsed)
        if data is None:
            self.send_error(404, "segment not published")
            return None
        return self.body(data, "video/mp2t")

    def body(self, data, content_type, cache=False):
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        if not cache:
            # A live playlist changes with every segment.
            self.send_header("Cache-Control", "no-store")
        self.end_headers()
        return io.BytesIO(data)

    def log_message(self, fmt, *args):
        sys.stdout.write("%s %s\n" % (self.log_date_time_string(), fmt % args))
        sys.stdout.flush()


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--loop", action="store_true",
                        help="serve each channel as an endless live stream")
    parser.add_argument("--window", type=int, default=30,
                        help="segments in a looping live playlist (default 30)")
    parser.add_argument("port", nargs="?", type=int, default=8089)
    parser.add_argument("directory", nargs="?", default=os.path.join(root, "fixtures", "hls"))
    args = parser.parse_args()
    if args.window < 1:
        parser.error("--window needs at least one segment")
    directory = args.directory

    class Server(Handler):
        loops = Loops(args.window) if args.loop else None

    def handler(*a, **kwargs):
        return Server(*a, directory=directory, **kwargs)

    server = http.server.ThreadingHTTPServer(("0.0.0.0", args.port), handler)
    mode = f"looping live, {args.window}-segment window" if args.loop else "ending VOD"
    print(f"mock server on :{args.port} serving {directory} ({mode}; /archive/* -> /*)", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
