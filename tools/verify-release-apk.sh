#!/usr/bin/env bash
# Checks a release APK before distribution: validly signed, not debug-signed,
# not debuggable, not the test APK, no capture/evidence files, and exactly the
# permissions declared in the source manifest (REQ-017.3, REQ-017.7).
# Usage: tools/verify-release-apk.sh <apk>
set -euo pipefail

apk="${1:?usage: tools/verify-release-apk.sh <apk>}"
repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
manifest="$repo_dir/android/app/src/main/AndroidManifest.xml"
sdk_dir="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
bt="$(ls -d "$sdk_dir"/build-tools/*/ 2>/dev/null | sort -V | tail -1 || true)"
bt="${bt%/}"
[[ -x "$bt/apksigner" && -x "$bt/aapt2" ]] || { echo "No Android build-tools (apksigner, aapt2) under $sdk_dir" >&2; exit 1; }
fail() { echo "verify-release-apk: FAIL: $*" >&2; exit 1; }

certs="$("$bt/apksigner" verify --print-certs "$apk")" || fail "apksigner could not verify the signature"
grep -q 'CN=Android Debug' <<<"$certs" && fail "APK is signed with the Android debug certificate"
grep -q 'certificate DN' <<<"$certs" || fail "no signer certificate found"

badging="$("$bt/aapt2" dump badging "$apk")"
grep -q "^package: name='de.obdentic.app'" <<<"$badging" || fail "unexpected package name"
grep -q '^application-debuggable' <<<"$badging" && fail "APK is debuggable"
grep -q '^instrumentation' <<<"$badging" && fail "APK is an instrumentation (test) APK"

if unzip -Z1 "$apk" | grep -Eiq '(^|/)(captures|evidence)(/|$)|\.(jsonl|btsnoop)$'; then
  fail "APK contains capture/evidence-like files"
fi

want="$(sed -n 's/.*<uses-permission android:name="\([^"]*\)".*/\1/p' "$manifest" | sort -u)"
have="$("$bt/aapt2" dump permissions "$apk" | sed -n "s/^uses-permission[^:]*: name='\([^']*\)'.*/\1/p" | sort -u)"
[[ -n "$want" && "$want" == "$have" ]] || {
  echo "expected permissions:" >&2; echo "$want" >&2
  echo "APK permissions:" >&2; echo "$have" >&2
  fail "APK permissions differ from AndroidManifest.xml"
}
echo "verify-release-apk: OK ($(grep -o "versionName='[^']*'" <<<"$badging" | head -1), $(grep -o "versionCode='[^']*'" <<<"$badging" | head -1))"
