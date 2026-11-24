#!/usr/bin/env python3
"""Nearest-rank percentiles per (stratum,size) over raw CSVs: stratum,size,i,total_ns,write_ns,sync_ns,pre_ns,ok"""
import sys, glob, math, collections
d = collections.defaultdict(list); bad = collections.Counter()
for f in sorted(sys.argv[1:]):
    for l in open(f):
        p = l.strip().split(',')
        if len(p) < 8: continue
        st, sz = p[0], int(p[1])
        if st.startswith('warmup') : continue
        if p[7] != 'ok': bad[(st, sz)] += 1; continue
        d[(st, sz)].append((int(p[3]) / 1e6, int(p[5]) / 1e6, int(p[4]) / 1e6))
def pr(v, q):
    v = sorted(v); return v[max(0, math.ceil(q * len(v)) - 1)]
print("stratum,size,ok,fail,p50_ms,p95_ms,p99_ms,max_ms,sync_p95_ms,write_p95_ms,gt25ms")
for (st, sz), v in sorted(d.items()):
    t = [x[0] for x in v]
    print(f"{st},{sz},{len(v)},{bad[(st,sz)]},{pr(t,.5):.2f},{pr(t,.95):.2f},{pr(t,.99):.2f},{max(t):.2f},{pr([x[1] for x in v],.95):.2f},{pr([x[2] for x in v],.95):.2f},{sum(1 for x in t if x>25)}")
