#!/usr/bin/env python3
"""Exercise concurrent and long-lived TCP connections through lflare."""

import argparse
import concurrent.futures
import http.client
import socket
import struct
import threading
import time


SOCKET_TIMEOUT = 60


def payload(label: int, size: int) -> bytes:
    seed = f"flare-connection-{label:08x}\n".encode()
    return (seed * ((size + len(seed) - 1) // len(seed)))[:size]


def recv_all(sock: socket.socket) -> bytes:
    chunks = []
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            return b"".join(chunks)
        chunks.append(chunk)


def recv_exact(sock: socket.socket, size: int) -> bytes:
    data = bytearray()
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise RuntimeError(f"unexpected EOF after {len(data)}/{size} bytes")
        data.extend(chunk)
    return bytes(data)


def connect(host: str, port: int) -> socket.socket:
    sock = socket.create_connection((host, port), timeout=SOCKET_TIMEOUT)
    sock.settimeout(SOCKET_TIMEOUT)
    return sock


def round_trip(host: str, port: int, label: int, size: int) -> None:
    expected = payload(label, size)
    with connect(host, port) as sock:
        sock.sendall(expected)
        actual = recv_exact(sock, len(expected))
    if actual != expected:
        raise RuntimeError(
            f"connection {label} payload mismatch: {len(actual)}/{len(expected)} bytes"
        )


def parallel_round_trips(
    host: str, port: int, first_label: int, connections: int, size: int
) -> None:
    with concurrent.futures.ThreadPoolExecutor(max_workers=connections) as pool:
        futures = [
            pool.submit(round_trip, host, port, first_label + index, size)
            for index in range(connections)
        ]
        for future in futures:
            future.result()


def run_concurrent(args: argparse.Namespace) -> None:
    parallel_round_trips(args.host, args.port, 0, args.connections, args.bytes)
    print(
        f"concurrent OK: {args.connections} connections x {args.bytes} bytes",
        flush=True,
    )


def http_requests(
    host: str, port: int, connection_id: int, request_count: int
) -> None:
    connection = http.client.HTTPConnection(host, port, timeout=SOCKET_TIMEOUT)
    try:
        for request_id in range(request_count):
            path = f"/connection/{connection_id}/request/{request_id}"
            expected = f"flare-http:{path}\n".encode()
            connection.request("GET", path)
            response = connection.getresponse()
            actual = response.read()
            if response.status != 200 or actual != expected:
                raise RuntimeError(
                    f"HTTP response mismatch on {connection_id}/{request_id}: "
                    f"status={response.status}, bytes={len(actual)}"
                )
    finally:
        connection.close()


def run_http(args: argparse.Namespace) -> None:
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.connections) as pool:
        futures = [
            pool.submit(http_requests, args.host, args.port, index, args.requests)
            for index in range(args.connections)
        ]
        for future in futures:
            future.result()
    total = args.connections * args.requests
    print(
        f"HTTP keep-alive OK: {args.connections} connections, {total} requests",
        flush=True,
    )


def run_churn(args: argparse.Namespace) -> None:
    for wave in range(args.waves):
        parallel_round_trips(
            args.host,
            args.port,
            wave * args.parallel,
            args.parallel,
            args.bytes,
        )
        print(f"churn wave {wave + 1}/{args.waves} OK", flush=True)
    total = args.waves * args.parallel
    print(f"churn OK: {total} short connections", flush=True)


def reset_connection(host: str, port: int, label: int, size: int) -> None:
    sock = connect(host, port)
    try:
        sock.sendall(payload(label, size))
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
    finally:
        sock.close()


def run_resets(args: argparse.Namespace) -> None:
    for wave in range(args.waves):
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.parallel) as pool:
            futures = [
                pool.submit(
                    reset_connection,
                    args.host,
                    args.port,
                    wave * args.parallel + index,
                    args.bytes,
                )
                for index in range(args.parallel)
            ]
            for future in futures:
                future.result()
        print(f"reset wave {wave + 1}/{args.waves} sent", flush=True)

    # Give both TCP stacks time to process the final RST wave, then prove
    # that stale sockets do not interfere with fresh connections.
    time.sleep(2)
    parallel_round_trips(args.host, args.port, 1_000_000, 16, args.bytes)
    total = args.waves * args.parallel
    print(f"reset recovery OK: {total} resets followed by 16 round trips", flush=True)


