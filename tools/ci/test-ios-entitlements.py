#!/usr/bin/env python3
import importlib.util
import contextlib
import io
from pathlib import Path
import plistlib
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('ios', Path(__file__).with_name('ios-entitlements.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
spec = importlib.util.spec_from_file_location('inventory', Path(__file__).with_name('inspect-ios-bundle.py'))
inventory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inventory)


class EntitlementsTests(unittest.TestCase):
    def test_bundle_inventory_reports_missing_declarations_without_signing_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            bundle = Path(temporary) / 'Synthetic.app'
            bundle.mkdir()
            (bundle / 'Synthetic').write_bytes(b'synthetic executable')
            (bundle / 'Info.plist').write_bytes(plistlib.dumps({
                'CFBundleExecutable': 'Synthetic', 'CFBundleIdentifier': 'example.synthetic',
                'UIDeviceFamily': [2],
            }))
            (bundle / 'embedded.mobileprovision').write_bytes(b'private synthetic profile')
            def run(arguments):
                return 0, b'', b'Authority=Private synthetic identity\n'
            result = inventory.inspect(bundle, run)
            self.assertTrue(result['embedded_profile_present'])
            self.assertEqual(result['signature_kind'], 'certificate')
            self.assertTrue(result['signature_verifies'])
            self.assertIsNone(result['info']['ITSAppUsesNonExemptEncryption'])
            self.assertEqual(result['privacy_manifests'], [])
            self.assertNotIn('Private synthetic identity', str(result))
            self.assertNotIn('private synthetic profile', str(result))
            (bundle / 'PrivacyInfo.xcprivacy').write_bytes(plistlib.dumps({
                'NSPrivacyTracking': False, 'NSPrivacyCollectedDataTypes': [],
                'NSPrivacyAccessedAPITypes': [{'NSPrivacyAccessedAPIType': 'NSPrivacyAccessedAPICategoryFileTimestamp',
                                             'NSPrivacyAccessedAPITypeReasons': ['C617.1']}],
            }))
            result = inventory.inspect(bundle, lambda arguments: (1, b'', b'Synthetic: code object is not signed at all'))
            self.assertEqual(result['signature_kind'], 'absent')
            self.assertFalse(result['signature_verifies'])
            self.assertEqual(result['privacy_manifests'][0]['accessed_api_reasons'][0]
                             ['NSPrivacyAccessedAPITypeReasons'], ['C617.1'])


class BundleInventoryTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.bundle = Path(temporary.name) / 'Synthetic.app'
        self.bundle.mkdir()
        (self.bundle / 'Synthetic').write_bytes(b'synthetic executable')
        self.info = {'CFBundleExecutable': 'Synthetic', 'CFBundleIdentifier': 'example.synthetic'}
        self.write_info()

    def write_info(self):
        (self.bundle / 'Info.plist').write_bytes(plistlib.dumps(self.info))

    def test_signature_kind_is_independent_of_verification(self):
        for status, output, expected in (
            (0, b'Signature=adhoc\n', 'ad-hoc'),
            (0, b'Authority=private synthetic authority\n', 'certificate'),
            (0, b'Unexpected display result\n', 'unknown'),
            (1, b'Corrupt signature: private details\n', 'unknown'),
            (1, b'Synthetic: code object is not signed at all\n', 'absent'),
        ):
            def run(arguments):
                self.assertIn('--', arguments)
                return (status, b'', output) if '--display' in arguments else (1, b'', b'private error')
            result = inventory.inspect(self.bundle, run)
            self.assertEqual(result['signature_kind'], expected)
            self.assertFalse(result['signature_verifies'])
            self.assertNotIn('private', str(result))

    def test_nested_sdk_manifest_retains_disclosure_dimensions(self):
        framework = self.bundle / 'Frameworks' / 'SyntheticSDK.framework'
        framework.mkdir(parents=True)
        collected = [{'NSPrivacyCollectedDataType': 'NSPrivacyCollectedDataTypeCrashData',
                      'NSPrivacyCollectedDataTypeLinked': False, 'NSPrivacyCollectedDataTypeTracking': False,
                      'NSPrivacyCollectedDataTypePurposes': ['NSPrivacyCollectedDataTypePurposeAppFunctionality']}]
        (framework / 'PrivacyInfo.xcprivacy').write_bytes(plistlib.dumps({
            'NSPrivacyTracking': True, 'NSPrivacyTrackingDomains': ['tracking.example.invalid'],
            'NSPrivacyCollectedDataTypes': collected,
        }))
        result = inventory.inspect(self.bundle, lambda arguments: (0, b'', b'Signature=adhoc\n'))
        self.assertEqual(result['frameworks'], ['SyntheticSDK.framework'])
        manifest = result['privacy_manifests'][0]
        self.assertEqual(manifest['collected_data_types'], collected)
        self.assertEqual(manifest['tracking_domains'], ['tracking.example.invalid'])

    def test_external_executable_and_invalid_manifest_types_are_rejected(self):
        executable = self.bundle / 'Synthetic'
        executable.unlink()
        executable.symlink_to(Path(__file__).resolve())
        with self.assertRaises(ValueError):
            inventory.inspect(self.bundle)
        executable.unlink()
        executable.write_bytes(b'synthetic executable')
        for manifest in ([], {'NSPrivacyTracking': 'private invalid flag'},
                         {'NSPrivacyCollectedDataTypes': ['private invalid declaration']}):
            (self.bundle / 'PrivacyInfo.xcprivacy').write_bytes(plistlib.dumps(manifest))
            with self.assertRaises(ValueError):
                inventory.inspect(self.bundle)

    def test_cli_errors_are_redacted_including_serialization_and_tool_failures(self):
        for value in ([], b'private plist data'):
            self.info['CFBundleExecutable'] = value
            self.write_info()
            with patch.object(sys, 'argv', ['inspect-ios-bundle.py', str(self.bundle)]), \
                    contextlib.redirect_stderr(io.StringIO()) as errors, self.assertRaises(SystemExit):
                inventory.main()
            self.assertNotIn('private ', errors.getvalue())
            self.assertNotIn('Traceback', errors.getvalue())
        for failure in (TypeError('private serialization detail'), OSError('private tool detail')):
            with patch.object(sys, 'argv', ['inspect-ios-bundle.py', str(self.bundle)]), \
                    patch.object(inventory, 'inspect', side_effect=failure), \
                    contextlib.redirect_stderr(io.StringIO()) as errors, self.assertRaises(SystemExit):
                inventory.main()
            self.assertNotIn('private ', errors.getvalue())
            self.assertNotIn('Traceback', errors.getvalue())

    def test_exact_and_wildcard_profiles(self):
        for allowed in ['example.synthetic', 'example.*', '*']:
            profile = {'Entitlements': {'application-identifier': 'SYNTHETIC.' + allowed, 'get-task-allow': False}}
            result = module.entitlements(profile, {'CFBundleIdentifier': 'example.synthetic'}, {'id': 'example.synthetic'})
            self.assertEqual(result['application-identifier'], 'SYNTHETIC.example.synthetic')
            self.assertFalse(result['get-task-allow'])

    def test_keychain_wildcards_specialize_without_altering_named_groups(self):
        profile = {'Entitlements': {'application-identifier': 'SYNTHETIC.*', 'keychain-access-groups': ['SYNTHETIC.*', 'SYNTHETIC.shared']}}
        result = module.entitlements(profile, {'CFBundleIdentifier': 'example.synthetic'}, {'id': 'example.synthetic'})
        self.assertEqual(result['keychain-access-groups'], ['SYNTHETIC.example.synthetic', 'SYNTHETIC.shared'])
        self.assertEqual(profile['Entitlements']['keychain-access-groups'][0], 'SYNTHETIC.*')
        profile['Entitlements']['keychain-access-groups'] = ['OTHER.*']
        with self.assertRaisesRegex(ValueError, 'Keychain wildcard'):
            module.entitlements(profile, {'CFBundleIdentifier': 'example.synthetic'}, {'id': 'example.synthetic'})

    def test_wrong_profile_or_unvalidated_custom_entitlement_refused(self):
        for allowed, config in [('other.app', {'id': 'example.synthetic'}), ('*', {'id': 'example.synthetic', 'ios': {'entitlements': {'example.capability': True}}})]:
            with self.assertRaises(ValueError):
                module.entitlements({'Entitlements': {'application-identifier': 'SYNTHETIC.' + allowed}}, {'CFBundleIdentifier': 'example.synthetic'}, config)


if __name__ == '__main__':
    unittest.main()
