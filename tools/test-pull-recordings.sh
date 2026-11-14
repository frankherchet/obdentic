#!/usr/bin/env bash
# Checks for tools/pull-recordings.sh (REQ-019.1 .. REQ-019.8, REQ-029.6) against a fake adb and cargo.
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
    if [ "$(basename "$2")" = "${FAKE_ADB_PULL_FAIL:-}" ]; then
      printf 'partial' >"$3"
      exit 1
    fi
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
undated="$dest/undatiert"

# REQ-019.1: copy, skip on rerun, counts, same-name-different-size re-pull, unsafe names skipped.
# REQ-019.7: names without a timestamp (a.jsonl, b.jsonl) go to undatiert under the custom --dest.
run "$script" --dest "$dest"
first_out="$OUT"
check REQ-019.1 "first run exits 0" [ "$RC" -eq 0 ]
check REQ-019.1 "a.jsonl copied byte-for-byte" cmp -s "$phone/a.jsonl" "$undated/a.jsonl"
check REQ-019.1 "b.jsonl copied byte-for-byte" cmp -s "$phone/b.jsonl" "$undated/b.jsonl"
check REQ-019.1 "first run prints copy counts" has "Summary: copied 2 (1 partial), skipped 0, failed 0"
check REQ-019.1 "unsafe name skipped with warning" grep -qF "skipping unexpected name: bad name.jsonl" <<<"$first_out"
check REQ-019.1 "unsafe name never pulled" not grep -qF "bad name" "$all_adb_log"
check REQ-019.1 "shell-metacharacter name never pulled" not grep -qF "evil" "$all_adb_log"
check REQ-019.1 "non-jsonl file not copied" [ ! -e "$undated/notes.txt" ]
check REQ-019.7 "name without timestamp goes to undatiert under --dest" [ -f "$undated/a.jsonl" ]
check REQ-019.7 "nothing is written flat into --dest" [ -z "$(find "$dest" -maxdepth 1 -type f)" ]
check REQ-019.7 "no temporary pull file is left behind" [ -z "$(find "$dest" -name '*.pulling')" ]

run "$script" --dest "$dest"
check REQ-019.1 "rerun exits 0" [ "$RC" -eq 0 ]
check REQ-019.1 "rerun skips files with same name and size" has "skipped a.jsonl (already present, same size)"
check REQ-019.1 "rerun prints skip counts" has "Summary: copied 0 (0 partial), skipped 2, failed 0"

# REQ-019.8: same name, different size: the old bytes are kept under konflikte/<UTC>/ before the replacement.
printf 'old a bytes\n' >"$undated/a.jsonl"
cp "$undated/a.jsonl" "$work/old-a.jsonl"
run "$script" --dest "$dest"
archived_a="$(find "$dest/konflikte" -type f -name a.jsonl 2>/dev/null || true)"
check REQ-019.1 "same name but different size is pulled again" cmp -s "$phone/a.jsonl" "$undated/a.jsonl"
check REQ-019.1 "size mismatch counts as copy, not skip" has "Summary: copied 1 (0 partial), skipped 1, failed 0"
check REQ-019.8 "old different-size copy is kept byte-for-byte under konflikte" cmp -s "$work/old-a.jsonl" "$archived_a"
check REQ-019.8 "konflikte folder is named with a UTC timestamp and keeps the original name" grep -qE '/konflikte/[0-9]{8}T[0-9]{6}Z(-[0-9]+)?/a\.jsonl$' <<<"$archived_a"
check REQ-019.8 "archiving is announced" has "archived previous a.jsonl (different size)"

# REQ-019.2: a destination git does not ignore is refused before it is created.
run "$script" --dest "tools/tmp-pull-$$"
check REQ-019.2 "non-ignored destination exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.2 "refusal is explained" has "refusing destination that git does not ignore"
check REQ-019.2 "non-ignored destination not created" [ ! -e "$refused_dir" ]

