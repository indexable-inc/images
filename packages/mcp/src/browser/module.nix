{
  bundledSource,
  bundledTestPython,
  fontsConf,
  ixNotebookMcpModule,
  lib,
  pkgs,
  playwrightBrowsers,
}: let
  # Browser automation over CDP: `import browser`, then `await browser.goto(url)`
  # / `await browser.shot()` drive a Chromium-family browser already running with
  # --remote-debugging-port (the standard 9222 by default). Pure Python over the
  # bundled playwright (already in this interpreter, so no `pip`/`playwright
  # install`); runs on the kernel loop and returns the raw Playwright objects plus
  # a screenshot Result. Cross-platform.
  browserPythonSource = bundledSource {
    name = "ix-mcp-browser-python-source";
    path = ./.;
  };
  browserModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-browser-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [ixNotebookMcpModule];
      meta.description = "Playwright-over-CDP browser helper bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/browser"
      mkdir -p "$site"
      cp -r ${browserPythonSource}/browser/. "$site/"
    ''
  );
  # The browser module: it drives a Chromium-family browser over CDP with the
  # bundled playwright. A real browser needs a display, and we NEVER run headless,
  # so the sandbox cannot launch one; instead this asserts the contract that does
  # not need a browser -- the API shape, the standard/persistent defaults, the
  # never-headless launch argv, the clear error when nothing is on the port, and
  # that api() now lists both the module and the bundled playwright library.
  browserTestPy = pkgs.writeText "ix-mcp-browser-test.py" ''
    # python
    import asyncio
    import sys

    import browser
    from ix_notebook_mcp import runtime

    # Standard CDP port + a persistent, module-owned profile, so repeat launches
    # reuse one instance instead of spawning a new window each time.
    assert browser.DEFAULT_ENDPOINT == "http://127.0.0.1:9222", browser.DEFAULT_ENDPOINT
    assert browser.DEFAULT_APP == "Google Chrome", browser.DEFAULT_APP
    for fn in ("get_or_create_browser", "connect", "context", "page", "goto", "shot", "read", "vdom", "close"):
        assert callable(getattr(browser, fn)), fn
    # `vdom()` returns a Vdom: a clean, filtered, machine-readable map of the page.
    assert isinstance(browser.Vdom, type), browser.Vdom

    udd = browser._default_user_data_dir(browser.DEFAULT_APP)
    assert udd.endswith(".cdp-google-chrome-profile"), udd
    argv = browser._launch_argv(browser.DEFAULT_APP, 9222, udd)
    # The launched browser is ALWAYS a visible window -- never headless.
    assert not any("headless" in a for a in argv), ("launch must never be headless", argv)
    assert "--remote-debugging-port=9222" in argv, argv
    assert ("--user-data-dir=" + udd) in argv, argv
    if sys.platform == "darwin":
        assert argv[:3] == ["open", "-na", "Google Chrome"], argv
    assert browser._port_of("http://127.0.0.1:9222") == 9222

    async def _dead():
        # Nothing is listening on port 1: connect() must fail clearly and point at
        # get_or_create_browser() (which would launch one) rather than hang.
        try:
            await browser.connect("http://127.0.0.1:1")
        except ConnectionError as exc:
            assert "get_or_create_browser" in str(exc), exc
        else:
            raise SystemExit("connect() to a dead port should raise ConnectionError")

    asyncio.run(_dead())

    # Discoverability: api() lists the browser module AND playwright as a bundled
    # library, so neither looks absent to an agent treating api() as the catalog.
    rows = runtime._api_rows()
    wheres = {r["where"] for r in rows}
    assert "browser" in wheres, ("browser module missing from api()", sorted(wheres))
    libs = {r["name"] for r in rows if r["where"] == "library"}
    assert "playwright" in libs, ("playwright not listed as a bundled library", sorted(libs))

    # shot() cost controls (no browser needed -- _encode_shot is a pure helper):
    # a model-bound shot caps its longest edge and re-encodes, so a full-res 2x
    # capture cannot flood context. Build a busy 1746x2406 PNG (the size the
    # friction report measured) and check each path.
    import io as _io
    import os as _os

    from PIL import Image as _Image

    _src = _Image.frombytes("RGB", (1746, 2406), _os.urandom(1746 * 2406 * 3))
    _buf = _io.BytesIO()
    _src.save(_buf, format="PNG")
    _raw = _buf.getvalue()

    # Default model path: JPEG, longest edge -> _SHOT_MAX_DIM (1024).
    _data, _mime = browser._encode_shot(
        _raw, max_dim=browser._SHOT_MAX_DIM, fmt="jpeg", quality=72
    )
    assert _mime == "image/jpeg", _mime
    assert max(_Image.open(_io.BytesIO(_data)).size) == browser._SHOT_MAX_DIM, (
        _Image.open(_io.BytesIO(_data)).size
    )
    # A busy screenshot as JPEG is far smaller than the raw full-res PNG.
    assert len(_data) < len(_raw) // 10, (len(_data), len(_raw))

    # PNG path also downscales the longest edge.
    _pdata, _pmime = browser._encode_shot(_raw, max_dim=1024, fmt="png", quality=72)
    assert _pmime == "image/png", _pmime
    assert max(_Image.open(_io.BytesIO(_pdata)).size) == 1024

    # max_dim=0 + png is an exact passthrough (no needless re-encode).
    _ndata, _nmime = browser._encode_shot(_raw, max_dim=0, fmt="png", quality=72)
    assert _ndata is _raw and _nmime == "image/png", (_nmime, _ndata is _raw)

    # Never raises: junk bytes come back untouched rather than blowing up a shot.
    _gdata, _gmime = browser._encode_shot(b"not an image", max_dim=1024, fmt="jpeg", quality=72)
    assert _gdata == b"not an image" and _gmime == "image/png", (_gmime, _gdata)

    # shot() validates its enum-ish knobs up front.
    import asyncio as _aio
    for _bad in (dict(format="webp"), dict(scale="2x")):
        try:
            _aio.run(browser.shot(**_bad))
        except ValueError:
            pass
        else:
            raise SystemExit(f"shot({_bad}) should raise ValueError")

    # --- live dashboard resource -------------------------------------------
    # A connected browser publishes itself as a live resource: a throttled
    # screenshot of the front tab. No real Chromium needed -- fake the context.
    EP = "http://127.0.0.1:9222"

    class _FakePage:
        url = "https://example.com/"

        last_screenshot_kw = {}

        async def title(self):
            return "Example"

        async def screenshot(self, **_kw):
            _FakePage.last_screenshot_kw = _kw
            return b"NOT-A-REAL-PNG"  # _encode_shot tolerates non-images

    class _FakeCtx:
        def __init__(self, pages):
            self.pages = pages

    class _FakeBrowser:
        def __init__(self):
            self.connected = True

        def is_connected(self):
            return self.connected

    _orig_context = browser.context
    _pages = [_FakePage()]

    async def _fake_context(endpoint=EP):
        return _FakeCtx(_pages)

    browser.context = _fake_context

    # A page renders to an inline <img> with its title/url.
    browser._resource_html_cache.clear()
    _h = _aio.run(browser._resource_html(EP))
    assert "<img" in _h and "example.com" in _h, _h[:200]

    # Passive capture uses device scale, never css: css scale makes Playwright
    # push a per-shot DPR Emulation override that relayouts and visibly flickers
    # the live HiDPI window on every ~1.5s tick of this loop.
    assert _FakePage.last_screenshot_kw.get("scale") == "device", _FakePage.last_screenshot_kw

    # Throttled: a call within the TTL reuses the cache even though the tab list
    # changed underneath it (the screenshot is the expensive part).
    _pages.clear()
    assert _aio.run(browser._resource_html(EP)) == _h

    # No open tabs: a passive placeholder, and never creates a tab.
    browser._resource_html_cache.clear()
    assert "no open tabs" in _aio.run(browser._resource_html(EP))

    # Render never raises: a failing capture becomes an error card.
    async def _boom(endpoint=EP):
        raise RuntimeError("kaboom")

    browser.context = _boom
    browser._resource_html_cache.clear()
    _e = _aio.run(browser._resource_html(EP))
    assert "render failed" in _e and "kaboom" in _e, _e[:200]

    # connect() publishes the resource on a fresh connection; mimic that here.
    browser.context = _fake_context
    _pages[:] = [_FakePage()]
    runtime.resources.clear()
    browser._browsers.clear()
    _fb = _FakeBrowser()
    browser._browsers[EP] = _fb
    _res = browser._register_resource(EP)
    _rid = "browser:" + EP
    assert _res is not None and _rid in runtime.resources, list(runtime.resources)
    assert _res.kind == "browser" and _res.title == "browser · " + EP, (_res.kind, _res.title)
    assert _res.alive() is True
    browser._resource_html_cache.clear()
    assert "<img" in _aio.run(_res.render_html())

    # Keyed by endpoint: a reconnect refreshes the one card, never stacks.
    browser._register_resource(EP)
    assert sum(1 for k in runtime.resources if k == _rid) == 1

    # alive() drops the card once the connection is gone (the sweep then closes it).
    _fb.connected = False
    assert _res.alive() is False
    _fb.connected = True
    browser._browsers.pop(EP)
    assert _res.alive() is False

    # Leave the module clean for any later assertions.
    browser.context = _orig_context
    browser._browsers.clear()
    runtime.resources.clear()
    browser._resource_html_cache.clear()

    print("browser-ok", browser.__version__)
  '';
  # Shared by browserSmoke and browserVdomSmoke below.
  browserTestPython = bundledTestPython [browserModule];
  browserSmoke =
    pkgs.runCommand "ix-mcp-browser-smoke"
    {
      nativeBuildInputs = [browserTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe browserTestPython} ${browserTestPy} >stdout 2>stderr || {
        echo "ix-mcp browser smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^browser-ok' stdout || {
        echo "ix-mcp browser smoke did not confirm the browser module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  # The clean-vdom contract, exercised against a real (headless) browser. Unlike
  # the launch smoke above, `vdom()` only reads the DOM, so it can run headless on
  # a `data:` fixture with no display and no network: it asserts the filtering
  # (hidden / aria-hidden pruned), the wrapper-chain collapse, that every kept node
  # has geometry and a CSS selector that actually resolves, and that the
  # interactive_only / viewport_only lean modes behave.
  browserVdomTestPy = pkgs.writeText "ix-mcp-browser-vdom-test.py" ''
    # python
    import asyncio
    import sys

    import browser
    from playwright.async_api import async_playwright

    # A fixture page exercising the cleaning rules: landmarks, a collapsible wrapper
    # chain, hidden + aria-hidden subtrees, a named image, and a form. Served as a
    # data: URL so the test needs no network and no on-screen window.
    FIXTURE = (
        "data:text/html," + (
            "<html><head><title>Fixture</title></head><body>"
            "<header><a id='home' href='/'><img alt='Logo'></a>"
            "<nav><a href='/a'>Alpha</a><a href='/b'>Beta</a></nav></header>"
            "<main><h1>Heading</h1><p>Visible paragraph text.</p>"
            "<div><div><div><button id='go' onclick='void 0'>Click me</button></div></div></div>"
            "<form><input type='search' placeholder='Find'><button type='submit'>Go</button></form>"
            "<div style='display:none'><a href='/hidden'>Hidden</a></div>"
            "<span aria-hidden='true'><a href='/aria'>AriaHidden</a></span>"
            "</main></body></html>"
        )
    )


    def names(flat):
        return {n.get("name") for n in flat if not n.get("group")}


    def by_tag(flat, tag):
        return [n for n in flat if n.get("tag") == tag and not n.get("group")]


    async def main():
        pw = await async_playwright().start()
        b = await pw.chromium.launch(
            headless=True, args=["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage"]
        )
        ctx = await b.new_context(viewport={"width": 1000, "height": 800})
        pg = await ctx.new_page()
        await pg.goto(FIXTURE)

        v = await browser.vdom(pg)

        # It is the documented type, with the page's identity captured.
        assert isinstance(v, browser.Vdom), type(v)
        assert v.title == "Fixture", v.title
        assert v.viewport.get("w") == 1000, v.viewport

        ns = names(v.flat)
        # Hidden (display:none) and aria-hidden subtrees are pruned entirely.
        assert "Hidden" not in ns, ns
        assert "AriaHidden" not in ns, ns
        # Landmarks, heading, controls and the named image survive.
        assert any(n.get("role") == "banner" for n in v.flat), "banner landmark missing"
        assert any(n.get("role") == "navigation" for n in v.flat), "nav landmark missing"
        assert any(n.get("role") == "main" for n in v.flat), "main landmark missing"
        assert any(n.get("role") == "heading" and n.get("name") == "Heading" for n in v.flat), ns
        assert "Logo" in ns, ("named image missing", ns)
        assert any(n.get("name") == "Alpha" and n.get("interactive") for n in v.flat), ns

        # The triple-nested wrapper <div><div><div> around the button is collapsed:
        # the button is reached without a chain of empty group nodes above it.
        btn = next(n for n in v.flat if n.get("tag") == "button" and n.get("name") == "Click me")
        assert btn["depth"] <= 2, ("wrapper chain not collapsed", btn["depth"])

        # Every kept node carries a usable on-screen box and a working CSS selector.
        for n in v.flat:
            if n.get("group"):
                continue
            assert n.get("w", 0) > 0 and n.get("h", 0) > 0, ("no geometry", n)
            sel = n.get("selector")
            assert sel, ("no selector", n)
            assert await pg.query_selector(sel) is not None, ("selector did not resolve", sel)

        # Refs are dense and 1-based; node(ref) round-trips; df/json agree on counts.
        refs = [n["ref"] for n in v.flat if not n.get("group")]
        assert refs == list(range(1, len(refs) + 1)), refs
        assert v.node(refs[-1]) is not None
        n_real = len(refs)
        assert v.df.height == len(v.flat), (v.df.height, len(v.flat))
        assert v.df.filter(v.df["interactive"]).height >= 4  # 4 links + 2 buttons + 1 field

        # The compact glance is bounded and self-describes; the full map lives in .df.
        txt = repr(v)
        assert "Fixture" in txt and "nodes" in txt, txt[:200]

        # interactive_only drops body text but keeps the controls.
        vi = await browser.vdom(pg, interactive_only=True)
        nsi = names(vi.flat)
        assert "Visible paragraph text." not in nsi, nsi
        assert any(n.get("name") == "Alpha" for n in vi.flat), nsi

        # viewport_only keeps only on-screen nodes (all fixture nodes are on screen,
        # so it must still find the controls -- and never error).
        vvp = await browser.vdom(pg, viewport_only=True)
        assert any(n.get("interactive") for n in vvp.flat), "viewport_only lost controls"

        await b.close()
        await pw.stop()
        print("vdom-ok", browser.__version__, n_real, "nodes")


    # A sandboxed headless chromium occasionally tears down mid-run
    # (TargetClosedError); that is environment flake, not a vdom regression, so
    # retry the whole run a couple of times before failing the gate.
    for attempt in range(3):
        try:
            asyncio.run(main())
            break
        except Exception as exc:
            if attempt == 2 or "closed" not in str(exc).lower():
                raise
            print(f"retry {attempt + 1}: transient browser teardown: {exc}", file=sys.stderr)
  '';
  browserVdomSmoke =
    pkgs.runCommand "ix-mcp-browser-vdom-smoke"
    {
      nativeBuildInputs = [browserTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      # `vdom()` launches a (headless) browser, so point Playwright at the bundled
      # browser bundle -- the bare test env has no wrapper to set this (only the
      # `ix-mcp` entrypoint does).
      export PLAYWRIGHT_BROWSERS_PATH=${lib.escapeShellArg playwrightBrowsers}
      export FONTCONFIG_FILE=${fontsConf}
      ${lib.getExe browserTestPython} ${browserVdomTestPy} >stdout 2>stderr || {
        echo "ix-mcp browser vdom smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^vdom-ok' stdout || {
        echo "ix-mcp browser vdom smoke did not confirm the clean vdom:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = browserModule;
  tests = {
    inherit
      browserSmoke
      browserVdomSmoke
      ;
  };
}
