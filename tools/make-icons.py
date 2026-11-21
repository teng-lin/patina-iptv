"""Builds the app's icons and TV banner from the master artwork in art/icon.

Masters (see art/icon/README.md):
  p-overlay.png  the P with its shadows, RGBA, over a 593 px tile
  ground.png     the tile's navy ground, 593 px
  banner.png     the TV banner, 732x412

Writes:
  crates/iptv-app/icon.png                     desktop icon, 1024 (nsh.json
                                               "icon": macOS .icns, Linux)
  crates/iptv-app/icon.ico                     Windows executable/window icon
  crates/iptv-app/icon-ios.png                 opaque, full-bleed iOS icon, 1024
  hosts/android/.../mipmap-*/ic_launcher_{foreground,background}.png
  hosts/android/.../drawable/ic_launcher_monochrome.xml  themed icon (vector)
  hosts/android/.../drawable-*/banner.png      Android TV banner, 320x180 dp
  art/icon/play-512.png                        Play Store listing icon

Needs numpy, Pillow and rsvg-convert:
  uvx --with numpy --with pillow python -I tools/make-icons.py

Use --only ios to rebuild just the iOS icon (no rsvg-convert needed).
"""
import argparse
import math
import pathlib
import subprocess

import numpy as np
from PIL import Image, ImageFilter

ROOT = pathlib.Path(__file__).resolve().parent.parent
ART = ROOT / "art" / "icon"
RES = ROOT / "hosts" / "android" / "app" / "src" / "main" / "res"

P = Image.open(ART / "p-overlay.png")
GROUND = Image.open(ART / "ground.png")
TILE = 593  # master tile size
PH = 477  # height of the metal P in the master tile
rng = np.random.default_rng(7)


def ground(n, grain=0.010):
    """The navy ground at n px, with the render's fine grain."""
    g = GROUND.resize((n, n), Image.BICUBIC)
    if not grain:
        return g
    a = np.asarray(g, dtype=np.float64) / 255
    a += rng.normal(0, grain, a.shape[:2])[..., None]
    return Image.fromarray((np.clip(a, 0, 1) * 255 + 0.5).astype(np.uint8))


def put_p(canvas, height, cx, cy):
    """Draws the P `height` px tall, its master tile centered on (cx, cy),
    so it keeps the render's placement within the tile."""
    n = round(TILE * height / PH)
    canvas.alpha_composite(P.resize((n, n), Image.LANCZOS), (round(cx - n / 2), round(cy - n / 2)))
    return canvas


def tile(n, p_frac=PH / TILE, grain=0.010):
    return put_p(ground(n, grain).convert("RGBA"), n * p_frac, n / 2, n / 2)


def squircle(x, y, w, h, r):
    """Apple's continuous-corner rounded rectangle."""
    k = [1.52866483, 1.08849323, 0.86840689, 0.66993427, 0.06549600, 0.37282392, 0.18120408]
    a, b, c, d, e, f, g = (v * r for v in k)
    X, Y = x + w, y + h
    return (
        f"M{x + a:.2f},{y:.2f}H{X - a:.2f}"
        f"C{X - b:.2f},{y:.2f} {X - c:.2f},{y:.2f} {X - d:.2f},{y + e:.2f}"
        f"C{X - f:.2f},{y + g:.2f} {X - g:.2f},{y + f:.2f} {X - e:.2f},{y + d:.2f}"
        f"C{X:.2f},{y + c:.2f} {X:.2f},{y + b:.2f} {X:.2f},{y + a:.2f}"
        f"V{Y - a:.2f}"
        f"C{X:.2f},{Y - b:.2f} {X:.2f},{Y - c:.2f} {X - e:.2f},{Y - d:.2f}"
        f"C{X - g:.2f},{Y - f:.2f} {X - f:.2f},{Y - g:.2f} {X - d:.2f},{Y - e:.2f}"
        f"C{X - c:.2f},{Y:.2f} {X - b:.2f},{Y:.2f} {X - a:.2f},{Y:.2f}"
        f"H{x + a:.2f}"
        f"C{x + b:.2f},{Y:.2f} {x + c:.2f},{Y:.2f} {x + d:.2f},{Y - e:.2f}"
        f"C{x + f:.2f},{Y - g:.2f} {x + g:.2f},{Y - f:.2f} {x + e:.2f},{Y - d:.2f}"
        f"C{x:.2f},{Y - c:.2f} {x:.2f},{Y - b:.2f} {x:.2f},{Y - a:.2f}"
        f"V{y + a:.2f}"
        f"C{x:.2f},{y + b:.2f} {x:.2f},{y + c:.2f} {x + e:.2f},{y + d:.2f}"
        f"C{x + g:.2f},{y + f:.2f} {x + f:.2f},{y + g:.2f} {x + d:.2f},{y + e:.2f}"
        f"C{x + c:.2f},{y:.2f} {x + b:.2f},{y:.2f} {x + a:.2f},{y:.2f}Z"
    )


