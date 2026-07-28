"""Report which pod and node this server runs on.

The whole demo workload: one JSON document naming the pod and the node it
landed on (both injected by the downward API, see workload.nix), so curling
the Service visibly round-robins across pods. The port comes in as argv so
ports.nix stays the single statement of it.
"""

import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def main() -> None:
    body = json.dumps(
        {"pod": os.environ["POD_NAME"], "node": os.environ["NODE_NAME"]}
    ).encode()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    ThreadingHTTPServer(("", int(sys.argv[1])), Handler).serve_forever()


if __name__ == "__main__":
    main()