# REQ-019.3: complete vs partial reporting, partial files kept.
check REQ-019.3 "session_end file reported complete" grep -qF "copied a.jsonl (complete)" <<<"$first_out"
check REQ-019.3 "file without session_end reported partial" grep -qF "copied b.jsonl (partial)" <<<"$first_out"
check REQ-019.3 "partial file kept on disk" cmp -s "$phone/b.jsonl" "$undated/b.jsonl"

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

# REQ-019.6: --report runs the report for newly pulled files only; a failing report keeps the file.
report_dest="$evidence_root/report"
run "$script" --report --dest "$report_dest"
check REQ-019.6 "report runs once per newly pulled file" [ "$(grep -c . "$FAKE_CARGO_LOG")" -eq 2 ]
check REQ-019.6 "report gets the local a.jsonl path" grep -qF "$report_dest/undatiert/a.jsonl" "$FAKE_CARGO_LOG"
check REQ-019.6 "report gets the local b.jsonl path" grep -qF "$report_dest/undatiert/b.jsonl" "$FAKE_CARGO_LOG"
check REQ-019.6 "report output is shown" has "report ok: $report_dest/undatiert/a.jsonl"

run "$script" --report --dest "$report_dest"
check REQ-019.6 "rerun reports nothing for skipped files" [ ! -s "$FAKE_CARGO_LOG" ]

export FAKE_CARGO_FAIL=1
run "$script" --report --dest "$evidence_root/report-fail"
unset FAKE_CARGO_FAIL
check REQ-019.6 "failing report does not fail the pull" [ "$RC" -eq 0 ]
check REQ-019.6 "failing report prints a warning" has "warning: report failed for a.jsonl; file kept"
check REQ-019.6 "failing report keeps the file" cmp -s "$phone/a.jsonl" "$evidence_root/report-fail/undatiert/a.jsonl"

# Fixtures for date grouping: timestamped recordings, a sealed app log, a leap day, impossible dates.
# The phone is read-only for the script; these only extend the fake phone folder.
printf 'line\n' >"$phone/app-20261029-120000-000.log"
printf 'x\n' >"$phone/evil name.log"
cp "$phone/a.jsonl" "$phone/20261029-181530-123-live.jsonl"
cp "$phone/b.jsonl" "$phone/20261030-000001-005-demo.jsonl"
cp "$phone/a.jsonl" "$phone/20240229-120000-000-leap.jsonl"
cp "$phone/a.jsonl" "$phone/20261331-250000-000-bad.jsonl"
cp "$phone/a.jsonl" "$phone/20260230-100000-000-feb.jsonl"
live_name=20261029-181530-123-live.jsonl
log_name=app-20261029-120000-000.log

all_ignored() { # all_ignored <dir>: every file below it is ignored by git (privacy)
  local f
  while IFS= read -r f; do
    (cd "$repo_dir" && git check-ignore -q "$f") || return 1
  done < <(find "$1" -type f)
}

