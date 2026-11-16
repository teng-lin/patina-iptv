#!/usr/bin/env bash
# Build and fetch all iOS inputs before the workflow exposes signing secrets.
set -euo pipefail
for name in IOS_CERTIFICATE IOS_CERTIFICATE_PASSWORD IOS_PROFILE IOS_SIGN_IDENTITY; do
  if [ -n "${!name:-}" ]; then
    echo "Signing inputs must not be present during iOS preparation" >&2
    exit 1
  fi
done
rustup target add aarch64-apple-ios
bash tools/fetch-mpvkit.sh
root=$PWD
source "$root/tools/ci/locked-build-env.sh"
(cd crates/iptv-app && "$NSH" package . --target ios --icon icon-ios.png -o "$root/target/ios")
bash tools/ci/notices.sh
python3 tools/ci/mpvkit-notices.py
cp -R target/notices target/ios/Patina.app/notices
python3 tools/ci/check-notices.py target/ios/Patina.app mpvkit/components.json
