# App icon and banner masters

The metal P comes from a Gemini render chosen for the product name
*Patina*. Two files were cut out of that render:

- `p-overlay.png`: the P with its shadows, transparent elsewhere, placed in
  a 593 px tile;
- `ground.png`: the tile's navy ground (a smooth fit, no P).

`banner.png` (732×412) is Gemini's banner render with its wordmark's own
"P" removed: the big metal P is the word's first letter and "atina" (the
render's lettering, same size) moved up to it. The background where the
wordmark was is filled back in, dot screen included.

`tools/make-icons.py` builds every icon from them (desktop, iOS, Android
adaptive and themed layers, TV banner, `play-512.png` for the Play
listing). The 593 px tile is the limit on sharpness: the 1024 px desktop
icon is drawn about 1.3× up. A larger render of the same P can replace
`p-overlay.png` and `ground.png` (same tile layout) without other changes.

iOS packages use `crates/iptv-app/icon-ios.png`, an opaque 1024×1024 tile
with no inset or corner mask; iPadOS supplies the mask. The desktop icon's
transparent margins must not be used for iOS. Rebuild only this asset with
`uv run --no-project --with numpy --with pillow python tools/make-icons.py --only ios`.
The release preparation script selects it with `--icon icon-ios.png`.
