#!/usr/bin/env bash
# Generates synthetic HLS test channels (ffmpeg test patterns, no real content)
# into fixtures/hls/ch<N>/index.m3u8 and serves them on :8089.
# The Android emulator reaches the host at http://10.0.2.2:8089/ch<N>/index.m3u8.
# Usage: tools/make-hls-fixtures.sh [--serve | --android-smoke]
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
dir="$root/fixtures/hls"
mkdir -p "$dir"
patterns=(testsrc2 smptebars rgbtestsrc)
duration=30
video_options=(-c:v libx264 -preset veryfast -g 50 -pix_fmt yuv420p)
if [ "${1:-}" = "--android-smoke" ]; then
  # The shared live transport concatenates TS bytes. Encode one continuous
  # timeline for the full smoke journey, including its 180-second refresh wait.
  duration=300
  video_options+=(-b:v 600k -maxrate 700k -bufsize 1200k)
fi
for n in 0 1 2; do
  out="$dir/ch$n"
  if [ "$duration" = 30 ] && [ -f "$out/index.m3u8" ]; then
    continue
  fi
  mkdir -p "$out"
  ffmpeg -loglevel error -y \
    -f lavfi -i "${patterns[$n]}=size=1280x720:rate=25" \
    -f lavfi -i "sine=frequency=$((440 + n * 220)):sample_rate=48000" \
    -t "$duration" "${video_options[@]}" \
    -c:a aac -b:a 96k \
    -f hls -hls_time 2 -hls_playlist_type vod \
    -hls_segment_filename "$out/seg%03d.ts" "$out/index.m3u8"
  echo "made ch$n (${patterns[$n]})"
done
if [ "${1:-}" = "--serve" ]; then
  exec python3 -m http.server 8089 --bind 0.0.0.0 --directory "$dir"
fi