def rounded_poly(pts, r):
    """A convex polygon (clockwise on screen) with every corner rounded by r."""
    d = ""
    for i in range(len(pts)):
        p0, p1, p2 = pts[i - 1], pts[i], pts[(i + 1) % len(pts)]
        v1 = (p0[0] - p1[0], p0[1] - p1[1])
        v2 = (p2[0] - p1[0], p2[1] - p1[1])
        l1, l2 = math.hypot(*v1), math.hypot(*v2)
        u1 = (v1[0] / l1, v1[1] / l1)
        u2 = (v2[0] / l2, v2[1] / l2)
        t = r / math.tan(math.acos(max(-1.0, min(1.0, u1[0] * u2[0] + u1[1] * u2[1]))) / 2)
        a = (p1[0] + u1[0] * t, p1[1] + u1[1] * t)
        b = (p1[0] + u2[0] * t, p1[1] + u2[1] * t)
        d += ("M" if i == 0 else "L") + f"{a[0]:.2f},{a[1]:.2f}A{r:.2f},{r:.2f} 0 0 1 {b[0]:.2f},{b[1]:.2f}"
    return d + "Z"


def mono_path(off=18.0):
    """The themed icon's P in dp on the 108 dp layer: a flat-topped D bowl,
    a square stem and a play-arrow counter, drawn to the metal P's outline
    (inside the 66 dp safe circle)."""
    rc, rj = 1.6, 2.0
    x0, y0, xs, yb, yf, xr = 18.5 + off, 10.1 + off, 29.5 + off, 48.0 + off, 61.2 + off, 57.0 + off
    rb = (yb - y0) / 2
    cxb = xr - rb
    d = (f"M{x0 + rc:.2f},{y0:.2f}H{cxb:.2f}A{rb:.2f},{rb:.2f} 0 0 1 {cxb:.2f},{yb:.2f}H{xs + rj:.2f}"
         f"Q{xs:.2f},{yb:.2f} {xs:.2f},{yb + rj:.2f}V{yf - rc:.2f}Q{xs:.2f},{yf:.2f} {xs - rc:.2f},{yf:.2f}"
         f"H{x0 + rc:.2f}Q{x0:.2f},{yf:.2f} {x0:.2f},{yf - rc:.2f}V{y0 + rc:.2f}Q{x0:.2f},{y0:.2f} {x0 + rc:.2f},{y0:.2f}Z")
    return d + rounded_poly([(27.8 + off, 20.8 + off), (45.0 + off, 29.0 + off), (27.8 + off, 37.2 + off)], 1.5)


def svg_alpha(svg):
    png = subprocess.run(["rsvg-convert"], input=svg.encode(), capture_output=True, check=True).stdout
    tmp = ART / ".mask.png"
    tmp.write_bytes(png)
    try:
        return Image.open(tmp).getchannel("A").copy()
    finally:
        tmp.unlink()


