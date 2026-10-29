#!/usr/bin/env bash
# Checks for tools/pull-recordings.sh (REQ-019.1 .. REQ-019.6) against a fake adb and cargo.
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
script="$repo_dir/tools/pull-recordings.sh"
work="$(mktemp -d "${TMPDIR:-/tmp}/pull-recordings-test.XXXXXX")"
evidence_root="$repo_dir/evidence/test-pull-$$"
refused_dir="$repo_dir/tools/tmp-pull-$$"
trap 'rm -rf "$work" "$evidence_root" "$refused_dir"' EXIT

phone="$work/phone"
mkdir -p "$phone"
printf '%s\n' '{"type":"session_start"}' '{"type":"frame","n":1}' '{"type":"session_end","ok":true}' '' '' >"$phone/a.jsonl"
printf '%s\n' '{"type":"session_start"}' '{"type":"frame","n":1}' >"$phone/b.jsonl"
printf 'not a recording\n' >"$phone/notes.txt"
printf 'x\n' >"$phone/bad name.jsonl"
printf 'x\n' >"$phone/evil;touch.jsonl"

# Fake adb: read-only emulation of devices, shell ls/stat/test and pull. Anything else fails loudly.
adb_stub="$work/adb"
cat >"$adb_stub" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "$*" >>"$FAKE_ADB_LOG"
remote_dir=/sdcard/Download/OBDentic
case "${1:-}" in
  devices)
    echo "List of devices attached"
    i=0
    while [ "$i" -lt "${FAKE_ADB_DEVICES:-1}" ]; do
      printf 'PHONE%d\tdevice\n' "$i"
      i=$((i + 1))
    done
    printf 'OFFLINE0\toffline\n'
    ;;
  shell)
    case "${2:-}" in
      test)
        [ "${4:-}" = "$remote_dir" ] || exit 1
        ;;
      ls)
        [ "${!#}" = "$remote_dir" ] || exit 1
        ls -1 "$FAKE_PHONE_DIR" | while IFS= read -r f; do printf '%s\r\n' "$f"; done
        ;;
      stat)
        f="$FAKE_PHONE_DIR/$(basename "${!#}")"
        [ -f "$f" ] || exit 1
        printf '%s\r\n' "$(wc -c <"$f" | tr -d ' ')"
        ;;
      *) echo "stub adb: unsupported shell command: $*" >&2; exit 2 ;;
    esac
    ;;
  pull)
    cp "$FAKE_PHONE_DIR/$(basename "$2")" "$3"
    ;;
  *) echo "stub adb: unsupported command: $*" >&2; exit 2 ;;
esac
EOF

# Fake cargo: records the report invocation; FAKE_CARGO_FAIL=1 makes it fail.
cargo_stub="$work/cargo"
cat >"$cargo_stub" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$FAKE_CARGO_LOG"
if [ -n "${FAKE_CARGO_FAIL:-}" ]; then
  echo "fake report failure" >&2
  exit 1
fi
echo "report ok: ${!#}"
EOF
chmod +x "$adb_stub" "$cargo_stub"

export ADB="$adb_stub" PULL_RECORDINGS_CARGO="$cargo_stub"
export FAKE_PHONE_DIR="$phone" FAKE_ADB_LOG="$work/adb.log" FAKE_CARGO_LOG="$work/cargo.log"
export FAKE_ADB_DEVICES=1
all_adb_log="$work/adb-all.log"
: >"$all_adb_log"

# run <command...>: runs with output captured in OUT and status in RC, logging adb calls.
run() {
  : >"$FAKE_ADB_LOG"
  : >"$FAKE_CARGO_LOG"
  RC=0
  OUT="$("$@" 2>&1)" || RC=$?
  cat "$FAKE_ADB_LOG" >>"$all_adb_log"
}

has() { grep -qF -- "$1" <<<"$OUT"; }
not() { if "$@"; then return 1; else return 0; fi; }

failures=0
check() { # check <req id> <description> <command...>
  local req="$1" desc="$2"
  shift 2
  if "$@"; then
    echo "PASS $req $desc"
  else
    echo "FAIL $req $desc"
    failures=$((failures + 1))
  fi
}

dest="$evidence_root/phone"

# REQ-019.1: copy, skip on rerun, counts, same-name-different-size re-pull, unsafe names skipped.
run "$script" --dest "$dest"
first_out="$OUT"
check REQ-019.1 "first run exits 0" [ "$RC" -eq 0 ]
check REQ-019.1 "a.jsonl copied byte-for-byte" cmp -s "$phone/a.jsonl" "$dest/a.jsonl"
check REQ-019.1 "b.jsonl copied byte-for-byte" cmp -s "$phone/b.jsonl" "$dest/b.jsonl"
check REQ-019.1 "first run prints copy counts" has "Summary: copied 2 (1 partial), skipped 0, failed 0"
check REQ-019.1 "unsafe name skipped with warning" grep -qF "skipping unexpected name: bad name.jsonl" <<<"$first_out"
check REQ-019.1 "unsafe name never pulled" not grep -qF "bad name" "$all_adb_log"
check REQ-019.1 "shell-metacharacter name never pulled" not grep -qF "evil" "$all_adb_log"
check REQ-019.1 "non-jsonl file not copied" [ ! -e "$dest/notes.txt" ]

run "$script" --dest "$dest"
check REQ-019.1 "rerun exits 0" [ "$RC" -eq 0 ]
check REQ-019.1 "rerun skips files with same name and size" has "skipped a.jsonl (already present, same size)"
check REQ-019.1 "rerun prints skip counts" has "Summary: copied 0 (0 partial), skipped 2, failed 0"

