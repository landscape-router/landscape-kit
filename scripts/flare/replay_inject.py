#!/usr/bin/env python3
"""Capture live Terrain DATA frames and inject replays of them.

Used by the docker e2e test: the peer must drop replayed frames (stale
sequence numbers, per-direction replay window) without corrupting the
ongoing tunnel transfer.

Runs inside the client container on its own interface, so it sees both
directions of the session traffic. Only client->server DATA frames are
captured and re-sent: replaying a frame whose source MAC is the local one
does not confuse the L2 switch's MAC learning, so the replayed frame
really reaches the server and exercises its replay window.

Usage: replay_inject.py [DURATION_SECONDS]
"""
import socket
import struct
import sys
import time

ETH_P_ALL = 0x0003
ETHERTYPE = 0x88B6
MAGIC = b"TERR"
VERSION = 0x05
TYPE_DATA = 0x07
REPLAY_INTERVAL = 0.05


def own_mac():
    with open("/sys/class/net/eth0/address") as f:
        return bytes(int(x, 16) for x in f.read().strip().split(":"))


def is_terrain_data(frame, me):
    if len(frame) < 14 + 16 + 16:
        return False
    if frame[12:14] != struct.pack(">H", ETHERTYPE):
        return False
    if frame[6:12] != me:
        return False
    p = frame[14:]
    return p[:4] == MAGIC and p[4] == VERSION and p[5] == TYPE_DATA


def main():
    duration = float(sys.argv[1]) if len(sys.argv) > 1 else 8.0
    me = own_mac()
    sock = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(ETH_P_ALL))
    sock.bind(("eth0", 0))
    sock.settimeout(0.2)

    captured = []
    replays = 0
    end = time.time() + duration
    last_replay = 0.0
    while time.time() < end:
        try:
            frame, _ = sock.recvfrom(65536)
        except socket.timeout:
            frame = None
        except OSError:
            break
        if frame is not None and is_terrain_data(frame, me):
            captured.append(frame)
            captured = captured[-4:]
        now = time.time()
        if captured and now - last_replay >= REPLAY_INTERVAL:
            for f in captured:
                try:
                    sock.sendto(f, ("eth0", 0))
                    replays += 1
                except OSError:
                    pass
            last_replay = now
    print(
        f"captured {len(captured)} data frames, injected {replays} replays",
        flush=True,
    )


if __name__ == "__main__":
    main()
