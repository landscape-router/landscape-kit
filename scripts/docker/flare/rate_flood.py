#!/usr/bin/env python3
"""Flood the L2 segment with Terrain DISCOVER frames from a fixed fake MAC.

Used by the docker e2e test: the server's per-MAC token bucket must rate
limit the flood and log "rate-limited", and the fake MAC must never be
able to authenticate.

Frames are valid v5 DISCOVER frames (magic, version, type, len, seq),
sent from a separate container on the same segment.

Usage: rate_flood.py [RATE_PER_SECOND] [DURATION_SECONDS]
"""
import socket
import struct
import sys
import time

ETH_P_ALL = 0x0003
ETHERTYPE = 0x88B6
MAGIC = b"TERR"
VERSION = 0x05
TYPE_DISCOVER = 0x01
FAKE_SRC = bytes.fromhex("020000000099")


def discover_frame():
    name = b"flooder"
    payload = struct.pack(">H", len(name)) + name
    terrain = (
        MAGIC
        + bytes([VERSION, TYPE_DISCOVER])
        + b"\x00" * 4
        + struct.pack(">H", len(payload))
        + b"\x00" * 4
        + payload
    )
    return b"\xff" * 6 + FAKE_SRC + struct.pack(">H", ETHERTYPE) + terrain


def main():
    rate = float(sys.argv[1]) if len(sys.argv) > 1 else 60.0
    duration = float(sys.argv[2]) if len(sys.argv) > 2 else 3.0
    sock = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(ETH_P_ALL))
    sock.bind(("eth0", 0))
    frame = discover_frame()
    interval = 1.0 / rate
    end = time.time() + duration
    sent = 0
    while time.time() < end:
        sock.sendto(frame, ("eth0", 0))
        sent += 1
        time.sleep(interval)
    print(f"sent {sent} discover frames", flush=True)


if __name__ == "__main__":
    main()
