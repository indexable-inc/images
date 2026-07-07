"""Polars-returning SSH fan-out source for the ix-mcp kernel.

Bundled like ``view``/``sh``/``search`` so every session can ``import fleet``
with no setup. The point: a ``python_exec`` cell often wants the same file or the
same command's output from *many* fleet hosts at once (every node's journald
tail, every host's disk usage, a config that should be identical everywhere),
and then to slice the combined result with polars. Hand-rolling that means a
loop of ``ssh`` subprocesses, manual stdout parsing, and a fragile merge. This
module does the fan-out on the shared async loop (``asyncssh`` + a bounded
``asyncio.Semaphore``), parses each host's bytes into a ``polars.DataFrame``,
tags every row with its ``host``, and stitches the per-host frames into one with
``pl.concat(how="diagonal_relaxed")`` so mismatched schemas still combine.

Everything that touches the network is ``async def`` because the kernel is one
shared event loop: a blocking ``ssh`` subprocess or ``time.sleep`` would freeze
every other job. Cells ``await`` the read functions.

Usage::

    import fleet

    # Every host's free-disk JSON (host-side `df` emitting NDJSON), one frame.
    df = await fleet.scan(
        ["hc1.ts.net:9999", "hc2.ts.net:9999"],
        "df -h --output=source,size,used,avail,target | tail -n +2 "
        "| awk '{print \"{\\\"src\\\":\\\"\"$1\"\\\",\\\"avail\\\":\\\"\"$4\"\\\"}\"}'",
    )
    df.filter(pl.col("avail").str.contains("G")).sort("host")

    # Unstructured log lines, one row per line, tagged by host.
    logs = await fleet.read_text(hosts, "/var/log/syslog")

    # One host, one multi-line script, raw typed outcome: the script is shipped
    # base64-encoded into bash, so no quoting layer ever touches it.
    r = await fleet.ssh_run("hc1.ts.net:9999", "set -e\nuptime\ndf -h /")
    r.exit_code, r.stdout, r.stderr

    # Survive a down host instead of failing the whole batch.
    df = await fleet.scan(hosts, "uptime", parser=fleet.text_parser,
                          on_error="collect")
    df.attrs  # {"fleet_failures": {host: "error string", ...}}

The SSH defaults match how the fleet is already reached: key auth via
``~/.ssh/id_ed25519``, ``known_hosts`` resolved non-interactively (an unknown
host is accepted and recorded rather than blocking on a prompt), and a per-entry
port so ``*.ts.net`` hosts on ``:9999`` and plain hosts on ``:22`` mix freely in
one call.
"""

from __future__ import annotations

import asyncio
import base64
import dataclasses
import io
import os
import re
import shlex
from collections.abc import Awaitable, Callable, Mapping, Sequence
from pathlib import Path
from typing import Any

import asyncssh
import polars as pl

# The cluster surface (discovery, Ray distributed exec, live-kernel peek) lives
# in `cluster.py` so the SSH fan-out below stays a self-contained unit; both are
# re-exported here, so `import fleet` gives the whole API in one namespace.
from .cluster import (
    EXEC_PORT,
    SPARK_CONNECT_PORT,
    ClusterError,
    connect,
    get,
    in_kernel,
    nodes,
    put,
    run,
    spark,
    submit,
    up,
)

__version__ = "0.1.0"

__all__ = [
    "EXEC_PORT",
    "SPARK_CONNECT_PORT",
    # Cluster surface (cluster.py)
    "ClusterError",
    "FleetError",
    "HostSpec",
    "SshResult",
    "connect",
    # SSH shell fan-out (this module)
    "csv_parser",
    "get",
    "in_kernel",
    "ndjson_parser",
    "nodes",
    "parquet_parser",
    "put",
    "read_csv",
    "read_ndjson",
    "read_parquet",
    "read_text",
    "run",
    "scan",
    "spark",
    "ssh_run",
    "submit",
    "text_parser",
    "up",
]

