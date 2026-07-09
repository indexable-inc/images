"""Tool results as live weave views (weave2 n-toolviews).

A finished cell whose Result carries a human HTML view persists that html to
CAS once and asserts a cas-html view entity child_of the run - the contract
pinned in weave crates/store/src/views.dl - and the view id rides the job
summary so the server attaches it to the tool result's `_meta` under
mcp_ui.WEAVE_VIEW_META_KEY. With WEAVE_URL=off nothing is minted and the
reply metadata is byte-identical to before the feature existed.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from ix_notebook_mcp import runtime, store


def _wire(monkeypatch: pytest.MonkeyPatch, conn: store.WeaveStore) -> None:
    monkeypatch.setattr(runtime, "_store", store)
    monkeypatch.setattr(runtime, "_store_conn", conn)


# --------------------------------------------------------------------------- #
# store.save_tool_view: the cas-html view fact shape, exactly as pinned.
# --------------------------------------------------------------------------- #


def test_save_tool_view_asserts_pinned_contract_shape(tmp_path: Path, fake_weave: object) -> None:
    html = "<table><tr><td>1</td></tr></table>"
    conn = store.connect(tmp_path / "views.ixnb")
    ent = store.save_tool_view(conn, id="ab12", html=html, label="count rows")
    conn.close()

    assert ent == "view:ab12"
    # Exactly one CAS blob, holding the html verbatim.
    assert list(fake_weave.blobs.values()) == [html.encode("utf-8")]
    (digest,) = fake_weave.blobs
    assert fake_weave.facts[("view:ab12", "type")] == "view"
    assert fake_weave.facts[("view:ab12", "renderer")] == "cas-html"
    assert fake_weave.facts[("view:ab12", "body")] == digest
    assert fake_weave.facts[("view:ab12", "label")] == "count rows"
    # Lineage is child_of the run (from_msg is for chat-driven views), so
    # cascade cleanup and the sidebar's lineage chip follow the run entity.
    assert fake_weave.facts[("view:ab12", "child_of")] == "run:ab12"
    # The body rides as a typed hash value, like every other CAS blob ref.
    body_writes = [
        item["fact"]
        for item in fake_weave.writes
        if item.get("fact", {}).get("attr") == "body"
    ]
    assert [f["value"]["t"] for f in body_writes] == ["hash"]


def test_save_tool_view_off_mints_nothing(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "off")

    def boom(*args: object, **kwargs: object) -> object:
        raise AssertionError("WEAVE_URL=off must never reach the network")

    monkeypatch.setattr(store, "_http_json", boom)
    monkeypatch.setattr(store, "_http_bytes", boom)
    conn = store.connect(tmp_path / "off.ixnb")
    assert store.save_tool_view(conn, id="ab12", html="<p>hi</p>", label="x") is None
    conn.close()


# --------------------------------------------------------------------------- #
# runtime plumbing: _persist_final mints the view, the job summary carries it.
# --------------------------------------------------------------------------- #


def test_persist_final_mints_one_view_and_summary_carries_it(
    tmp_path: Path, fake_weave: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    html = "<table><tr><td>1</td></tr></table>"
    conn = store.connect(tmp_path / "run.ixnb")
    _wire(monkeypatch, conn)
    job = runtime.Job("x = 1", name="count rows")
    job._result = runtime.Result(user_html=html, llm_result="1")
    job.status = "done"
    job.ended = job.started + 1.0

    runtime._persist_final(job)
    conn.close()

    view = f"view:{job.id}"
    assert job.weave_view == view
    # The summary is what the server reads the id back out of.
    assert runtime._job_summary(job)["weave_view"] == view
    # Exactly one CAS blob holds the html (finish()'s result/outputs/bindings
    # blobs are distinct payloads).
    html_blobs = [b for b in fake_weave.blobs.values() if b == html.encode("utf-8")]
    assert len(html_blobs) == 1
    assert fake_weave.facts[(view, "renderer")] == "cas-html"
    assert fake_weave.facts[(view, "body")] == hashlib.sha256(html.encode("utf-8")).hexdigest()
    assert fake_weave.facts[(view, "label")] == "count rows"
    assert fake_weave.facts[(view, "child_of")] == f"run:{job.id}"


def test_persist_final_skips_replays_and_htmlless_results(
    tmp_path: Path, fake_weave: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    conn = store.connect(tmp_path / "skip.ixnb")
    _wire(monkeypatch, conn)
    # A replayed cell already minted its view in the session that first ran it;
    # minting again on reopen would duplicate the tab.
    replay = runtime.Job("x = 1", name="again", kind="replay")
    replay._result = runtime.Result(user_html="<p>hi</p>", llm_result="hi")
    replay.status = "done"
    replay.ended = replay.started
    # A text-only result has no human view to share.
    plain = runtime.Job("2 + 2", name="sum")
    plain._result = runtime.Result(llm_result="4")
    plain.status = "done"
    plain.ended = plain.started

    runtime._persist_final(replay)
    runtime._persist_final(plain)
    conn.close()

    assert replay.weave_view is None
    assert plain.weave_view is None
    assert runtime._job_summary(plain)["weave_view"] is None
    assert not [e for (e, _a) in fake_weave.facts if e.startswith("view:")]


def test_persist_final_off_leaves_job_and_summary_unchanged(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("WEAVE_URL", "off")
    conn = store.connect(tmp_path / "off.ixnb")
    _wire(monkeypatch, conn)
    job = runtime.Job("x = 1", name="count rows")
    job._result = runtime.Result(user_html="<p>hi</p>", llm_result="hi")
    job.status = "done"
    job.ended = job.started

    runtime._persist_final(job)
    conn.close()

    assert job.weave_view is None
    assert runtime._job_summary(job)["weave_view"] is None
