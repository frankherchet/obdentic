#!/usr/bin/env bash
# Pull OBDentic recordings (*.jsonl) and sealed app logs (*.log) from the phone over adb into a git-ignored
# local folder, sorted into one folder per day. Read-only on the phone: only adb devices, shell ls/stat/test, pull.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: tools/pull-recordings.sh [--report] [--source <phone dir>] [--dest <dir>]

Copies every *.jsonl and *.log file from the phone folder into the local destination,
one folder per day: <dest>/YYYY-MM-DD/<original file name>. The day is the local
wall-clock date the app wrote into the file name (yyyyMMdd-HHmmss-SSS or
app-yyyyMMdd-HHmmss-SSS); the phone's time zone is not recorded, so no offset is
applied or assumed. Names without a valid timestamp go to <dest>/undatiert/.
File names and bytes are never changed.

A file already present in its day folder (or, for older imports, flat in <dest>) with the
same size is skipped. A file with the same name but a different size is pulled to a
temporary name first (<name>.pulling, which must not exist yet: a leftover one may hold the only
copy of an earlier interrupted pull, so it is never overwritten and that file is skipped until you
move or delete it); only after a successful pull the old copy is moved, unchanged, to
<dest>/konflikte/<UTC timestamp>/<name> and replaced. Existing files are never deleted or
overwritten; their old versions are kept in konflikte/. A failed pull removes only its own partial temporary file.

Options:
  --source <dir>  phone folder (default: /sdcard/Download/OBDentic)
  --dest <dir>    local folder, relative paths resolve from the repo root
                  (default: evidence/phone); must be git-ignored
  --report        run obdentic-mobile-report on each newly pulled recording
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

# Never write through a symlinked destination (it could point anywhere, including at other evidence).
if [ -L "$dest_dir" ]; then
  die "refusing destination that is a symbolic link: $dest_dir"
fi

# The fixed sub-folders are checked up front as well: git cannot judge paths beyond a symlink, so without this
# a symlinked konflikte/ or undatiert/ would only be reported as "not ignored" and the cause would stay hidden.
for fixed_dir in undatiert konflikte; do
  if [ -L "$dest_dir/$fixed_dir" ]; then
    die "refusing symlink: $dest_dir/$fixed_dir (nothing was imported)"
  fi
done

# Refuse before anything is created: recordings must only land in git-ignored paths. Check every kind of
# path the script writes (day folder, undatiert, konflikte, temporary pull name) for both file types.
for probe_dir in 2000-01-01 undatiert konflikte/20000101T000000Z; do
  for probe in probe.jsonl probe.log probe.jsonl.pulling probe.log.pulling; do
    if ! (cd "$repo_dir" && git check-ignore -q "$dest_dir/$probe_dir/$probe" 2>/dev/null); then
      die "refusing destination that git does not ignore: $dest_dir (use a path under evidence/)"
    fi
  done
done

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

