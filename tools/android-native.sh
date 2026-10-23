#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
sdk_dir="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Library/Android/sdk}}"
ndk_dir="${ANDROID_NDK_HOME:-$sdk_dir/ndk/27.2.12479018}"
case "$(uname -s)" in
  Darwin) ndk_host=darwin-x86_64 ;;
  Linux) ndk_host=linux-x86_64 ;;
  *) echo 'Android build currently supports macOS and Linux hosts.' >&2; exit 1 ;;
esac
linker="$ndk_dir/toolchains/llvm/prebuilt/$ndk_host/bin/aarch64-linux-android26-clang"
if [[ ! -x "$linker" ]]; then
  echo "Install Android NDK 27.2.12479018 or set ANDROID_NDK_HOME: missing $linker" >&2
  exit 1
fi
rtk env PATH="$HOME/.cargo/bin:/opt/local/bin:/usr/bin:/bin" \
  CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$linker" \
  cargo +1.98.0 build --manifest-path "$repo_dir/android/native/Cargo.toml" \
  --target aarch64-linux-android --release --locked
mkdir -p "$repo_dir/android/app/src/main/jniLibs/arm64-v8a"
cp "$repo_dir/android/native/target/aarch64-linux-android/release/libobdentic_mobile.so" \
  "$repo_dir/android/app/src/main/jniLibs/arm64-v8a/"
