"""Drive the unibind phase-2 conformance surface and print a case matrix.

Each case prints ``CASE <name>... ok (<evidence>)``; a failure prints the
traceback and flips the exit code. Only the stdlib is used so the nix check
needs nothing beyond the pinned interpreter and the built cdylib.
"""

from __future__ import annotations

import asyncio
import contextlib
import ctypes
import enum
import gc
import threading
import time
import traceback
import warnings
from collections.abc import Awaitable, Callable

import _conformance as conf


def _addr_of(buffer: bytearray) -> int:
    """Address of the first byte of ``buffer``, via a ctypes buffer export."""
    view = (ctypes.c_char * len(buffer)).from_buffer(buffer)
    address = ctypes.addressof(view)
    del view
    return address


async def case_echo_types() -> str:
    """Every scalar/container/record round-trips; errors map to ValueError."""
    truthy = True
    assert conf.echo_bool(truthy) is True
    assert conf.echo_int(-(2**40)) == -(2**40)
    assert conf.echo_float(3.5) == 3.5
    assert conf.echo_str("héllo") == "héllo"
    assert conf.echo_bytes(b"\x00\xffab") == b"\x00\xffab"
    assert conf.echo_option(None) is None
    assert conf.echo_option(7) == 7
    assert conf.echo_vec([1, 2, 3]) == [1, 2, 3]
    assert conf.echo_map({"a": 1.5, "b": -2.0}) == {"a": 1.5, "b": -2.0}
    point = conf.echo_record(conf.Point(1.25, -4.5))
    assert (point.x, point.y) == (1.25, -4.5)
    assert conf.add_with_default(10) == 42
    assert conf.add_with_default(10, 5) == 15
    assert await conf.sleep_ms_then(10, 99) == 99
    caught: ValueError | None = None
    try:
        conf.throw_value_error()
    except ValueError as exc:
        caught = exc
    assert caught is not None, "throw_value_error did not raise"
    assert isinstance(caught, conf.ConformanceError)
    message = str(caught)
    return f"all round-trips exact; raised {message!r} as ValueError subclass"


async def case_unit_enums() -> str:
    """A unit enum crosses as a StrEnum member, both ways, and refuses junk."""
    # The member is an enum member AND a str, which is the whole point: code
    # written against the old stringly surface keeps working.
    assert issubclass(conf.Severity, enum.StrEnum)
    assert conf.Severity.HARD_FAILURE == "hard_failure"
    assert isinstance(conf.Severity.INFO, str)

    # Returned values are real members, not bare strings.
    echoed = conf.echo_severity(conf.Severity.WARNING)
    assert echoed is conf.Severity.WARNING, f"got {echoed!r}"
    assert isinstance(echoed, conf.Severity)

    # A bare string is accepted on the way in and still comes back a member.
    from_str = conf.echo_severity("info")
    assert from_str is conf.Severity.INFO, f"got {from_str!r}"

    # Rust matched on the variant rather than passing a string through.
    assert conf.escalate(conf.Severity.INFO) is conf.Severity.WARNING
    assert conf.escalate(conf.Severity.WARNING) is conf.Severity.HARD_FAILURE

    # `rename_all` decides the wire spelling; the member name stays the
    # Python convention.
    assert conf.FrameKind.STARTED == "Started"
    assert conf.echo_optional_kind(None) is None
    assert conf.echo_optional_kind(conf.FrameKind.FINISHED) is conf.FrameKind.FINISHED

    # Enum-typed record fields cross in both directions.
    finding = conf.echo_finding(
        conf.Finding(conf.Severity.HARD_FAILURE, conf.FrameKind.FINISHED, "boom")
    )
    assert finding.severity is conf.Severity.HARD_FAILURE
    assert finding.kind is conf.FrameKind.FINISHED
    assert finding.to_dict()["severity"] is conf.Severity.HARD_FAILURE

    # A word outside the closed set is refused by name, not silently mapped.
    caught: ValueError | None = None
    try:
        conf.echo_severity("catastrophe")
    except ValueError as exc:
        caught = exc
    assert caught is not None, "an unknown variant did not raise"
    message = str(caught)
    assert "catastrophe" in message, message
    assert "hard_failure" in message, message
    return f"StrEnum both ways; unknown variant raised {message!r}"


