#!/usr/bin/env python3
"""Extract MIUI Android 10's BTSNOOP_LOG_SUMMARY into a Wireshark log."""

import base64
import os
import re
import struct
import sys
import tempfile
import zlib
from pathlib import Path

EPOCH_DELTA_US = 0x00DCDDB30F2F8000
HCI_TYPES = {0x10: (1, 4), 0x11: (1, 2), 0x20: (0, 1), 0x21: (0, 2)}


def decode(encoded):
    version, last_timestamp_us = struct.unpack_from("<bQ", encoded)
    if version != 2:
        raise ValueError(f"unsupported btsnooz version: {version}")

    data = zlib.decompress(encoded[9:])
    records = []
    offset = 0
    while offset < len(data):
        length, original_length, delta_us = struct.unpack_from("<HHQ", data, offset)
        payload = data[offset + 12 : offset + 12 + length]
        if len(payload) != length or not payload or payload[0] not in HCI_TYPES:
            raise ValueError(f"invalid MIUI btsnooz record at byte {offset}")
        records.append((length, original_length, delta_us, payload))
        offset += 12 + length

    timestamp_us = last_timestamp_us + EPOCH_DELTA_US - sum(record[2] for record in records)
    output = bytearray(b"btsnoop\0" + struct.pack(">II", 1, 1002))
    for length, original_length, delta_us, payload in records:
        timestamp_us += delta_us
        direction, hci_type = HCI_TYPES[payload[0]]
        output += struct.pack(">IIIIQ", original_length, length, direction, 0, timestamp_us)
        output += bytes([hci_type]) + payload[1:]
    return bytes(output)


def extract(report):
    match = re.search(
        rb"--- BEGIN:BTSNOOP_LOG_SUMMARY[^-]*---\r?\n(.*?)\r?\n--- END:BTSNOOP_LOG_SUMMARY",
        report,
        re.DOTALL,
    )
    if not match:
        raise ValueError("bugreport contains no BTSNOOP_LOG_SUMMARY")
    return decode(base64.b64decode(match.group(1)))


def write_private(path, data):
    path = Path(path)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(data)
    except Exception:
        path.unlink(missing_ok=True)
        raise


def self_test():
    payload = b"\x20\x03\x0c\x00"
    encoded = struct.pack("<bQ", 2, 1_000_000) + zlib.compress(
        struct.pack("<HHQ", len(payload), len(payload), 0) + payload
    )
    result = decode(encoded)
    assert result[:16] == b"btsnoop\0" + struct.pack(">II", 1, 1002)
    assert result[-4:] == b"\x01\x03\x0c\x00"
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "capture.btsnoop"
        write_private(path, result)
        assert path.read_bytes() == result
        assert path.stat().st_mode & 0o777 == 0o600


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
    elif len(sys.argv) == 3:
        write_private(sys.argv[2], extract(Path(sys.argv[1]).read_bytes()))
    else:
        raise SystemExit(f"usage: {sys.argv[0]} <bugreport.txt> <capture.btsnoop> | --self-test")