# A parser turns one host's raw stdout bytes into a DataFrame. Kept as a plain
# callable so a caller can pass `pl.read_ndjson`, a lambda, or one of the
# wrappers below interchangeably.
Parser = Callable[[bytes], pl.DataFrame]

# How a caller may name a host: a bare hostname (default port), "host:port", or
# a dict of asyncssh connect kwargs (must carry at least "host").
HostSpec = "str | Mapping[str, Any]"

# Default identity. The fleet is reached with this key already; resolve it once
# here rather than relying on an ssh-agent that a non-interactive kernel may not
# have. A missing file is simply not passed, so agent/other auth still works.
_DEFAULT_KEY = Path("~/.ssh/id_ed25519").expanduser()


class FleetError(Exception):
    """A fan-out failed.

    Raised by :func:`scan` when ``on_error="raise"`` and one or more hosts
    failed. ``failures`` maps each failed host label to its error string so the
    caller sees every failure at once, not just the first.
    """

    def __init__(self, failures: dict[str, str]) -> None:
        self.failures = failures
        joined = "; ".join(f"{h}: {e}" for h, e in failures.items())
        super().__init__(f"fleet scan failed on {len(failures)} host(s): {joined}")


def ndjson_parser(data: bytes) -> pl.DataFrame:
    """Parse newline-delimited JSON stdout. The default :func:`scan` parser."""
    # Empty stdout (a command that printed nothing) is a legitimate result, not
    # an error: return an empty frame so the host still contributes a `host`
    # column under `diagonal_relaxed` rather than blowing up read_ndjson.
    if not data.strip():
        return pl.DataFrame()
    return pl.read_ndjson(io.BytesIO(data))


def csv_parser(data: bytes) -> pl.DataFrame:
    """Parse CSV stdout into a DataFrame."""
    if not data.strip():
        return pl.DataFrame()
    return pl.read_csv(io.BytesIO(data))


def parquet_parser(data: bytes) -> pl.DataFrame:
    """Parse Parquet bytes (e.g. from ``cat file.parquet``) into a DataFrame."""
    if not data:
        return pl.DataFrame()
    return pl.read_parquet(io.BytesIO(data))


def text_parser(data: bytes) -> pl.DataFrame:
    """Parse raw stdout into one ``line`` column, one row per line.

    For unstructured output (logs, shell history, ``uptime``) where there is no
    record format to read. The ``host`` column is added by :func:`scan` when
    ``tag_host`` is set.
    """
    text = data.decode("utf-8", errors="replace")
    # Splitlines (not split on "\n") so a trailing newline does not yield a
    # spurious empty final row, and CRLF logs split cleanly too.
    lines = text.splitlines()
    return pl.DataFrame({"line": lines}, schema={"line": pl.String})


def _normalize_host(
    spec: str | Mapping[str, Any],
    *,
    username: str | None,
    connect_kwargs: dict[str, Any],
) -> tuple[str, dict[str, Any]]:
    """Resolve one host entry into (label, asyncssh.connect kwargs).

    Accepts "host", "host:port", or a dict of connect kwargs. The label is what
    shows up in the `host` column and in failure keys: a stable "host:port" so
    two ports on one hostname stay distinct.
    """
    opts: dict[str, Any] = dict(connect_kwargs)

    if isinstance(spec, Mapping):
        opts.update(spec)
        host = opts.get("host")
        if not host:
            raise ValueError(f"host dict missing 'host' key: {spec!r}")
        port = opts.get("port", 22)
    else:
        host = spec
        # "[v6::addr]:port" is out of scope here; the fleet is named hosts. A
        # single trailing ":port" is the only split, so rsplit once.
        if ":" in spec:
            host, _, port_s = spec.rpartition(":")
            port = int(port_s)
        else:
            port = 22
        opts["host"] = host
        opts["port"] = port

    if username is not None:
        opts.setdefault("username", username)

    # Non-interactive known_hosts: never prompt (a prompt would hang the kernel
    # forever). known_hosts=None tells asyncssh to accept and not verify the
    # host key. Callers who want strict checking pass known_hosts= explicitly.
    opts.setdefault("known_hosts", None)

    # Default identity, only if present and the caller did not specify auth.
    if (
        "client_keys" not in opts
        and "password" not in opts
        and _DEFAULT_KEY.exists()
    ):
        opts["client_keys"] = [str(_DEFAULT_KEY)]

    label = f"{opts['host']}:{opts['port']}"
    return label, opts