async def case_cancel_mid_flight() -> str:
    """asyncio cancellation must drop the in-flight Rust future."""
    task = asyncio.ensure_future(conf.hold_guard_forever())
    await asyncio.sleep(0.2)
    live, dropped = conf.live_guards(), conf.dropped_guards()
    assert (live, dropped) == (1, 0), f"pre-cancel live={live} dropped={dropped}"
    started = time.monotonic()
    task.cancel()
    with contextlib.suppress(asyncio.CancelledError):
        await task
    # Bounded poll (~2s): the Drop runs on a tokio worker, so give it a
    # beat to land after the cancelled await returns.
    for _ in range(200):
        if conf.dropped_guards() != 0:
            break
        await asyncio.sleep(0.01)
    elapsed_ms = (time.monotonic() - started) * 1000.0
    live, dropped = conf.live_guards(), conf.dropped_guards()
    assert (live, dropped) == (0, 1), f"post-cancel live={live} dropped={dropped}"
    return f"live 1->0, dropped 0->1; Drop observed {elapsed_ms:.1f}ms after cancel()"


async def case_stream_backpressure() -> str:
    """Streams are pull-based: production tracks consumption, not creation."""
    base = conf.produced_count()
    stream = conf.counting_stream(1000)
    consumed: list[int] = []
    async for item in stream:
        consumed.append(item)
        if len(consumed) == 3:
            break
    produced = conf.produced_count() - base
    assert consumed == [0, 1, 2], f"consumed {consumed}"
    assert produced <= 4, f"produced {produced} for 3 consumed"
    await asyncio.sleep(0.2)
    settled = conf.produced_count() - base
    assert settled == produced, f"idle production: {produced} -> {settled}"
    del stream
    gc.collect()
    small_base = conf.produced_count()
    drained = [item async for item in conf.counting_stream(5)]
    assert drained == [0, 1, 2, 3, 4], f"drained {drained}"
    assert conf.produced_count() - small_base == 5
    records = await conf.record_stream(3)
    points = [(point.x, point.y) async for point in records]
    assert points == [(0.0, -0.0), (1.0, -1.0), (2.0, -2.0)], f"points {points}"
    return (
        f"consumed=3 produced={produced}, unchanged after 0.2s idle; "
        f"counting_stream(5) drained in order to StopAsyncIteration; "
        f"record stream yielded {points}"
    )


async def case_resource_lifecycle() -> str:
    """Leaked resources warn; async-with and explicit close are clean."""
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        leaky = conf.Gate("leaky")
        assert leaky.is_open()
        assert await leaky.ping(10) == 10
        del leaky
        gc.collect()
    leak_warnings = [
        warning
        for warning in caught
        if issubclass(warning.category, ResourceWarning) and "Gate" in str(warning.message)
    ]
    texts = [str(warning.message) for warning in caught]
    assert len(leak_warnings) == 1, f"expected one Gate ResourceWarning, saw {texts}"
    leak_text = str(leak_warnings[0].message)

    closed_base = conf.closed_gates()
    with warnings.catch_warnings(record=True) as clean_caught:
        warnings.simplefilter("always")
        async with conf.Gate("clean") as gate:
            assert gate.is_open()
            assert gate.label() == "clean"
        assert not gate.is_open()
        assert conf.closed_gates() == closed_base + 1
        idempotent = conf.Gate("idempotent")
        await idempotent.close()
        await idempotent.close()
        assert conf.closed_gates() == closed_base + 2, "double close closed twice"
        del gate, idempotent
        gc.collect()
    stray = [
        str(warning.message)
        for warning in clean_caught
        if issubclass(warning.category, ResourceWarning)
    ]
    assert stray == [], f"clean paths warned: {stray}"
    try:
        conf.Gate("")
    except ValueError as exc:
        constructor_error = str(exc)
    else:
        raise AssertionError("empty label did not raise")
    return (
        f"leak warned once: {leak_text!r}; async-with closed exactly once and "
        f"double close() bumped once; empty label raised {constructor_error!r}"
    )


async def case_async_object_return() -> str:
    """An async, fallible method hands back a live wrapper for another object."""
    closed_base = conf.closed_shells()
    async with conf.Gate("shells") as gate:
        shell = await gate.open_shell("bash")
        # A wrapper instance, not an opaque capsule: the class the module
        # exports, and its methods answer.
        assert isinstance(shell, conf.Shell), f"got {type(shell).__name__}"
        assert type(shell) is conf.Shell, f"got {type(shell).__name__}"
        assert shell.command() == "shells/bash", shell.command()
        assert shell.is_open()
        await shell.close()
        assert not shell.is_open()
        assert conf.closed_shells() == closed_base + 1, "the minted object's close never ran"

        # The error arm of the same shape still maps onto the exception
        # hierarchy rather than resolving to a half-built handle.
        caught: ValueError | None = None
        try:
            await gate.open_shell("")
        except ValueError as exc:
            caught = exc
        assert caught is not None, "open_shell('') resolved instead of raising"
        assert isinstance(caught, conf.ConformanceError)
    return (
        f"await gate.open_shell('bash') -> {type(shell).__name__} instance; "
        f"command() == {shell.command()!r}; close() observed; "
        f"empty command raised {str(caught)!r}"
    )