# REQ-019.7: timestamped recordings and app logs are grouped per day by the date in the file name.
layout="$evidence_root/layout"
run "$script" --dest "$layout"
layout_out="$OUT"
check REQ-019.7 "dated import exits 0" [ "$RC" -eq 0 ]
check REQ-019.7 "dated import counts" has "Summary: copied 8 (2 partial), skipped 0, failed 0"
check REQ-019.7 "recording goes to its day folder, bytes unchanged" cmp -s "$phone/$live_name" "$layout/2026-10-29/$live_name"
check REQ-019.7 "app log goes to the same day folder, bytes unchanged" cmp -s "$phone/$log_name" "$layout/2026-10-29/$log_name"
check REQ-019.7 "recording of the next day goes to its own folder" cmp -s "$phone/20261030-000001-005-demo.jsonl" "$layout/2026-10-30/20261030-000001-005-demo.jsonl"
check REQ-019.7 "leap day 2024-02-29 is a valid day folder" cmp -s "$phone/20240229-120000-000-leap.jsonl" "$layout/2024-02-29/20240229-120000-000-leap.jsonl"
check REQ-019.7 "month 13 / hour 25 name goes to undatiert" cmp -s "$phone/20261331-250000-000-bad.jsonl" "$layout/undatiert/20261331-250000-000-bad.jsonl"
check REQ-019.7 "2026-02-30 goes to undatiert" cmp -s "$phone/20260230-100000-000-feb.jsonl" "$layout/undatiert/20260230-100000-000-feb.jsonl"
check REQ-019.7 "name without timestamp goes to undatiert" cmp -s "$phone/a.jsonl" "$layout/undatiert/a.jsonl"
check REQ-019.7 "only day folders and undatiert are created" [ "$(ls "$layout" | tr '\n' ' ')" = "2024-02-29 2026-10-29 2026-10-30 undatiert " ]
check REQ-019.7 "nothing is written flat into the destination" [ -z "$(find "$layout" -maxdepth 1 -type f)" ]
check REQ-019.7 "every phone file exists once with its original name" [ "$(find "$layout" -type f | wc -l | tr -d ' ')" -eq 8 ]
check REQ-019.7 "no temporary pull file is left" [ -z "$(find "$layout" -name '*.pulling')" ]
check REQ-019.8 "no conflicts folder without a conflict" [ ! -e "$layout/konflikte" ]
check REQ-019.7 "reported path is the actual day-folder path" grep -qF "copied $live_name (complete) -> $layout/2026-10-29/$live_name" <<<"$layout_out"
check REQ-019.3 "dated file without session_end reported partial" grep -qF "copied 20261030-000001-005-demo.jsonl (partial)" <<<"$layout_out"
check REQ-019.2 "every imported file lies in a git-ignored path" all_ignored "$layout"
check REQ-019.2 "no imported file is tracked by git" [ -z "$(git -C "$repo_dir" ls-files -- "$layout")" ]

# REQ-019.1: reimport is idempotent: same name and size in the day folder is skipped, nothing is pulled.
run "$script" --dest "$layout"
check REQ-019.1 "reimport skips every file in its day folder" has "Summary: copied 0 (0 partial), skipped 8, failed 0"
check REQ-019.1 "reimport pulls nothing" not grep -q "^pull " "$FAKE_ADB_LOG"
check REQ-019.1 "reimport skips the dated recording" has "skipped $live_name (already present, same size)"
check REQ-019.8 "reimport creates no conflicts folder" [ ! -e "$layout/konflikte" ]

# REQ-019.7: a relative --dest resolves from the repo root and is grouped the same way.
run "$script" --dest "evidence/test-pull-$$/rel"
check REQ-019.7 "relative --dest gets day folders" cmp -s "$phone/$live_name" "$evidence_root/rel/2026-10-29/$live_name"
check REQ-019.7 "relative --dest gets undatiert" [ -f "$evidence_root/rel/undatiert/a.jsonl" ]

# REQ-019.1: an older flat <dest>/<name> with the same size counts as present (no reimport); a flat copy
# with a different size is never touched and the day-folder version is pulled next to it.
legacy="$evidence_root/legacy"
mkdir -p "$legacy"
cp "$phone/$live_name" "$phone/a.jsonl" "$legacy/"
printf 'old flat b\n' >"$legacy/b.jsonl"
run "$script" --dest "$legacy"
check REQ-019.1 "flat copy with same size is skipped" has "skipped $live_name (already present at legacy flat path $legacy/$live_name, same size)"
check REQ-019.1 "flat skip counts" has "Summary: copied 6 (2 partial), skipped 2, failed 0"
check REQ-019.1 "flat copy is not pulled again" not grep -qF "pull /sdcard/Download/OBDentic/$live_name" "$FAKE_ADB_LOG"
check REQ-019.1 "no day-folder duplicate of the flat recording" [ ! -e "$legacy/2026-10-29/$live_name" ]
check REQ-019.1 "no undatiert duplicate of the flat a.jsonl" [ ! -e "$legacy/undatiert/a.jsonl" ]
check REQ-019.8 "flat copy with different size stays unchanged" [ "$(cat "$legacy/b.jsonl")" = "old flat b" ]
check REQ-019.8 "day-folder version is pulled next to the differing flat copy" cmp -s "$phone/b.jsonl" "$legacy/undatiert/b.jsonl"
check REQ-019.8 "a differing flat copy needs no konflikte archive" [ ! -e "$legacy/konflikte" ]
check REQ-019.1 "other files are imported into day folders" cmp -s "$phone/$log_name" "$legacy/2026-10-29/$log_name"

