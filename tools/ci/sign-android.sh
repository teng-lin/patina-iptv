#!/usr/bin/env bash
# Choose the key the signing job signs the Android packages with, then sign
# and verify them (package-android.sh).
#
# A release (REHEARSAL=false) signs with the production keystore of the
# `release` environment and fails without it. There is no debug or throwaway
# fallback: packages signed with any other key could never update an installed
# release.
#
# A rehearsal (REHEARSAL=true, manual runs only) never receives that keystore
# and refuses to run if any of it arrived. It signs with a key pair made here
# for this one run: a PKCS12 store under RUNNER_TEMP, outside the checkout,
# behind a random masked password, deleted when this script ends (and again by
# the job's always() step). Its packages prove the signing and verification
# path; they are not release artifacts and share no key with one.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${RUNNER_TEMP:?}"
umask 077

fail() {
  echo "error: $*" >&2
  exit 1
}

case "${REHEARSAL:-}" in
  true | false) ;;
  *) fail "REHEARSAL must be true or false, not '${REHEARSAL:-}'" ;;
esac
# Signing material never lives where an artifact upload could pick it up.
[ -d "$RUNNER_TEMP" ] || fail "RUNNER_TEMP is not a directory"
temp=$(cd "$RUNNER_TEMP" && pwd -P)
case "$temp/" in
  "$(pwd -P)"/*) fail "RUNNER_TEMP is inside the checkout" ;;
esac

keystore=
trap '[ -z "$keystore" ] || rm -f "$keystore"' EXIT

if [ "$REHEARSAL" = true ]; then
  # Publication to a public release happens only on a tag push, so a package
  # signed with a throwaway key cannot reach one.
  [ "${GITHUB_EVENT_NAME:-}" = workflow_dispatch ] ||
    fail "a rehearsal is a manual run, not a '${GITHUB_EVENT_NAME:-}' event: refusing to sign with a throwaway key"
  for name in KEYSTORE KEYSTORE_PATH STORE_PASSWORD KEY_PASSWORD KEY_ALIAS; do
    [ -z "${!name:-}" ] || fail "$name is set: a rehearsal must not receive release signing credentials"
  done
  command -v keytool >/dev/null || fail "keytool is not installed"
  keystore="$temp/rehearsal-android.p12"
  rm -f "$keystore"
  password=$(openssl rand -hex 24)
  [ "${#password}" -eq 48 ] || fail "could not generate a keystore password"
  echo "::add-mask::$password"
  # PKCS12 has one password for the store and its key.
  export KEYSTORE_PATH="$keystore" STORE_PASSWORD="$password" KEY_PASSWORD="$password" KEY_ALIAS=rehearsal
  keytool -genkeypair -noprompt -storetype PKCS12 -keystore "$keystore" -alias "$KEY_ALIAS" \
    -keyalg RSA -keysize 2048 -sigalg SHA256withRSA -validity 2 \
    -dname "CN=Patina release rehearsal, OU=Throwaway key, O=Not for distribution" \
    -storepass:env STORE_PASSWORD -keypass:env KEY_PASSWORD
  certificate=$(keytool -exportcert -keystore "$keystore" -alias "$KEY_ALIAS" -storepass:env STORE_PASSWORD |
    openssl dgst -sha256 -r | cut -d' ' -f1)
  echo "::notice::Rehearsal: the Android packages of this run are signed with a throwaway key" \
    "(certificate SHA-256 $certificate). They are not release artifacts."
else
  missing=
  [ -n "${KEYSTORE:-}" ] || missing="$missing ANDROID_KEYSTORE_BASE64"
  [ -n "${STORE_PASSWORD:-}" ] || missing="$missing ANDROID_STORE_PASSWORD"
  [ -n "${KEY_PASSWORD:-}" ] || missing="$missing ANDROID_KEY_PASSWORD"
  [ -n "${KEY_ALIAS:-}" ] || missing="$missing ANDROID_KEY_ALIAS"
  [ -z "$missing" ] ||
    fail "release signing secrets are not set (${missing# }): an Android release is signed only with the" \
      "production keystore of the release environment, never with a debug or throwaway key"
  keystore="$temp/release.jks"
  printf '%s' "$KEYSTORE" | base64 --decode > "$keystore"
  unset KEYSTORE
  [ -s "$keystore" ] || fail "ANDROID_KEYSTORE_BASE64 decodes to an empty keystore"
  export KEYSTORE_PATH="$keystore"
fi

bash tools/ci/package-android.sh
