"""Property-based tests for the browser module's accessibility-snapshot helpers.

`browser.vdom()` walks a live DOM and returns a clean, filtered tree (roles,
accessible names, on-screen boxes, a CSS selector per node), pruning
script/style/hidden/aria-hidden noise and collapsing wrapper <div> chains;
`browser.read()` is the text-first wrapper. These tests defend the snapshot's
core promise -- that it is a faithful, actionable map -- over randomly generated
HTML bodies via Hypothesis.

The properties asserted (the subset that is honestly guaranteed by the
implementation):

1. Selector integrity: every non-group node's non-empty selector resolves to
   exactly one element (the central promise Playwright relies on).
2. Exclusion: no sentinel inside a display:none / visibility:hidden /
   aria-hidden / <script> / <style> subtree, nor an element with opacity:0 set
   directly on it, surfaces as a node DERIVED FROM the excluded element (its own
   tag+name) or on any interactive node. (Text from an excluded child CAN leak
   into an ancestor landmark's innerText-derived accessible name; that is
   documented behavior and is not asserted away. opacity:0 on an *ancestor* is
   deliberately NOT in the excluded set -- see indexable-inc/index#1077, where
   the implementation keeps such subtrees; asserting their exclusion would be
   dishonest, so this test pins opacity:0 only when set directly on the element.)
3. Name clamping: every node name and attrs value is <= max_text (default 120),
   and a smaller max_text is honored.
4. Ref contiguity & lookup: non-group nodes are numbered 1..N in document order
   with no gaps/dupes; group nodes have ref None; node(ref) round-trips and
   excludes tree-walk keys (children/group/depth).
5. Determinism: vdom() twice on the same static page yields identical .json.
6. interactive_only subset: interactive nodes from vdom(interactive_only=True)
   are a subset (by selector) of those from full vdom().
7. Geometry: every node has integer x,y,w,h with w>=0 and h>=0.
8. read() agreement: read()'s "interactive (N)" count equals
   min(200, #interactive nodes in vdom(interactive_only=True)).

The browser launches once per module (a single headless Chromium, one reused
page set_content'd per example) so the suite stays fast.
"""

from __future__ import annotations

import asyncio
import json
import re

import pytest

# browser + playwright are interpreter-bundled modules; skip cleanly rather than
# error if this test is collected under a bare interpreter without them.
browser = pytest.importorskip("browser")
async_playwright = pytest.importorskip("playwright.async_api").async_playwright
hypothesis = pytest.importorskip("hypothesis")

from hypothesis import HealthCheck, given, settings  # noqa: E402
from hypothesis import strategies as st  # noqa: E402

MAX_TEXT_DEFAULT = 120


# ---------------------------------------------------------------------------
# A module-scoped headless browser + one reused page. Launching per Hypothesis
# example would be unbearably slow, so we set_content per example instead.
# ---------------------------------------------------------------------------


class _Browser:
    """One headless Chromium + one page, driven on a private event loop so the
    sync Hypothesis test body can `run(...)` each example against it."""

    def __init__(self):
        self.loop = asyncio.new_event_loop()
        self._pw = None
        self._browser = None
        self._ctx = None
        self.page = None

    def run(self, coro):
        return self.loop.run_until_complete(coro)

    def start(self):
        async def _start():
            self._pw = await async_playwright().start()
            self._browser = await self._pw.chromium.launch(
                headless=True,
                args=["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage"],
            )
            self._ctx = await self._browser.new_context(
                viewport={"width": 1000, "height": 800}
            )
            self.page = await self._ctx.new_page()

        self.run(_start())

    def stop(self):
        async def _stop():
            if self._browser is not None:
                await self._browser.close()
            if self._pw is not None:
                await self._pw.stop()

        try:
            self.run(_stop())
        finally:
            self.loop.close()


@pytest.fixture(scope="module")
def live():
    b = _Browser()
    b.start()
    try:
        yield b
    finally:
        b.stop()


# ---------------------------------------------------------------------------
# A Hypothesis strategy for random-but-valid HTML document bodies: nested
# wrappers, interactive controls, landmarks, headings, text runs, named images,
# and deliberately-excluded subtrees carrying unique sentinel strings.
# ---------------------------------------------------------------------------

_uniq = st.integers(min_value=0, max_value=10**9)


