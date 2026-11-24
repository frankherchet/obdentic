#!/bin/bash
# Cold strata for the given dest:strat combos. Every sample = force-stop + fresh process + one first append (on-device loop).
# usage: cold.sh [N=100] saf:wa saf:wfd ...    env SIZES="1024 16384 65536" KINDS="new existing" TAG=""
source "$(dirname "$0")/lib.sh"
cd "$(dirname "$0")/../data"
N=${N:-100}; SIZES=${SIZES:-"1024 16384 65536"}; KINDS=${KINDS:-"new existing"}; TAG=${TAG:-}
DIR=/sdcard/Documents/obdentic-spike378-audit
clean(){ # dest pattern
  case $1 in saf|path) $A shell "rm -f $DIR/$2";;
    *) bc clean-$1-$RANDOM --es cmd clean --es dest $1 --es prefix "${2%\*}" --es csv clean >/dev/null;; esac; }
state(){ echo "$1 $(date +%H:%M:%S) $($A shell dumpsys battery | egrep 'temperature|level' | tr -d ' \n')" >> context-run.txt; }
for combo in "$@"; do
  dest=${combo%%:*}; strat=${combo##*:}
  state "start $combo"
  for kind in $KINDS; do for size in $SIZES; do
    csv=cold-$dest-$strat-$kind-$size$TAG; $A shell rm -f /sdcard/Android/data/$PK/files/$csv.csv
    $A shell am force-stop $PK
    clean $dest "cold-c*"
    ex=""
    if [ $kind = existing ]; then
      clean $dest "cold-existing-$dest-$strat.jsonl"; bc mk-$combo --es cmd mkexisting --es dest $dest --es strat $strat --es csv mk >/dev/null
      ex="--ez existing true"
    fi
    [ -n "${NOEXTRA:-}" ] && ex="$ex --ez noextra true"
    $A shell "for i in \$(seq 1 $N); do am force-stop $PK; am broadcast --include-stopped-packages -n $PK/.Cmd --es run c\$i --es csv $csv --es cmd cold --es dest $dest --es strat $strat --ei size $size $ex >/dev/null; done"
    pull $csv > raw-$csv.csv; echo "$csv $(wc -l < raw-$csv.csv) lines"
    clean $dest "cold-c*"
  done; done
  state "end $combo"
done