async def case_bytes_stream() -> str:
    """A UniStream<Vec<u8>> off an object method arrives as exact `bytes`."""
    async with conf.Gate("blobs") as gate:
        shell = await gate.open_shell("cat")
        stream = shell.output(3)
        stream_class = type(stream).__name__
        chunks = [chunk async for chunk in stream]
        await shell.close()
    expected = [b"\x00\xffblobs/cat" + str(index).encode() for index in range(3)]
    kinds = {type(chunk).__name__ for chunk in chunks}
    assert kinds == {"bytes"}, f"stream items arrived as {kinds}"
    # 0x00 and 0xFF are both invalid on their own in UTF-8 text, so a lossy
    # decode/encode anywhere on the item path cannot produce these exactly.
    assert chunks == expected, f"{chunks} != {expected}"
    assert all(0xFF in chunk for chunk in chunks), "the high byte did not survive"
    return (
        f"{stream_class} yielded {len(chunks)} bytes objects, "
        f"first {chunks[0]!r} (NUL + 0xFF intact)"
    )


async def case_zero_copy_gil() -> str:
    """&[u8] args alias Python memory; blocking fns release the GIL."""
    payload = bytearray(b"unibind" * 1024)
    expected_addr = _addr_of(payload)
    direct = conf.buffer_addr(payload)
    through_view = conf.buffer_addr(memoryview(payload))
    assert direct == expected_addr, f"bytearray copied: 0x{direct:x} != 0x{expected_addr:x}"
    assert through_view == expected_addr, f"memoryview copied: 0x{through_view:x}"
    started = time.monotonic()
    workers = [
        threading.Thread(target=conf.blocking_sleep_ms, args=(300,)) for _ in range(2)
    ]
    for worker in workers:
        worker.start()
    for worker in workers:
        worker.join()
    wall_ms = (time.monotonic() - started) * 1000.0
    # Serialized (GIL held) would be >=600ms; overlapped is ~300ms.
    assert wall_ms < 550.0, f"wall {wall_ms:.0f}ms suggests the GIL stayed held"
    total = conf.checksum(payload)
    assert total == sum(payload), f"checksum {total} != {sum(payload)}"
    return (
        f"buffer@0x{expected_addr:x} matches buffer_addr for bytearray and "
        f"memoryview; 2x300ms blocking sleeps walled {wall_ms:.0f}ms; "
        f"checksum {total} == python sum"
    )


async def case_panic_containment() -> str:
    """Rust panics surface as catchable exceptions; the interpreter survives."""
    # except BaseException: pyo3's PanicException deliberately derives it.
    sync_error: BaseException | None = None
    try:
        conf.panic_sync()
    except BaseException as exc:
        sync_error = exc
    assert sync_error is not None, "panic_sync did not raise"
    assert "deliberate sync panic" in str(sync_error), f"sync message: {sync_error}"
    sync_name = type(sync_error).__name__
    async_error: BaseException | None = None
    try:
        await conf.panic_async()
    except BaseException as exc:
        async_error = exc
    assert async_error is not None, "panic_async did not raise"
    assert "deliberate async panic" in str(async_error), f"async message: {async_error}"
    async_name = type(async_error).__name__
    assert conf.echo_int(1) == 1
    return f"sync panic -> {sync_name}, async panic -> {async_name}; interpreter still live"


async def case_static_factory() -> str:
    """Static factories construct the object, sync and async, and stay resources."""
    closed_base = conf.closed_gates()

    # The shape `__new__` cannot take: a static that awaits before it has
    # an instance to hand back.
    opened = await conf.Gate.opened("gamma")
    assert isinstance(opened, conf.Gate), f"async factory returned {type(opened)!r}"
    assert opened.label() == "gamma"
    assert opened.is_open()

    # A second factory on the same object, sync this time.
    copy = conf.Gate.named_after("gamma")
    assert isinstance(copy, conf.Gate), f"sync factory returned {type(copy)!r}"
    assert copy.label() == "gamma-copy"

    # A factory's errors raise like any other call's.
    try:
        await conf.Gate.opened("")
    except ValueError as exc:
        async_error = str(exc)
    else:
        raise AssertionError("empty label did not raise from the async factory")
    try:
        conf.Gate.named_after("")
    except ValueError:
        pass
    else:
        raise AssertionError("empty label did not raise from the sync factory")

    # What a factory returns is a real resource, so `async with` closes it.
    async with await conf.Gate.opened("delta") as scoped:
        assert scoped.label() == "delta"
    assert not scoped.is_open()

    # An associated function that does not return the object is a plain
    # staticmethod.
    assert conf.Gate.describe("epsilon") == "gate:epsilon"

    await opened.close()
    await copy.close()
    assert conf.closed_gates() == closed_base + 3, "each factory instance closed once"
    return f"factories construct and close; async factory error: {async_error}"