def slow_round_trip(
    args: argparse.Namespace, label: int, connected: threading.Barrier
) -> None:
    expected = payload(label, args.bytes)
    with connect(args.host, args.port) as sock:
        connected.wait(timeout=SOCKET_TIMEOUT)
        sock.sendall(expected)
        time.sleep(args.hold_seconds)
        actual = recv_exact(sock, len(expected))
    if actual != expected:
        raise RuntimeError(
            f"slow connection {label} payload mismatch: {len(actual)}/{len(expected)} bytes"
        )


def run_backpressure(args: argparse.Namespace) -> None:
    connected = threading.Barrier(args.connections + 1)
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.connections) as pool:
        futures = [
            pool.submit(slow_round_trip, args, index, connected)
            for index in range(args.connections)
        ]
        connected.wait(timeout=SOCKET_TIMEOUT)
        time.sleep(0.25)

        # The slow readers have filled their per-connection queues. A new
        # connection must still be accepted and make progress independently.
        round_trip(args.host, args.port, 2_000_000, 4096)
        for future in futures:
            future.result()
    print(
        f"backpressure OK: fast connection completed beside "
        f"{args.connections} slow readers",
        flush=True,
    )


def run_idle(args: argparse.Namespace) -> None:
    before = payload(3_000_000, args.bytes)
    after = payload(3_000_001, args.bytes)
    with connect(args.host, args.port) as sock:
        sock.sendall(before)
        if recv_exact(sock, len(before)) != before:
            raise RuntimeError("payload mismatch before idle period")
        time.sleep(args.seconds)
        sock.sendall(after)
        if recv_exact(sock, len(after)) != after:
            raise RuntimeError("payload mismatch after idle period")
    print(f"idle connection OK after {args.seconds} seconds", flush=True)


def add_endpoint(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)

    concurrent_parser = commands.add_parser("concurrent")
    add_endpoint(concurrent_parser)
    concurrent_parser.add_argument("--connections", type=int, default=32)
    concurrent_parser.add_argument("--bytes", type=int, default=131072)
    concurrent_parser.set_defaults(run=run_concurrent)

    http_parser = commands.add_parser("http")
    add_endpoint(http_parser)
    http_parser.add_argument("--connections", type=int, default=16)
    http_parser.add_argument("--requests", type=int, default=8)
    http_parser.set_defaults(run=run_http)

    churn_parser = commands.add_parser("churn")
    add_endpoint(churn_parser)
    churn_parser.add_argument("--waves", type=int, default=20)
    churn_parser.add_argument("--parallel", type=int, default=24)
    churn_parser.add_argument("--bytes", type=int, default=2048)
    churn_parser.set_defaults(run=run_churn)

    reset_parser = commands.add_parser("resets")
    add_endpoint(reset_parser)
    reset_parser.add_argument("--waves", type=int, default=8)
    reset_parser.add_argument("--parallel", type=int, default=24)
    reset_parser.add_argument("--bytes", type=int, default=2048)
    reset_parser.set_defaults(run=run_resets)

    backpressure_parser = commands.add_parser("backpressure")
    add_endpoint(backpressure_parser)
    backpressure_parser.add_argument("--connections", type=int, default=12)
    backpressure_parser.add_argument("--bytes", type=int, default=262144)
    backpressure_parser.add_argument("--hold-seconds", type=float, default=2)
    backpressure_parser.set_defaults(run=run_backpressure)

    idle_parser = commands.add_parser("idle")
    add_endpoint(idle_parser)
    idle_parser.add_argument("--seconds", type=float, default=50)
    idle_parser.add_argument("--bytes", type=int, default=4096)
    idle_parser.set_defaults(run=run_idle)

    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