async def _run_one(
    label: str,
    opts: dict[str, Any],
    command: str,
    sem: asyncio.Semaphore,
) -> bytes:
    """Open one connection, run one command, return stdout bytes.

    The semaphore is acquired around the whole connect+run so concurrency caps
    the number of *live connections*, not just queued coroutines.
    """
    async with sem, asyncssh.connect(**opts) as conn:
        # encoding=None keeps stdout as bytes so a binary payload (parquet
        # over `cat`) survives; text parsers decode themselves.
        result = await conn.run(command, encoding=None, check=True)
        out = result.stdout
        return out if isinstance(out, bytes) else bytes(out or b"")


async def scan(
    hosts: Sequence[str | Mapping[str, Any]],
    command: str,
    *,
    parser: Parser | None = None,
    concurrency: int = 16,
    tag_host: bool = True,
    username: str | None = None,
    on_error: str = "collect",
    **connect_kwargs: Any,  # noqa: ANN401 -- passed through to asyncssh.connect
) -> pl.DataFrame:
    """Run ``command`` on every host in parallel and combine into one frame.

    One ``asyncssh`` connection per host, bounded by
    ``asyncio.Semaphore(concurrency)`` so at most ``concurrency`` connections are
    live at once. Each host's stdout bytes go through ``parser`` (default
    :func:`ndjson_parser`); per-host frames are combined with
    ``pl.concat(how="diagonal_relaxed")`` so heterogeneous schemas still merge.

    Args:
        hosts: host entries as ``"host"``, ``"host:port"``, or a connect-kwargs
            ``dict`` (must include ``"host"``). Ports are per entry, so a
            ``*.ts.net:9999`` host and a plain ``:22`` host mix in one call.
        command: the remote command. Prefer host-side filtering (``rg``/``jq``/
            ``tail``) so less crosses the wire.
        parser: ``bytes -> pl.DataFrame``. Defaults to NDJSON. Use
            :func:`csv_parser`, :func:`parquet_parser`, :func:`text_parser`, or
            your own.
        concurrency: max simultaneous SSH connections.
        tag_host: add a ``host`` literal column (the ``"host:port"`` label) to
            each host's rows. The first column in the result.
        username: SSH username applied to every host that does not set its own.
        on_error: ``"collect"`` gathers per-host failures into
            ``result.attrs["fleet_failures"]`` (a ``{label: error}`` dict) and
            returns the successful rows; ``"raise"`` aggregates all failures and
            raises :class:`FleetError`.
        **connect_kwargs: extra kwargs passed to every ``asyncssh.connect``
            (e.g. ``known_hosts=...``, ``client_keys=...``) unless a host dict
            overrides them.

    Returns:
        A combined ``polars.DataFrame``. An all-empty fan-out returns an empty
        frame rather than raising. When ``on_error="collect"``, per-host
        failures are under ``result.attrs["fleet_failures"]``.

    Example::

        df = await fleet.scan(
            ["hc1.ts.net:9999", "hc2.ts.net:9999"],
            "cat /proc/loadavg | awk '{print \"{\\\"load1\\\":\"$1\"}\"}'",
        )
        df.sort("load1", descending=True)
    """
    if on_error not in ("collect", "raise"):
        raise ValueError(f"on_error must be 'collect' or 'raise', got {on_error!r}")
    if parser is None:
        parser = ndjson_parser

    sem = asyncio.Semaphore(max(1, concurrency))
    normalized = [
        _normalize_host(h, username=username, connect_kwargs=connect_kwargs)
        for h in hosts
    ]

    # Launch every host concurrently; the semaphore inside _run_one is what
    # actually caps in-flight connections. return_exceptions so one bad host
    # never cancels the gather.
    tasks = [_run_one(label, opts, command, sem) for label, opts in normalized]
    results = await asyncio.gather(*tasks, return_exceptions=True)

    frames: list[pl.DataFrame] = []
    failures: dict[str, str] = {}
    for (label, _opts), res in zip(normalized, results, strict=False):
        if isinstance(res, BaseException):
            failures[label] = f"{type(res).__name__}: {res}"
            continue
        frame = parser(res)
        if tag_host and frame.width > 0:
            # Prepend a host literal so every row carries its origin and the
            # column lands first. An empty per-host frame stays empty (no rows
            # to tag); diagonal_relaxed still unions it harmlessly.
            frame = frame.with_columns(pl.lit(label).alias("host")).select(
                ["host", *[c for c in frame.columns if c != "host"]]
            )
        frames.append(frame)

    if on_error == "raise" and failures:
        raise FleetError(failures)

    if not frames or all(f.width == 0 for f in frames):
        combined = pl.DataFrame()
    else:
        # diagonal_relaxed: union of columns across hosts, with dtype coercion
        # when the same column came back as different types. Drop the truly
        # empty (0-width) frames first so they do not force an all-null schema.
        non_empty = [f for f in frames if f.width > 0]
        combined = pl.concat(non_empty, how="diagonal_relaxed")

    # Surface failures on the frame itself so a cell can inspect them without a
    # separate return value. attrs is a plain dict polars passes through.
    combined.attrs = {"fleet_failures": failures}  # type: ignore[attr-defined]
    return combined



