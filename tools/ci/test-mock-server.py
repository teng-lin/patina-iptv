#!/usr/bin/env python3
"""Checks the mock server's ending and looping live HLS channels.

The looping channel is followed over simulated time: its sliding playlist,
media sequence numbers, the wrap back to the first generated segment and
the timeline of the segments joined into one stream, as the app joins them.
"""

import http.client
import http.server
import importlib.util
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import unittest

spec = importlib.util.spec_from_file_location(
    "mock_server", Path(__file__).resolve().parents[1] / "mock-server.py"
)
mock = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mock)

TICKS = mock.TICKS
VIDEO, AUDIO, PAT = 0x100, 0x101, 0x000


def timestamp(prefix, value):
    field = bytearray(5)
    field[0] = prefix << 4
    mock.write_timestamp(field, value)
    return bytes(field)


def adts(length, rate=3):
    """One synthetic ADTS frame header (48 kHz by default, one block of
    1024 samples) padded with silence to `length` bytes."""
    return bytes([0xFF, 0xF1, 0x40 | rate << 2, 0x80 | length >> 11, length >> 3 & 0xFF, (length & 0x07) << 5 | 0x1F, 0xFC]) + bytes(length - 7)


def packet(pid, counter, pts=None, dts=None, pcr=None, frames=None, start=True):
    """One synthetic 188-byte packet: a PES start with PTS (and DTS), or a
    PSI section, optionally with a PCR in its adaptation field. With
    `frames` it is audio and its payload is that many ADTS frames; without
    `start` it continues a PES with them."""
    head = bytes([0x47, (0x40 if start else 0) | pid >> 8, pid & 0xFF])
    adaptation = b""
    if pcr is not None:
        base, extension = pcr
        adaptation = bytes([7, 0x10, base >> 25 & 0xFF, base >> 17 & 0xFF, base >> 9 & 0xFF,
                            base >> 1 & 0xFF, (base & 1) << 7 | 0x7E | extension >> 8, extension & 0xFF])
    control = 0x30 if adaptation else 0x10
    stream = b"\xc0" if frames else b"\xe0"
    if not start:
        payload = b""
    elif pts is None:
        payload = b"\x00\x00\xb0\x0d" + b"\x5a" * 9
    elif dts is None:
        payload = b"\x00\x00\x01" + stream + b"\x00\x00\x80\x80\x05" + timestamp(2, pts)
    else:
        payload = b"\x00\x00\x01" + stream + b"\x00\x00\x80\xc0\x0a" + timestamp(3, pts) + timestamp(1, dts)
    body = head + bytes([control | counter]) + adaptation + payload
    room = 188 - len(body)
    if frames:
        sizes = [room // frames] * frames
        sizes[-1] += room % frames
        return body + b"".join(adts(size) for size in sizes)
    return body + b"\xff" * room


def segment(index):
    """A 2-second synthetic segment: PAT, a video frame with PCR and DTS,
    and an audio frame, with counters continuing from the previous one."""
    start = 126_000 + index * 2 * TICKS
    return (
        packet(PAT, index % 16)
        + packet(VIDEO, (2 * index) % 16, pts=start + 3600, dts=start, pcr=(start - 6300, 17))
        + packet(VIDEO, (2 * index + 1) % 16, pts=start + TICKS + 3600, dts=start + TICKS)
        + packet(AUDIO, index % 16, pts=start, frames=1)
    )


def manifest(count):
    lines = ["#EXTM3U", "#EXT-X-VERSION:3", "#EXT-X-TARGETDURATION:2",
             "#EXT-X-MEDIA-SEQUENCE:0", "#EXT-X-PLAYLIST-TYPE:VOD"]
    for n in range(count):
        lines += ["#EXTINF:2.000000,", f"seg{n:03}.ts"]
    return "\n".join(lines + ["#EXT-X-ENDLIST", ""])


def fields(data):
    """Each packet's (pid, counter, pts, dts, pcr base) from a stream."""
    out = []
    for start in range(0, len(data), 188):
        p = data[start:start + 188]
        pid = (p[1] & 0x1F) << 8 | p[2]
        offset = 4
        pcr = None
        if p[3] & 0x20:
            if p[4] and p[5] & 0x10:
                pcr = p[6] << 25 | p[7] << 17 | p[8] << 9 | p[9] << 1 | p[10] >> 7
            offset += 1 + p[4]
        pes = p[offset:]
        pts = dts = None
        # Only a PES start: a continuation's payload may open with a start code.
        if p[3] & 0x10 and p[1] & 0x40 and pes[:3] == b"\x00\x00\x01" and len(pes) >= 14 and pes[7] & 0x80:
            pts = mock.read_timestamp(pes[9:14])
            if pes[7] >> 6 == 3:
                dts = mock.read_timestamp(pes[14:19])
        out.append((pid, p[3] & 0x0F, pts, dts, pcr))
    return out


def tags(playlist):
    return [line for line in playlist.splitlines() if line.startswith("#")]


def sequence(playlist):
    return int(next(line for line in playlist.splitlines() if line.startswith("#EXT-X-MEDIA-SEQUENCE:")).split(":")[1])


def uris(playlist):
    return [line for line in playlist.splitlines() if line and not line.startswith("#")]


class Timestamps(unittest.TestCase):
    def test_fields_round_trip_across_all_33_bits(self):
        for value in [0, 1, 126_000, (1 << 32) + 12345, (1 << 33) - 1]:
            self.assertEqual(mock.read_timestamp(timestamp(2, value)), value)

    def test_shift_moves_every_clock_and_counter_and_nothing_else(self):
        data = packet(VIDEO, 15, pts=9000, dts=5400, pcr=(1800, 299)) + packet(AUDIO, 3, pts=7200) + packet(PAT, 0)
        shifted = mock.shift_ts(data, 900_000, {VIDEO: 2, PAT: 15})
        self.assertEqual(fields(shifted), [
            (VIDEO, 1, 909_000, 905_400, 901_800),
            (AUDIO, 3, 907_200, None, None),
            (PAT, 15, None, None, None),
        ])
        self.assertEqual(shifted[10] & 0x7F, data[10] & 0x7F, "PCR reserved bits and extension kept")
        self.assertEqual(shifted[11], data[11])
        for start in [0, 188, 376]:
            # Payloads past the timestamp fields are untouched.
            self.assertEqual(shifted[start + 40:start + 188], data[start + 40:start + 188])

    def test_adaptation_only_packets_repeat_the_moved_counter(self):
        # Such a packet carries the counter of the PID's last payload packet.
        stuffing = bytearray(packet(VIDEO, 5, pcr=(1800, 0)))
        stuffing[3] = 0x20 | 5
        stuffing[4] = 183
        shifted = mock.shift_ts(packet(VIDEO, 5, pts=9000) + bytes(stuffing), 0, {VIDEO: 3})
        self.assertEqual([counter for _, counter, *_ in fields(shifted)], [8, 8])

    def test_timestamps_wrap_at_33_bits(self):
        data = packet(VIDEO, 0, pts=(1 << 33) - 10, dts=(1 << 33) - 20, pcr=((1 << 33) - 30, 0))
        self.assertEqual(fields(mock.shift_ts(data, 100, {}))[0][2:], (90, 80, 70))

    def test_broken_streams_are_rejected(self):
        with self.assertRaises(ValueError):
            mock.shift_ts(b"\x47" * 100, 0, {})
        with self.assertRaises(ValueError):
            mock.shift_ts(b"\x00" * 188, 0, {})
        # A PES start whose timestamps do not fit after a long adaptation field.
        crowded = bytearray(packet(VIDEO, 0, pts=9000, dts=5400))
        crowded[3] |= 0x20
        crowded[4:] = bytes([170]) + bytes(170) + crowded[4:17]
        with self.assertRaises(ValueError):
            mock.shift_ts(bytes(crowded), 0, {})
        crowded[4:] = bytes([178]) + bytes(178) + b"\x00\x00\x01\xe0\x00"
        with self.assertRaises(ValueError):
            mock.shift_ts(bytes(crowded), 0, {})


class Lengths(unittest.TestCase):
    def test_adts_frames_are_counted_by_their_rate_and_blocks(self):
        self.assertEqual(mock.adts_ticks(adts(20) + adts(30)), 3840)
        self.assertEqual(mock.adts_ticks(adts(20, rate=5)), 2880)  # 32 kHz
        self.assertIsNone(mock.adts_ticks(adts(20)[:-1] + b""), "a frame cut short")
        self.assertIsNone(mock.adts_ticks(adts(20)[:-2]))
        self.assertIsNone(mock.adts_ticks(b"\x00" * 20))
        self.assertIsNone(mock.adts_ticks(b""))

    def test_audio_ends_with_the_last_frame_of_its_last_pes(self):
        media = mock.Media()
        media.add(packet(AUDIO, 0, pts=0, frames=1) + packet(AUDIO, 1, pts=1920, frames=1))
        # The last PES spans two packets and holds three frames.
        media.add(packet(AUDIO, 2, pts=3840, frames=2) + packet(AUDIO, 3, frames=1, start=False))
        self.assertEqual(media.length(), 3840 + 3 * 1920)
        self.assertEqual(media.counts, {AUDIO: 4})

    def test_video_ends_one_step_after_its_last_frame(self):
        media = mock.Media()
        media.add(b"".join(packet(VIDEO, n, pts=900 + 3600 * n, dts=3600 * n) for n in [0, 2, 1, 3]))
        self.assertEqual(media.length(), 4 * 3600)


class LoopingLive(unittest.TestCase):
    def setUp(self):
        self.reads = []

        def read(name):
            self.reads.append(name)
            return segment(int(name[3:6]))

        self.read = read
        self.loop = mock.Loop(manifest(3), read, window=4)

    def test_playlist_slides_with_time_and_never_ends(self):
        loop = self.loop
        self.assertEqual(loop.preroll(), 8 * TICKS)
        start = loop.playlist(loop.preroll())
        self.assertEqual(uris(start), ["live-0.ts", "live-1.ts", "live-2.ts", "live-3.ts"])
        self.assertEqual(sequence(start), 0)
        self.assertEqual(loop.playlist(loop.preroll() + 2 * TICKS - 1), start)
        previous = -1
        for second in range(0, 3600, 7):
            text = loop.playlist(loop.preroll() + second * TICKS)
            first = sequence(text)
            self.assertGreaterEqual(first, previous)
            previous = first
            self.assertEqual(uris(text), [f"live-{n}.ts" for n in range(first, first + 4)])
            self.assertNotIn("#EXT-X-ENDLIST", text)
            self.assertNotIn("#EXT-X-PLAYLIST-TYPE", text)
            self.assertNotIn("#EXT-X-DISCONTINUITY", text)
            self.assertIn("#EXT-X-TARGETDURATION:2", tags(text))
            self.assertEqual(text.count("#EXTINF:2.000000,"), 4)
        # One hour later the sequence has advanced by an hour of segments.
        self.assertEqual(sequence(loop.playlist(loop.preroll() + 3600 * TICKS)), 1800)

    def test_early_playlists_are_short_and_none_is_refused(self):
        self.assertEqual(uris(self.loop.playlist(2 * TICKS)), ["live-0.ts"])
        with self.assertRaises(ValueError):
            self.loop.playlist(2 * TICKS - 1)

    def test_only_published_segments_are_served(self):
        elapsed = self.loop.preroll()
        self.assertIsNotNone(self.loop.segment(3, elapsed))
        self.assertIsNone(self.loop.segment(4, elapsed))
        self.assertIsNone(self.loop.segment(-1, elapsed))

    def test_first_repeat_is_the_generated_media(self):
        for n in range(3):
            self.assertEqual(self.loop.segment(n, 10**9), segment(n))

    def test_joined_segments_keep_one_timeline_through_each_wrap(self):
        joined = b"".join(self.loop.segment(n, 10**9) for n in range(10))
        last = {}
        for pid, counter, pts, dts, pcr in fields(joined):
            previous = last.get(pid)
            if previous:
                self.assertEqual(counter, (previous[0] + 1) % 16, f"PID {pid:#x} counter")
                for now, before in zip((pts, dts, pcr), previous[1:]):
                    if now is not None and before is not None:
                        self.assertGreater(now, before, f"PID {pid:#x} timestamps")
            last[pid] = (counter, pts, dts, pcr)
        # Segment 9 is the original segment 0 in its fourth repeat: 3 x 6 s on.
        self.assertEqual(fields(self.loop.segment(9, 10**9))[1][2:],
                         (126_000 + 3600 + 18 * TICKS, 126_000 + 18 * TICKS, 126_000 - 6300 + 18 * TICKS))

    def test_media_is_scanned_once(self):
        for n in range(3, 12):
            self.loop.segment(n, 10**9)
        self.assertEqual(len(self.reads), 3 + 9)

    def test_nonfixture_playlists_are_not_looped(self):
        for text in [
            manifest(3).replace("#EXT-X-ENDLIST\n", ""),
            manifest(3).replace("#EXT-X-PLAYLIST-TYPE:VOD\n", ""),
            manifest(3).replace("seg001.ts", "https://example.test/synthetic.ts"),
            manifest(3).replace("seg001.ts", "#EXT-X-DISCONTINUITY\nseg001.ts"),
            manifest(3).replace("2.000000", "nan", 1),
            manifest(3).replace("#EXT-X-TARGETDURATION:2\n", ""),
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:2\n#EXT-X-ENDLIST\n",
        ]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                mock.Loop(text, self.read, window=4)
        with self.assertRaises(ValueError):
            mock.Loop(manifest(3), self.read, window=0)
        with self.assertRaises(ValueError):
            mock.Loop(manifest(3), lambda name: segment(int(name[3:6]))[:-1], window=4)

    def test_a_sequence_has_one_name(self):
        self.assertIsNone(mock.LIVE_SEGMENT.fullmatch("live-03.ts"))
        self.assertEqual(mock.LIVE_SEGMENT.fullmatch("live-0.ts").group(1), "0")

    def test_audio_that_outlasts_the_video_lengthens_the_period(self):
        # 48 kHz AAC frames (1920 ticks) cover 6.016 s; the video ends at 6 s.
        frames = 282

        def read(name):
            n = int(name[3:6])
            audio = [k for k in range(frames) if n * 2 * TICKS <= k * 1920 < (n + 1) * 2 * TICKS or (n == 2 and k * 1920 >= 6 * TICKS)]
            return segment(n)[:3 * 188] + b"".join(packet(AUDIO, k % 16, pts=126_000 + k * 1920, frames=1) for k in audio)

        loop = mock.Loop(manifest(3), read, window=4)
        self.assertEqual(loop.period, frames * 1920)
        self.assertIn("#EXTINF:2.016000,", loop.playlist(10**9))
        self.assertEqual(loop.playlist(10**9).count("#EXTINF:2.016000,"), 1)
        joined = fields(b"".join(loop.segment(n, 10**9) for n in range(7)))
        audio = [pts for pid, _, pts, _, _ in joined if pid == AUDIO]
        self.assertEqual(audio, [126_000 + k * 1920 for k in range(len(audio))], "audio runs on without a step")
        video = [dts for pid, _, _, dts, _ in joined if pid == VIDEO]
        self.assertTrue(all(b > a for a, b in zip(video, video[1:])))
        self.assertEqual(video[6] - video[5], TICKS + frames * 1920 - 6 * TICKS, "the video waits out the audio at the wrap")

    def test_a_window_longer_than_the_media_repeats_it(self):
        loop = mock.Loop(manifest(3), self.read, window=7)
        text = loop.playlist(loop.preroll())
        self.assertEqual(uris(text), [f"live-{n}.ts" for n in range(7)])


class Clock:
    def __init__(self):
        self.now = 1000.0

    def __call__(self):
        return self.now


class Served(unittest.TestCase):
    """The HTTP paths: the ending variant is served as generated."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)
        channel = self.root / "ch0"
        channel.mkdir()
        (channel / "index.m3u8").write_text(manifest(3))
        for n in range(3):
            (channel / f"seg{n:03}.ts").write_bytes(segment(n))
        self.clock = Clock()

    def serve(self, loop):
        root = str(self.root)

        class Server(mock.Handler):
            loops = mock.Loops(4, self.clock) if loop else None

            def log_message(self, *args):
                pass

        server = http.server.ThreadingHTTPServer(
            ("127.0.0.1", 0), lambda *a, **k: Server(*a, directory=root, **k))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return server.server_address[1]

    def get(self, port, path):
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        self.addCleanup(connection.close)
        connection.request("GET", path)
        response = connection.getresponse()
        return response.status, response.getheader("Content-Type"), response.read()

    def test_ending_variant_is_the_generated_playlist(self):
        port = self.serve(loop=False)
        status, _, body = self.get(port, "/ch0/index.m3u8")
        self.assertEqual((status, body.decode()), (200, manifest(3)))
        self.assertTrue(body.decode().endswith("#EXT-X-ENDLIST\n"))
        self.assertEqual(self.get(port, "/ch0/seg001.ts")[2], segment(1))
        self.assertEqual(self.get(port, "/ch0/live-0.ts")[0], 404)

    def test_looping_variant_slides_and_catch_up_still_ends(self):
        port = self.serve(loop=True)
        status, kind, body = self.get(port, "/ch0/index.m3u8")
        self.assertEqual((status, kind), (200, "application/vnd.apple.mpegurl"))
        self.assertEqual(uris(body.decode()), ["live-0.ts", "live-1.ts", "live-2.ts", "live-3.ts"])
        self.clock.now += 3600
        body = self.get(port, "/ch0/index.m3u8")[2].decode()
        self.assertEqual(sequence(body), 1800)
        self.assertNotIn("#EXT-X-ENDLIST", body)
        status, kind, data = self.get(port, "/ch0/live-1803.ts")
        self.assertEqual((status, kind), (200, "video/mp2t"))
        self.assertEqual(data, mock.shift_ts(segment(0), 601 * 6 * TICKS, {VIDEO: 601 * 6 % 16, AUDIO: 601 * 3 % 16, PAT: 601 * 3 % 16}))
        self.assertEqual(self.get(port, "/ch0/live-1804.ts")[0], 404)
        self.assertEqual(self.get(port, "/archive/ch0/index.m3u8?utc=1")[2].decode(), manifest(3))
        self.assertEqual(self.get(port, "/ch0/seg002.ts")[2], segment(2))


@unittest.skipUnless(shutil.which("ffmpeg") and shutil.which("ffprobe"), "needs ffmpeg and ffprobe")
class EncodedMedia(unittest.TestCase):
    """Real encoder output joined across two wraps plays as one timeline."""

    def test_joined_live_segments_have_rising_timestamps(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            subprocess.run([
                "ffmpeg", "-loglevel", "error", "-y",
                "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25",
                "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
                "-t", "6", "-c:v", "libx264", "-preset", "veryfast", "-g", "50", "-pix_fmt", "yuv420p",
                "-c:a", "aac", "-b:a", "64k", "-f", "hls", "-hls_time", "2", "-hls_playlist_type", "vod",
                "-hls_segment_filename", str(out / "seg%03d.ts"), str(out / "index.m3u8"),
            ], check=True)
            loop = mock.Loop((out / "index.m3u8").read_text(), lambda name: (out / name).read_bytes(), window=3)
            (out / "joined.ts").write_bytes(b"".join(loop.segment(n, 10**9) for n in range(9)))
            joined = (out / "joined.ts").read_bytes()
            result = subprocess.run([
                "ffprobe", "-v", "error", "-show_entries", "packet=stream_index,dts",
                "-of", "csv=p=0", str(out / "joined.ts"),
            ], check=True, capture_output=True, text=True)
            self.assertEqual(result.stderr, "", "the demuxer reports no errors")
            probe = result.stdout
        last = {}
        count = 0
        for line in probe.split():
            stream, dts = line.split(",")[:2]
            if dts == "N/A":
                continue
            dts = int(dts)
            self.assertGreater(dts, last.get(stream, -1), f"stream {stream}")
            last[stream] = dts
            count += 1
        self.assertGreater(count, 600)
        # The encoder's continuity counters and PCR run on through each wrap too.
        counters, clocks = {}, []
        for start, (pid, counter, _, _, pcr) in zip(range(0, len(joined), 188), fields(joined)):
            if pid in counters:
                # A packet without payload repeats the counter.
                step = 1 if joined[start + 3] & 0x10 else 0
                self.assertEqual(counter, (counters[pid] + step) % 16, f"PID {pid:#x} counter")
            counters[pid] = counter
            if pcr is not None:
                clocks.append(pcr)
        self.assertTrue(clocks and all(b > a for a, b in zip(clocks, clocks[1:])), "PCR rises")
        # Three repeats of 6 s: both streams' last packets are about 18 s in.
        self.assertEqual(len(last), 2)
        self.assertGreater(min(last.values()) - 126_000, 17 * TICKS)


if __name__ == "__main__":
    unittest.main()
