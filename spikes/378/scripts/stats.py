#!/usr/bin/env python3
"""Nearest-rank percentiles per label+size over raw CSVs.
Row: label,size,i,total_ns,c0..c8,status  (cold: c = create,open,scan,prealloc,write,sync,dirsync,append2,append3; gc/idle: c0 = batch size or write_ns,c1 = sync_ns)
Rows whose label ends in -warmup and failures (status not starting with ok) are excluded from percentiles; failures are counted."""
import sys, math, collections, statistics
d = collections.defaultdict(list); bad = collections.Counter(); comp = collections.defaultdict(lambda: collections.defaultdict(list))
for f in sorted(sys.argv[1:]):
    for l in open(f):
        p = l.strip().split(',')
        if len(p) < 14: continue
        lab, sz = p[0], int(p[1])
        if lab.endswith('-warmup'): continue
        if not p[-1].startswith('ok'): bad[(lab, sz)] += 1; continue
        d[(lab, sz)].append(int(p[3]) / 1e6)
        for k, nm in enumerate(['create','open','scan','pre','write','sync','dirsync','a2','a3']):
            comp[(lab, sz)][nm].append(int(p[4 + k]) / 1e6)
def pr(v, q):
    v = sorted(v); return v[max(0, math.ceil(q * len(v)) - 1)]
print("label,size,ok,fail,p50_ms,p95_ms,p99_ms,max_ms,gt25ms,sync_p50,create_p50,open_p50,dirsync_p50,a2_p50")
for (lab, sz), t in sorted(d.items()):
    c = comp[(lab, sz)]
    m = lambda n: f"{pr([x for x in c[n] if x >= 0], .5):.2f}" if any(x >= 0 for x in c[n]) else "-"
    print(f"{lab},{sz},{len(t)},{bad[(lab,sz)]},{pr(t,.5):.2f},{pr(t,.95):.2f},{pr(t,.99):.2f},{max(t):.2f},{sum(1 for x in t if x>25)},{m('sync')},{m('create')},{m('open')},{m('dirsync')},{m('a2')}")
for k, v in bad.items():
    if k not in d: print(f"{k[0]},{k[1]},0,{v},-,-,-,-,-,-,-,-,-,-")