def _interactive(sid: int):
    return st.sampled_from(
        [
            f"<a href='/p{sid}'>Link{sid}</a>",
            f"<button>Btn{sid}</button>",
            f"<input type='text' placeholder='In{sid}'>",
            "<input type='checkbox'>",
            f"<select><option>Opt{sid}</option></select>",
            f"<textarea placeholder='Ta{sid}'></textarea>",
        ]
    )


_LANDMARKS = ["main", "nav", "header", "footer", "section"]
_HEADINGS = ["h1", "h2", "h3"]

# Each excluder both renders an excluded subtree AND records, per sentinel, the
# kind of exclusion. Every kind here is one the implementation actually prunes
# (the subtree never surfaces as a node derived from the excluded element):
#  - display:none / visibility:hidden / aria-hidden wrappers cascade to the
#    descendant's own computed state, so the child <a> is dropped.
#  - <script>/<style> tags are in SKIP_TAGS.
#  - opacity:0 set DIRECTLY on the element zeroes its own computed opacity.
# opacity:0 on an *ancestor* is intentionally absent: it is NOT pruned today
# (indexable-inc/index#1077), so including it would assert a false guarantee.
def _excluded_subtree(sid: int):
    sent = f"SENTINEL{sid}X"

    def link(wrap_kind: str, css: str):
        return (
            f"<div style='{css}'><a href='/e{sid}'>{sent}{wrap_kind}</a></div>",
            {f"{sent}{wrap_kind}": wrap_kind},
        )

    options = [
        link("none", "display:none"),
        link("vishidden", "visibility:hidden"),
        (
            f"<div aria-hidden='true'><a href='/e{sid}'>{sent}ariahidden</a></div>",
            {f"{sent}ariahidden": "ariahidden"},
        ),
        (
            f"<a href='/e{sid}' style='opacity:0'>{sent}opacitydirect</a>",
            {f"{sent}opacitydirect": "opacitydirect"},
        ),
        (f"<script>var x='{sent}script';</script>", {f"{sent}script": "script"}),
        (f"<style>/*{sent}style*/</style>", {f"{sent}style": "style"}),
    ]
    return st.sampled_from(options)


@st.composite
def _body(draw, depth=0):
    """Return (html, {sentinel: excluded_kind})."""
    parts: list[str] = []
    excluded: dict[str, str] = {}
    for _ in range(draw(st.integers(min_value=1, max_value=5))):
        sid = draw(_uniq)
        choice = draw(st.integers(min_value=0, max_value=99))
        if choice < 25 and depth < 3:
            inner, exc = draw(_body(depth=depth + 1))
            for _ in range(draw(st.integers(min_value=1, max_value=4))):
                inner = f"<div>{inner}</div>"
            parts.append(inner)
            excluded.update(exc)
        elif choice < 45:
            parts.append(draw(_interactive(sid)))
        elif choice < 55 and depth < 4:
            tag = draw(st.sampled_from(_LANDMARKS))
            inner, exc = draw(_body(depth=depth + 2))
            parts.append(f"<{tag}>{inner}</{tag}>")
            excluded.update(exc)
        elif choice < 65:
            tag = draw(st.sampled_from(_HEADINGS))
            parts.append(f"<{tag}>Head{sid}</{tag}>")
        elif choice < 75:
            parts.append(f"<p>Para text {sid} lorem ipsum dolor.</p>")
        elif choice < 82:
            parts.append(f"<img alt='Img{sid}'>")
        else:
            frag, exc = draw(_excluded_subtree(sid))
            parts.append(frag)
            excluded.update(exc)
    return "".join(parts), excluded


# max_text is drawn small sometimes to exercise the clamp opt.
_doc = st.tuples(_body(), st.sampled_from([MAX_TEXT_DEFAULT, 40]))


# ---------------------------------------------------------------------------
# The per-example assertion, run on the browser's loop against the reused page.
# ---------------------------------------------------------------------------


