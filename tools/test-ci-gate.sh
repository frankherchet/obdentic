#!/usr/bin/env bash
# Synthetic checks for tools/ci-gate.sh (#341): which paths need the Rust job and when `check` may be green.
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
gate="$repo_dir/tools/ci-gate.sh"
failures=0

fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

expect_classify() {
  local want="$1" name="$2" got
  shift 2
  got="$({ [ "$#" -eq 0 ] || printf '%s\n' "$@"; } | "$gate" classify)"
  [ "$got" = "$want" ] || fail "#341 classify $name: want $want, got $got"
}

expect_verdict() {
  local want="$1" name="$2" status=0
  shift 2
  "$gate" verdict "$@" >/dev/null 2>&1 || status=$?
  if [ "$want" = pass ] && [ "$status" -ne 0 ]; then
    fail "#341 verdict $name: want pass, got exit $status"
  elif [ "$want" = reject ] && [ "$status" -eq 0 ]; then
    fail "#341 verdict $name: want reject, got pass"
  fi
}

# classify: documentation-only changes skip Rust; anything else, or unknown, runs it.
expect_classify false "requirements only" docs/requirements.yaml
expect_classify false "docs, memos and markdown" docs/android.md .memento/INDEX.md README.md
expect_classify true "source code" src/mobile.rs
expect_classify true "docs mixed with code" docs/android.md src/lib.rs
expect_classify true "workflow file" .github/workflows/ci.yml
expect_classify true "tools script" tools/ci-gate.sh
expect_classify true "android file" android/app/build.gradle
expect_classify true "lockfile" Cargo.lock
expect_classify true "yaml outside docs" requirements.yaml
expect_classify false "no changed paths"

# verdict <changes result> <code output> <rust result>
expect_verdict pass "docs-only: rust skipped" success false skipped
expect_verdict pass "code: rust succeeded" success true success
expect_verdict reject "code: rust failed" success true failure
expect_verdict reject "code: rust cancelled" success true cancelled
expect_verdict reject "code: rust skipped" success true skipped
expect_verdict reject "docs-only: rust ran anyway" success false success
expect_verdict reject "docs-only: rust failed" success false failure
expect_verdict reject "change detection failed" failure "" skipped
expect_verdict reject "change detection cancelled" cancelled "" skipped
expect_verdict reject "change detection skipped" skipped "" skipped
expect_verdict reject "code output missing" success "" skipped
expect_verdict reject "code output unknown" success maybe success
expect_verdict reject "too few arguments" success
expect_verdict reject "empty rust result" success true ""

[ "$failures" -eq 0 ] || exit 1
echo "ci-gate tests passed"