def desktop():
    """macOS Big Sur grid: an 824 px body on a 1024 canvas, a drop shadow
    and a thin light rim. Linux shows the same picture."""
    S = 1024
    body = squircle(100, 100, 824, 824, 185.4)
    mask = svg_alpha(f'<svg xmlns="http://www.w3.org/2000/svg" width="{S}" height="{S}"><path d="{body}"/></svg>')
    out = Image.new("RGBA", (S, S))
    shadow = Image.new("RGBA", (S, S))
    shadow.putalpha(mask.point(lambda v: v * 0.34).filter(ImageFilter.GaussianBlur(14)))
    out.alpha_composite(shadow, (0, 12))
    face = Image.new("RGBA", (S, S))
    face.paste(tile(824, 0.74), (100, 100))
    face.putalpha(mask)
    out.alpha_composite(face)
    rim = Image.new("RGBA", (S, S), (150, 205, 215, 0))
    edge = np.asarray(mask, float) - np.asarray(mask.filter(ImageFilter.MinFilter(5)), float)
    rim.putalpha(Image.fromarray((np.clip(edge, 0, 255) * 0.30).astype(np.uint8)))
    out.alpha_composite(rim)
    out.save(ROOT / "crates" / "iptv-app" / "icon.png", optimize=True)
    out.save(ROOT / "crates" / "iptv-app" / "icon.ico",
             sizes=[(n, n) for n in (16, 24, 32, 48, 64, 128, 256)])


def android():
    """Adaptive icon layers, 108 dp: the visible 72 dp middle shows the
    master tile; the P (52.5 dp tall) stays inside the 66 dp safe circle."""
    for d, k in {"mdpi": 1, "hdpi": 1.5, "xhdpi": 2, "xxhdpi": 3, "xxxhdpi": 4}.items():
        n, vis = round(108 * k), round(72 * k)
        pad = (n - vis) // 2
        g = np.pad(np.asarray(ground(vis, grain=0)), ((pad, n - vis - pad), (pad, n - vis - pad), (0, 0)), mode="edge")
        out = RES / f"mipmap-{d}"
        out.mkdir(exist_ok=True)
        Image.fromarray(g).save(out / "ic_launcher_background.png", optimize=True)
        put_p(Image.new("RGBA", (n, n)), 52.5 * k, n / 2, n / 2).save(out / "ic_launcher_foreground.png", optimize=True)
    (RES / "drawable" / "ic_launcher_monochrome.xml").write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n'
        "<!-- Themed icon layer (Android 13+): the P's silhouette, tinted by the\n"
        "     system. Generated by tools/make-icons.py. -->\n"
        '<vector xmlns:android="http://schemas.android.com/apk/res/android"\n'
        '    android:width="108dp" android:height="108dp"\n'
        '    android:viewportWidth="108" android:viewportHeight="108">\n'
        f'    <path android:fillColor="#FFFFFFFF" android:fillType="evenOdd"\n        android:pathData="{mono_path()}" />\n'
        "</vector>\n")


def ios():
    """Opaque square tile; iPadOS supplies the outer mask and shadow."""
    tile(1024, p_frac=0.74, grain=0).convert("RGB").save(
        ROOT / "crates" / "iptv-app" / "icon-ios.png", optimize=True
    )


def banner():
    """Android TV banner, 320x180 dp."""
    master = Image.open(ART / "banner.png").convert("RGB")
    for d, k in {"xhdpi": 1, "xxhdpi": 1.5, "xxxhdpi": 2}.items():
        out = RES / f"drawable-{d}"
        out.mkdir(exist_ok=True)
        master.resize((round(320 * k), round(180 * k)), Image.LANCZOS).save(out / "banner.png", optimize=True)


def play():
    tile(512).convert("RGB").save(ART / "play-512.png", optimize=True)


if __name__ == "__main__":
    generators = {"desktop": desktop, "ios": ios, "android": android, "banner": banner, "play": play}
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", choices=["all", *generators], default="all")
    selected = parser.parse_args().only
    for name, generate in generators.items():
        if selected in ("all", name):
            generate()
    print("icons written")
