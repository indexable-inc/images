{
  bundledSource,
  bundledTestPython,
  importTest,
  ixNotebookMcpModule,
  lib,
  pkgs,
}: let
  # Native macOS VM control, bundled like `screen` so every session can
  # `import vmkit` on Darwin. Pure Python: it spawns the `vmkit` binary (a
  # Rust binding over Virtualization.framework) and returns guest screenshots as
  # PIL images. macOS-only; on a non-Darwin platform the module raises.
  vmkitPythonSource = bundledSource {
    name = "ix-mcp-vmkit-python-source";
    path = ./.;
  };
  vmkitModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-vmkit-python-module"
    {
      strictDeps = true;
      meta.description = "Native macOS VM control bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/vmkit"
      mkdir -p "$site"
      cp -r ${vmkitPythonSource}/vmkit/. "$site/"
    ''
  );
  vmkitBundled = importTest [vmkitModule] "vmkit" "import vmkit; print('vmkit-ok', callable(vmkit.boot_linux), callable(vmkit.drive), callable(vmkit.screenshot))";
  # The vmkit guest surfaces as a live dashboard resource: a booted Driver shows
  # up in Driver.list_all(), renders its framebuffer to inline-PNG HTML, and the
  # runtime's resource provider discovers it. Uses a fake proc + a seeded frame
  # so no real VM (or Virtualization.framework entitlement) is needed; pure
  # in-process, sandbox-safe.
  vmkitResourceTestPy = pkgs.writeText "ix-mcp-vmkit-resource-test.py" ''
    # python
    import asyncio
    import os
    import time
    import types

    import vmkit
    from ix_notebook_mcp import runtime

    d = vmkit.Driver(bundle="/tmp/guest.bundle")
    assert d.is_alive is False
    assert d.id and len(d.id) == 8
    assert d.title == "vm · guest.bundle", repr(d.title)
    # No frame yet, not booted: a placeholder, and no capture is attempted.
    assert "booting" in d.resource_html() and "<img" not in d.resource_html()

    # Pretend the guest is booted (poll() is None == running) and has a frame.
    d._proc = types.SimpleNamespace(poll=lambda: None)
    assert d.is_alive is True
    d._frame_png = b"\x89PNG\r\n\x1a\nFRAME"
    d._frame_at = time.time()
    html = d.resource_html()
    assert 'img src="data:image/png;base64,' in html, html[:120]

    vmkit.Driver._live[d.id] = d
    assert d in vmkit.Driver.list_all()

    # The runtime provider discovers it, keyed vm:<id>, kind "vm", and renders.
    runtime.resources.clear()
    runtime._discover_vmkit_resources()
    rid = "vm:" + d.id
    assert rid in runtime.resources, list(runtime.resources)
    res = runtime.resources[rid]
    assert res.kind == "vm" and res.alive() is True
    assert "<img" in asyncio.run(res.render_html())
    # Idempotent: a second sweep does not duplicate the resource.
    runtime._discover_vmkit_resources()
    assert sum(1 for k in runtime.resources if k == rid) == 1
    # The bounded grab read never holds the lockstep pipe forever, and a
    # timed-out ack is drained before the next command (no desync). Fake pipe:
    # stdin is a sink, stdout is a real os.pipe we feed.
    dd = vmkit.Driver(bundle="/tmp/g")
    rfd, wfd = os.pipe()
    rfile = os.fdopen(rfd, "r")
    wfile = os.fdopen(wfd, "w", buffering=1)

    class _Sink:
        def write(self, s):
            pass

        def flush(self):
            pass

    dd._proc = types.SimpleNamespace(
        poll=lambda: None, returncode=None, stdin=_Sink(), stdout=rfile
    )
    # Guest serial console can share stdout (Linux guest): non-ack lines are
    # skipped, the real ok/err ack is returned.
    wfile.write("[  OK  ] Reached target Initrd Root Device.\n")
    wfile.write("Starting File System Check...\n")
    wfile.write("\n")
    wfile.write("ok size 1280 800\n")
    assert dd._send_locked("size") == "ok size 1280 800"
    wfile.write("[  OK  ] Started Service.\n")
    wfile.write("err size guest framebuffer not available yet\n")
    try:
        dd._send_locked("size")
        raise AssertionError("err ack should raise")
    except vmkit.VmkitError as exc:
        assert "framebuffer not available" in str(exc), exc
    try:
        dd._send_locked("shot /tmp/x", ack_timeout=0.2)
        raise AssertionError("bounded read should have timed out")
    except vmkit.VmkitError as exc:
        assert "timed out" in str(exc), exc
    assert dd._pending_acks == 1
    wfile.write("ok\n")    # the late ack for the timed-out shot
    wfile.write("done\n")  # the next command's own ack
    wfile.flush()
    assert dd._send_locked("size") == "done"  # drained "ok", read its own ack
    assert dd._pending_acks == 0

    print("vmkit-resource-ok")
  '';
  vmkitResourceTestPython = bundledTestPython [
    vmkitModule
    ixNotebookMcpModule
  ];
  vmkitResourceSmoke =
    pkgs.runCommand "ix-mcp-vmkit-resource-smoke"
    {
      nativeBuildInputs = [vmkitResourceTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe vmkitResourceTestPython} ${vmkitResourceTestPy} >stdout 2>stderr || {
        echo "ix-mcp vmkit resource smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'vmkit-resource-ok' stdout || {
        echo "ix-mcp vmkit resource smoke did not confirm the resource path:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = vmkitModule;
  darwinOnly = true;
  tests = lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
    inherit
      vmkitBundled
      vmkitResourceSmoke
      ;
  };
}