# REQ-019.8: a same-name different-size file in a day folder is kept, each time under a unique folder.
live_target="$layout/2026-10-29/$live_name"
printf 'old one\n' >"$live_target"
run "$script" --report --dest "$layout"
check REQ-019.8 "replaced day-folder file holds the phone bytes" cmp -s "$phone/$live_name" "$live_target"
check REQ-019.8 "replacement counts as copy" has "Summary: copied 1 (0 partial), skipped 7, failed 0"
check REQ-019.8 "archived old file is announced with its folder" has "Archived 1 previous version(s) with a different size in $layout/konflikte/"
check REQ-019.8 "old bytes are kept under konflikte with the original name" grep -rqF "old one" "$layout/konflikte"
check REQ-019.6 "report runs on the replaced file's actual path" grep -qF "$live_target" "$FAKE_CARGO_LOG"
printf 'old two, longer\n' >"$live_target"
run "$script" --dest "$layout"
check REQ-019.8 "second replacement keeps the first archive" grep -rqF "old one" "$layout/konflikte"
check REQ-019.8 "second replacement is archived as well" grep -rqF "old two, longer" "$layout/konflikte"
check REQ-019.8 "each archive gets a unique folder" [ "$(find "$layout/konflikte" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" -eq 2 ]
check REQ-019.8 "konflikte stays git-ignored" all_ignored "$layout/konflikte"

# REQ-019.8: a failed pull leaves the old file intact and writes nothing else.
fail_dest="$evidence_root/fail"
mkdir -p "$fail_dest/undatiert"
printf 'old a\n' >"$fail_dest/undatiert/a.jsonl"
export FAKE_ADB_PULL_FAIL=a.jsonl
run "$script" --dest "$fail_dest"
unset FAKE_ADB_PULL_FAIL
check REQ-019.8 "failed pull exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.8 "failed pull is reported" has "pull failed for a.jsonl"
check REQ-019.8 "failed pull counts" has "Summary: copied 7 (2 partial), skipped 0, failed 1"
check REQ-019.8 "failed pull leaves the old file intact" [ "$(cat "$fail_dest/undatiert/a.jsonl")" = "old a" ]
check REQ-019.8 "failed pull archives nothing" [ ! -e "$fail_dest/konflikte" ]
check REQ-019.8 "failed pull leaves no temporary file" [ -z "$(find "$fail_dest" -name '*.pulling')" ]

fail_new="$evidence_root/fail-new"
export FAKE_ADB_PULL_FAIL=b.jsonl
run "$script" --dest "$fail_new"
unset FAKE_ADB_PULL_FAIL
check REQ-019.8 "failed pull of a new file creates no file" [ ! -e "$fail_new/undatiert/b.jsonl" ]
check REQ-019.8 "failed pull of a new file leaves no temporary file" [ -z "$(find "$fail_new" -name '*.pulling')" ]
check REQ-019.8 "other files are still imported after a failed pull" cmp -s "$phone/a.jsonl" "$fail_new/undatiert/a.jsonl"

# REQ-019.8: when the old copy cannot be archived (konflikte is blocked), it stays in place.
block_dest="$evidence_root/blocked"
mkdir -p "$block_dest/undatiert"
printf 'old a\n' >"$block_dest/undatiert/a.jsonl"
printf 'blocker' >"$block_dest/konflikte"
run "$script" --dest "$block_dest"
check REQ-019.8 "blocked archive exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.8 "blocked archive is explained" has "could not create $block_dest/konflikte"
check REQ-019.8 "blocked archive keeps the old file" [ "$(cat "$block_dest/undatiert/a.jsonl")" = "old a" ]
check REQ-019.8 "blocked archive leaves no temporary file" [ -z "$(find "$block_dest" -name '*.pulling')" ]

