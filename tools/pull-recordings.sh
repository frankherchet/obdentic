#!/usr/bin/env bash
# Pull OBDentic recordings (*.jsonl) and sealed app logs (*.log) from the phone over adb into a git-ignored
# local folder. Read-only on the phone: only adb devices, shell ls/stat/test, pull.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: tools/pull-recordings.sh [--report] [--source <phone dir>] [--dest <dir>]

Copies every *.jsonl and *.log file from the phone folder into the local destination.
Files already present with the same name and size are skipped.

Options:
  --source <dir>  phone folder (default: /sdcard/Download/OBDentic)
  --dest <dir>    local folder, relative paths resolve from the repo root
                  (default: evidence/phone); must be git-ignored
  --report        run obdentic-mobile-report on each newly pulled file
  -h, --help      show this help
EOF
}

die() {
  echo "pull-recordings: $*" >&2
  exit 1
}

warn() {
  echo "pull-recordings: warning: $*" >&2
}

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
adb_bin="${ADB:-adb}"
phone_dir=/sdcard/Download/OBDentic
dest="evidence/phone"
report=0

while [ $# -gt 0 ]; do
  case "$1" in
    --report) report=1; shift ;;
    --source) [ $# -ge 2 ] || die "--source needs a value"; phone_dir="$2"; shift 2 ;;
    --dest) [ $# -ge 2 ] || die "--dest needs a value"; dest="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: $1" ;;
  esac
done

# The phone path is interpolated into a remote shell command: keep it to safe characters.
case "$phone_dir" in
  ""|*[!A-Za-z0-9_./-]*) die "unsupported phone folder (use letters, digits, _ . / -): $phone_dir" ;;
esac

case "$dest" in
  /*) dest_dir="$dest" ;;
  *) dest_dir="$repo_dir/$dest" ;;
esac
dest_dir="${dest_dir%/}"

# Refuse before anything is created: recordings must only land in git-ignored paths.
if ! (cd "$repo_dir" && git check-ignore -q "$dest_dir/probe.jsonl" 2>/dev/null); then
  die "refusing destination that git does not ignore: $dest_dir (use a path under evidence/)"
fi

command -v "$adb_bin" >/dev/null 2>&1 || die "adb not found (install platform-tools or set ADB)"

devices_out="$("$adb_bin" devices)" || die "adb devices failed"
device_count="$(printf '%s\n' "$devices_out" | awk '$2 == "device" { n++ } END { print n + 0 }')"
case "$device_count" in
  0) die "no adb device in state 'device'; connect the phone over USB or wireless adb" ;;
  1) ;;
  *) die "$device_count adb devices in state 'device'; connect exactly one" ;;
esac

"$adb_bin" shell test -d "$phone_dir" </dev/null || die "phone folder not found: $phone_dir"
listing="$("$adb_bin" shell ls -1 "$phone_dir" </dev/null)" || die "could not list $phone_dir"

# Local size: GNU stat takes -c, BSD/macOS stat takes -f (and rejects -c).
local_size() {
  stat -c %s "$1" 2>/dev/null || stat -f %z "$1"
}

# Runs the report for one file from the repo root. PULL_RECORDINGS_CARGO overrides cargo for tests.
run_report() {
  if [ -n "${PULL_RECORDINGS_CARGO:-}" ]; then
    cargo_cmd=("$PULL_RECORDINGS_CARGO")
  else
    cargo_cmd=(env PATH="$HOME/.cargo/bin:/opt/local/bin:/usr/bin:/bin" cargo)
  fi
  (cd "$repo_dir" && "${cargo_cmd[@]}" +1.98.0 run --locked --quiet --bin obdentic-mobile-report -- "$1")
}

mkdir -p "$dest_dir"

copied=0
partial=0
skipped=0
failed=0
name_re='^[A-Za-z0-9_.-]+\.(jsonl|log)$'

while IFS= read -r line; do
  name="${line//$'\r'/}"
  [ -n "$name" ] || continue
  if ! [[ $name =~ $name_re ]]; then
    warn "skipping unexpected name: $name"
    continue
  fi

  if ! remote_size="$("$adb_bin" shell stat -c %s "$phone_dir/$name" </dev/null)"; then
    warn "could not stat $phone_dir/$name; skipped"
    failed=$((failed + 1))
    continue
  fi
  remote_size="${remote_size//$'\r'/}"
  case "$remote_size" in
    ""|*[!0-9]*) warn "unexpected size for $name: $remote_size; skipped"; failed=$((failed + 1)); continue ;;
  esac

  local_file="$dest_dir/$name"
  if [ -f "$local_file" ] && [ "$(local_size "$local_file")" = "$remote_size" ]; then
    echo "skipped $name (already present, same size)"
    skipped=$((skipped + 1))
    continue
  fi

  # Pull to a temporary name so a failed pull never clobbers an existing copy.
  tmp_file="$local_file.pulling"
  if ! "$adb_bin" pull "$phone_dir/$name" "$tmp_file" </dev/null >/dev/null; then
    rm -f "$tmp_file"
    warn "pull failed for $name; skipped"
    failed=$((failed + 1))
    continue
  fi
  mv -f "$tmp_file" "$local_file"

  case "$name" in
    *.log) echo "copied $name (log)"; copied=$((copied + 1)); continue ;;
  esac
  last_line="$(awk '{ sub(/\r$/, "") } NF { last = $0 } END { print last }' "$local_file")"
  case "$last_line" in
    *'"type":"session_end"'*) state=complete ;;
    *) state=partial; partial=$((partial + 1)) ;;
  esac
  echo "copied $name ($state)"
  copied=$((copied + 1))

  if [ "$report" -eq 1 ]; then
    run_report "$local_file" || warn "report failed for $name; file kept"
  fi
done <<<"$listing"

echo "Summary: copied $copied ($partial partial), skipped $skipped, failed $failed"
[ "$failed" -eq 0 ]
