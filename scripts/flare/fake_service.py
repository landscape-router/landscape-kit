#!/usr/bin/env python3
"""Fake echo and HTTP services used by the flare Docker e2e tests.

Used by the docker e2e test as the target of the Terrain port forward.
"""
import http.server
import socket
import threading


class ThreadingHTTPServer(http.server.ThreadingHTTPServer):
    request_queue_size = 128


class HTTPHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        body = f"flare-http:{self.path}\n".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        pass


httpd = ThreadingHTTPServer(("127.0.0.1", 8080), HTTPHandler)
threading.Thread(target=httpd.serve_forever, daemon=True).start()

srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", 6443))
# The connection-lifecycle e2e opens concurrent waves. Keep the target
# service backlog above the tested concurrency so it does not become the
# limiting component under test.
srv.listen(128)


def echo(c):
    try:
        while True:
            d = c.recv(65536)
            if not d:
                break
            c.sendall(d)
    except OSError:
        # Reset scenarios intentionally tear down sockets mid-transfer.
        pass
    finally:
        c.close()


while True:
    c, _ = srv.accept()
    threading.Thread(target=echo, args=(c,), daemon=True).start()