# Hardening: a symlinked destination, day folder, konflikte, target file or leftover temporary name is never
# followed. Each victim folder lives outside the destination; it must come out of the run unchanged.
victim_dir() { mkdir -p "$work/victim-$1" && echo "$work/victim-$1"; }

victim="$(victim_dir dest)"
ln -s "$victim" "$evidence_root/dest-link"
run "$script" --dest "$evidence_root/dest-link"
check REQ-019.2 "symlinked destination exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.2 "symlinked destination is explained" has "refusing destination that is a symbolic link"
check REQ-019.2 "symlinked destination pulls nothing" not grep -q "^pull " "$FAKE_ADB_LOG"
check REQ-019.2 "symlinked destination target stays empty" [ -z "$(ls -A "$victim")" ]

victim="$(victim_dir day)"
cp "$phone/$live_name" "$victim/$live_name"
printf 'victim log\n' >"$victim/$log_name"
day_link="$evidence_root/day-link"
mkdir -p "$day_link"
ln -s "$victim" "$day_link/2026-10-29"
run "$script" --dest "$day_link"
check REQ-019.8 "symlinked day folder exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.8 "symlinked day folder is refused" has "refusing symlink: $day_link/2026-10-29"
check REQ-019.8 "symlinked day folder counts its files as failed" has "Summary: copied 6 (2 partial), skipped 0, failed 2"
check REQ-019.8 "symlinked day folder is not treated as present" not grep -qF "skipped $live_name" <<<"$OUT"
check REQ-019.8 "symlinked day folder keeps the link" [ -L "$day_link/2026-10-29" ]
check REQ-019.8 "symlinked day folder target is unchanged" [ "$(cat "$victim/$log_name")" = "victim log" ]
check REQ-019.8 "symlinked day folder target gets nothing new" [ "$(ls -A "$victim" | wc -l | tr -d ' ')" -eq 2 ]
check REQ-019.8 "other days are still imported" cmp -s "$phone/20240229-120000-000-leap.jsonl" "$day_link/2024-02-29/20240229-120000-000-leap.jsonl"

victim="$(victim_dir konflikte)"
konf_link="$evidence_root/konflikte-link"
mkdir -p "$konf_link/undatiert"
printf 'old a\n' >"$konf_link/undatiert/a.jsonl"
ln -s "$victim" "$konf_link/konflikte"
run "$script" --dest "$konf_link"
check REQ-019.8 "symlinked konflikte exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.8 "symlinked konflikte is refused" has "refusing symlink: $konf_link/konflikte"
check REQ-019.8 "symlinked konflikte keeps the old file" [ "$(cat "$konf_link/undatiert/a.jsonl")" = "old a" ]
check REQ-019.8 "symlinked konflikte target stays empty" [ -z "$(ls -A "$victim")" ]
check REQ-019.8 "symlinked konflikte leaves no temporary file" [ -z "$(find "$konf_link" -name '*.pulling')" ]
check REQ-019.8 "symlinked konflikte pulls and imports nothing" not grep -q "^pull " "$FAKE_ADB_LOG"
check REQ-019.8 "symlinked konflikte keeps the link" [ -L "$konf_link/konflikte" ]

victim="$(victim_dir pulling)"
printf 'victim secret\n' >"$victim/secret"
pull_link="$evidence_root/pulling-link"
mkdir -p "$pull_link/undatiert"
ln -s "$victim/secret" "$pull_link/undatiert/a.jsonl.pulling"
ln -s "$victim/created-by-pull" "$pull_link/undatiert/b.jsonl.pulling"
run "$script" --dest "$pull_link"
check REQ-019.8 "symlinked temporary name exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.8 "symlinked temporary name is refused" has "refusing existing temporary file (kept untouched; move or delete it to retry): $pull_link/undatiert/a.jsonl.pulling"
check REQ-019.8 "dangling temporary symlink is refused too" has "refusing existing temporary file (kept untouched; move or delete it to retry): $pull_link/undatiert/b.jsonl.pulling"
check REQ-019.8 "temporary symlink target is not overwritten" [ "$(cat "$victim/secret")" = "victim secret" ]
check REQ-019.8 "dangling temporary symlink target is not created" [ ! -e "$victim/created-by-pull" ]
check REQ-019.8 "refused files are not pulled" not grep -qE "^pull .*/(a|b)\.jsonl " "$FAKE_ADB_LOG"
check REQ-019.8 "refused a.jsonl is not created" [ ! -e "$pull_link/undatiert/a.jsonl" ]
check REQ-019.8 "refused b.jsonl is not created" [ ! -e "$pull_link/undatiert/b.jsonl" ]
check REQ-019.8 "refusal counts as failed, the rest is imported" has "Summary: copied 6 (1 partial), skipped 0, failed 2"

