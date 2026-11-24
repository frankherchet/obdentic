A="adb -s ${ANDROID_SERIAL:?set ANDROID_SERIAL to the reference device}"
PK=de.obdentic.spike378
# bc <run> <extras...>: broadcast and wait for DONE (or timeout 120s)
bc(){ local run=$1; shift; $A logcat -c; $A shell "am broadcast -n $PK/.Cmd --es run $run $* >/dev/null"
  for i in $(seq 1 240); do $A logcat -d -s Spike378:I | grep -q "DONE .* run=$run" && return 0; sleep 0.5; done; echo "TIMEOUT $run"; return 1; }
pull(){ $A shell cat /sdcard/Android/data/$PK/files/$1.csv; }
