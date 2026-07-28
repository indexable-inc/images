"""MCP stdio server exposing one typed `run_command` tool over Minecraft RCON.

Hermes talks the Model Context Protocol to its configured servers; this one
wraps the Source RCON protocol (which Minecraft's `enable-rcon` implements) so
the agent gets a typed `run_command(command: str) -> str` tool instead of
shelling out to a raw client. Typed means the schema is the contract: the
agent cannot pass argv arrays, env vars, or shell metacharacters anywhere
meaningful, only one console-command string that the Minecraft server parses
with its own command grammar and permission model.

Connection parameters come from the environment (set declaratively in the
fleet preset): RCON_HOST, RCON_PORT, RCON_PASSWORD. A fresh TCP connection is
made per call; RCON has no session state worth keeping, and the server's
connection cap is generous compared to an agent's call rate.

Protocol notes:
- JSON-RPC 2.0 over stdio, one message per line (MCP's stdio transport).
- RCON packet: <i32 length><i32 id><i32 type><body bytes>\x00\x00, little
  endian. Auth is type 3 (response echoes the id, or -1 on bad password);
  commands are type 2; responses are type 0 and may span several packets, so
  a sentinel command with a distinct id is sent after the real one and
  packets are accumulated until the sentinel echoes back.
"""

import json
import os
import socket
import struct
import sys

# Decoded JSON-RPC messages and MCP result payloads: string keys, arbitrary
# JSON values. The transport hands us untrusted objects, so values stay
# `object` and are narrowed (e.g. `isinstance`) before use.
type JsonObject = dict[str, object]

SERVERDATA_AUTH = 3
SERVERDATA_AUTH_RESPONSE = 2
SERVERDATA_EXECCOMMAND = 2

# Console commands are short; anything longer is a prompt-injection or a bug.
MAX_COMMAND_LENGTH = 1000


class RconError(Exception):
    """Transport or authentication failure talking to the RCON port."""


def _send_packet(sock: socket.socket, packet_id: int, packet_type: int, body: str) -> None:
    payload = struct.pack("<ii", packet_id, packet_type) + body.encode("utf-8") + b"\x00\x00"
    sock.sendall(struct.pack("<i", len(payload)) + payload)


def _recv_exact(sock: socket.socket, count: int) -> bytes:
    chunks = b""
    while len(chunks) < count:
        chunk = sock.recv(count - len(chunks))
        if not chunk:
            raise RconError("connection closed by server")
        chunks += chunk
    return chunks


def _recv_packet(sock: socket.socket) -> tuple[int, int, str]:
    (length,) = struct.unpack("<i", _recv_exact(sock, 4))
    if length < 10 or length > 8192:
        raise RconError(f"implausible packet length {length}")
    payload = _recv_exact(sock, length)
    packet_id, packet_type = struct.unpack("<ii", payload[:8])
    body = payload[8:-2].decode("utf-8", errors="replace")
    return packet_id, packet_type, body


def run_command(host: str, port: int, password: str, command: str) -> str:
    """Authenticate, run one console command, and return its full response."""
    with socket.create_connection((host, port), timeout=10) as sock:
        sock.settimeout(10)

        _send_packet(sock, 1, SERVERDATA_AUTH, password)
        # Some servers send an empty type-0 packet before the auth reply;
        # skip until the type-2 SERVERDATA_AUTH_RESPONSE arrives.
        while True:
            packet_id, packet_type, _ = _recv_packet(sock)
            if packet_type == SERVERDATA_AUTH_RESPONSE:
                break
        if packet_id == -1:
            raise RconError("authentication rejected (wrong RCON password)")

        # The real command, then a sentinel with a different id. The server
        # answers in order, so everything before the sentinel's echo is the
        # command's (possibly multi-packet) response.
        _send_packet(sock, 7, SERVERDATA_EXECCOMMAND, command)
        _send_packet(sock, 8, SERVERDATA_EXECCOMMAND, "")
        response = ""
        while True:
            packet_id, _, body = _recv_packet(sock)
            if packet_id == 8:
                return response
            if packet_id == 7:
                response += body


TOOL = {
    "name": "run_command",
    "description": (
        "Run one Minecraft server console command over RCON and return the "
        "server's response. The command uses console grammar (no leading "
        "slash), e.g. 'list', 'whitelist add <player>', 'worldborder set "
        "2000'. One command per call."
    ),
    "inputSchema": {
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Console command without a leading slash.",
            }
        },
        "required": ["command"],
    },
}


def _tool_call(arguments: JsonObject) -> JsonObject:
    command = arguments.get("command")
    if not isinstance(command, str) or not command.strip():
        return _tool_error("`command` must be a non-empty string")
    command = command.strip().lstrip("/")
    if len(command) > MAX_COMMAND_LENGTH:
        return _tool_error(f"command exceeds {MAX_COMMAND_LENGTH} characters")

    host = os.environ.get("RCON_HOST", "127.0.0.1")
    port = int(os.environ.get("RCON_PORT", "25575"))
    password = os.environ.get("RCON_PASSWORD", "")
    if not password:
        return _tool_error("RCON_PASSWORD is not set in the server environment")

    try:
        response = run_command(host, port, password, command)
    except (RconError, OSError) as error:
        return _tool_error(f"RCON failure: {error}")
    return {
        "content": [{"type": "text", "text": response or "(no output)"}],
        "isError": False,
    }


def _tool_error(message: str) -> JsonObject:
    return {"content": [{"type": "text", "text": message}], "isError": True}


def _handle(request: JsonObject) -> JsonObject | None:
    method = request.get("method")
    request_id = request.get("id")
    if request_id is None:
        # Notifications (e.g. notifications/initialized) need no reply.
        return None

    result: JsonObject
    if method == "initialize":
        result = {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "minecraft-rcon", "version": "1.0.0"},
        }
    elif method == "tools/list":
        result = {"tools": [TOOL]}
    elif method == "tools/call":
        raw_params = request.get("params")
        params: JsonObject = raw_params if isinstance(raw_params, dict) else {}
        if params.get("name") != "run_command":
            return _error_response(request_id, -32602, f"unknown tool {params.get('name')!r}")
        raw_arguments = params.get("arguments")
        arguments: JsonObject = raw_arguments if isinstance(raw_arguments, dict) else {}
        result = _tool_call(arguments)
    elif method == "ping":
        result = {}
    else:
        return _error_response(request_id, -32601, f"method {method!r} not supported")
    return {"jsonrpc": "2.0", "id": request_id, "result": result}


def _error_response(request_id: object, code: int, message: str) -> JsonObject:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


def main() -> None:
    for raw_line in sys.stdin:
        line = raw_line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue
        response = _handle(request)
        if response is not None:
            sys.stdout.write(json.dumps(response) + "\n")
            sys.stdout.flush()


if __name__ == "__main__":
    main()
