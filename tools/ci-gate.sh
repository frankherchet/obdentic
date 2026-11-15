#!/usr/bin/env bash
# CI gate helpers (#341), used by .github/workflows/ci.yml.
#
#   ci-gate.sh classify                      changed paths on stdin -> "true" if Rust checks are needed, else "false"
#   ci-gate.sh verdict <changes> <code> <rust>
#                                            exit 0 only if the `check` gate may be green
#
# <changes> and <rust> are job results (success|failure|cancelled|skipped), <code> is the classify output.
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

verdict() {
  [ "$#" -eq 3 ] || { echo "usage: ci-gate.sh verdict <changes> <code> <rust>" >&2; return 2; }
  local changes="$1" code="$2" rust="$3"
  if [ "$changes" != success ]; then
    echo "change detection: $changes" >&2
    return 1
  fi
  case "$code:$rust" in
    true:success)
      echo "Rust checks passed"
      ;;
    false:skipped)
      echo "documentation-only change: Rust checks not required"
      ;;
    *)
      echo "check rejected: code=$code rust=$rust" >&2
      return 1
      ;;
  esac
}

case "${1:-}" in
  classify) classify ;;
  verdict)
    shift
    verdict "$@"
    ;;
  *)
    echo "usage: ci-gate.sh classify|verdict ..." >&2
    exit 2
    ;;
esac
