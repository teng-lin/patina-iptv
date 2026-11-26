"""Viewer package identities, architecture sets and host requirements for releases."""


def formats():
    formats = [
        ("android", "apk", "android-tablet-tv.apk", ["arm64-v8a", "armeabi-v7a"], {"androidApi": 26}),
        ("windows", "exe", "windows-x86_64-setup.exe", ["x86_64"], {"windows": "10.0"}),
        ("macos", "dmg", "macos-arm64.dmg", ["arm64"], {"macos": "14.0"}),
    ]
    for kind, suffix in (("appimage", "AppImage"), ("deb", "deb"), ("rpm", "rpm"), ("pacman", "pkg.tar.zst")):
        minimum = {"glibc": "2.39"}
        if kind != "appimage":
            minimum["gtk"] = "4.14"
        formats.append(("linux", kind, f"linux-x86_64.{suffix}", ["x86_64"], minimum))
    return formats