async def _assert_invariants(page, body: str, excluded: dict, max_text: int):
    await page.set_content(
        f"<!doctype html><html><head><title>T</title></head><body>{body}</body></html>"
    )
    v = await browser.vdom(page, max_text=max_text)
    assert isinstance(v, browser.Vdom)

    non_group = [n for n in v.flat if not n.get("group")]

    # 1. Selector integrity.
    for n in non_group:
        sel = n.get("selector")
        if sel:
            cnt = await page.evaluate(
                "(s) => document.querySelectorAll(s).length", sel
            )
            assert cnt == 1, ("selector not 1:1", sel, cnt)

    # 2. Exclusion, honestly scoped: no sentinel surfaces as a node derived from
    # the excluded element itself (its own tag+name) or on any interactive node.
    # Leakage into an ancestor landmark's innerText-derived name is allowed.
    for sent in excluded:
        for n in non_group:
            name = n.get("name") or ""
            if sent in name:
                assert n.get("tag") not in ("a", "text"), (
                    "excluded element surfaced as its own node",
                    sent,
                    n.get("tag"),
                    name[:40],
                )
                assert not n.get("interactive"), (
                    "excluded content became an actionable node",
                    sent,
                    n.get("tag"),
                    name[:40],
                )

    # 3. Name clamping (names and attrs values), honoring max_text.
    for n in v.flat:
        assert len(n.get("name") or "") <= max_text, ("name too long", n.get("name"))
        for k, val in (n.get("attrs") or {}).items():
            if isinstance(val, str):
                assert len(val) <= max_text, ("attr too long", k, len(val))

    # 4. Ref contiguity & node() lookup.
    refs = [n["ref"] for n in non_group]
    assert refs == list(range(1, len(refs) + 1)), ("refs not 1..N dense", refs)
    for n in v.flat:
        if n.get("group"):
            assert n.get("ref") is None, ("group node has a ref", n.get("ref"))
    if refs:
        nd = v.node(refs[-1])
        assert nd is not None and nd.get("ref") == refs[-1]
        for pruned in ("children", "group", "depth"):
            assert pruned not in nd, ("node() leaked tree-walk key", pruned)
    assert v.node(max(refs, default=0) + 1) is None  # out-of-range -> None

    # 7. Geometry: integer x,y,w,h with w,h >= 0.
    for n in v.flat:
        for ax in ("x", "y", "w", "h"):
            assert isinstance(n.get(ax), int), ("geometry not int", ax, n.get(ax))
        assert (n.get("w") or 0) >= 0 and (n.get("h") or 0) >= 0, ("negative size", n)

    # 5. Determinism: a second snapshot of the same static page matches .json.
    v2 = await browser.vdom(page, max_text=max_text)
    assert json.dumps(v.json, sort_keys=True) == json.dumps(v2.json, sort_keys=True)

    # 6. interactive_only is a subset (by selector) of the full snapshot.
    full_int = {
        n.get("selector")
        for n in v.flat
        if n.get("interactive") and n.get("selector")
    }
    io = await browser.vdom(page, interactive_only=True, max_text=max_text)
    io_int = {
        n.get("selector")
        for n in io.flat
        if n.get("interactive") and n.get("selector")
    }
    assert io_int <= full_int, ("interactive_only not a subset", io_int - full_int)

    # 8. read() agreement: its "interactive (N)" header equals
    # min(200, #interactive nodes in vdom(interactive_only=True)).
    r = await browser.read(page)
    text = r.llm_result if hasattr(r, "llm_result") else r
    expected = min(200, sum(1 for n in io.flat if n.get("interactive")))
    m = re.search(r"## interactive \((\d+)\)", text)
    assert m is not None, ("read() has no interactive header", text[:200])
    assert int(m.group(1)) == expected, ("read() count disagrees", int(m.group(1)), expected)


@settings(
    max_examples=40,
    deadline=None,  # browser layout is variable-latency
    suppress_health_check=[HealthCheck.function_scoped_fixture, HealthCheck.too_slow],
)
@given(doc=_doc)
def test_vdom_invariants(live, doc):
    (body, excluded), max_text = doc
    live.run(_assert_invariants(live.page, body, excluded, max_text))


def test_max_text_smaller_opt_is_honored(live):
    """A long accessible name is clamped to a smaller max_text when one is passed,
    independent of the random sweep (a direct check of the maxText knob)."""

    async def _check():
        long = "WORD " * 60  # ~300 chars, far over any cap
        await live.page.set_content(
            f"<!doctype html><html><head><title>T</title></head>"
            f"<body><main><button>{long}</button></main></body></html>"
        )
        for cap in (120, 30, 10):
            v = await browser.vdom(live.page, max_text=cap)
            btn = next(n for n in v.flat if n.get("tag") == "button")
            assert len(btn["name"]) <= cap, (cap, len(btn["name"]))
            # the clamp marker proves it actually truncated rather than fit.
            assert btn["name"].endswith("…"), btn["name"]

    live.run(_check())