: >"$dest/a.jsonl"
run "$script" --dest "$dest"
check REQ-019.1 "same name but different size is pulled again" cmp -s "$phone/a.jsonl" "$dest/a.jsonl"
check REQ-019.1 "size mismatch counts as copy, not skip" has "Summary: copied 1 (0 partial), skipped 1, failed 0"

# REQ-019.2: a destination git does not ignore is refused before it is created.
run "$script" --dest "tools/tmp-pull-$$"
check REQ-019.2 "non-ignored destination exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.2 "refusal is explained" has "refusing destination that git does not ignore"
check REQ-019.2 "non-ignored destination not created" [ ! -e "$refused_dir" ]

# REQ-019.3: complete vs partial reporting, partial files kept.
check REQ-019.3 "session_end file reported complete" grep -qF "copied a.jsonl (complete)" <<<"$first_out"
check REQ-019.3 "file without session_end reported partial" grep -qF "copied b.jsonl (partial)" <<<"$first_out"
check REQ-019.3 "partial file kept on disk" cmp -s "$phone/b.jsonl" "$dest/b.jsonl"

# REQ-019.4: zero devices, two devices, missing folder: non-zero, nothing written.
none_dest="$evidence_root/none"

export FAKE_ADB_DEVICES=0
run "$script" --dest "$none_dest"
check REQ-019.4 "zero devices exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.4 "zero devices explained" has "no adb device in state 'device'"
check REQ-019.4 "zero devices writes nothing" [ ! -e "$none_dest" ]
check REQ-019.4 "zero devices pulls nothing" not grep -q "^pull " "$FAKE_ADB_LOG"

export FAKE_ADB_DEVICES=2
run "$script" --dest "$none_dest"
check REQ-019.4 "two devices exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.4 "two devices explained" has "2 adb devices in state 'device'"
check REQ-019.4 "two devices writes nothing" [ ! -e "$none_dest" ]
check REQ-019.4 "two devices pulls nothing" not grep -q "^pull " "$FAKE_ADB_LOG"

export FAKE_ADB_DEVICES=1
run "$script" --dest "$none_dest" --source /sdcard/Missing
check REQ-019.4 "missing phone folder exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.4 "missing phone folder explained" has "phone folder not found: /sdcard/Missing"
check REQ-019.4 "missing phone folder writes nothing" [ ! -e "$none_dest" ]
check REQ-019.4 "missing phone folder pulls nothing" not grep -q "^pull " "$FAKE_ADB_LOG"

# REQ-019.5: only read-only adb commands reach the phone.
check REQ-019.5 "adb invocations were recorded" test -s "$all_adb_log"
check REQ-019.5 "only devices, shell ls/stat/test and pull were used" not grep -qvE '^(devices$|shell (ls|stat|test) |pull )' "$all_adb_log"
check REQ-019.5 "no push, rm, install or write shell command" not grep -qE '(push|rm|install)' "$all_adb_log"

# REQ-019.6: --report runs the report for newly pulled files only; a failing report keeps the file.
report_dest="$evidence_root/report"
run "$script" --report --dest "$report_dest"
check REQ-019.6 "report runs once per newly pulled file" [ "$(grep -c . "$FAKE_CARGO_LOG")" -eq 2 ]
check REQ-019.6 "report gets the local a.jsonl path" grep -qF "$report_dest/a.jsonl" "$FAKE_CARGO_LOG"
check REQ-019.6 "report gets the local b.jsonl path" grep -qF "$report_dest/b.jsonl" "$FAKE_CARGO_LOG"
check REQ-019.6 "report output is shown" has "report ok: $report_dest/a.jsonl"

run "$script" --report --dest "$report_dest"
check REQ-019.6 "rerun reports nothing for skipped files" [ ! -s "$FAKE_CARGO_LOG" ]

export FAKE_CARGO_FAIL=1
run "$script" --report --dest "$evidence_root/report-fail"
unset FAKE_CARGO_FAIL
check REQ-019.6 "failing report does not fail the pull" [ "$RC" -eq 0 ]
check REQ-019.6 "failing report prints a warning" has "warning: report failed for a.jsonl; file kept"
check REQ-019.6 "failing report keeps the file" cmp -s "$phone/a.jsonl" "$evidence_root/report-fail/a.jsonl"

# REQ-029.6: sealed app logs are pulled byte-for-byte, are not classified as partial recordings, and unsafe log names are skipped.
printf 'line\n' >"$phone/app-20261029-120000-000.log"
printf 'x\n' >"$phone/evil name.log"
log_dest="$evidence_root/logs"
run "$script" --dest "$log_dest"
check REQ-029.6 "sealed log copied byte-for-byte" cmp -s "$phone/app-20261029-120000-000.log" "$log_dest/app-20261029-120000-000.log"
check REQ-029.6 "log reported as log, not partial" has "copied app-20261029-120000-000.log (log)"
check REQ-029.6 "unsafe log name skipped" has "skipping unexpected name: evil name.log"

# Usage text.
run "$script" --help
check usage "--help prints usage and exits 0" [ "$RC" -eq 0 ]
check usage "--help shows the Usage line" has "Usage: tools/pull-recordings.sh"

echo
if [ "$failures" -eq 0 ]; then
  echo "all checks passed"
else
  echo "$failures check(s) failed"
  exit 1
fi
