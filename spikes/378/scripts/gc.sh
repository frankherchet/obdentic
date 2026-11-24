#!/bin/bash
# Warm group commit. usage: gc.sh dest:strat ...   env N=100 SIZE=16384 K="1 2 4 8" WINS="0 5"
source "$(dirname "$0")/lib.sh"; cd "$(dirname "$0")/../data"
N=${N:-100}; SIZE=${SIZE:-16384}; K=${K:-"1 2 4 8"}; WINS=${WINS:-"0 5"}
for combo in "$@"; do dest=${combo%%:*}; strat=${combo##*:}
  for k in $K; do for w in $WINS; do
    csv=gc-$dest-$strat-k$k-w$w; $A shell rm -f /sdcard/Android/data/$PK/files/$csv.csv
    bc $csv --es csv $csv --es cmd gc --es dest $dest --es strat $strat --ei size $SIZE --ei n $N --ei k $k --ei win $w >/dev/null
    pull $csv > raw-$csv.csv; echo "$csv $(wc -l < raw-$csv.csv)"
    $A shell "rm -f /sdcard/Documents/obdentic-spike378-audit/gc-*"
  done; done
done