# A leftover regular .pulling may hold the only copy of an interrupted pull: it is never overwritten or removed.
leftover_dest="$evidence_root/leftover-pulling"
mkdir -p "$leftover_dest/undatiert"
printf 'recovered partial bytes\n' >"$leftover_dest/undatiert/a.jsonl.pulling"
cp "$leftover_dest/undatiert/a.jsonl.pulling" "$work/leftover-before"
run "$script" --dest "$leftover_dest"
check REQ-019.8 "leftover .pulling exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.8 "leftover .pulling is refused" has "refusing existing temporary file (kept untouched; move or delete it to retry): $leftover_dest/undatiert/a.jsonl.pulling"
check REQ-019.8 "leftover .pulling bytes are untouched" cmp -s "$work/leftover-before" "$leftover_dest/undatiert/a.jsonl.pulling"
check REQ-019.8 "leftover .pulling blocks the pull of that file" not grep -qE "^pull .*/a\.jsonl " "$FAKE_ADB_LOG"
check REQ-019.8 "leftover .pulling file is not imported" [ ! -e "$leftover_dest/undatiert/a.jsonl" ]
check REQ-019.8 "leftover .pulling counts as failed, the rest is imported" has "Summary: copied 7 (2 partial), skipped 0, failed 1"

victim="$(victim_dir target)"
printf 'victim target\n' >"$victim/target"
file_link="$evidence_root/file-link"
mkdir -p "$file_link/undatiert"
ln -s "$victim/target" "$file_link/undatiert/a.jsonl"
run "$script" --dest "$file_link"
check REQ-019.8 "symlinked target file exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.8 "symlinked target file is refused" has "refusing symlink: $file_link/undatiert/a.jsonl"
check REQ-019.8 "symlinked target file keeps its target unchanged" [ "$(cat "$victim/target")" = "victim target" ]
check REQ-019.8 "symlinked target file is not archived" [ ! -e "$file_link/konflikte" ]

# Archive folder creation: a name that is taken is retried with a suffix; any other mkdir failure stops at once.
stub_bin="$work/stub-bin"
mkdir -p "$stub_bin"
cat >"$stub_bin/mkdir" <<'EOF'
#!/usr/bin/env bash
last=""
for a in "$@"; do last="$a"; done
case "$last" in
  */konflikte/2*)
    echo "$last" >>"$FAKE_MKDIR_LOG"
    if [ -n "${FAKE_MKDIR_FAIL:-}" ]; then exit 1; fi
    ;;
esac
exec /bin/mkdir "$@"
EOF
cat >"$stub_bin/date" <<'EOF'
#!/usr/bin/env bash
echo 20000101T000000Z
EOF
chmod +x "$stub_bin/mkdir" "$stub_bin/date"
export FAKE_MKDIR_LOG="$work/mkdir.log"
orig_path="$PATH"

mkfail_dest="$evidence_root/mkdir-fail"
mkdir -p "$mkfail_dest/undatiert" "$mkfail_dest/konflikte"
printf 'old a\n' >"$mkfail_dest/undatiert/a.jsonl"
: >"$FAKE_MKDIR_LOG"
PATH="$stub_bin:$orig_path"
export FAKE_MKDIR_FAIL=1
run "$script" --dest "$mkfail_dest"
PATH="$orig_path"
unset FAKE_MKDIR_FAIL
check REQ-019.8 "failing archive mkdir exits non-zero" [ "$RC" -ne 0 ]
check REQ-019.8 "failing archive mkdir is explained" has "could not create $mkfail_dest/konflikte"
check REQ-019.8 "failing archive mkdir is tried once, not retried" [ "$(grep -c . "$FAKE_MKDIR_LOG")" -eq 1 ]
check REQ-019.8 "failing archive mkdir keeps the old file" [ "$(cat "$mkfail_dest/undatiert/a.jsonl")" = "old a" ]
check REQ-019.8 "failing archive mkdir leaves no temporary file" [ -z "$(find "$mkfail_dest" -name '*.pulling')" ]

