"""Fail-fast status of the external credentials the bundled tooling needs.

Each credentialed module already raises clearly at call time (``linear`` names
``LINEAR_API_KEY``, ``slack`` points at ``slack.login``); what this adds is the
aggregated, ahead-of-time view. One local-only probe per credential declared in
:mod:`.registry` -- env vars and token-file *existence*, never a file's content,
never the network, never a secret's value -- so the report costs nothing and is
safe to run anywhere.

Three surfaces consume it, all fail-fast: ``ix-mcp serve`` yells once on stderr
at startup (before the first call can waste a budget discovering the gap), the
``ix-mcp requirements`` subcommand exits non-zero when anything is missing (so
setup scripts can gate on it), and the server instructions name the credentialed
modules via :func:`.guide.credentials_note`.
"""

from __future__ import annotations

import dataclasses
import os
from collections.abc import Callable
from pathlib import Path

from . import registry


@dataclasses.dataclass(frozen=True)
class Status:
    """One credential's probe result: where it resolves from, or the remedy."""

    name: str  # module/library the credential serves ("search")
    service: str  # external service it authenticates to ("Mixedbread")
    satisfied_via: str | None  # "MXBAI_API_KEY" / "token at ~/.mgrep/token.json" / None
    remedy: str  # how to satisfy it, shown verbatim when missing

    @property
    def line(self) -> str:
        """The human-readable report line for this status."""
        if self.satisfied_via:
            return f"{self.name}: {self.service} credential via {self.satisfied_via}"
        return f"{self.name}: no {self.service} credential; calls will fail until you {self.remedy}"


def _probe(credential: registry.Credential) -> str | None:
    """Where the credential would resolve from, or ``None``. Local-only: env
    vars and token-file existence, mirroring the order the owning module
    documents, so the report names the source the call would actually use."""
    for var in credential.env:
        if os.environ.get(var, "").strip():
            return var
    if credential.token_path and Path(credential.token_path).expanduser().exists():
        return f"token at {credential.token_path}"
    return None


def _remedy(credential: registry.Credential) -> str:
    """One clause per way to satisfy the credential, joined with "or", e.g.
    ``set MXBAI_API_KEY (get one at https://www.mixedbread.com) or run `mgrep
    login```. Composed here so every service's remedy reads the same."""
    ways: list[str] = []
    if credential.env:
        get_one = f" (get one at {credential.url})" if credential.url else ""
        ways.append(f"set {credential.env[0]}{get_one}")
    if credential.login:
        ways.append(credential.login)
    return " or ".join(ways)


def statuses() -> tuple[Status, ...]:
    """Probe every credential declared in the registry, in registry order."""
    return tuple(
        Status(
            name=name,
            service=credential.service,
            satisfied_via=_probe(credential),
            remedy=_remedy(credential),
        )
        for name, credential in registry.credentialed()
    )


def report(emit: Callable[[str], None]) -> bool:
    """Emit one status line per credential and return whether all are present.

    ``serve`` routes ``emit`` to stderr at startup; the ``requirements``
    subcommand routes it to stdout and turns the bool into its exit code.
    """
    all_satisfied = True
    for status in statuses():
        all_satisfied &= status.satisfied_via is not None
        emit(status.line)
    return all_satisfied
