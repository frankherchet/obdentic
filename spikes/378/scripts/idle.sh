#!/bin/bash
# H2 test: warm process, fixed pause between appends. usage: idle.sh   (writes data/raw-idle-*.csv)
source "$(dirname "$0")/lib.sh"; cd "$(dirname "$0")/../data"
for combo in ${COMBOS:-path:pre saf:pre path:wa saf:wa}; do dest=${combo%%:*}; strat=${combo##*:}
  for iv in ${IVS:-0 50 600 3000}; do reps=1; [ $iv = 3000 ] && reps=3; for rep in $(seq 1 $reps); do
    n=100; [ $iv = 600 ] && n=60; [ $iv = 3000 ] && n=10
    csv=idle-$dest-$strat-$iv; [ $iv = 3000 ] && csv=$csv-r$rep; $A shell rm -f /sdcard/Android/data/$PK/files/$csv.csv
    $A logcat -c; $A shell "am broadcast -n $PK/.Cmd --es run $csv --es csv $csv --es cmd idle --es dest $dest --es strat $strat --ei size 16384 --ei n $n --ei interval $iv >/dev/null"
    for i in $(seq 1 90); do $A logcat -d -s Spike378:I | grep -q "DONE idle run=$csv" && break; sleep 1; done
    pull $csv > raw-$csv.csv; echo "$csv $(wc -l < raw-$csv.csv)"
  done; done; $A shell "rm -f /sdcard/Documents/obdentic-spike378-audit/idle-*"
done
