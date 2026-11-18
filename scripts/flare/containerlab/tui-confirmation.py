"""Exercise the shipped TUI on the containerlab client's real terminal and Ethernet link."""
import os
from pathlib import Path
import socket
import sys
import time

import pexpect
import pyte


def auth_requests(observation):
    count = 0
    while True:
        try:
            header, address = observation.recvfrom(20)
        except BlockingIOError:
            return count
        if address[2] == socket.PACKET_OUTGOING and header[12:20] == b"\x88\xb6TERR\x06\x03":
            count += 1


def exercise(child, observation, fingerprints):
    screen = pyte.Screen(120, 32)
    stream = pyte.Stream(screen)

    def wait_for(*labels):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            current = "\n".join(screen.display)
            if all(label in current for label in labels):
                return
            try:
                stream.feed(child.read_nonblocking(8192, timeout=0.25))
            except pexpect.TIMEOUT:
                pass
        raise AssertionError(f"TUI did not reach {labels!r}")

    picker = "Select a device"
    wait_for(picker)
    # Cycle from Auto through the container's interfaces to the isolated link.
    for _ in range(len(socket.if_nameindex()) + 1):
        if all(fp[:16] in "".join(screen.display) for fp in fingerprints):
            break
        child.send("i")
        wait_for("Scanning for devices")
        wait_for(picker)
    wait_for(picker, *(fp[:16] for fp in fingerprints))
    for _ in range(2):
        child.send("\r")
        wait_for(fingerprints[0], "Enter confirms")
        child.send("\x1b")
        wait_for(picker)
        assert auth_requests(observation) == 0, "cancelling the fingerprint sent credentials"
    child.send("\r")
    wait_for(fingerprints[0], "Enter confirms")
    child.send("\r")
    wait_for("2 Linux root password")
    child.send("2")
    wait_for("Flare login password")
    assert auth_requests(observation) == 0, "auth started before password submission"
    child.send(Path("/opt/fixture-password").read_text().rstrip("\n") + "\r")
    wait_for("Connected · session")
    assert auth_requests(observation) > 0, "the positive control missed the real authentication"


if __name__ == "__main__":
    with socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(3)) as observer:
        observer.bind(("eth1", 0))
        observer.setblocking(False)
        terminal = pexpect.spawn("/opt/bin/lflare", ["--lang", "en"], dimensions=(32, 120),
                                 encoding="utf-8", env={**os.environ, "TERM": "xterm-256color"})
        try:
            exercise(terminal, observer, sorted(sys.argv[1:]))
        finally:
            terminal.close(force=True)
    print("PASS: cancel twice, confirm again and authenticate only after submitting the password")
