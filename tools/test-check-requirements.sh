#!/usr/bin/env bash
# Synthetic checks for tools/check-requirements.py and its CI step (#372, REQ-056).
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
check="$repo_dir/tools/check-requirements.py"
schema="$repo_dir/docs/requirements.schema.json"
workflow="$repo_dir/.github/workflows/ci.yml"
failures=0
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}

# run <file>: the check's exit code in $code and its output in $out
run() {
  set +e
  out="$(uv run --quiet --script "$check" "$1" "$schema" 2>&1)"
  code=$?
  set -e
}

# expect_reject <criterion> <name> <yaml> <expected output part>...
expect_reject() {
  local id="$1" name="$2" yaml="$3" part
  shift 3
  printf '%s' "$yaml" > "$tmp/case.yaml"
  run "$tmp/case.yaml"
  [ "$code" -ne 0 ] || fail "$id $name: accepted, want rejected"
  for part in "$@"; do
    case "$out" in *"$part"*) ;; *) fail "$id $name: output lacks '$part': $out" ;; esac
  done
}

valid='version: 1
requirements:
  - id: REQ-001
    title: First
    type: functional
    priority: must
    status: accepted
    acceptance:
      - id: REQ-001.1
        text: works
  - id: REQ-002
    title: Second
    type: constraint
    priority: could
    status: draft
    depends_on: [REQ-001]
    acceptance:
      - id: REQ-002.1
        text: also works
'

# REQ-056.1: a valid file passes, and so does the repository's own file
printf '%s' "$valid" > "$tmp/valid.yaml"
run "$tmp/valid.yaml"
[ "$code" -eq 0 ] || fail "REQ-056.1 valid synthetic file rejected: $out"
run "$repo_dir/docs/requirements.yaml"
[ "$code" -eq 0 ] || fail "REQ-056.1 docs/requirements.yaml rejected: $out"

# REQ-056.2: invalid YAML names the line; a schema violation names the JSON path and the rule
expect_reject REQ-056.2 "invalid YAML" 'version: 1
requirements:
  - id: REQ-001
    title: [unclosed
' "line"
expect_reject REQ-056.2 "unknown status" "${valid/status: draft/status: done}" '$.requirements[1].status' "enum"
expect_reject REQ-056.2 "unknown key" "${valid/    title: First/    title: First
    owner: me}" '$.requirements[0]' "additionalProperties"
expect_reject REQ-056.2 "missing text" "${valid/        text: works
/}" '$.requirements[0].acceptance[0]' "required"
expect_reject REQ-056.2 "bad id pattern" "${valid/id: REQ-001.1/id: REQ-1.1}" '$.requirements[0].acceptance[0].id' "pattern"

# REQ-056.3: ID consistency the schema cannot express, each offending ID named
expect_reject REQ-056.3 "duplicate requirement" "${valid/id: REQ-002
/id: REQ-001
}" "REQ-001"
expect_reject REQ-056.3 "duplicate criterion" "${valid/id: REQ-002.1/id: REQ-002.1
        text: twice
      - id: REQ-002.1}" "REQ-002.1"
expect_reject REQ-056.3 "foreign criterion prefix" "${valid/id: REQ-002.1/id: REQ-003.1}" "REQ-003.1"
expect_reject REQ-056.3 "unknown dependency" "${valid/depends_on: \[REQ-001\]/depends_on: [REQ-009]}" "REQ-009"

# REQ-056.4: a validator that cannot be installed fails, it never passes
set +e
UV_OFFLINE=1 UV_CACHE_DIR="$tmp/empty-cache" UV_PYTHON_INSTALL_DIR="$tmp/no-python" uv run --quiet --script "$check" "$tmp/valid.yaml" "$schema" > "$tmp/offline.out" 2>&1
offline=$?
set -e
[ "$offline" -ne 0 ] || fail "REQ-056.4 the check passed although its libraries could not be installed"

# REQ-056.5: libraries pinned exactly, resolution frozen in time
grep -Eq '"pyyaml==[0-9.]+"' "$check" && grep -Eq '"jsonschema==[0-9.]+"' "$check" || fail "REQ-056.5 libraries are not pinned to exact versions"
grep -Eq '^# exclude-newer = "[0-9-]+T' "$check" || fail "REQ-056.5 transitive libraries are not frozen with exclude-newer"

# REQ-056.1/.4/.5 workflow: an unconditional step, the action pinned by commit and uv by version
grep -Eq '^      - uses: astral-sh/setup-uv@[0-9a-f]{40} ' "$workflow" || fail "REQ-056.5 setup-uv is not pinned to a commit"
grep -Eq '^          version: "[0-9]+\.[0-9]+\.[0-9]+"' "$workflow" || fail "REQ-056.5 uv is not pinned to an exact version"
step="$(awk '/name: Validate requirements/ {found=1} found && /^      - / && !/name: Validate requirements/ {exit} found {print}' "$workflow")"
[ -n "$step" ] || fail "REQ-056.1 workflow has no 'Validate requirements' step"
case "$step" in *"if:"*) fail "REQ-056.1 the validation step must not be conditional" ;; esac
case "$step" in *"uv run --script tools/check-requirements.py"*) ;; *) fail "REQ-056.5 CI does not run the documented command" ;; esac

[ "$failures" -eq 0 ] || exit 1
echo "requirements check tests passed"
