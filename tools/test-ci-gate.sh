#!/usr/bin/env bash
# Synthetic checks for tools/ci-gate.sh and the shape of .github/workflows/ci.yml (#341).
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
gate="$repo_dir/tools/ci-gate.sh"
workflow="$repo_dir/.github/workflows/ci.yml"
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

# detect <base> <head>, run inside a throwaway repository: want is the printed value, or "fail" for a non-zero exit.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
git_t() { git -C "$tmp" -c user.name=test -c user.email=test@example.invalid -c commit.gpgsign=false "$@"; }
git_t init -q
mkdir "$tmp/docs" "$tmp/src"
echo base > "$tmp/README.md"
git_t add -A
git_t commit -q -m base
base="$(git_t rev-parse HEAD)"
echo more >> "$tmp/README.md"
echo adr > "$tmp/docs/note.md"
git_t add -A
git_t commit -q -m docs
docs="$(git_t rev-parse HEAD)"
echo 'fn main() {}' > "$tmp/src/main.rs"
git_t add -A
git_t commit -q -m code
code="$(git_t rev-parse HEAD)"

expect_detect() {
  local want="$1" name="$2" got status=0
  shift 2
  got="$(cd "$tmp" && "$gate" detect "$@" 2>/dev/null)" || status=$?
  if [ "$want" = fail ]; then
    [ "$status" -ne 0 ] || fail "#341 detect $name: want failure, got exit 0 with '$got'"
  else
    [ "$status" -eq 0 ] || fail "#341 detect $name: want $want, got exit $status"
    [ "$got" = "$want" ] || fail "#341 detect $name: want $want, got '$got'"
  fi
}

expect_detect false "docs-only range" "$base" "$docs"
expect_detect true "range with code" "$base" "$code"
expect_detect false "identical base and head" "$docs" "$docs"
expect_detect true "empty base" "" "$code"
expect_detect true "all-zero base" 0000000000000000000000000000000000000000 "$code"
expect_detect true "base not in history" 1111111111111111111111111111111111111111 "$code"
expect_detect fail "unknown head" "$base" 2222222222222222222222222222222222222222
expect_detect fail "missing head" "$base"
expect_detect fail "no arguments"

# Workflow shape: documentation-only runs must leave no skipped job in the PR check rollup, because
# xp-ship forbids merging with a skipped check. So `check` is the only job, has no job-level `if`,
# and the Rust steps are conditional steps (a skipped step is not a skipped check).
jobs="$(awk '/^jobs:/ {in_jobs=1; next} in_jobs && /^[^ #]/ {in_jobs=0} in_jobs && /^  [A-Za-z0-9_-]+:/ {sub(/^  /, ""); sub(/:.*/, ""); print}' "$workflow")"
[ "$jobs" = check ] || fail "#341 workflow: expected exactly the job 'check', found: $(echo "$jobs" | tr '\n' ' ')"
if grep -Eq '^    (if|needs):' "$workflow"; then
  fail "#341 workflow: a job-level 'if' or 'needs' can skip a job"
fi
# Unknown change detection must still run Rust: the Rust steps skip only on an exact 'false'.
if grep -E "steps\.diff\.outputs\.code == " "$workflow" >/dev/null; then
  fail "#341 workflow: Rust steps must run unless the output is exactly 'false' (use !=)"
fi
rust_steps="$(grep -c "if: steps.diff.outputs.code != 'false'" "$workflow" || true)"
[ "$rust_steps" -ge 6 ] || fail "#341 workflow: expected the Rust steps to be conditional on change detection, found $rust_steps"

[ "$failures" -eq 0 ] || exit 1
echo "ci-gate tests passed"