def _doc_sites() -> list[tuple[str, str]]:
    """Every docstring the extension exposes, by dotted name."""
    sites: list[tuple[str, str]] = [("<module>", conf.__doc__ or "")]
    for name in dir(conf):
        if name.startswith("_"):
            continue
        member = getattr(conf, name)
        doc = getattr(member, "__doc__", None)
        if isinstance(doc, str):
            sites.append((name, doc))
        if not isinstance(member, type):
            continue
        for attr in vars(member):
            if attr.startswith("_"):
                continue
            attr_doc = getattr(getattr(member, attr), "__doc__", None)
            if isinstance(attr_doc, str):
                sites.append((f"{name}.{attr}", attr_doc))
    return sites


async def case_doc_links() -> str:
    """Intra-doc links reach __doc__ in Python's spelling, not rustdoc's."""
    # One line per target kind the resolver distinguishes, each expectation
    # anchored on prose from the fixture: a rendered name on its own can
    # also come from a docstring the stub emitter synthesizes, which is how
    # the first draft of this passed against a link pointing elsewhere.
    rendered: tuple[tuple[str, str | None, str], ...] = (
        # an object type, reached by the inline `[text](Target)` form
        ("Shell.output", conf.Shell.output.__doc__, "opened under; see `Gate`."),
        # a method on the object this one mints
        ("Gate.open_shell", conf.Gate.open_shell.__doc__,
         "Its bytes come back through `Shell.output`."),
        # an error variant, which Python spells as its own exception class
        ("Gate.opened", conf.Gate.opened.__doc__,
         "Raises `Deliberate` on an empty label"),
        # a sibling associated function, reached through `Self`
        ("Gate.opened", conf.Gate.opened.__doc__,
         "refusal `Gate.named_after` makes."),
        # a record type, from an enumeration's own doc comment
        ("Severity", conf.Severity.__doc__, "one to a `Finding`."),
        # a method, from the object doc comment that already carried a link
        ("Shell", conf.Shell.__doc__, "minted only by `Gate.open_shell`"),
    )
    for site, doc, expected in rendered:
        assert doc is not None, f"{site} carries no docstring"
        assert expected in doc, f"{site} docstring lacks {expected!r}: {doc!r}"

    # No doc site keeps rustdoc syntax at runtime, records included: the macro
    # writes the resolved lines back over the struct's own doc comments, which
    # is the one site the generated wrappers do not own.
    leftover = {name for name, doc in _doc_sites() if "[`" in doc}
    assert not leftover, (
        f"rustdoc link syntax survives into __doc__ at {sorted(leftover)}"
    )
    tail = conf.Shell.output.__doc__.splitlines()[-1].strip()
    return (
        f"{len(rendered)} link renderings match Python spelling "
        f"(Shell.output ends {tail!r}); no doc site keeps rustdoc syntax"
    )


CASES: tuple[tuple[str, Callable[[], Awaitable[str]]], ...] = (
    ("echo-types", case_echo_types),
    ("unit-enums", case_unit_enums),
    ("cancel-mid-flight", case_cancel_mid_flight),
    ("stream-backpressure", case_stream_backpressure),
    ("drop-without-close", case_resource_lifecycle),
    ("async-object-return", case_async_object_return),
    ("static-factory", case_static_factory),
    ("bytes-stream", case_bytes_stream),
    ("zero-copy-gil", case_zero_copy_gil),
    ("panic-containment", case_panic_containment),
    ("doc-links", case_doc_links),
)


async def _run_all() -> int:
    failures = 0
    for name, case in CASES:
        try:
            evidence = await case()
        except BaseException:  # report every failure, keep going
            failures += 1
            print(f"CASE {name}... FAIL")
            traceback.print_exc()
        else:
            print(f"CASE {name}... ok ({evidence})")
    return failures


def main() -> None:
    """Run every case; exit non-zero if any failed."""
    failures = asyncio.run(_run_all())
    if failures:
        raise SystemExit(f"{failures} conformance case(s) failed")
    print(f"conformance: all {len(CASES)} cases passed")


if __name__ == "__main__":
    main()
