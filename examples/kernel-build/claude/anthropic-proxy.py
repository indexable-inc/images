"""Loopback proxy that owns the Anthropic API key so the agent never does.

Runs as the `anthropic-proxy` user, the only uid that can read the key file
(materialized 0400 by ix's secret machinery). It accepts plain HTTP on
127.0.0.1, drops whatever credential the client sent, injects the real key,
and forwards the request to api.anthropic.com over TLS, streaming the
response back chunk by chunk so SSE works. The sandboxed agent's entire
network world is this listener (claude.nix pins its uid to it with
nftables); this process's entire upstream world is the UPSTREAM constant
below.

argv: PORT KEY_FILE -- wired by claude.nix, the single source of truth.
"""

import http.client
import http.server
import sys
from pathlib import Path

UPSTREAM = "api.anthropic.com"

# Client-supplied identity and framing headers are rewritten here, never
# passed through: the whole point is that the client's credential is a dummy.
STRIPPED_REQUEST_HEADERS = frozenset(
    {"authorization", "connection", "content-length", "host", "x-api-key"}
)

# Upstream framing headers that stop being true once the body is re-streamed
# over a close-delimited connection.
STRIPPED_RESPONSE_HEADERS = frozenset(
    {"connection", "content-length", "transfer-encoding"}
)


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    # Assigned in main() before the server starts.
    key_file: Path

    def do_GET(self) -> None:
        self._forward()

    def do_POST(self) -> None:
        self._forward()

    def do_PUT(self) -> None:
        self._forward()

    def do_PATCH(self) -> None:
        self._forward()

    def do_DELETE(self) -> None:
        self._forward()

    def do_HEAD(self) -> None:
        self._forward()

    def _forward(self) -> None:
        # Read the key per request, not at startup: the service comes up on a
        # fresh VM before the secret is attached, and rotation (recreate the
        # VM, or replace the file) needs no proxy restart.
        try:
            key = self.key_file.read_text().strip()
        except OSError:
            self.send_error(
                503,
                "API key not materialized",
                "attach the anthropic_api_key secret (ix secret set "
                "anthropic_api_key, then recreate the VM)",
            )
            return

        # The Anthropic SDK always sends Content-Length; an absent header is
        # a bodiless request, not a chunked one.
        length = int(self.headers.get("Content-Length") or "0")
        body = self.rfile.read(length) if length > 0 else b""

        headers: dict[str, str] = {
            name: value
            for name, value in self.headers.items()
            if name.lower() not in STRIPPED_REQUEST_HEADERS
        }
        headers["Host"] = UPSTREAM
        headers["x-api-key"] = key
        headers["Content-Length"] = str(len(body))
        headers["Connection"] = "close"

        # The timeout bounds the gap between reads, not the whole response,
        # so long SSE streams survive as long as they keep producing.
        upstream = http.client.HTTPSConnection(UPSTREAM, timeout=600)
        try:
            upstream.request(self.command, self.path, body=body, headers=headers)
            response = upstream.getresponse()
        except OSError:
            self.send_error(502, "upstream unreachable")
            upstream.close()
            return

        self.send_response(response.status)
        for name, value in response.getheaders():
            if name.lower() not in STRIPPED_RESPONSE_HEADERS:
                self.send_header(name, value)
        self.send_header("Connection", "close")
        self.close_connection = True
        self.end_headers()
        try:
            # read1 returns whatever is buffered instead of blocking for a
            # full chunk, which is what keeps SSE tokens flowing immediately.
            while chunk := response.read1(65536):
                self.wfile.write(chunk)
                self.wfile.flush()
        except OSError:
            # Client hung up mid-stream; nothing to salvage.
            pass
        finally:
            upstream.close()


def main() -> None:
    port = int(sys.argv[1])
    Handler.key_file = Path(sys.argv[2])
    # Threading: the agent runs tool calls and API turns concurrently.
    http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()


if __name__ == "__main__":
    main()
