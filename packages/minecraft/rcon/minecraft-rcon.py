#!/usr/bin/env python3
import socket
import struct
import sys
from collections.abc import Sequence
from pathlib import Path

from pydantic import Field, model_validator
from pydantic_settings import BaseSettings, CliPositionalArg, CliSettingsSource, PydanticBaseSettingsSource, SettingsConfigDict


AUTH = 3
COMMAND = 2

# Sentinel default used to mark fields that are required at the CLI even though
# the type checker needs a syntactic default value.
_UNSET_PORT: int = -1
_UNSET_COMMAND: list[str] = []


class RconSettings(BaseSettings):
    model_config = SettingsConfigDict(
        cli_parse_args=True,
        cli_kebab_case=True,
        cli_prog_name="minecraft-rcon",
        cli_enforce_required=True,
    )

    host: str = "127.0.0.1"
    port: int = Field(default=_UNSET_PORT)
    password: str | None = None
    password_file: str | None = None
    command: CliPositionalArg[list[str]] = _UNSET_COMMAND

    @classmethod
    def settings_customise_sources(
        cls,
        settings_cls: type[BaseSettings],
        init_settings: PydanticBaseSettingsSource,
        env_settings: PydanticBaseSettingsSource,
        dotenv_settings: PydanticBaseSettingsSource,
        file_secret_settings: PydanticBaseSettingsSource,
    ) -> tuple[PydanticBaseSettingsSource, ...]:
        # Return only init + CLI sources; drop env/dotenv/secrets so that
        # ambient env vars (e.g. PASSWORD, HOST) cannot silently populate
        # settings (argparse parity, CWE-15).
        return (
            init_settings,
            CliSettingsSource(settings_cls, cli_parse_args=True),
        )

    @model_validator(mode="after")
    def validate_required_fields_and_password(self) -> "RconSettings":
        if self.port == _UNSET_PORT:
            raise ValueError("--port is required")
        if not self.command:
            raise ValueError("command is required")
        if self.password is not None and self.password_file is not None:
            raise ValueError(
                "argument --password-file: not allowed with argument --password"
            )
        if self.password is None and self.password_file is None:
            raise ValueError(
                "one of the arguments --password --password-file is required"
            )
        return self


def packet(request_id: int, kind: int, payload: str) -> bytes:
    body = struct.pack("<ii", request_id, kind) + payload.encode("utf-8") + b"\0\0"
    return struct.pack("<i", len(body)) + body


def read_packet(sock: socket.socket) -> tuple[int, int, str]:
    header = sock.recv(4)
    if len(header) != 4:
        raise RuntimeError("short RCON length header")
    (length,) = struct.unpack("<i", header)
    body = b""
    while len(body) < length:
        chunk = sock.recv(length - len(body))
        if not chunk:
            raise RuntimeError("RCON connection closed")
        body += chunk
    request_id, kind = struct.unpack("<ii", body[:8])
    payload = body[8:-2].decode("utf-8", errors="replace")
    return int(request_id), int(kind), payload


def resolve_password(settings: RconSettings) -> str:
    if settings.password_file is not None:
        with Path(settings.password_file).open(encoding="utf-8") as f:
            return f.readline().rstrip("\n")
    # model_validator guarantees exactly one is set
    assert settings.password is not None
    return settings.password


def main(argv: Sequence[str] | None = None) -> int:
    if argv is not None:
        saved = sys.argv[:]
        sys.argv = [sys.argv[0], *list(argv)]
        try:
            settings = RconSettings()
        finally:
            sys.argv = saved
    else:
        settings = RconSettings()

    password = resolve_password(settings)
    command = " ".join(settings.command)

    with socket.create_connection((settings.host, settings.port), timeout=10) as sock:
        sock.sendall(packet(1, AUTH, password))
        auth_id, _, auth_payload = read_packet(sock)
        if auth_id == -1:
            print("RCON authentication failed", file=sys.stderr)
            return 1
        if auth_payload:
            print(auth_payload)

        sock.sendall(packet(2, COMMAND, command))
        _, _, output = read_packet(sock)
        if output:
            print(output)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