# Environment variable names accepted by `ssh_run(env=...)`. Values are shell-
# quoted; a name that needs quoting (spaces, "=") is a caller bug, not
# something to work around.
_ENV_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")

# Seconds to wait for the TCP + SSH handshake before giving up on a host
# (OpenSSH's ConnectTimeout role): without it a down host holds the call for
# the OS-level connect timeout, which can be minutes.
_CONNECT_TIMEOUT = 10.0


@dataclasses.dataclass(frozen=True)
class SshResult:
    """The outcome of one :func:`ssh_run` script on one host."""

    host: str  # the resolved "host:port" label the script ran on
    exit_code: int  # the script's exit status; negative signal number if killed
    stdout: str
    stderr: str

    @property
    def ok(self) -> bool:
        """Whether the script exited 0."""
        return self.exit_code == 0


def _ssh_command(script: str, *, sudo: bool, env: Mapping[str, str] | None) -> str:
    """The remote command line that runs ``script`` under ``bash``.

    The script travels base64-encoded and is decoded straight into ``bash`` on
    the host, so no quoting layer -- the local shell, the SSH exec channel, or
    a remote login shell that is not bash (nushell, fish) -- ever parses it.
    The pipeline's exit status is bash's, i.e. the script's own.
    """
    for name in env or ():
        if not _ENV_NAME.match(name):
            raise ValueError(f"invalid environment variable name: {name!r}")
    runner = "bash"
    if env:
        pairs = " ".join(shlex.quote(f"{k}={v}") for k, v in env.items())
        runner = f"env {pairs} {runner}"
    if sudo:
        # -n: fail immediately when a password would be required; an
        # interactive sudo prompt over the exec channel would just hang.
        runner = f"sudo -n {runner}"
    encoded = base64.b64encode(script.encode()).decode("ascii")
    return f"echo {encoded} | base64 -d | {runner}"


