#!/bin/bash
# Throwaway build of the #378 spike app (aapt2 + javac + d8 + apksigner). Output: build/spike378.apk
set -euo pipefail
cd "$(dirname "$0")/.."
SDK=${ANDROID_HOME:-$HOME/Library/Android/sdk}; BT=$SDK/build-tools/35.0.0; AJ=$SDK/platforms/android-36/android.jar
rm -rf build && mkdir -p build/classes
$BT/aapt2 link -o build/base.apk -I "$AJ" --manifest AndroidManifest.xml --min-sdk-version 26 --target-sdk-version 36
javac -source 8 -target 8 -Xlint:-options -cp "$AJ" -d build/classes src/de/obdentic/spike378/*.java
$BT/d8 --lib "$AJ" --min-api 26 --output build build/classes/de/obdentic/spike378/*.class
cp build/base.apk build/unsigned.apk && (cd build && zip -q unsigned.apk classes.dex)
$BT/zipalign -f 4 build/unsigned.apk build/aligned.apk
[ -f .ks.jks ] || keytool -genkeypair -keystore .ks.jks -storepass spike378 -keypass spike378 -alias k -dname "CN=spike378" -keyalg RSA -keysize 2048 -validity 30 >/dev/null 2>&1
$BT/apksigner sign --ks .ks.jks --ks-pass pass:spike378 --out build/spike378.apk build/aligned.apk
echo built
