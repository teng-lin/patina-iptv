#!/usr/bin/env python3
"""Integrity regressions for native license downloads."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('notices', Path(__file__).with_name('mpvkit-notices.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class NoticesTests(unittest.TestCase):
    def run_fixture(self, checksum=None, artifact_checksum=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = {'name': 'Libtest', 'url': 'https://github.com/example/test', 'sha256': 'a' * 64, 'components': ['test']}
            manifest = {'artifacts': [artifact], 'components': [{'component': 'test', 'files': [{'path': 'COPYING', 'url': 'https://example.test/license', 'sha256': checksum or hashlib.sha256(b'license text').hexdigest()}]}]}
            (root / 'manifest.json').write_text(json.dumps(manifest))
            (root / 'fetch.sh').write_text(f"Libtest {artifact['url']} {artifact_checksum or artifact['sha256']}\n")
            with patch.object(module.urllib.request, 'urlopen', return_value=io.BytesIO(b'license text')) as download:
                module.generate(root / 'manifest.json', root / 'out', root / 'fetch.sh')
                self.assertEqual((root / 'out/test/COPYING').read_bytes(), b'license text')
                self.assertTrue((root / 'out/components.json').exists())
                download.assert_called_once()

    def test_verified_text_is_packaged(self):
        self.run_fixture()

    def test_corrupt_download_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
            self.run_fixture(checksum='b' * 64)

    def test_changed_artifact_requires_manifest_update(self):
        with self.assertRaisesRegex(ValueError, 'inventory is stale'):
            self.run_fixture(artifact_checksum='c' * 64)


if __name__ == '__main__':
    unittest.main()
