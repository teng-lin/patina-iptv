#!/usr/bin/env python3
"""Check layer boundaries for every Patina dependency declared to Cargo."""

import json
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
MANIFESTS = ("Cargo.toml", "hosts/tizen-nsh/Cargo.toml", "fuzz/Cargo.toml")
LAYERS = {
    "iptv-core": "domain",
    "iptv-epg": "domain",
    "iptv-guide": "domain",
    "iptv-import": "domain",
    "iptv-library": "domain",
    "iptv-providers": "domain",
    "iptv-search": "domain",
    "iptv-settings": "domain",
    "iptv-net": "adapters",
    "iptv-player-mpv": "adapters",
    "iptv-sqlcipher-init": "adapters",
    "iptv-store-sqlite": "adapters",
    "iptv-web": "adapters",
    "iptv-runtime": "application",
    "iptv-nav": "presentation",
    "iptv-app": "view",
    "nsh-tv": "view",
    "iptv-tui": "host",
    "patina-tizen": "host",
    "iptv-import-fuzz": "fuzz",
}
ALLOWED = {
    "domain": {"domain"},
    "adapters": {"domain", "adapters"},
    "application": {"domain", "adapters"},
    "presentation": {"domain"},
    "view": {"domain", "adapters", "application", "presentation", "view"},
    "host": {"view", "domain", "adapters"},
    "fuzz": {"domain", "adapters"},
}
# The old bridge shares iptv-web with the browser's adapters. Only the
# bridge can use presentation, and only while its feature is enabled.
BRIDGE_DEPENDENCIES = {"iptv-nav"}
# The native player draws through NSH. The views and entry points reach NSH
# directly to assemble a platform host, and nsh-tv decodes its input.
NSH_USERS = {"iptv-player-mpv", "iptv-app", "nsh-tv", "iptv-tui", "patina-tizen"}


def metadata(manifest):
    return json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1",
             "--manifest-path", ROOT / manifest],
            cwd=ROOT,
        )
    )


def bridge_gates(package, dependency):
    name = dependency["rename"] or dependency["name"]
    activation = f"dep:{name}"
    features = package["features"]
    return (
        dependency["kind"] is None
        and dependency["optional"]
        and activation in features.get("bridge", ())
        and all(
            effect not in {activation, name} and not effect.startswith(f"{name}/")
            for feature, effects in features.items()
            if feature != "bridge"
            for effect in effects
        )
    )


def adapters_only(package, dependency):
    name = dependency["rename"] or dependency["name"]
    return (
        not dependency["uses_default_features"]
        and "bridge" not in dependency["features"]
        and not any(
            effect in {f"{name}/bridge", f"{name}?/bridge"}
            for effects in package["features"].values()
            for effect in effects
        )
    )


def allowed(package, dependency):
    user, name = package["name"], dependency["name"]
    if name in BRIDGE_DEPENDENCIES and user == "iptv-web":
        return bridge_gates(package, dependency)
    if name == "iptv-web" and LAYERS[user] in {"adapters", "application"}:
        return adapters_only(package, dependency)
    if name in LAYERS:
        return LAYERS[name] in ALLOWED[LAYERS[user]]
    if name.startswith("nsh-"):
        return user in NSH_USERS
    return not name.startswith("iptv-")


class LayerBoundaries(unittest.TestCase):
    def test_manifest_dependencies(self):
        packages = {}
        for manifest in MANIFESTS:
            workspace = metadata(manifest)
            members = set(workspace["workspace_members"])
            packages.update(
                (package["name"], package)
                for package in workspace["packages"]
                if package["id"] in members
            )
        self.assertEqual(set(packages), set(LAYERS))
        for user, package in packages.items():
            for dependency in package["dependencies"]:
                name = dependency["name"]
                kind = dependency["kind"] or "normal"
                target = dependency["target"] or "all targets"
                with self.subTest(user=user, name=name, kind=kind, target=target):
                    self.assertTrue(allowed(package, dependency),
                                    f"{user} cannot depend on {name} ({kind}, {target})")

    def test_bridge_dependency_requires_the_bridge_feature(self):
        package = {"name": "iptv-web", "features": {"bridge": ["dep:nav"]}}
        dependency = {"name": "iptv-nav", "rename": "nav", "optional": True, "kind": None}
        self.assertTrue(allowed(package, dependency))
        for change in ({"optional": False}, {"kind": "dev"}, {"kind": "build"}):
            self.assertFalse(allowed(package, {**dependency, **change}))
        for effect in ("dep:nav", "nav", "nav/some-feature"):
            package["features"]["other"] = [effect]
            self.assertFalse(allowed(package, dependency))

    def test_lower_layer_cannot_enable_the_web_bridge(self):
        package = {"name": "iptv-runtime", "features": {}}
        dependency = {"name": "iptv-web", "rename": "web",
                      "uses_default_features": False, "features": []}
        self.assertTrue(allowed(package, dependency))
        self.assertFalse(allowed(package, {**dependency, "uses_default_features": True}))
        self.assertFalse(allowed(package, {**dependency, "features": ["bridge"]}))
        package["features"]["ui"] = ["web?/bridge"]
        self.assertFalse(allowed(package, dependency))

    def test_renamed_target_dependency_cannot_cross_layers(self):
        runtime = {"name": "iptv-runtime"}
        dependency = {"name": "iptv-nav", "rename": "remote_keys",
                      "optional": True, "kind": "dev", "target": "cfg(windows)"}
        self.assertFalse(allowed(runtime, dependency))


if __name__ == "__main__":
    unittest.main()
