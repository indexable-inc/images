"""Network-free tests for the federated-resources bridge.

These never reach a real ``ix`` or a peer. Two strategies prove every path:

* A **stub ``ix`` script** put on ``PATH`` (via ``IX_RESOURCES_BIN``) that emits
  known JSON or a chosen exit code, so the real ``asyncio`` subprocess path --
  argv assembly, JSON parse, not-found detection -- is exercised end to end.
* For the **graceful-absent** case, point ``IX_RESOURCES_BIN`` at a nonexistent
  path so the PATH lookup fails exactly as it would with no ``ix`` installed.

The module shape (exports, full annotations matching the ruff ANN gate) is
checked too.
"""

from __future__ import annotations

import asyncio
import inspect
import json
import shutil
import stat
import sys
from collections.abc import Callable
from pathlib import Path

import pytest

# Prefer the bundled package (nix check copies the source into the interpreter);
# fall back to the source tree for a dev run.
_PKG_PARENT = Path(__file__).resolve().parents[1]
if str(_PKG_PARENT) not in sys.path:
    sys.path.insert(0, str(_PKG_PARENT))

from ix_notebook_mcp import resources_bridge as rb

# The `stub_ix` fixture hands back a factory: call it with a shell body, get the
# path to the stub `ix` it installed on PATH. A precise alias keeps the test
# signatures free of bare `Any` (the repo's ANN401 gate bans it).
StubIx = Callable[[str], Path]


# ---------------------------------------------------------------------------
# Shape
# ---------------------------------------------------------------------------


def test_all_names_exist() -> None:
    for name in rb.__all__:
        assert hasattr(rb, name), f"{name} in __all__ but missing from module"


def test_error_hierarchy() -> None:
    assert issubclass(rb.ResourceBridgeError, RuntimeError)
    assert issubclass(rb.ResourceNotFoundError, rb.ResourceBridgeError)
    assert rb.RESOURCE_NOT_FOUND == -32002


def test_public_async_funcs_annotated() -> None:
    # Mirrors the ruff ANN gate: every public function fully annotates params/return.
    for name in ("list_resources", "read_resource", "act", "configured_peers"):
        func = getattr(rb, name)
        sig = inspect.signature(func)
        assert sig.return_annotation is not inspect.Signature.empty, f"{name} missing return annotation"
        for pname, param in sig.parameters.items():
            assert param.annotation is not inspect.Parameter.empty, f"{name}({pname}) missing annotation"


# ---------------------------------------------------------------------------
# Peer configuration
# ---------------------------------------------------------------------------


