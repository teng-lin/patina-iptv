#!/usr/bin/env python3
"""Checks generated smoke-manifest lifetime and timestamp-reset boundaries."""

import importlib.util
from pathlib import Path
import re
import unittest

spec = importlib.util.spec_from_file_location(
    "repeat_hls", Path(__file__).with_name("repeat-hls-fixtures.py")
)
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)

ORIGINAL = """#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:0
#EXT-X-PLAYLIST-TYPE:VOD
#EXTINF:2.000000,
seg000.ts
#EXTINF:1.500000,
seg001.ts
#EXT-X-ENDLIST
"""


def seconds(text):
    return sum(float(value) for value in re.findall(r"#EXTINF:([^,]+),", text))


class RepeatedFixture(unittest.TestCase):
    def test_duration_and_discontinuities_cover_the_complete_smoke_job(self):
        text = fixture.repeat_manifest(ORIGINAL, 120)
        self.assertEqual(seconds(text), 420)
        self.assertEqual(text.count("#EXT-X-DISCONTINUITY\n"), 119)
        self.assertEqual(text.count("seg000.ts\n"), 120)
        self.assertEqual(text.count("#EXT-X-ENDLIST\n"), 1)
        self.assertTrue(text.endswith("#EXT-X-ENDLIST\n"))
        self.assertIn("#EXT-X-TARGETDURATION:2\n", text)
        self.assertIn("#EXT-X-MEDIA-SEQUENCE:0\n", text)
        generated = ORIGINAL.split("#EXTINF:", 1)[0] + "\n".join(
            f"#EXTINF:2.000000,\nseg{number:03}.ts" for number in range(15)
        ) + "\n#EXT-X-ENDLIST\n"
        self.assertEqual(seconds(fixture.repeat_manifest(generated, 120)), 3600)

    def test_reapplying_or_shortening_does_not_multiply_previous_repetitions(self):
        text = fixture.repeat_manifest(ORIGINAL, 120)
        self.assertEqual(fixture.repeat_manifest(text, 120), text)
        once = fixture.repeat_manifest(text, 1)
        self.assertEqual(seconds(once), 3.5)
        self.assertNotIn("#EXT-X-DISCONTINUITY", once)

    def test_malformed_or_nonfixture_media_is_rejected(self):
        for text in [
            ORIGINAL.replace("#EXT-X-ENDLIST", ""),
            ORIGINAL.replace("seg000.ts", "https://example.test/synthetic.ts"),
            ORIGINAL.replace("2.000000", "nan"),
            ORIGINAL.replace("2.000000", "-2"),
            ORIGINAL.replace("seg001.ts", "#EXT-X-DISCONTINUITY\nseg001.ts"),
        ]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                fixture.repeat_manifest(text, 120)
        for cycles in [0, 241]:
            with self.subTest(cycles=cycles), self.assertRaises(ValueError):
                fixture.repeat_manifest(ORIGINAL, cycles)


if __name__ == "__main__":
    unittest.main()
