#!/usr/bin/env python3
"""Loopback TCP proxy used by the Docker functional E2E.

lkit only accepts HTTP repositories on loopback addresses, while RustFS
runs in a separate container. This proxy exposes RustFS as 127.0.0.1.
"""
import asyncio
import sys


async def forward(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        writer.close()


async def handle(client_reader, client_writer, upstream_host, upstream_port):
    try:
        upstream_reader, upstream_writer = await asyncio.open_connection(
            upstream_host, upstream_port
        )
    except OSError:
        client_writer.close()
        return
    await asyncio.gather(
        forward(client_reader, upstream_writer),
        forward(upstream_reader, client_writer),
        return_exceptions=True,
    )


async def main():
    listen_host, listen_port = sys.argv[1], int(sys.argv[2])
    upstream_host, upstream_port = sys.argv[3], int(sys.argv[4])
    server = await asyncio.start_server(
        lambda reader, writer: handle(reader, writer, upstream_host, upstream_port),
        listen_host,
        listen_port,
    )
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