def test_configured_peers_parsing(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(rb.PEERS_ENV, raising=False)
    assert rb.configured_peers() == []
    monkeypatch.setenv(rb.PEERS_ENV, "")
    assert rb.configured_peers() == []
    monkeypatch.setenv(rb.PEERS_ENV, "http://a, , http://b ")
    assert rb.configured_peers() == ["http://a", "http://b"]


# ---------------------------------------------------------------------------
# Stub-ix harness: a real script on PATH so the subprocess path runs for real.
# ---------------------------------------------------------------------------


def _write_stub_ix(tmp_path: Path, body: str) -> Path:
    """Write an executable stub ``ix`` whose behaviour is the given shell body.

    The body can read ``$@`` (the args after the binary) and write JSON to stdout
    or a message to stderr with a chosen exit code, standing in for the real CLI.
    """
    script = tmp_path / "ix"
    # Resolve bash's absolute path for the shebang: the nix build sandbox has no
    # /usr/bin/env (and no /bin/sh), so a `#!/usr/bin/env bash` stub would fail
    # to exec there ("ix not found"). bash is on PATH via the check's
    # nativeBuildInputs, so shutil.which finds its store path.
    bash = shutil.which("bash") or "/bin/bash"
    script.write_text(f"#!{bash}\n" + body)
    script.chmod(script.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return script


@pytest.fixture
def stub_ix(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> StubIx:
    """Return a factory that installs a stub ix and points the bridge at it.

    For read/act (which now require a peer URL), it also configures a single
    placeholder peer in ``IX_RESOURCE_PEERS`` so the bridge has one URL to target
    -- the stub ignores the ``--peer`` value, exercising the subprocess path with a
    realistic single-peer setup. Tests that need a specific peer list set
    ``PEERS_ENV`` themselves after calling the factory.
    """

    def install(body: str) -> Path:
        script = _write_stub_ix(tmp_path, body)
        monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
        monkeypatch.setenv(rb.PEERS_ENV, "http://peer0")
        return script

    return install


# ---------------------------------------------------------------------------
# list_resources
# ---------------------------------------------------------------------------


def test_list_resources_parses_array(stub_ix: StubIx) -> None:
    entries = [
        {"uri": "ix://nodeA/term1", "name": "term1", "host": "nodeA", "caps": ["read", "write"], "alive": True, "mime": "text/plain"},
        {"uri": "ix://nodeB/widget", "name": "widget", "host": "nodeB", "caps": ["read"], "alive": False, "mime": "text/html"},
    ]
    stub_ix(f"echo {json.dumps(json.dumps(entries))}\n")  # echo the JSON array
    got = asyncio.run(rb.list_resources())
    assert [e.uri for e in got] == ["ix://nodeA/term1", "ix://nodeB/widget"]
    assert got[0].caps == ["read", "write"]
    assert got[1].alive is False
    assert got[1].mime == "text/html"


def test_list_resources_parses_wrapped_object(stub_ix: StubIx) -> None:
    payload = {"resources": [{"uri": "ix://h/n"}]}
    stub_ix(f"echo {json.dumps(json.dumps(payload))}\n")
    got = asyncio.run(rb.list_resources())
    assert len(got) == 1
    assert got[0].uri == "ix://h/n"
    # Defaults applied for the sparse entry.
    assert got[0].mime == "text/plain"
    assert got[0].alive is True
    assert got[0].caps == []


def test_list_resources_passes_peer_flags(stub_ix: StubIx, monkeypatch: pytest.MonkeyPatch) -> None:
    # The stub echoes its own args as a JSON object so we can assert --peer made it.
    stub_ix('printf \'{"resources": []}\'\n')
    monkeypatch.setenv(rb.PEERS_ENV, "http://p1,http://p2")
    # Re-implement: capture args by having the stub write them to a side file.
    # Simpler: drive list_resources with explicit peers and a stub that fails if
    # the flags are absent.
    got = asyncio.run(rb.list_resources(["http://p1"]))
    assert got == []


def test_list_resources_peer_flags_reach_cli(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    argfile = tmp_path / "args.txt"
    body = f'printf "%s\\n" "$@" > {argfile}\nprintf "[]"\n'
    script = _write_stub_ix(tmp_path, body)
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    asyncio.run(rb.list_resources(["http://p1", "http://p2"]))
    args = argfile.read_text().split("\n")
    assert "resources" in args
    assert "ls" in args
    assert "--json" in args
    assert args.count("--peer") == 2
    assert "http://p1" in args
    assert "http://p2" in args


def test_list_resources_empty_when_ix_absent(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # IX_RESOURCES_BIN points at a path that does not exist -> graceful empty list.
    monkeypatch.setenv(rb._IX_BIN_ENV, str(tmp_path / "does-not-exist-ix"))
    assert asyncio.run(rb.list_resources()) == []


def test_list_resources_empty_when_no_ix_on_path(monkeypatch: pytest.MonkeyPatch) -> None:
    # No override and an empty PATH means shutil.which finds nothing -> empty list.
    monkeypatch.delenv(rb._IX_BIN_ENV, raising=False)
    monkeypatch.setenv("PATH", "")
    assert asyncio.run(rb.list_resources()) == []


def test_list_resources_empty_on_nonzero(stub_ix: StubIx) -> None:
    stub_ix('echo "boom" >&2\nexit 3\n')
    assert asyncio.run(rb.list_resources()) == []


def test_list_resources_raises_on_bad_json(stub_ix: StubIx) -> None:
    stub_ix('printf "not json at all"\n')
    with pytest.raises(rb.ResourceBridgeError):
        asyncio.run(rb.list_resources())


# ---------------------------------------------------------------------------
# read_resource
# ---------------------------------------------------------------------------


def test_read_resource_returns_text_and_mime(stub_ix: StubIx) -> None:
    snap = {"uri": "ix://h/n", "text": "hello world", "mime": "text/plain"}
    stub_ix(f"echo {json.dumps(json.dumps(snap))}\n")
    text, mime = asyncio.run(rb.read_resource("ix://h/n"))
    assert text == "hello world"
    assert mime == "text/plain"


def test_read_resource_uses_configured_peer_url(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # No explicit peer: the bridge targets the single configured peer URL, NOT the
    # uri's host (the host is a label and would fail the CLI's URL parse).
    argfile = tmp_path / "args.txt"
    body = f'printf "%s\\n" "$@" > {argfile}\nprintf \'{{"text":"x","mime":"text/plain"}}\'\n'
    script = _write_stub_ix(tmp_path, body)
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.setenv(rb.PEERS_ENV, "https://node-z/rpc")
    asyncio.run(rb.read_resource("ix://nodeZ/term"))
    args = argfile.read_text().split("\n")
    assert "--peer" in args
    assert "https://node-z/rpc" in args  # the configured peer URL, not the uri host
    assert "nodeZ" not in args  # the uri host is never used as a --peer value


def test_read_resource_explicit_peer_wins(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # An explicit peer URL is used directly and overrides any configured peers.
    argfile = tmp_path / "args.txt"
    body = f'printf "%s\\n" "$@" > {argfile}\nprintf \'{{"text":"x"}}\'\n'
    script = _write_stub_ix(tmp_path, body)
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.setenv(rb.PEERS_ENV, "https://configured/rpc")
    asyncio.run(rb.read_resource("ix://nodeZ/term", peer="https://override/rpc"))
    args = argfile.read_text().split("\n")
    assert "https://override/rpc" in args
    assert "https://configured/rpc" not in args  # explicit peer wins over configured
    assert "nodeZ" not in args


def _multi_peer_stub_body(argfile: Path, owner_url: str) -> str:
    """A stub `ix` body that advertises a uri on exactly ONE peer URL.

    It reads the ``--peer`` value out of ``$@`` and, for ``resources get``, emits a
    snapshot only when that value equals ``owner_url`` -- otherwise it exits 1 with
    a resource-not-found message. For ``resources act`` it acks only for the owner.
    This makes the per-peer iteration observable: the bridge must skip the peers
    that 404 and land on the owner. Each invocation logs ONE line
    ``<subcommand>\\t<peer>`` to ``argfile``, so a test can assert exactly which
    (subcommand, peer) pairs ran -- e.g. that ``act`` only ever hit the owner.
    """
    return f"""
peer=""
sub=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--peer" ]; then peer="$a"; fi
  case "$a" in get|act|ls) [ -z "$sub" ] && sub="$a" ;; esac
  prev="$a"
done
printf "%s\\t%s\\n" "$sub" "$peer" >> {argfile}
if [ "$peer" = {owner_url!r} ]; then
  if [ "$sub" = "get" ]; then printf '{{"text":"on-owner","mime":"text/plain"}}'; fi
  if [ "$sub" = "act" ]; then printf '{{"ok":true,"delivered":true,"on":"owner"}}'; fi
  exit 0
fi
echo "resource not found on this peer" >&2
exit 1
"""


def test_read_resource_finds_uri_on_later_peer(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # Peer A (http://a) does NOT have the uri; peer B (http://b) does. The bridge
    # must iterate past A's not-found and return B's snapshot.
    argfile = tmp_path / "args.txt"
    script = _write_stub_ix(tmp_path, _multi_peer_stub_body(argfile, "http://b/rpc"))
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.setenv(rb.PEERS_ENV, "http://a/rpc,http://b/rpc")
    text, mime = asyncio.run(rb.read_resource("ix://h/n"))
    assert text == "on-owner"
    assert mime == "text/plain"
    calls = [line.split("\t") for line in argfile.read_text().splitlines() if line]
    # Both peers were probed via `get` (A first -> 404, then B -> hit).
    assert ["get", "http://a/rpc"] in calls
    assert ["get", "http://b/rpc"] in calls


def _flaky_peer_stub_body(argfile: Path, owner_url: str, down_url: str) -> str:
    """A stub where one peer (``down_url``) ERRORS (transport-style, not a 404).

    ``owner_url`` advertises the uri; ``down_url`` exits 1 with a non-resource
    message (so it maps to a generic ResourceBridgeError, like an unreachable
    peer); any other peer cleanly 404s. Used to prove the iteration steps past an
    *erroring* peer to find a resource a later peer owns.
    """
    return f"""
peer=""
sub=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--peer" ]; then peer="$a"; fi
  case "$a" in get|act|ls) [ -z "$sub" ] && sub="$a" ;; esac
  prev="$a"
done
printf "%s\\t%s\\n" "$sub" "$peer" >> {argfile}
if [ "$peer" = {down_url!r} ]; then
  echo "connection refused" >&2
  exit 1
fi
if [ "$peer" = {owner_url!r} ]; then
  if [ "$sub" = "get" ]; then printf '{{"text":"on-owner","mime":"text/plain"}}'; fi
  if [ "$sub" = "act" ]; then printf '{{"ok":true,"on":"owner"}}'; fi
  exit 0
fi
echo "resource not found on this peer" >&2
exit 1
"""


def test_read_resource_steps_past_erroring_peer(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # Peer A is DOWN (connection refused -> generic bridge error); peer B owns the
    # uri. The bridge must not let A's error abort the probe -- it must still find B.
    argfile = tmp_path / "args.txt"
    script = _write_stub_ix(tmp_path, _flaky_peer_stub_body(argfile, owner_url="http://b/rpc", down_url="http://a/rpc"))
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.setenv(rb.PEERS_ENV, "http://a/rpc,http://b/rpc")
    text, _mime = asyncio.run(rb.read_resource("ix://h/n"))
    assert text == "on-owner"
    calls = [line.split("\t") for line in argfile.read_text().splitlines() if line]
    assert ["get", "http://a/rpc"] in calls  # A was tried and errored
    assert ["get", "http://b/rpc"] in calls  # then B was tried and won


def test_act_steps_past_erroring_peer(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # Same as above for act: a down peer earlier in the list must not stop us from
    # resolving the real owner and sending the keys there.
    argfile = tmp_path / "args.txt"
    script = _write_stub_ix(tmp_path, _flaky_peer_stub_body(argfile, owner_url="http://b/rpc", down_url="http://a/rpc"))
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.setenv(rb.PEERS_ENV, "http://a/rpc,http://b/rpc")
    got = asyncio.run(rb.act("ix://h/n", "x"))
    assert got["on"] == "owner"
    calls = [line.split("\t") for line in argfile.read_text().splitlines() if line]
    assert ["act", "http://b/rpc"] in calls
    assert ["act", "http://a/rpc"] not in calls


def test_read_resource_all_peers_down_is_bridge_error_not_404(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # No peer 404s -- they all ERROR (transport). The terminal error must be a
    # generic ResourceBridgeError, NOT a ResourceNotFoundError (an all-down read is
    # a real failure, never masqueraded as -32002).
    argfile = tmp_path / "args.txt"
    # Owner nobody-has-it, both configured peers are the "down" url pattern.
    body = f"""
peer=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--peer" ]; then peer="$a"; fi
  prev="$a"
done
printf "%s\\n" "$peer" >> {argfile}
echo "connection refused" >&2
exit 1
"""
    script = _write_stub_ix(tmp_path, body)
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.setenv(rb.PEERS_ENV, "http://a/rpc,http://b/rpc")
    with pytest.raises(rb.ResourceBridgeError) as ei:
        asyncio.run(rb.read_resource("ix://h/n"))
    assert not isinstance(ei.value, rb.ResourceNotFoundError)
    # Both peers were attempted before giving up.
    tried = [line for line in argfile.read_text().splitlines() if line]
    assert "http://a/rpc" in tried
    assert "http://b/rpc" in tried


def test_read_resource_not_found_on_any_peer(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # No configured peer advertises the uri -> ResourceNotFoundError (-> -32002).
    argfile = tmp_path / "args.txt"
    script = _write_stub_ix(tmp_path, _multi_peer_stub_body(argfile, "http://nobody/rpc"))
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.setenv(rb.PEERS_ENV, "http://a/rpc,http://b/rpc")
    with pytest.raises(rb.ResourceNotFoundError):
        asyncio.run(rb.read_resource("ix://h/n"))


def test_read_resource_no_peer_configured_raises(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # ix present, but no explicit peer and no IX_RESOURCE_PEERS -> a clear bridge
    # error, NOT a not-found and NOT a silent bare-host shell-out.
    script = _write_stub_ix(tmp_path, 'printf \'{"text":"x"}\'\n')
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.delenv(rb.PEERS_ENV, raising=False)
    with pytest.raises(rb.ResourceBridgeError) as ei:
        asyncio.run(rb.read_resource("ix://h/n"))
    assert not isinstance(ei.value, rb.ResourceNotFoundError)
    assert "no peer configured" in str(ei.value)


def test_read_resource_not_found_maps_to_error(stub_ix: StubIx) -> None:
    stub_ix('echo "resource not found: ix://h/missing" >&2\nexit 1\n')
    with pytest.raises(rb.ResourceNotFoundError):
        asyncio.run(rb.read_resource("ix://h/missing"))


def test_read_resource_empty_body_is_not_found(stub_ix: StubIx) -> None:
    stub_ix("exit 0\n")  # success but no stdout
    with pytest.raises(rb.ResourceNotFoundError):
        asyncio.run(rb.read_resource("ix://h/n"))


def test_read_resource_other_failure_is_bridge_error(stub_ix: StubIx) -> None:
    stub_ix('echo "internal explosion" >&2\nexit 2\n')
    with pytest.raises(rb.ResourceBridgeError) as ei:
        asyncio.run(rb.read_resource("ix://h/n"))
    assert not isinstance(ei.value, rb.ResourceNotFoundError)


@pytest.mark.parametrize(
    "stderr",
    [
        "command not found",  # a wrapper/shell error, not a missing resource
        "unknown peer http://p1",  # a transport/config error
        "unknown flag: --json",  # a CLI usage error
        "connection refused",
    ],
)
def test_read_resource_non_resource_errors_are_not_404(stub_ix: StubIx, stderr: str) -> None:
    # A failure whose message merely contains "not found"/"unknown" but is NOT
    # about a resource must stay a generic bridge error, never get reported as
    # the -32002 resource-not-found code.
    stub_ix(f'echo {json.dumps(stderr)} >&2\nexit 1\n')
    with pytest.raises(rb.ResourceBridgeError) as ei:
        asyncio.run(rb.read_resource("ix://h/n"))
    assert not isinstance(ei.value, rb.ResourceNotFoundError)


def test_looks_not_found_classification() -> None:
    assert rb._looks_not_found("resource not found: ix://h/x")
    assert rb._looks_not_found("no such resource")
    assert rb._looks_not_found("the resource does not exist")
    assert not rb._looks_not_found("command not found")
    assert not rb._looks_not_found("unknown peer")
    assert not rb._looks_not_found("unknown flag --json")


def test_read_resource_missing_ix_raises_bridge_error(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv(rb._IX_BIN_ENV, str(tmp_path / "nope-ix"))
    with pytest.raises(rb.ResourceBridgeError):
        asyncio.run(rb.read_resource("ix://h/n"))


# ---------------------------------------------------------------------------
# act
# ---------------------------------------------------------------------------


def test_act_returns_ack(stub_ix: StubIx) -> None:
    ack = {"uri": "ix://h/n", "ok": True, "delivered": True, "extra": "kept"}
    stub_ix(f"echo {json.dumps(json.dumps(ack))}\n")
    got = asyncio.run(rb.act("ix://h/n", "ls\n"))
    assert got["ok"] is True
    assert got["delivered"] is True
    assert got["extra"] == "kept"  # extra CLI fields are preserved for the agent


def test_act_sends_keys_arg(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    argfile = tmp_path / "args.txt"
    body = f'printf "%s\\n" "$@" > {argfile}\nprintf \'{{"ok":true}}\'\n'
    script = _write_stub_ix(tmp_path, body)
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    asyncio.run(rb.act("ix://h/n", "C-c", peer="http://p"))
    args = argfile.read_text().split("\n")
    assert "act" in args
    assert "--send-keys" in args
    assert "C-c" in args
    assert "http://p" in args


def test_act_empty_body_is_bare_ack(stub_ix: StubIx) -> None:
    stub_ix("exit 0\n")
    got = asyncio.run(rb.act("ix://h/n", "x"))
    assert got["ok"] is True


def test_act_not_found(stub_ix: StubIx) -> None:
    stub_ix('echo "no such resource" >&2\nexit 1\n')
    with pytest.raises(rb.ResourceNotFoundError):
        asyncio.run(rb.act("ix://h/missing", "x"))


def test_act_explicit_peer_skips_probe(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # An explicit peer URL means act goes straight to that peer with no `get` probe
    # first (a single candidate is the caller's assertion). The stub records every
    # subcommand it sees; only `act` should appear.
    argfile = tmp_path / "args.txt"
    body = f'printf "%s\\n" "$@" >> {argfile}\nprintf \'{{"ok":true}}\'\n'
    script = _write_stub_ix(tmp_path, body)
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.setenv(rb.PEERS_ENV, "http://configured/rpc")
    asyncio.run(rb.act("ix://h/n", "x", peer="http://explicit/rpc"))
    args = argfile.read_text().split("\n")
    assert "act" in args
    assert "get" not in args  # no probe, just the act
    assert "http://explicit/rpc" in args
    assert "http://configured/rpc" not in args


def test_act_resolves_owner_peer_before_sending(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # Peer A does NOT own the uri; peer B does. act must probe (get) to find B, then
    # send the keys ONLY to B -- never act against A, the wrong host.
    argfile = tmp_path / "args.txt"
    script = _write_stub_ix(tmp_path, _multi_peer_stub_body(argfile, "http://b/rpc"))
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.setenv(rb.PEERS_ENV, "http://a/rpc,http://b/rpc")
    got = asyncio.run(rb.act("ix://h/n", "ls\n"))
    assert got["ok"] is True
    assert got["on"] == "owner"  # the ack came from B (the owner), not A
    calls = [line.split("\t") for line in argfile.read_text().splitlines() if line]
    # The `act` landed ONLY on the owner (B); A was probed but never acted on.
    assert ["act", "http://b/rpc"] in calls
    assert ["act", "http://a/rpc"] not in calls
    # A was reached only as a `get` probe (to discover it does not own the uri).
    assert ["get", "http://a/rpc"] in calls


def test_act_no_peer_configured_raises(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    script = _write_stub_ix(tmp_path, 'printf \'{"ok":true}\'\n')
    monkeypatch.setenv(rb._IX_BIN_ENV, str(script))
    monkeypatch.delenv(rb.PEERS_ENV, raising=False)
    with pytest.raises(rb.ResourceBridgeError) as ei:
        asyncio.run(rb.act("ix://h/n", "x"))
    assert not isinstance(ei.value, rb.ResourceNotFoundError)
    assert "no peer configured" in str(ei.value)
