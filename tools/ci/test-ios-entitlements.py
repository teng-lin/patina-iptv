#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import plistlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('ios', Path(__file__).with_name('ios-entitlements.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class EntitlementsTests(unittest.TestCase):
    def test_bundle_inventory_reports_missing_declarations_without_signing_identity(self):
        spec = importlib.util.spec_from_file_location('inventory', Path(__file__).with_name('inspect-ios-bundle.py'))
        inventory = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(inventory)
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
            result = inventory.inspect(bundle, lambda arguments: (1, b'', b'unsigned'))
            self.assertEqual(result['signature_kind'], 'absent')
            self.assertFalse(result['signature_verifies'])
            self.assertEqual(result['privacy_manifests'][0]['accessed_api_reasons'][0]
                             ['NSPrivacyAccessedAPITypeReasons'], ['C617.1'])

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
