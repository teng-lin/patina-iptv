"""Notices for the direct Windows binary (there is no generated NSH runner)."""
from pathlib import Path
import hashlib
import json
import shutil
import subprocess
import sys

stage = Path(sys.argv[1])
runtime = Path("target/mpv")
pin = json.loads(Path("tools/mpv-runtime/runtime.json").read_text(encoding="utf-8"))
inventory = json.loads((runtime / "components.json").read_text(encoding="utf-8"))
def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()

if pin.get("schema") != 1 or inventory.get("schema") != 1:
    raise RuntimeError("Unsupported libmpv runtime manifest")
for dll in (runtime / "libmpv-2.dll", stage / "libmpv-2.dll"):
    actual = sha256(dll)
    if actual != pin["dll_sha256"] or actual != inventory["build"]["dll_sha256"]:
        raise RuntimeError("Installer libmpv DLL does not match its pinned source inventory")
if any(inventory["build"].get(key) != pin[key] for key in ("tag", "source_url", "source_sha256")):
    raise RuntimeError("libmpv inventory does not match retained source release")
lock = json.loads((runtime / "sources.lock.json").read_text(encoding="utf-8"))
locked = lock["sources"] + ([lock["toolchain"]] if lock.get("toolchain") else [])
components = inventory.get("components", [])
if len(components) != len(locked) or {row["name"] for row in components} != {row["name"] for row in locked}:
    raise RuntimeError("Incomplete libmpv source inventory")
for component in components:
    source = next(row for row in locked if row["name"] == component["name"])
    if any(component.get(key) != value for key, value in source.items()):
        raise RuntimeError("libmpv component differs from its source lock")
    if [row["path"] for row in component["licenses"]] != source["license_files"]:
        raise RuntimeError("Incomplete libmpv license inventory")
    for license_file in component["licenses"]:
        path = runtime / "licenses" / component["name"] / license_file["path"]
        if not path.resolve().is_relative_to((runtime / "licenses").resolve()) or sha256(path) != license_file["sha256"]:
            raise RuntimeError("Missing or changed libmpv component notice")
runtime_notices = ("licenses", "components.json", "sources.lock.json", "SOURCE.txt", "NOTICE.txt", "build-config")
for name in runtime_notices:
    if not (runtime / name).exists():
        raise RuntimeError(f"Missing libmpv distribution material: {name}")
notices = stage / "notices"
notices.mkdir(parents=True)
tv = notices / "nsh-tv"
tv.mkdir()
for name in ("LICENSE", "README.md"):
    shutil.copy2(Path("vendor/nsh-tv") / name, tv / name)
subprocess.run(["cargo", "about", "generate", "--fail", "--locked", "--target", "x86_64-pc-windows-msvc",
                "--config", "about.toml", "--output-file", str(notices / "THIRD-PARTY.html"), "tools/ci/notices.hbs"], check=True)
metadata = subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"])
with (notices / "components.json").open("wb") as out:
    subprocess.run([sys.executable, "tools/ci/component-inventory.py", "Cargo.lock"], input=metadata, stdout=out, check=True)
subprocess.run([sys.executable, "tools/ci/crypto-native-notices.py", str(notices / "native-crypto")], check=True)
native = notices / "libmpv"
native.mkdir()
for name in runtime_notices:
    source = runtime / name
    if source.is_dir():
        shutil.copytree(source, native / name)
    else:
        shutil.copy2(source, native / name)
shutil.copy2("LICENSE", stage / "LICENSE")
shutil.copy2("crates/iptv-app/fonts/OFL.txt", notices / "OFL.txt")
shutil.copy2("crates/iptv-cast/NOTICE-pyatv.md", notices / "NOTICE-pyatv.md")
(notices / "MSVC-RUNTIME.txt").write_text(
    "Microsoft Visual C++ Runtime (vcruntime140.dll and vcruntime140_1.dll)\n"
    "Copyright Microsoft Corporation. Distributed with Patina under the Visual Studio redistributable license.\n"
    "https://visualstudio.microsoft.com/license-terms/\n", encoding="utf-8")
