"""Validate a proposed ECU-target document against the proposed v3 schema.

Usage: python3 -I check_proposal.py SCHEMA.json TARGETS.yaml
Needs jsonschema and PyYAML (same as obdentic-knowledge/requirements.txt).
"""
import json, sys
import yaml
from jsonschema import Draft202012Validator

schema = json.load(open(sys.argv[1]))
doc = yaml.safe_load(open(sys.argv[2]))
Draft202012Validator.check_schema(schema)
errors = sorted(Draft202012Validator(schema).iter_errors(doc), key=lambda e: list(e.path))
for e in errors:
    print("schema:", "/".join(map(str, e.path)), e.message)
t = doc["ecu_targets"]
for key in ("id",):
    ids = [x[key] for x in t]
    assert len(ids) == len(set(ids)), f"duplicate {key}"
req = [x["addressing"]["request_id"] for x in t]
resp = [x["addressing"]["response_id"] for x in t]
assert len(set(req)) == len(req), "duplicate request_id"
assert len(set(resp)) == len(resp), "duplicate response_id"
assert not set(req) & set(resp), "request/response id overlap"
for x in t:
    a = x["addressing"]
    off = int(a["response_id"], 16) - int(a["request_id"], 16)
    assert off == (8 if a["response_id_rule"] == "iso15765_plus_8" else 0x6A), a
    s = x["provenance"]["sources"][0]
    assert s["profile_count"] <= s["profile_total"] and s["plain_read_profile_count"] <= s["profile_count"]
assert not errors, f"{len(errors)} schema error(s)"
print(f"ok: {len(t)} targets valid")
