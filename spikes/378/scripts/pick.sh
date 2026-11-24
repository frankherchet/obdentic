#!/bin/bash
# Drives the system folder picker on the phone (phone stays awake/unlocked; only taps inside the picker).
A="adb -s ${ANDROID_SERIAL:?set ANDROID_SERIAL to the reference device}"
find_btn(){ $A shell uiautomator dump /sdcard/ui.xml >/dev/null; $A shell cat /sdcard/ui.xml | python3 -c "
import sys,re
x=sys.stdin.read(); t=sys.argv[1]
for m in re.finditer(r'<node[^>]*?text=\"([^\"]*)\"[^>]*?bounds=\"\[(\d+),(\d+)\]\[(\d+),(\d+)\]\"',x):
  if m.group(1).startswith(t): print((int(m.group(2))+int(m.group(4)))//2,(int(m.group(3))+int(m.group(5)))//2); break" "$1"; }
$A shell am start -n de.obdentic.spike378/.PickActivity >/dev/null; sleep 3
read x y < <(find_btn obdentic-spike378-audit); $A shell input tap $x $y; sleep 2
read x y < <(find_btn "Diesen Ordner"); $A shell input tap $x $y; sleep 2
read x y < <(find_btn "Zulassen"); $A shell input tap $x $y; sleep 2
