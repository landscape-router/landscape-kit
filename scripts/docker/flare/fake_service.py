#!/usr/bin/env python3
"""Fake service listening on 127.0.0.1:6443 that echoes everything back.

Used by the docker e2e test as the target of the Terrain port forward.
"""
import socket
import threading

srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", 6443))
srv.listen(5)


def echo(c):
    while True:
        d = c.recv(65536)
        if not d:
            break
        c.sendall(d)
    c.close()


while True:
    c, _ = srv.accept()
    threading.Thread(target=echo, args=(c,), daemon=True).start()
