#!/usr/bin/env bash
# CI gate helpers (#341), used by .github/workflows/ci.yml.
#
#   ci-gate.sh classify               changed paths on stdin -> "true" if Rust checks are needed, else "false"
#   ci-gate.sh detect <base> <head>   same answer for the git range <base>..<head>; fails if git cannot diff it
#
set -euo pipefail

classify() {
  local path
  while IFS= read -r path; do
    case "$path" in
      *.md | docs/* | .memento/*) ;;
      *)
        echo true
        return
        ;;
    esac
  done
  echo false
}

# detect <base> <head>: "true" if the changes since <base> need the Rust checks, "false" if they are documentation only.
# An empty, all-zero or unknown base means nothing to compare against: "true". A failing git diff fails the command.
detect() {
  [ "$#" -eq 2 ] || { echo "usage: ci-gate.sh detect <base> <head>" >&2; return 2; }
  local base="$1" head="$2" paths
  if [ -z "$base" ] || [[ "$base" =~ ^0+$ ]] || ! git cat-file -e "$base^{commit}" 2>/dev/null; then
    echo true
    return 0
  fi
  paths="$(git diff --name-only "$base" "$head")" || return 1
  if [ -z "$paths" ]; then
    echo false
  else
    printf '%s\n' "$paths" | classify
  fi
}

case "${1:-}" in
  classify) classify ;;
  detect)
    shift
    detect "$@"
    ;;
  *)
    echo "usage: ci-gate.sh classify|detect ..." >&2
    exit 2
    ;;
esac
