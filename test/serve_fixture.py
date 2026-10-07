#!/usr/bin/env python3
"""Run a command against an isolated HTTP fixture instead of stubbing curl."""
import http.server
import os
from pathlib import Path
import socket
import socketserver
import subprocess
import sys
import threading
import time
from urllib.parse import urlsplit


class LoopbackHTTPServer(http.server.ThreadingHTTPServer):
    """Bind numeric loopback addresses without a reverse-DNS lookup."""

    def server_bind(self):
        # HTTPServer.server_bind calls socket.getfqdn(), which can stall for
        # tens of seconds on hosted macOS runners before the server listens.
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


def server(fixture, health=b"{}", log=None, delay=0):
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if log:
                with open(log, "a", encoding="utf-8") as output:
                    output.write(self.path + "\n")
            if self.path == "/health":
                body = health
            elif self.path == "/usage" and fixture:
                if delay:
                    time.sleep(delay)
                body = Path(fixture).read_bytes()
            else:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def log_message(self, *_args):
            pass

    result = LoopbackHTTPServer(("127.0.0.1", 0), Handler)
    result.daemon_threads = True
    return result


def main():
    args = sys.argv[1:]
    if args[0] == "--server":
        ready, log = args[1:3]
        instance = server(None, b'{"version":"CodexBar v9.8.7"}', log)
        Path(ready).write_text(f"http://127.0.0.1:{instance.server_port}\n")
        instance.serve_forever()
        return 0
    expected = None
    if args[0] == "--timeout-check":
        expected = int(args[1])
        args = args[2:]
    env = os.environ.copy()
    instance = server(env.get("SHOWY_QUOTA_TEST_SERVE_FIXTURE"), delay=(expected + 1) if expected else 0)
    thread = threading.Thread(target=instance.serve_forever, daemon=True)
    thread.start()
    configured = env.get("SHOWY_QUOTA_CODEXBAR_SERVE_URL", "")
    fixture_url = env.get("SHOWY_QUOTA_TEST_SERVE_URL", "")
    unavailable = None
    parsed = urlsplit(configured)
    local = (parsed.scheme == "http" and parsed.hostname in ("127.0.0.1", "localhost", "::1")
             and parsed.username is None and parsed.password is None and parsed.path in ("", "/"))
    if local and configured and configured.rstrip("/") == fixture_url.rstrip("/"):
        env["SHOWY_QUOTA_CODEXBAR_SERVE_URL"] = f"http://127.0.0.1:{instance.server_port}"
    elif configured.startswith("http://127.0.0.1:") and fixture_url:
        # Hold an unlistened socket so an unavailable fixture cannot hit a live service.
        unavailable = socket.socket()
        unavailable.bind(("127.0.0.1", 0))
        env["SHOWY_QUOTA_CODEXBAR_SERVE_URL"] = f"http://127.0.0.1:{unavailable.getsockname()[1]}"
    started = time.monotonic()
    try:
        result = subprocess.run(args, env=env, stdout=subprocess.PIPE if expected else None)
        elapsed = time.monotonic() - started
        if expected:
            if result.returncode != 0 and expected - 0.5 <= elapsed <= expected + 3:
                print(expected)
                return 0
            print(f"rc={result.returncode}; elapsed={elapsed:.3f}")
            return 1
        return result.returncode
    finally:
        instance.shutdown()
        instance.server_close()
        if unavailable:
            unavailable.close()


if __name__ == "__main__":
    sys.exit(main())
