#!/usr/bin/env python3
"""I1/I2 process-death runs with a host-side oracle. usage: interrupt.py i1|i2 dest:strat [runs]
ACK oracle = logcat line 'ACK run seq' (emitted after the sync returned). File is read with `adb shell cat` (independent of the app/SAF)."""
import os, sys, re, subprocess, time, json, zlib, random
S = os.environ["ANDROID_SERIAL"]; PK = "de.obdentic.spike378"; DIR = "/sdcard/Documents/obdentic-spike378-audit"
def adb(*a, text=True):
    return subprocess.run(["adb", "-s", S, *a], capture_output=True, text=text).stdout
def verify(run, strat):
    raw = subprocess.run(["adb", "-s", S, "shell", f"cat {DIR}/seq-{run}.jsonl"], capture_output=True).stdout
    nul = raw.find(b"\0"); prefix = raw if nul < 0 else raw[:nul]
    lastnl = prefix.rfind(b"\n") + 1; complete = prefix[:lastnl]; torn = prefix[lastnl:]
    good = {}; corrupt = 0
    for ln in complete.split(b"\n"):
        if not ln: continue
        try:
            j = json.loads(ln); ok = zlib.crc32(j["pad"].encode()) == int(j["crc"], 16) and j["run"] == run
        except Exception: ok = False
        if ok: good[j["seq"]] = 1
        else: corrupt += 1
    return good, corrupt, len(torn), len(raw)
def logs(run):
    t = adb("logcat", "-d", "-v", "epoch", "-s", "Spike378:I")
    acks, disp, ev = [], [], []
    for l in t.splitlines():
        if f"run={run} " not in l and not l.rstrip().endswith(f"run={run}"): continue
        m = re.search(r"ACK run=\S+ seq=(\d+)", l)
        if m: acks.append(int(m.group(1)))
        m = re.search(r"DISPATCH run=\S+ seq=(\d+)", l)
        if m: disp.append((int(m.group(1)), len(acks)))
        if any(x in l for x in ("KILL", "SUMMARY", "ERR", "REFUSE", "FATAL")): ev.append(l.split("Spike378:")[1].strip())
    return acks, disp, ev
def main():
    mode, combo = sys.argv[1], sys.argv[2]; runs = int(sys.argv[3]) if len(sys.argv) > 3 else (20 if mode == "i1" else 10)
    dest, strat = combo.split(":")
    points = [("after_create", 0), ("after_write", 3), ("after_sync", 3), ("after_ack", 3)] if mode == "i1" else [("force_stop", -1)]
    out = open(f"{os.path.dirname(__file__)}/../data/raw-{mode}-{dest}-{strat}.csv", "a")
    for point, kseq in points:
        for r in range(runs):
            run = f"{mode}{dest[0]}{strat}{point[:6]}{r}{random.randrange(10**4)}".replace("_", "")
            adb("logcat", "-c"); adb("shell", f"rm -f {DIR}/seq-{run}.jsonl")
            base = f"am broadcast --include-stopped-packages -n {PK}/.Cmd --es run {run} --es csv {mode} --es cmd seq --es dest {dest} --es strat {strat} --ei size 4096 --ei n "
            if mode == "i1":
                adb("shell", base + f"10 --es kill {point} --ei killseq {kseq} >/dev/null")
                for _ in range(100):
                    if any(k in adb("logcat", "-d", "-s", "Spike378:I") for k in (f"KILL run={run}", f"SUMMARY run={run}")): break
                    time.sleep(0.1)
            else:
                # am broadcast blocks until the (goAsync) receiver finishes, so it must not be awaited
                subprocess.Popen(["adb", "-s", S, "shell", base + "3000 --ei interval 20 >/dev/null"])
                time.sleep(random.uniform(1.0, 3.0)); adb("shell", f"am force-stop {PK}"); time.sleep(0.3)
            adb("shell", f"am force-stop {PK}"); time.sleep(0.2)
            acks, disp, ev = logs(run); good, corrupt, torn, size = verify(run, strat)
            lost = [s for s in acks if s not in good]
            unack_present = [s for s in good if s not in acks]
            disp_bad = [s for s, n in disp if s not in acks[:n]]
            row = f"{mode},{dest},{strat},{point},{run},acked={len(acks)},present={len(good)},lost={len(lost)},unacked_present={len(unack_present)},corrupt_lines={corrupt},torn_bytes={torn},dispatch_before_ack={len(disp_bad)},killed={'KILL' in ' '.join(ev) or mode=='i2'}"
            out.write(row + "\n"); out.flush(); print(row, flush=True)
            adb("shell", f"rm -f {DIR}/seq-{run}.jsonl")
main()