# Sets `bucket` to the folder for a file name: YYYY-MM-DD from the local wall-clock timestamp in the name
# (recordings yyyyMMdd-HHmmss-SSS-..., logs app-yyyyMMdd-HHmmss-SSS.log), or undatiert when the name has no
# valid timestamp. The date is taken as written; no time zone is applied.
date_re='^(app-)?([0-9]{4})([0-9]{2})([0-9]{2})-([0-9]{2})([0-9]{2})([0-9]{2})-[0-9]{3}[-.]'
bucket_for() {
  local y mo d h mi s dim
  bucket=undatiert
  [[ $1 =~ $date_re ]] || return 0
  y=$((10#${BASH_REMATCH[2]})); mo=$((10#${BASH_REMATCH[3]})); d=$((10#${BASH_REMATCH[4]}))
  h=$((10#${BASH_REMATCH[5]})); mi=$((10#${BASH_REMATCH[6]})); s=$((10#${BASH_REMATCH[7]}))
  if [ "$y" -lt 1970 ] || [ "$y" -gt 2099 ] || [ "$mo" -lt 1 ] || [ "$mo" -gt 12 ]; then return 0; fi
  if [ "$h" -gt 23 ] || [ "$mi" -gt 59 ] || [ "$s" -gt 59 ]; then return 0; fi
  case "$mo" in
    1|3|5|7|8|10|12) dim=31 ;;
    4|6|9|11) dim=30 ;;
    *) if [ $((y % 4)) -eq 0 ] && { [ $((y % 100)) -ne 0 ] || [ $((y % 400)) -eq 0 ]; }; then dim=29; else dim=28; fi ;;
  esac
  if [ "$d" -lt 1 ] || [ "$d" -gt "$dim" ]; then return 0; fi
  bucket="${BASH_REMATCH[2]}-${BASH_REMATCH[3]}-${BASH_REMATCH[4]}"
}

# Returns 0 when $1 is a real directory (created if missing). A symlink is refused, never followed or replaced,
# so nothing is written through a link that points elsewhere.
ensure_dir() {
  if [ -L "$1" ]; then
    warn "refusing symlink: $1"
    return 1
  fi
  [ -d "$1" ] || mkdir -- "$1"
}

# Creates (once per run) a fresh <dest>/konflikte/<UTC timestamp> folder for replaced older versions; mkdir
# guarantees uniqueness, a numeric suffix resolves a second run within the same second. Only a name that is
# already taken is retried: any other mkdir failure (permissions, full disk) returns at once.
conflict_dir=""
make_conflict_dir() {
  local stamp candidate n=1
  if [ -n "$conflict_dir" ]; then return 0; fi
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  ensure_dir "$dest_dir/konflikte" || return 1
  candidate="$dest_dir/konflikte/$stamp"
  until mkdir -- "$candidate" 2>/dev/null; do
    if [ ! -e "$candidate" ] && [ ! -L "$candidate" ]; then return 1; fi
    n=$((n + 1))
    if [ "$n" -gt 10000 ]; then return 1; fi
    candidate="$dest_dir/konflikte/$stamp-$n"
  done
  conflict_dir="$candidate"
}

copied=0
partial=0
skipped=0
failed=0
archived=0
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

  bucket_for "$name"
  bucket_dir="$dest_dir/$bucket"
  local_file="$bucket_dir/$name"
  legacy_file="$dest_dir/$name" # flat layout of earlier imports

  # A symlinked day folder could point at other evidence: never look at, or write into, what it points to.
  if [ -L "$bucket_dir" ]; then
    warn "refusing symlink: $bucket_dir; $name skipped"
    failed=$((failed + 1))
    continue
  fi
  if [ -L "$local_file" ]; then
    warn "refusing symlink: $local_file; $name skipped"
    failed=$((failed + 1))
    continue
  fi
  if [ -L "$legacy_file" ]; then
    warn "ignoring symlink: $legacy_file"
    legacy_file=""
  fi

  replace=0
  if [ -e "$local_file" ] && [ ! -f "$local_file" ]; then
    warn "$local_file exists but is not a regular file; $name skipped"
    failed=$((failed + 1))
    continue
  elif [ -f "$local_file" ]; then
    if [ "$(local_size "$local_file")" = "$remote_size" ]; then
      echo "skipped $name (already present, same size)"
      skipped=$((skipped + 1))
      continue
    fi
    replace=1
  elif [ -n "$legacy_file" ] && [ -f "$legacy_file" ]; then
    if [ "$(local_size "$legacy_file")" = "$remote_size" ]; then
      echo "skipped $name (already present at legacy flat path $legacy_file, same size)"
      skipped=$((skipped + 1))
      continue
    fi
    warn "legacy flat copy $legacy_file has a different size and is kept unchanged"
  fi

  if ! { [ -d "$dest_dir" ] || mkdir -p -- "$dest_dir"; } || ! ensure_dir "$bucket_dir"; then
    warn "could not create $bucket_dir; $name skipped"
    failed=$((failed + 1))
    continue
  fi

  # Pull to a temporary name so a failed pull never clobbers an existing copy. Any existing temporary name is
  # refused, never overwritten or followed: a regular leftover may hold the only copy of an interrupted
  # pull, and a symlink would make adb pull write through it.
  tmp_file="$local_file.pulling"
  if [ -e "$tmp_file" ] || [ -L "$tmp_file" ]; then
    warn "refusing existing temporary file (kept untouched; move or delete it to retry): $tmp_file; $name skipped"
    failed=$((failed + 1))
    continue
  fi
  if ! "$adb_bin" pull "$phone_dir/$name" "$tmp_file" </dev/null >/dev/null; then
    rm -f -- "$tmp_file"
    rmdir -- "$bucket_dir" 2>/dev/null || true
    warn "pull failed for $name; skipped"
    failed=$((failed + 1))
    continue
  fi

  if [ "$replace" -eq 1 ]; then
    # The old bytes move, untouched, into konflikte/ before the new file takes their place.
    if ! make_conflict_dir; then
      rm -f -- "$tmp_file"
      warn "could not create $dest_dir/konflikte; kept $local_file, $name skipped"
      failed=$((failed + 1))
      continue
    fi
    archived_file="$conflict_dir/$name"
    if ! mv -- "$local_file" "$archived_file"; then
      rm -f -- "$tmp_file"
      warn "could not archive $local_file; kept it, $name skipped"
      failed=$((failed + 1))
      continue
    fi
  fi

  if ! mv -- "$tmp_file" "$local_file"; then
    if [ "$replace" -eq 1 ]; then
      mv -- "$archived_file" "$local_file" || warn "could not restore $local_file; old version is at $archived_file"
    fi
    warn "could not move pulled $name into place; pulled copy left at $tmp_file"
    failed=$((failed + 1))
    continue
  fi
  if [ "$replace" -eq 1 ]; then
    echo "archived previous $name (different size) -> $archived_file"
    archived=$((archived + 1))
  fi

  case "$name" in
    *.log) echo "copied $name (log) -> $local_file"; copied=$((copied + 1)); continue ;;
  esac
  last_line="$(awk '{ sub(/\r$/, "") } NF { last = $0 } END { print last }' "$local_file")"
  case "$last_line" in
    *'"type":"session_end"'*) state=complete ;;
    *) state=partial; partial=$((partial + 1)) ;;
  esac
  echo "copied $name ($state) -> $local_file"
  copied=$((copied + 1))

  if [ "$report" -eq 1 ]; then
    run_report "$local_file" || warn "report failed for $name; file kept"
  fi
done <<<"$listing"

if [ "$archived" -gt 0 ]; then
  echo "Archived $archived previous version(s) with a different size in $conflict_dir"
fi
echo "Summary: copied $copied ($partial partial), skipped $skipped, failed $failed"
[ "$failed" -eq 0 ]
