"""Native macOS VM control for the ix-mcp interpreter.

Bundled into the pinned interpreter the same way ``tui``, ``search`` and
``screen`` are, so every session can ``import vmkit`` with no setup. Where
``screen`` captures the host desktop, ``vmkit`` boots a guest VM and captures
*its* display, fully off-screen: nothing appears on the host desktop and the
host cursor is never touched. This is the way to verify on-screen rendering (a
GUI app, a boot screen) inside an isolated VM without taking over the machine.

    import vmkit
    print(vmkit.info())                       # is virtualization available?
    img = vmkit.screenshot("/path/to/guest")  # boot the guest, return a PIL.Image
    img                                        # auto-rendered inline by python_eval/exec

A guest is a *bundle* directory (``disk.img``, ``aux.img``,
``hardware-model.bin``, ``machine-id.bin``) created once with
:func:`install`::

    vmkit.install("/path/to/UniversalMac_26.5_Restore.ipsw", "/path/to/guest")

To *drive* a guest (synthetic keyboard/mouse plus on-demand screenshots), open a
:class:`Driver` as a context manager. It spawns the binary's ``drive-macos``
mode and talks to it in lockstep: every command returns the binary's one-line
acknowledgement, so a controller can capture a frame, locate a control in it
(with any host-side image tooling), click it, and capture again::

    with vmkit.Driver("/path/to/guest") as d:
        d.click(0.5, 0.5)          # left-click at the centre of the display
        d.type_("hello")           # type printable ASCII
        d.key("return")            # press a named key
        d.press_down("cmd"); d.key("space"); d.release("cmd")  # a chord (Spotlight)
        img = d.shot()             # screenshot the framebuffer as a PIL.Image

:func:`drive` is a one-shot convenience that opens a :class:`Driver`, sends a
list of command strings, and returns the acks. :func:`screenshot_many` boots
several bundles concurrently and returns one image per bundle.

A host directory can be shared into the guest over virtio-fs with the ``shares``
argument on :func:`screenshot`, :class:`Driver`, :func:`drive`, and
:func:`screenshot_many`: a list of ``"TAG=HOSTDIR"`` specs. Tag ``auto`` uses
the macOS automount tag, mounting at ``/Volumes/My Shared Files``. This is how a
GUI app on the host is run inside the guest: share its directory in and launch
it from the share.

Each guest is an independent ``vmkit`` process, so :class:`Driver` instances
and :func:`screenshot` calls are independent and safe to run in parallel; fan
out across several guests with :func:`screenshot_many` or by opening multiple
:class:`Driver` instances from separate threads.

The work is done by the ``vmkit`` binary (a thin Rust binding over Apple's
Virtualization.framework). It holds the ``com.apple.security.virtualization``
entitlement by self-signing into a per-user cache on first use, so no manual
``codesign`` is needed. The capture reads the guest framebuffer IOSurface
directly, so it needs no Screen-Recording permission.

This module is macOS-only: it raises on a non-Darwin platform, and the
``vmkit`` binary is only bundled into the interpreter on Darwin.

End-to-end example (one-time install, then provision and run an app). This needs
a live guest plus Apple's Virtualization.framework and the entitlement, so it is
**not** runnable in CI; run it on a Darwin host with a bundle on disk::

    import vmkit

    # 1. Install once from a local IPSW (~15-20 min), then provision the stopped
    #    guest past Setup Assistant to an auto-login desktop (offline disk edit).
    vmkit.install("/path/UniversalMac_26.5_Restore.ipsw", "/path/guest")
    vmkit.provision("/path/guest", user="ix", autologin=True)

    # 2. Stage a nix-built GUI app so its /nix/store dylibs resolve on the guest,
    #    then run it in the guest and capture a frame of the running app.
    staged = vmkit.stage_binary("/path/result/bin/bossbar-overlay",
                                "/path/app/bossbar-overlay")
    img = vmkit.run_app("/path/guest", "/path/app",
                        "'/Volumes/My Shared Files/bossbar-overlay'")
    img   # PIL.Image of the guest desktop with the app running, rendered inline

To drive the guest step by step instead, open a :class:`Driver` against the
provisioned bundle and exercise ``key``/``type_``/``click``/``press_down``/
``release``/``wait``/``shot`` in lockstep, asserting the returned frames change.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Iterable, Sequence

    from PIL import Image

__all__ = [
    "Driver",
    "VmkitError",
    "boot_linux",
    "boot_linux_gui",
    "drive",
    "drive_linux",
    "grid",
    "info",
    "install",
    "login",
    "provision",
    "run_app",
    "run_binary",
    "run_oci",
    "screenshot",
    "screenshot_many",
    "stage_binary",
]


class VmkitError(RuntimeError):
    """A vmkit invocation failed, or the platform/binary is unavailable."""


def _binary() -> str:
    if sys.platform != "darwin":
        raise VmkitError("vmkit is macOS-only")
    path = os.environ.get("IX_VMKIT_BIN")
    if not path:
        raise VmkitError(
            "IX_VMKIT_BIN is not set; the vmkit binary is bundled into ix-mcp "
            "on Darwin only"
        )
    return path


def _share_args(shares: Iterable[str] | None) -> list[str]:
    """Expand ``"TAG=HOSTDIR"`` specs into repeated ``--share`` arguments."""
    if not shares:
        return []
    args: list[str] = []
    for spec in shares:
        args += ["--share", str(spec)]
    return args


def info() -> str:
    """Return whether Virtualization.framework can run a VM on this host."""
    out = subprocess.run(
        [_binary(), "info"], capture_output=True, text=True, check=False
    )
    return (out.stdout or out.stderr).strip()


def install(ipsw: str | os.PathLike, bundle: str | os.PathLike, disk_gib: int = 64, timeout: float = 2400) -> None:
    """Install macOS into a fresh ``bundle`` directory from a local ``ipsw``.

    Takes ~15-20 minutes. Raises :class:`VmkitError` on failure.
    """
    try:
        result = subprocess.run(
            [_binary(), "install-macos", "--ipsw", str(ipsw), "--bundle", str(bundle), "--disk-gib", str(disk_gib)],
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        raise VmkitError(f"install-macos timed out after {timeout}s") from exc
    if result.returncode != 0:
        raise VmkitError(f"install-macos failed: {result.stderr.strip()}")


def stage_binary(
    path: str | os.PathLike,
    out: str | os.PathLike | None = None,
    timeout: float = 120,
) -> str:
    """Copy a nix-built macOS binary and make it guest-portable, returning the
    staged path.

    A nix-built binary links its dylibs by absolute ``/nix/store`` path, which a
    vanilla guest does not have. This repoints every ``/nix/store`` dependency to
    its ``/usr/lib`` system equivalent (libiconv, libc++, libresolv, …) or, when
    there is none, copies the dylib next to the output and rewrites it to an
    ``@loader_path`` reference, then ad-hoc re-signs the result. It verifies no
    ``/nix/store`` path remains and raises :class:`VmkitError` otherwise.

    Returns the staged binary's path, which is a *file*, not a directory. With
    ``out`` omitted it is written under a fresh temp directory using the same
    basename; pass ``out`` to choose the path (its parent is created). Note that
    :func:`run_app` shares a *directory*, so to run a staged binary either share
    its parent directory or, simpler, use :func:`run_binary`, which stages and
    runs in one call.
    """
    src = pathlib.Path(path)
    if out is None:
        # Deliberately not a TemporaryDirectory: the staged binary (and any
        # bundled dylibs beside it) is the return value and must outlive this
        # call, so the directory is left for the caller/OS to reap rather than
        # deleted on return.
        tmp = tempfile.mkdtemp(prefix="ix-vmkit-stage-")
        out_path = pathlib.Path(tmp) / src.name
    else:
        out_path = pathlib.Path(out)
    try:
        result = subprocess.run(
            [_binary(), "stage-binary", str(src), str(out_path)],
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        raise VmkitError(f"stage-binary timed out after {timeout}s") from exc
    if result.returncode != 0:
        raise VmkitError(f"stage-binary failed: {result.stderr.strip()}")
    # The binary prints the staged path on stdout; fall back to the requested
    # path if (unexpectedly) absent.
    staged = result.stdout.strip()
    return staged or str(out_path)


def provision(
    bundle: str | os.PathLike,
    user: str,
    autologin: bool = False,
    password: str = "",
    timeout: float = 300,
) -> None:
    """Provision a STOPPED guest ``bundle`` so it boots past Setup Assistant to a
    logged-in desktop.

    A host-side disk edit (the guest must not be running): it attaches the
    bundle's ``disk.img``, marks system and per-user Setup Assistant complete for
    ``user`` (the account created during :func:`install`), and detaches.

    With ``autologin`` the guest boots straight to ``user``'s desktop with no
    password prompt. This is the deterministic path, and it requires the *real*
    account password (the one set for ``user`` during install/Setup Assistant):
    macOS encodes it into ``/etc/kcpassword`` and auto-login fails to a login
    window if it does not match (a blank password never auto-logs in). The
    password is passed over stdin, never as an argument, so it stays out of the
    process table.

    The credential is also recorded in ``<bundle>/login.json`` (mode 0600) so a
    later session can drive the guest with :func:`login` without rediscovering
    it. Raises :class:`VmkitError` on failure, including if the image already
    appears in use, or if ``autologin`` is set without a password."""
    if autologin and not password:
        raise VmkitError(
            "autologin needs the account's real password (a blank password does "
            "not auto-log in); pass password=..."
        )
    args = [
        _binary(),
        "provision",
        "--bundle",
        str(bundle),
        "--user",
        user,
    ]
    # The password goes over stdin (`--password-stdin`), so it never lands in
    # argv where another user's `ps` could read it.
    stdin_input: str | None = None
    if autologin:
        args.append("--autologin")
        args.append("--password-stdin")
        stdin_input = password
    try:
        result = subprocess.run(
            args,
            input=stdin_input,
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        raise VmkitError(f"provision timed out after {timeout}s") from exc
    if result.returncode != 0:
        raise VmkitError(f"provision failed: {result.stderr.strip()}")
    # Record the credential beside the bundle whenever one was provided, so a
    # later session's login()/run_app can recover it without spelunking (not only
    # on the autologin path: a caller may record the password for login() use).
    if password:
        _write_bundle_login(bundle, user, password)


def _bundle_login_path(bundle: str | os.PathLike) -> pathlib.Path:
    return pathlib.Path(os.fspath(bundle)) / "login.json"


def _write_bundle_login(bundle: str | os.PathLike, user: str, password: str) -> None:
    """Persist ``{user, password}`` to ``<bundle>/login.json`` at mode 0600.

    Created 0600 from the start (no world-readable window before a later chmod),
    since the file holds a plaintext password."""
    import json

    path = _bundle_login_path(bundle)
    data = json.dumps({"user": user, "password": password})
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w") as handle:
            handle.write(data)
    finally:
        # If the file pre-existed with looser bits, O_CREAT did not change them;
        # narrow them now.
        try:
            os.chmod(path, 0o600)
        except OSError:
            pass


def _bundle_login(bundle: str | os.PathLike | None) -> dict[str, str] | None:
    """Read ``<bundle>/login.json`` if present, else ``None``."""
    if bundle is None:
        return None
    import json

    path = _bundle_login_path(bundle)
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None


def login(
    driver: "Driver",
    password: str | None = None,
    *,
    field: tuple[float, float] = (0.5, 0.83),
    settle: float = 8.0,
) -> None:
    """Log a guest in at the macOS login window through an open :class:`Driver`.

    For a bundle that is *not* set up for auto-login (see :func:`provision`), call
    this once the guest has reached the login window: it focuses the password
    field at fraction ``field``, types the password, presses Return, waits
    ``settle`` for the desktop, and verifies the screen changed. ``password``
    falls back to ``<bundle>/login.json`` (written by :func:`provision`). Raises
    :class:`VmkitError` if no password is available or the screen did not change
    (wrong password, or the guest was not at the login window).

    This types the password, so only call it when the guest is actually at the
    login window. A provisioned auto-login bundle reaches the desktop on its own
    and needs no :func:`login` call."""
    if password is None:
        creds = _bundle_login(driver._bundle)
        password = creds.get("password") if creds else None
    if not password:
        raise VmkitError(
            "no password for login (pass password=, or provision the bundle so "
            "login.json records it)"
        )
    if "\n" in password or "\r" in password:
        # `type_` sends one stdin line; a newline would split it and desync every
        # subsequent ack.
        raise VmkitError("login password must not contain a newline")
    before = driver.shot()
    driver.click(*field)
    driver.type_(password)
    driver.key("return")
    if settle > 0:
        driver.wait(settle)
    after = driver.shot()
    if not _frames_differ(before, after, 0.02):
        raise VmkitError(
            "login did not change the screen (wrong password, or the guest was "
            "not at the login window)"
        )


def screenshot(
    bundle: str | os.PathLike,
    seconds: int = 20,
    timeout: float | None = None,
    shares: Sequence[str] | None = None,
) -> "Image.Image":
    """Boot the macOS guest in ``bundle`` off-screen and return a ``PIL.Image``
    of its display after ``seconds`` (the last frame captured).

    ``shares`` is a list of ``"TAG=HOSTDIR"`` virtio-fs specs (see the
    module docstring). Raises :class:`VmkitError` if the binary fails, times out,
    or produces no frame.
    """
    from PIL import Image

    bin_path = _binary()
    deadline = timeout if timeout is not None else seconds + 120
    with tempfile.TemporaryDirectory(prefix="ix-vmkit-") as tmp:
        prefix = pathlib.Path(tmp) / "shot"
        try:
            result = subprocess.run(
                [bin_path, "boot-macos", "--bundle", str(bundle), "--out-prefix", str(prefix), "--seconds", str(seconds), *_share_args(shares)],
                capture_output=True,
                text=True,
                check=False,
                timeout=deadline,
            )
        except subprocess.TimeoutExpired as exc:
            raise VmkitError(f"boot-macos timed out after {deadline}s") from exc
        shots = sorted(pathlib.Path(tmp).glob("shot.*.png"))
        if not shots:
            raise VmkitError(
                f"boot-macos produced no screenshot (rc={result.returncode}): {result.stderr.strip()}"
            )
        # Load and detach from the temp file before it is removed.
        with Image.open(shots[-1]) as img:
            return img.convert("RGB")


def screenshot_many(
    bundles: Sequence[str | os.PathLike],
    seconds: int = 20,
    timeout: float | None = None,
    shares: Sequence[str] | None = None,
    max_workers: int | None = None,
) -> dict[str, "Image.Image"]:
    """Boot several guests off-screen *concurrently* and return one frame each.

    Each bundle runs in its own ``vmkit`` process, so the boots are fully
    independent and fan out across a thread pool. Returns a dict keyed by the
    string form of each input path to its last-frame ``PIL.Image``. ``shares``
    (if given) is applied to every guest.

    Raises :class:`VmkitError` (or the underlying error) if any guest fails; the
    first failure encountered is re-raised after every process has been
    resolved, so none is left orphaned. Tune parallelism with ``max_workers``
    (defaults to one worker per bundle).
    """
    from concurrent.futures import ThreadPoolExecutor

    keys = [str(b) for b in bundles]
    if not keys:
        return {}
    workers = max_workers if max_workers is not None else len(keys)
    results: dict[str, Image.Image] = {}
    error: BaseException | None = None
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = {
            key: pool.submit(screenshot, bundle, seconds, timeout, shares)
            for key, bundle in zip(keys, bundles)
        }
        # Resolve every future (each owns a process) before re-raising, so a
        # failure in one guest does not leak the others.
        for key, future in futures.items():
            try:
                results[key] = future.result()
            except BaseException as exc:  # noqa: BLE001 - first failure re-raised below
                if error is None:
                    error = exc
    if error is not None:
        raise error
    return results


def _resolve_disk(disk: str | os.PathLike) -> str:
    """Resolve a guest disk argument to a disk-image file path.

    The ``vz-linux-guest`` package output (and a ``nix build`` ``result``) is a
    *directory* containing ``nixos.img`` (make-disk-image's shape), so a directory
    is resolved to the single ``*.img``/``*.raw`` inside it. A file path is
    returned as-is. Raises :class:`VmkitError` if a directory holds zero or
    several images.
    """
    path = pathlib.Path(os.fspath(disk))
    if path.is_dir():
        imgs = sorted([*path.glob("*.img"), *path.glob("*.raw")])
        if len(imgs) != 1:
            raise VmkitError(
                f"disk {path} is a directory with {len(imgs)} disk images; "
                "pass the .img/.raw file (e.g. <result>/nixos.img)"
            )
        return str(imgs[0])
    return str(path)


def _writable_disk(disk: str | os.PathLike, staging_dir: str) -> str:
    """Return a writable disk-image path. VZ opens the boot disk read-write, so a
    read-only image (e.g. a `/nix/store` build) is copied into ``staging_dir``;
    an already-writable path is used in place (no multi-GiB copy). A directory
    argument is resolved to its image via :func:`_resolve_disk` first."""
    src = _resolve_disk(disk)
    if os.access(src, os.W_OK):
        return src
    dst = str(pathlib.Path(staging_dir) / "disk.img")
    # Try an APFS clone first (instant, sparse-preserving) when src and staging
    # share a volume; fall back to a full byte copy across volumes (the common
    # /nix/store case, which lives on its own volume).
    try:
        subprocess.run(["cp", "-c", src, dst], check=True, capture_output=True)
    except (subprocess.CalledProcessError, FileNotFoundError, OSError):
        shutil.copyfile(src, dst)
    os.chmod(dst, 0o644)
    return dst


def boot_linux(
    disk: str | os.PathLike,
    gpu: bool = False,
    cpus: int = 2,
    memory_mib: int = 1024,
    seconds: int = 20,
    timeout: float | None = None,
) -> str:
    """Boot an aarch64 Linux guest headlessly from a raw EFI-bootable ``disk``
    via libkrun (Hypervisor.framework), returning the guest serial console
    captured until it powers off or ``seconds`` elapses.

    Linux guests run on libkrun, not Virtualization.framework: libkrun is the
    only backend that gives a Linux guest GPU acceleration on Apple Silicon, a
    virtio-gpu Venus device (``gpu=True`` adds ``/dev/dri/renderD128``). The disk
    is a raw EFI image (a NixOS ``raw-efi`` image, a Fedora CoreOS raw, ...);
    libkrun's embedded OVMF firmware boots it. ``disk`` may be a package output
    directory (its image is found automatically) or an image file; libkrun opens
    the boot disk read-write, so a read-only image (e.g. a `/nix/store` build) is
    copied to a writable temp first. The headless, serial-only analogue of
    :func:`boot_linux_gui`. See the ``vmkit`` package's ``docs/linux-libkrun.md``.
    Raises :class:`VmkitError` if the binary fails or does not stop within the
    deadline.
    """
    deadline = timeout if timeout is not None else seconds + 60
    with tempfile.TemporaryDirectory(prefix="ix-vmkit-linux-") as tmp:
        work_disk = _writable_disk(disk, tmp)
        argv = [
            _binary(),
            "boot-linux",
            "--disk",
            work_disk,
            "--cpus",
            str(cpus),
            "--memory-mib",
            str(memory_mib),
            "--timeout-secs",
            str(seconds),
        ]
        if gpu:
            argv.append("--gpu")
        try:
            result = subprocess.run(
                argv, capture_output=True, text=True, check=False, timeout=deadline
            )
        except subprocess.TimeoutExpired as exc:
            raise VmkitError(f"boot-linux timed out after {deadline}s") from exc
    if result.returncode != 0:
        raise VmkitError(
            f"boot-linux failed (rc={result.returncode}): {result.stderr.strip()}"
        )
    # The guest serial console streams to stdout; boot/log lines go to stderr.
    return result.stdout


def boot_linux_gui(
    disk: str | os.PathLike,
    seconds: int = 60,
    timeout: float | None = None,
    efi_vars: str | os.PathLike | None = None,
) -> "Image.Image":
    """Boot an aarch64 Linux GUI guest from a raw EFI ``disk`` off-screen and
    return a ``PIL.Image`` of its display after ``seconds`` (the last frame).

    The Linux analogue of :func:`screenshot`: the disk boots into its own
    compositor/app, rendered with software graphics (Mesa lavapipe, since VZ's
    virtio-gpu has no 3D), and the host captures the guest framebuffer with no
    window and without touching the host cursor. ``disk`` may be the
    ``vz-linux-guest`` package output directory (its ``nixos.img`` is found
    automatically) or a ``.img``/``.raw`` file; a read-only image (e.g. a
    `/nix/store` build) is copied to a writable temp file first. Raises
    :class:`VmkitError` on failure or no frame.
    """
    from PIL import Image

    bin_path = _binary()
    # NixOS boot + compositor start is slower than a macOS-bundle boot, so give a
    # wider default deadline.
    deadline = timeout if timeout is not None else seconds + 180
    with tempfile.TemporaryDirectory(prefix="ix-vmkit-linux-") as tmp:
        work_disk = _writable_disk(disk, tmp)
        prefix = pathlib.Path(tmp) / "shot"
        argv = [
            bin_path,
            "boot-linux-gui",
            "--disk",
            work_disk,
            "--out-prefix",
            str(prefix),
            "--seconds",
            str(seconds),
        ]
        if efi_vars is not None:
            argv += ["--efi-vars", str(efi_vars)]
        try:
            result = subprocess.run(
                argv, capture_output=True, text=True, check=False, timeout=deadline
            )
        except subprocess.TimeoutExpired as exc:
            raise VmkitError(f"boot-linux-gui timed out after {deadline}s") from exc
        shots = sorted(pathlib.Path(tmp).glob("shot.*.png"))
        if not shots:
            raise VmkitError(
                f"boot-linux-gui produced no screenshot (rc={result.returncode}): {result.stderr.strip()}"
            )
        with Image.open(shots[-1]) as img:
            return img.convert("RGB")


class Driver:
    """Drive a booted macOS guest in lockstep over the binary's ``drive-macos``
    mode.

    Spawns one ``vmkit`` process that boots the guest off-screen and reads
    newline commands from stdin, acking each on stdout. Use it as a context
    manager so the guest is always stopped on exit::

        with vmkit.Driver("/path/to/guest", shares=["auto=/host/app"]) as d:
            d.click(0.5, 0.5)
            d.type_("ls")
            d.key("return")
            img = d.shot()

    Every method returns the binary's one-line acknowledgement; :meth:`shot`
    returns a ``PIL.Image`` instead. An ``err ...`` ack, or the process dying,
    raises :class:`VmkitError`. Each :class:`Driver` is its own process, so
    independent instances drive different guests in parallel.
    """

    def __init__(
        self,
        bundle: str | os.PathLike | None = None,
        shares: Sequence[str] | None = None,
        timeout: float = 120,
        *,
        disk: str | os.PathLike | None = None,
        efi_vars: str | os.PathLike | None = None,
    ) -> None:
        """Prepare a driver (the guest boots on :meth:`__enter__`).

        Pass exactly one guest: ``bundle`` for a macOS guest, or ``disk`` for an
        aarch64 Linux GUI guest (a raw EFI image; ``efi_vars`` defaults to
        ``<disk>.efivars``). The Linux ``disk`` must be writable (VZ opens it
        read-write); copy a `/nix/store` image out first.

        ``shares`` is a list of ``"TAG=HOSTDIR"`` virtio-fs specs (macOS only;
        see the module docstring). ``timeout`` bounds how long :meth:`close`
        waits for the process to quit; per-command reads block until the ack
        arrives, since a slow guest boot can delay the first one.
        """
        if (bundle is None) == (disk is None):
            raise VmkitError("Driver needs exactly one of bundle (macOS) or disk (Linux)")
        # virtio-fs shares are a macOS-guest feature here; the Linux GUI config
        # wires no sharing device, so reject rather than silently drop them.
        if disk is not None and shares:
            raise VmkitError("shares are macOS-only; the Linux GUI Driver wires no virtio-fs")
        self._bundle = str(bundle) if bundle is not None else None
        # Resolve a directory (the vz-linux-guest package output) to its image.
        self._disk = _resolve_disk(disk) if disk is not None else None
        self._efi_vars = str(efi_vars) if efi_vars is not None else None
        self._shares = list(shares) if shares else []
        self.timeout = timeout
        self._proc: subprocess.Popen[str] | None = None
        # Cached captured-framebuffer size in pixels (see `size()`).
        self._size: tuple[int, int] | None = None

    def __enter__(self) -> "Driver":
        bin_path = _binary()
        if self._disk is not None:
            # VZ opens the boot disk read-write; a read-only image (e.g. a
            # /nix/store build) would fail to attach and the process would exit
            # with only a "no ack" symptom. Fail clearly up front instead.
            if not os.access(self._disk, os.W_OK):
                raise VmkitError(
                    f"Linux Driver disk must be writable (copy the image out of the store first): {self._disk}"
                )
            argv = [bin_path, "drive-linux", "--disk", self._disk]
            if self._efi_vars is not None:
                argv += ["--efi-vars", self._efi_vars]
        else:
            # __init__ guarantees exactly one of bundle/disk is set, so here
            # (disk is None) the bundle is present.
            assert self._bundle is not None
            argv = [bin_path, "drive-macos", "--bundle", self._bundle, *_share_args(self._shares)]
        # stderr carries only the binary's boot/log lines; stdout carries the
        # acks, one per command. Send stderr to DEVNULL so a clean stdout read
        # never has to skip non-ack noise. The signed re-exec inherits these
        # pipes, so the channel survives it.
        self._proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def close(self) -> None:
        """Quit the guest and tear down the process. Idempotent."""
        proc = self._proc
        self._proc = None
        if proc is None:
            return
        try:
            # `quit` exits the process with no ack, so write it directly rather
            # than through `send` (which would wait for an ack that never comes).
            if proc.poll() is None and proc.stdin is not None:
                try:
                    proc.stdin.write("quit\n")
                    proc.stdin.flush()
                except (BrokenPipeError, OSError):
                    pass
            try:
                proc.wait(timeout=self.timeout)
            except subprocess.TimeoutExpired:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
        finally:
            for stream in (proc.stdin, proc.stdout):
                if stream is not None:
                    try:
                        stream.close()
                    except OSError:
                        pass

    def send(self, command: str) -> str:
        """Write one ``command`` line, flush, and return its one-line ack.

        Raises :class:`VmkitError` on an ``err ...`` ack, or if the driver
        process has died or closed its output.
        """
        proc = self._proc
        if proc is None or proc.stdin is None or proc.stdout is None:
            raise VmkitError("driver is not running (use it as a context manager)")
        if proc.poll() is not None:
            raise VmkitError(f"driver process exited with code {proc.returncode}")
        line = command.rstrip("\n")
        try:
            proc.stdin.write(line + "\n")
            proc.stdin.flush()
        except (BrokenPipeError, OSError) as exc:
            raise VmkitError(f"driver process closed its input: {exc}") from exc
        # stderr is discarded, so the next stdout line is this command's ack;
        # skip a stray blank line all the same.
        while True:
            ack = proc.stdout.readline()
            if ack == "":
                rc = proc.poll()
                raise VmkitError(
                    f"driver process gave no ack for {command!r} "
                    f"(process exited with code {rc})"
                )
            ack = ack.rstrip("\n")
            if ack != "":
                break
        if ack.startswith("err"):
            raise VmkitError(f"command {command!r} failed: {ack}")
        return ack

    def key(self, name: str, count: int = 1) -> str:
        """Press a named key (``return``, ``tab``, arrows, ``f1``..``f12``, a
        modifier) ``count`` times."""
        return self.send(f"key {name} {count}")

    def press_down(self, name: str) -> str:
        """Hold a key down (e.g. a modifier, to chord it with :meth:`key`)."""
        return self.send(f"down {name}")

    def release(self, name: str) -> str:
        """Release a key held with :meth:`press_down`."""
        return self.send(f"up {name}")

    def type_(self, text: str) -> str:
        """Type ``text`` as printable ASCII characters (US layout).

        A newline cannot be sent in ``text`` (it would split the stdin command);
        press ``return`` with :meth:`key` instead.
        """
        return self.send(f"type {text}")

    def click(self, fx: float, fy: float) -> str:
        """Left-click at fraction ``(fx, fy)`` of the display, from the top-left
        (resolution-independent, both in ``0..1``)."""
        return self.send(f"click {fx} {fy}")

    def move(self, fx: float, fy: float) -> str:
        """Move the pointer to fraction ``(fx, fy)`` of the display (top-left,
        both ``0..1``) without clicking, so hover-state UI appears and a target
        can be confirmed before committing a :meth:`click`."""
        return self.send(f"move {fx} {fy}")

    # `hover` reads better than `move` at some call sites; same command.
    hover = move

    def cursor(self) -> tuple[float, float]:
        """Return the last pointer fraction this driver set with
        :meth:`click`/:meth:`move` (top-left, ``0..1``). The pointer is absolute,
        so this is the driver's own record, not a guest read-back. Raises
        :class:`VmkitError` if no pointer command has run yet."""
        ack = self.send("cursor")  # 'ok cursor FX FY'
        parts = ack.split()
        return (float(parts[2]), float(parts[3]))

    def size(self) -> tuple[int, int]:
        """Return the captured framebuffer size in pixels ``(width, height)``,
        cached after the first call. The authoritative size for pixel<->fraction
        conversion (the configured and actual display sizes can differ)."""
        if self._size is None:
            ack = self.send("size")  # 'ok size W H'
            parts = ack.split()
            w, h = int(parts[2]), int(parts[3])
            if w <= 0 or h <= 0:
                raise VmkitError(f"guest reported a non-positive framebuffer size ({w}x{h})")
            self._size = (w, h)
        return self._size

    def show_cursor(self, on: bool = True) -> str:
        """Toggle drawing a pointer marker into subsequent :meth:`shot`s. Off by
        default, so a plain ``shot`` stays a faithful framebuffer capture."""
        return self.send(f"cursor-show {'on' if on else 'off'}")

    def click_px(self, x: int, y: int) -> str:
        """Left-click at pixel ``(x, y)`` of the captured framebuffer (top-left).
        Converts to a fraction via :meth:`size`."""
        w, h = self.size()
        return self.click(x / w, y / h)

    def move_px(self, x: int, y: int) -> str:
        """Move the pointer to pixel ``(x, y)`` of the captured framebuffer
        (top-left) without clicking. Converts to a fraction via :meth:`size`."""
        w, h = self.size()
        return self.move(x / w, y / h)

    def click_and_verify(
        self,
        fx: float,
        fy: float,
        *,
        settle: float = 0.4,
        min_changed: float = 0.002,
    ) -> bool:
        """Click at ``(fx, fy)`` and report whether the display changed.

        Positions the pointer first, captures, clicks, waits ``settle`` for the
        UI to react, captures again, and returns ``True`` if at least
        ``min_changed`` (fraction of pixels) differ notably. Turns a missed click
        (a wrong target, a no-op) into an observable ``False`` instead of a silent
        nothing. This is a heuristic: it moves the pointer to the target before
        both captures so the cursor is identical in each (otherwise a moved cursor
        always reads as change), but a screensaver, idle-dim, or an unrelated
        animation can still register; tune ``min_changed`` for the surface."""
        # Move first so the pointer sits at the target in both frames; only the
        # click's effect differs, not the cursor's position.
        self.move(fx, fy)
        self.wait(0.15)
        before = self.shot()
        self.click(fx, fy)
        if settle > 0:
            self.wait(settle)
        after = self.shot()
        return _frames_differ(before, after, min_changed)

    def wait(self, seconds: float) -> str:
        """Sleep ``seconds`` in the guest driver (fractional allowed)."""
        return self.send(f"wait {seconds}")

    def shot(self, path: str | os.PathLike | None = None) -> "Image.Image":
        """Screenshot the guest framebuffer and return a ``PIL.Image``.

        With ``path``, the PNG is also written there. With no ``path``, it goes
        to a temp file that is loaded and removed. Raises :class:`VmkitError` if
        the capture fails.
        """
        from PIL import Image

        if path is not None:
            out = pathlib.Path(path)
            self.send(f"shot {out}")
            with Image.open(out) as img:
                return img.convert("RGB")
        with tempfile.TemporaryDirectory(prefix="ix-vmkit-shot-") as tmp:
            out = pathlib.Path(tmp) / "shot.png"
            self.send(f"shot {out}")
            # Load and detach before the temp dir is removed.
            with Image.open(out) as img:
                return img.convert("RGB")


def drive(
    bundle: str | os.PathLike,
    commands: Sequence[str],
    shares: Sequence[str] | None = None,
    timeout: float = 120,
) -> list[str]:
    """Open a :class:`Driver` for ``bundle``, send each command, return the acks.

    A one-shot convenience for a fixed script: it boots the guest, runs
    ``commands`` in order, and stops the guest on the way out. ``shares`` and
    ``timeout`` are as on :class:`Driver`. Use :class:`Driver` directly when the
    next command depends on a captured frame. Raises :class:`VmkitError` on any
    failing command.
    """
    with Driver(bundle, shares=shares, timeout=timeout) as d:
        return [d.send(command) for command in commands]


def drive_linux(
    disk: str | os.PathLike,
    commands: Sequence[str],
    efi_vars: str | os.PathLike | None = None,
    timeout: float = 120,
) -> list[str]:
    """Open a :class:`Driver` for a Linux GUI ``disk``, send each command, return
    the acks. The Linux analogue of :func:`drive`; the ``disk`` must be writable.
    Use :class:`Driver` directly when the next command depends on a frame.
    """
    with Driver(disk=disk, efi_vars=efi_vars, timeout=timeout) as d:
        return [d.send(command) for command in commands]


# Where the macOS automount tag (`auto`) mounts a virtio-fs share inside the
# guest.
_AUTOMOUNT_DIR = "/Volumes/My Shared Files"


def run_app(
    bundle: str | os.PathLike,
    app_dir: str | os.PathLike,
    command: str,
    *,
    seconds: float = 15,
    shares_tag: str = "auto",
    boot_seconds: float = 30,
    login_password: str | None = None,
    timeout: float = 300,
) -> "Image.Image":
    """Share a host directory into a guest, launch a command in it, and return a
    frame of the guest display.

    The convenience that collapses the headline demo (run a host GUI app inside a
    guest, off-screen, and screenshot it) into one call. It shares ``app_dir``
    into the guest over virtio-fs, boots and drives the guest with a
    :class:`Driver`, opens Terminal via Spotlight, runs ``command``, waits
    ``seconds`` for it to render, and returns a ``PIL.Image`` of the display. The
    host cursor and desktop are never touched.

    ``command`` runs in the guest shell. Reference the shared files under the
    mount point: with ``shares_tag="auto"`` (the default) the share is at
    ``/Volumes/My Shared Files`` and ``app_dir``'s contents appear directly
    there, so a binary ``app_dir/myapp`` is ``"/Volumes/My Shared Files/myapp"``.
    Stage a nix-built binary first with :func:`stage_binary` (into ``app_dir``)
    so its dylibs resolve on the guest.

    ``boot_seconds`` is how long to wait after boot before driving (the guest
    must reach the desktop); ``seconds`` is how long to wait after launching the
    command before capturing. ``timeout`` bounds the whole driver session.

    The guest must reach a logged-in desktop. A bundle provisioned with
    ``autologin`` does this on its own; for a bundle that stops at the login
    window, pass ``login_password`` (or persist it via :func:`provision`) and
    this logs in first via :func:`login`. Raises :class:`VmkitError` on failure,
    including if ``command`` contains a newline.
    """
    # A newline in `command` would split the driver's `type` line into two stdin
    # commands, desyncing every subsequent ack. Reject it up front rather than
    # fail confusingly mid-run; run multiple commands with separate calls or `;`.
    if "\n" in command or "\r" in command:
        raise VmkitError("run_app command must not contain a newline")

    if shares_tag == "auto":
        share_spec = f"auto={app_dir}"
        mount = _AUTOMOUNT_DIR
    else:
        share_spec = f"{shares_tag}={app_dir}"
        # A named tag is mounted by the guest wherever it chooses; the caller is
        # responsible for referencing the right path in `command`. We still pass
        # the share through so the device exists.
        mount = ""

    with Driver(bundle, shares=[share_spec], timeout=timeout) as d:
        # Let the guest reach the desktop (or the login window) before driving it.
        if boot_seconds > 0:
            d.wait(boot_seconds)
        # A non-autologin bundle stops at the login window; log in first.
        if login_password is not None:
            login(d, login_password)
        # Open Spotlight, search for Terminal, launch it.
        d.press_down("cmd")
        d.key("space")
        d.release("cmd")
        d.wait(1.5)
        d.type_("Terminal")
        d.wait(1.5)
        d.key("return")
        d.wait(3)
        # Run the command. `cd` into the share first when using the automount so
        # a relative `command` resolves, then run it in the background so the
        # shell stays responsive and the app keeps running while we capture.
        if mount:
            d.type_(f"cd {_shell_quote(mount)}")
            d.key("return")
            d.wait(0.5)
        d.type_(command)
        d.key("return")
        # Wait for the app to render, then capture.
        if seconds > 0:
            d.wait(seconds)
        return d.shot()


def run_binary(
    bundle: str | os.PathLike,
    host_binary: str | os.PathLike,
    args: str = "",
    *,
    name: str | None = None,
    **run_app_kwargs: object,
) -> "Image.Image":
    """Stage a nix-built macOS binary guest-portable, run it in the guest, and
    return a frame of the display: :func:`stage_binary` + :func:`run_app` in one
    call.

    :func:`stage_binary` writes a single *file*, but :func:`run_app` shares a
    *directory*; this stages ``host_binary`` into a fresh directory, shares that
    directory in, and launches the binary from it in the background, so a caller
    never has to juggle the file-vs-directory handoff. ``args`` is appended to the
    launch command (already shell-safe for the binary path; quote your own args).
    ``name`` overrides the in-guest binary name (default: the host basename).
    Extra keyword arguments pass through to :func:`run_app` (``seconds``,
    ``boot_seconds``, ``timeout``). Raises :class:`VmkitError` on failure."""
    src = pathlib.Path(os.fspath(host_binary))
    app_name = name or src.name
    # The shared directory must outlive staging but not the call: the guest only
    # needs it while the Driver session in run_app is alive, so clean it after.
    staged_dir = tempfile.mkdtemp(prefix="ix-vmkit-app-")
    try:
        stage_binary(str(src), str(pathlib.Path(staged_dir) / app_name))
        launch = _shell_quote(f"./{app_name}")
        if args:
            launch += f" {args}"
        # Background it (and drop its output) so the guest shell stays responsive
        # while we wait for the app to render, then capture.
        command = f"{launch} >/tmp/ix-run-binary.log 2>&1 &"
        return run_app(bundle, staged_dir, command, **run_app_kwargs)  # type: ignore[arg-type]
    finally:
        shutil.rmtree(staged_dir, ignore_errors=True)


def run_oci(
    disk: str | os.PathLike,
    *,
    gui: bool = False,
    seconds: int | None = None,
    require_aarch64: bool = True,
    **kwargs: object,
) -> "str | Image.Image":
    """Boot a raw EFI-bootable Linux ``disk`` as a guest: the generic entry over
    :func:`boot_linux` (headless, libkrun) and :func:`boot_linux_gui` (GUI, VZ).

    Both back ends run *aarch64* guests on this aarch64 host; ``require_aarch64``
    guards that with a typed error rather than a confusing boot failure (set it
    false only if you know what you are doing).

    - ``gui=False`` (default): boot the disk headlessly under libkrun and return
      the guest serial console as ``str``. Pass ``gpu=True`` for a virtio-gpu
      (Venus) device. See the ``vmkit`` package's ``docs/linux-libkrun.md``.
    - ``gui=True``: boot the disk under Virtualization.framework (e.g. the
      ``vz-linux-guest`` image) and return a ``PIL.Image`` of the framebuffer.

    Extra keyword arguments pass through to the underlying boot function. Raises
    :class:`VmkitError` on an arch mismatch."""
    import platform

    if require_aarch64 and not platform.machine().lower().startswith(("arm64", "aarch64")):
        raise VmkitError(
            f"run_oci needs an aarch64 host (this is {platform.machine()})"
        )
    extra = dict(kwargs)
    if seconds is not None:
        extra["seconds"] = seconds
    if gui:
        return boot_linux_gui(disk, **extra)  # type: ignore[arg-type]
    return boot_linux(disk, **extra)  # type: ignore[arg-type]


def _shell_quote(path: str) -> str:
    """Single-quote a path for a guest POSIX shell (the driver types it as-is)."""
    return "'" + path.replace("'", "'\\''") + "'"


def _frames_differ(before: "Image.Image", after: "Image.Image", min_changed: float) -> bool:
    """Whether two frames differ in at least ``min_changed`` (fraction) of pixels.

    A per-pixel threshold first (ignoring tiny noise like antialiasing or a
    blinking caret), then a histogram count of the pixels that crossed it.
    """
    from PIL import ImageChops

    a = before.convert("RGB")
    b = after.convert("RGB")
    if a.size != b.size:
        b = b.resize(a.size)
    diff = ImageChops.difference(a, b).convert("L").point(lambda p: 255 if p > 24 else 0)
    changed = diff.histogram()[-1]  # count of pixels at 255 (crossed the threshold)
    total = diff.width * diff.height
    return total > 0 and changed / total >= min_changed


def grid(image: "Image.Image", n: int = 10) -> "Image.Image":
    """Return a copy of ``image`` with a labelled fraction grid drawn on top.

    A calibration aid for clicking: every line is annotated with its ``0..1``
    fraction (the same coordinate space :class:`Driver` ``click``/``move`` take),
    so a target's fraction can be read straight off a captured frame. ``n`` is
    the number of divisions per axis (default 10, i.e. every 0.1)."""
    from PIL import ImageDraw

    out = image.convert("RGB").copy()
    draw = ImageDraw.Draw(out)
    w, h = out.size
    line = (255, 40, 40)
    for i in range(1, n):
        f = i / n
        x = round(f * w)
        y = round(f * h)
        draw.line([(x, 0), (x, h)], fill=line, width=1)
        draw.line([(0, y), (w, y)], fill=line, width=1)
        draw.text((x + 2, 2), f"{f:.1f}", fill=line)
        draw.text((2, y + 2), f"{f:.1f}", fill=line)
    return out
