#!/usr/bin/env python3
"""Send AUTH_REQ frames with a wrong proof from this container's own MAC.

Used by the docker e2e test: auth failures spoofed against the MAC of an
authenticated client must NOT lock that client out (failures only count
against MACs without an active session). Frames are sent from this
container so the L2 switch's MAC learning stays consistent.

Usage: auth_req_flood.py [COUNT]
"""
import socket
import struct
import sys
import time

ETH_P_ALL = 0x0003
ETHERTYPE = 0x88B6
MAGIC = b"TERR"
VERSION = 0x05
TYPE_AUTH_REQ = 0x03


def own_mac():
    with open("/sys/class/net/eth0/address") as f:
        return bytes(int(x, 16) for x in f.read().strip().split(":"))


def auth_req_frame(me):
    user = b"admin"
    nonce = 1
    proof = b"\x00" * 32
    payload = struct.pack(">H", len(user)) + user + struct.pack(">Q", nonce) + proof
    terrain = (
        MAGIC
        + bytes([VERSION, TYPE_AUTH_REQ])
        + b"\x00" * 4
        + struct.pack(">H", len(payload))
        + b"\x00" * 4
        + payload
    )
    return b"\xff" * 6 + me + struct.pack(">H", ETHERTYPE) + terrain


def main():
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 12
    sock = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(ETH_P_ALL))
    sock.bind(("eth0", 0))
    frame = auth_req_frame(own_mac())
    for _ in range(count):
        sock.sendto(frame, ("eth0", 0))
        time.sleep(0.05)
    print(f"sent {count} auth req frames", flush=True)


if __name__ == "__main__":
    main()