async def ssh_run(
    host: str | Mapping[str, Any],
    script: str,
    *,
    sudo: bool = False,
    env: Mapping[str, str] | None = None,
    timeout: float | None = 300.0,
    connect_timeout: float = _CONNECT_TIMEOUT,
    username: str | None = None,
    **connect_kwargs: Any,  # noqa: ANN401 -- passed through to asyncssh.connect
) -> SshResult:
    """Run a multi-line ``bash`` script on one host and return its typed outcome.

    The single-host complement to :func:`scan`: where ``scan`` fans one command
    out across many hosts and parses stdout into a frame, this runs one script
    on one host and returns the raw outcome (:class:`SshResult` with
    ``exit_code`` / ``stdout`` / ``stderr``). Because the script is shipped
    base64-encoded into ``bash`` (see :func:`_ssh_command`), heredocs, quotes,
    and newlines survive untouched -- this replaces hand-rolling
    ``echo <b64> | base64 -d | bash`` through ``nu('^ssh ...')`` quoting.

    A non-zero exit is a *result*, not an exception: branch on ``result.ok``.
    A connection failure (unreachable host, bad auth) raises the underlying
    ``asyncssh`` error, and a script still running after ``timeout`` seconds
    raises :class:`asyncssh.TimeoutError` (which carries the partial output).

    Args:
        host: ``"host"``, ``"host:port"``, or a connect-kwargs ``dict`` (must
            include ``"host"``) -- the same forms one :func:`scan` entry takes,
            with the same key/known-hosts defaults.
        script: the bash script; any number of lines, any quoting.
        sudo: run the script under ``sudo -n`` (non-interactive: fails fast
            instead of hanging on a password prompt the kernel cannot answer).
        env: extra environment for the script, applied host-side via
            ``env NAME=value``; names must be valid identifiers.
        timeout: seconds the script may run before ``asyncssh.TimeoutError``;
            ``None`` waits forever.
        connect_timeout: seconds allowed for the TCP + SSH handshake
            (OpenSSH's ``ConnectTimeout``), so a down host fails fast.
        username: SSH username for the connection.
        **connect_kwargs: extra ``asyncssh.connect`` kwargs
            (``client_keys=...``, ``known_hosts=...``).

    Example::

        result = await fleet.ssh_run(
            "hil-compute-1",
            '''
            set -euo pipefail
            systemctl is-active ix-kernel
            journalctl -u ix-kernel --since -5min | tail -n 20
            ''',
            sudo=True,
            env={"RUST_LOG": "debug"},
        )
        if not result.ok:
            print(result.exit_code, result.stderr)
    """
    label, opts = _normalize_host(
        host, username=username, connect_kwargs=dict(connect_kwargs)
    )
    opts.setdefault("connect_timeout", connect_timeout)
    command = _ssh_command(script, sudo=sudo, env=env)

    async with asyncssh.connect(**opts) as conn:
        # check=False: the exit code is part of the returned result. Default
        # encoding keeps stdout/stderr as str (scripts are a text interface;
        # binary transfer is read_parquet/get territory).
        completed = await conn.run(command, check=False, timeout=timeout)

    stdout = completed.stdout
    stderr = completed.stderr
    return SshResult(
        host=label,
        exit_code=completed.returncode if completed.returncode is not None else -1,
        stdout=stdout if isinstance(stdout, str) else (stdout or b"").decode("utf-8", errors="replace"),
        stderr=stderr if isinstance(stderr, str) else (stderr or b"").decode("utf-8", errors="replace"),
    )


async def read_ndjson(
    hosts: Sequence[str | Mapping[str, Any]],
    remote_path: str,
    *,
    filter_cmd: str | None = None,
    **kw: Any,  # noqa: ANN401 -- forwarded to scan/asyncssh.connect
) -> pl.DataFrame:
    """Read an NDJSON file from every host into one frame.

    Runs ``cat <remote_path>`` by default. ``filter_cmd`` replaces that with a
    host-side pipeline so filtering happens at the source and less crosses the
    wire, e.g. ``filter_cmd="rg level=error /var/log/app.ndjson"`` or
    ``filter_cmd="tail -n 100 /var/log/app.ndjson | jq -c 'select(.ok)'"``.
    Extra kwargs pass through to :func:`scan`.
    """
    command = filter_cmd if filter_cmd is not None else f"cat {_q(remote_path)}"
    return await scan(hosts, command, parser=ndjson_parser, **kw)