taken_dest="$evidence_root/mkdir-taken"
mkdir -p "$taken_dest/undatiert" "$taken_dest/konflikte/20000101T000000Z" "$taken_dest/konflikte/20000101T000000Z-2"
printf 'old a\n' >"$taken_dest/undatiert/a.jsonl"
: >"$FAKE_MKDIR_LOG"
PATH="$stub_bin:$orig_path"
run "$script" --dest "$taken_dest"
PATH="$orig_path"
check REQ-019.8 "taken archive names are retried with a suffix" [ "$(cat "$taken_dest/konflikte/20000101T000000Z-3/a.jsonl")" = "old a" ]
check REQ-019.8 "taken archive names are tried in order" [ "$(grep -c . "$FAKE_MKDIR_LOG")" -eq 3 ]
check REQ-019.8 "retried archive run exits 0" [ "$RC" -eq 0 ]

# REQ-019.6: --report runs on the actual day-folder path of recordings, not on logs.
report_dated="$evidence_root/report-dated"
run "$script" --report --dest "$report_dated"
check REQ-019.6 "report runs once per pulled recording, not for the log" [ "$(grep -c . "$FAKE_CARGO_LOG")" -eq 7 ]
check REQ-019.6 "report gets the day-folder path" grep -qF "$report_dated/2026-10-29/$live_name" "$FAKE_CARGO_LOG"
check REQ-019.6 "report gets the undatiert path" grep -qF "$report_dated/undatiert/a.jsonl" "$FAKE_CARGO_LOG"
check REQ-019.6 "report is not run on the app log" not grep -qF "app-20261029" "$FAKE_CARGO_LOG"

# REQ-029.6: sealed app logs are pulled byte-for-byte into their day folder, are not classified as partial recordings, and unsafe log names are skipped.
log_dest="$evidence_root/logs"
run "$script" --dest "$log_dest"
check REQ-029.6 "sealed log copied byte-for-byte" cmp -s "$phone/$log_name" "$log_dest/2026-10-29/$log_name"
check REQ-029.6 "log reported as log, not partial" has "copied $log_name (log) -> $log_dest/2026-10-29/$log_name"
check REQ-029.6 "unsafe log name skipped" has "skipping unexpected name: evil name.log"

# REQ-019.5: only read-only adb commands reach the phone (all runs above).
check REQ-019.5 "adb invocations were recorded" test -s "$all_adb_log"
check REQ-019.5 "only devices, shell ls/stat/test and pull were used" not grep -qvE '^(devices$|shell (ls|stat|test) |pull )' "$all_adb_log"
check REQ-019.5 "no push, rm, install or write shell command" not grep -qE '^(push|rm|install) |^shell (rm|mv|cp|touch|mkdir|chmod|dd|sh) ' "$all_adb_log"
check REQ-019.5 "pull only reads from the phone folder" not grep -qE '^pull [^/]' "$all_adb_log"

# Usage text.
run "$script" --help
check usage "--help prints usage and exits 0" [ "$RC" -eq 0 ]
check usage "--help shows the Usage line" has "Usage: tools/pull-recordings.sh"
check REQ-019.7 "--help explains day folders and undatiert" has "<dest>/YYYY-MM-DD/<original file name>"
check REQ-019.7 "--help explains the missing time zone" has "no offset is"
check REQ-019.8 "--help explains konflikte archiving" has "<dest>/konflikte/<UTC timestamp>/<name>"

echo
if [ "$failures" -eq 0 ]; then
  echo "all checks passed"
else
  echo "$failures check(s) failed"
  exit 1
fi
