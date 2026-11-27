#!/usr/bin/env python3
"""REQ-046.23: inspect the synthetic Android audit and its recording offline.

Usage: python3 tools/check-shared-diagnosis.py OUTPUT_DIRECTORY PATH_TO_OBDENTIC_MOBILE_REPORT
The directory is the debug app's files/req-046-23, copied from the dedicated emulator.
"""
import json
import pathlib
import subprocess
import sys
import tempfile


def inspect(binary, path):
    return json.loads(subprocess.check_output([binary, "--agent-audit", str(path)]))


def main(directory, binary):
    directory = pathlib.Path(directory)
    audit = directory / "audit.jsonl"
    report = inspect(binary, audit)
    assert report["audit_status"] == "complete"
    assert report["diagnosis_status"] == "unknown"
    events = report["events"]
    responses = [e["data"]["text"] for e in events if e["type"] == "response_item"]
    assert responses == ["Read signals and stored codes.", "Read MAF and Knowledge; EGR position is missing."]
    proposals = [e for e in events if e["type"] == "tool_proposal"]
    assert [e["tool_call_id"] for e in proposals] == ["list", "rpm", "dtc", "maf", "knowledge", "egr", "denied"]
    decisions = [e for e in events if e["type"] == "tool_decision"]
    assert [e["data"]["allowed"] for e in decisions] == [True] * 6 + [False]
    ends = {e["tool_call_id"]: e for e in events if e["type"] == "tool_end"}
    assert set(ends) == {"list", "rpm", "dtc", "maf", "knowledge", "egr"}
    assert ends["rpm"]["data"]["result"]["value"] == 1726
    assert ends["maf"]["data"]["result"]["value"] == 10
    assert "P0401" in json.dumps(ends["dtc"]["data"]["result"])
    assert ends["knowledge"]["data"]["result"]["outcome"] == "ok"
    assert ends["egr"]["data"]["result"]["reason"] == "unknown_signal"
    assert all(ref["status"] == "resolved" for ref in report["source_references"])
    wires = []
    for proposal in proposals[:-1]:
        call = proposal["tool_call_id"]
        end = ends[call]["data"]
        ref = end["evidence_ref"]
        recording = (directory / ref["recording_ref"]).read_bytes()
        offset = ref["offset"]
        assert offset == 0 or recording[offset - 1:offset] == b"\n"
        lines = recording[offset:].splitlines()
        request = json.loads(proposal["data"]["arguments"])
        found = False
        for line in lines:
            event = json.loads(line)
            if event.get("type") == "wire" and event.get("direction") == "TX":
                wires.append(event["text"])
            if event.get("type") == "action" and event.get("action") == "assistant.tool":
                assert json.loads(event["argument"]) == request
                assert event["result"]["data"] == end["result"]
                found = True
                break
        assert found, call
    fixture = json.loads((pathlib.Path(__file__).resolve().parents[1] / "tests/fixtures/virtual-vehicle.json").read_text())
    assert wires == fixture["diagnosis_tx"]
    with tempfile.TemporaryDirectory() as temp:
        incomplete = pathlib.Path(temp) / "incomplete.jsonl"
        incomplete.write_bytes(audit.read_bytes().rsplit(b"\n", 2)[0] + b"\n")
        assert inspect(binary, incomplete)["audit_status"] == "incomplete"
    print("REQ-046.23 offline reconstruction passed (synthetic data only)")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