async def read_csv(
    hosts: Sequence[str | Mapping[str, Any]],
    remote_path: str,
    *,
    filter_cmd: str | None = None,
    **kw: Any,  # noqa: ANN401 -- forwarded to scan/asyncssh.connect
) -> pl.DataFrame:
    """Read a CSV file from every host into one frame.

    ``cat <remote_path>`` by default; ``filter_cmd`` substitutes a host-side
    pipeline (keep the header if you filter, e.g. with ``head -1; rg ...``).
    """
    command = filter_cmd if filter_cmd is not None else f"cat {_q(remote_path)}"
    return await scan(hosts, command, parser=csv_parser, **kw)


async def read_parquet(
    hosts: Sequence[str | Mapping[str, Any]],
    remote_path: str,
    *,
    use_sftp: bool = False,
    **kw: Any,  # noqa: ANN401 -- forwarded to scan/asyncssh.connect
) -> pl.DataFrame:
    """Read a Parquet file from every host into one frame.

    Parquet is binary, so by default this streams the bytes over ``cat`` (stdout
    is captured as bytes). ``use_sftp=True`` fetches via SFTP instead, which
    avoids any shell-quoting of the path and is friendlier to large files. Both
    paths feed the bytes to ``pl.read_parquet``.
    """
    if not use_sftp:
        return await scan(
            hosts, f"cat {_q(remote_path)}", parser=parquet_parser, **kw
        )
    # SFTP path: open one connection per host (bounded), read the whole file,
    # then reuse `scan`'s combine by faking a parser over the prefetched bytes
    # would be awkward, so do the gather here directly.
    concurrency = kw.pop("concurrency", 16)
    tag_host = kw.pop("tag_host", True)
    username = kw.pop("username", None)
    on_error = kw.pop("on_error", "collect")
    sem = asyncio.Semaphore(max(1, concurrency))
    normalized = [
        _normalize_host(h, username=username, connect_kwargs=kw) for h in hosts
    ]

    async def fetch(label: str, opts: dict[str, Any]) -> bytes:
        async with sem, asyncssh.connect(**opts) as conn, conn.start_sftp_client() as sftp, sftp.open(remote_path, "rb") as fh:
            return await fh.read()

    results = await asyncio.gather(
        *(fetch(label, opts) for label, opts in normalized),
        return_exceptions=True,
    )
    frames: list[pl.DataFrame] = []
    failures: dict[str, str] = {}
    for (label, _opts), res in zip(normalized, results, strict=False):
        if isinstance(res, BaseException):
            failures[label] = f"{type(res).__name__}: {res}"
            continue
        frame = parquet_parser(res)
        if tag_host and frame.width > 0:
            frame = frame.with_columns(pl.lit(label).alias("host")).select(
                ["host", *[c for c in frame.columns if c != "host"]]
            )
        frames.append(frame)
    if on_error == "raise" and failures:
        raise FleetError(failures)
    non_empty = [f for f in frames if f.width > 0]
    combined = (
        pl.concat(non_empty, how="diagonal_relaxed") if non_empty else pl.DataFrame()
    )
    combined.attrs = {"fleet_failures": failures}  # type: ignore[attr-defined]
    return combined


async def read_text(
    hosts: Sequence[str | Mapping[str, Any]],
    remote_path: str,
    *,
    filter_cmd: str | None = None,
    **kw: Any,  # noqa: ANN401 -- forwarded to scan/asyncssh.connect
) -> pl.DataFrame:
    """Read an unstructured file from every host as one row per line.

    Returns a frame with columns ``host`` and ``line`` (one row per line), for
    logs, shell history, and other text with no record format. ``filter_cmd``
    runs a host-side pipeline instead of ``cat`` so you can ``tail``/``rg`` at
    the source (e.g. ``filter_cmd="rg -i oom /var/log/kern.log"``).
    """
    command = filter_cmd if filter_cmd is not None else f"cat {_q(remote_path)}"
    return await scan(hosts, command, parser=text_parser, **kw)


def _q(path: str) -> str:
    """Shell-quote a remote path so spaces/specials are safe in the command."""
    import shlex

    return shlex.quote(path)
