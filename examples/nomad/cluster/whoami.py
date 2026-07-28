"""Report which allocation and node this server runs on.

The whole demo workload: nomad's raw_exec driver runs this straight from the
node's nix store, injecting the same NOMAD_* env it would hand a container
(alloc name, the port labeled `http` in job.nix).
"""

import json
import os
import socket
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def main() -> None:
    body = json.dumps(
        {"alloc": os.environ["NOMAD_ALLOC_NAME"], "node": socket.gethostname()}
    ).encode()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    # Nomad names the env var after the port label, conventionally
    # lowercase ("http" in job.nix), hence the SIM112 exemption.
    port = int(os.environ["NOMAD_PORT_http"])  # noqa: SIM112
    ThreadingHTTPServer(("", port), Handler).serve_forever()


if __name__ == "__main__":
    main()
