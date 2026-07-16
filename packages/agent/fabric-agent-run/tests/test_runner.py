from __future__ import annotations

import asyncio
import hashlib
import os
import stat
import sys
import tempfile
import unittest
from collections.abc import Mapping, Sequence
from pathlib import Path

from fabric_agent_run.backends import (
    Backend,
    BackendSpec,
    ProcessBackend,
    argv_for,
    child_environment,
)
from fabric_agent_run.journal import Fact, HashRef
from fabric_agent_run.runner import AgentSpec, RunState, run_agent


class FakeJournal:
    def __init__(self) -> None:
        self.facts: list[Fact] = []
        self.blobs: dict[str, bytes] = {}
        self.interrupt: str | None = None
        self.latest_error: Exception | None = None

    async def put_blob(self, body: bytes) -> HashRef:
        digest = hashlib.sha256(body).hexdigest()
        self.blobs[digest] = body
        return HashRef(f"blake3:{digest}")

    async def assert_facts(self, facts: Sequence[Fact]) -> None:
        self.facts.extend(facts)

    async def latest(self, entity: str, attr: str) -> str | None:
        del entity
        if self.latest_error is not None:
            raise self.latest_error
        return self.interrupt if attr == "interrupt" else None

    def states(self, task: str) -> list[str]:
        return [
            value
            for entity, attr, value in self.facts
            if entity == task and attr == "state" and isinstance(value, str)
        ]

    def blob(self, task: str, attr: str) -> bytes:
        refs = [
            value
            for entity, fact_attr, value in self.facts
            if entity == task and fact_attr == attr and isinstance(value, HashRef)
        ]
        ref = refs[-1]
        return self.blobs[ref.value.removeprefix("blake3:")]


class FakeBackend(Backend):
    def __init__(self) -> None:
        self.answer: asyncio.Future[str] = asyncio.get_running_loop().create_future()
        self.interrupts = 0
        self.closed = 0

    async def result(self) -> str:
        return await self.answer

    async def interrupt(self) -> None:
        self.interrupts += 1
        if not self.answer.done():
            self.answer.set_exception(RuntimeError("stopped"))

    async def close(self) -> None:
        self.closed += 1


def spec(*, timeout: float = 10.0) -> AgentSpec:
    return AgentSpec(
        name="quality",
        harness="codex",
        model="gpt-5.6-sol",
        effort="medium",
        prompt=b"fix it",
        cwd=Path.cwd(),
        timeout_seconds=timeout,
    )


class LifecycleTests(unittest.IsolatedAsyncioTestCase):
    async def test_success_is_submitted_running_done_with_cas_payloads(self) -> None:
        journal = FakeJournal()
        backend = FakeBackend()

        async def factory(
            backend_spec: BackendSpec,
            prompt: bytes,
            environ: Mapping[str, str],
        ) -> Backend:
            assert backend_spec.harness == "codex"
            assert prompt == b"fix it"
            assert environ["OPENAI_API_KEY"] == "secret"
            return backend

        task = asyncio.create_task(
            run_agent(
                spec(),
                journal=journal,
                backend_factory=factory,
                environ={"OPENAI_API_KEY": "secret"},
                poll_seconds=0.001,
            )
        )
        await asyncio.sleep(0)
        backend.answer.set_result("done well")
        outcome = await task

        assert outcome.state == RunState.DONE
        assert journal.states(outcome.task) == ["submitted", "running", "done"]
        assert "pending" not in journal.states(outcome.task)
        assert journal.blob(outcome.task, "prompt") == b"fix it"
        assert journal.blob(outcome.task, "result") == b"done well"
        assert journal.facts[-1] == (outcome.task, "state", "done")
        assert backend.closed == 1

    async def test_journal_interrupt_stops_backend_and_writes_one_terminal(self) -> None:
        journal = FakeJournal()
        backend = FakeBackend()

        async def factory(
            backend_spec: BackendSpec,
            prompt: bytes,
            environ: Mapping[str, str],
        ) -> Backend:
            del backend_spec, prompt, environ
            return backend

        task = asyncio.create_task(
            run_agent(
                spec(),
                journal=journal,
                backend_factory=factory,
                poll_seconds=0.001,
            )
        )
        await asyncio.sleep(0.01)
        journal.interrupt = "requested"
        outcome = await task

        assert outcome.state == RunState.INTERRUPTED
        assert journal.states(outcome.task) == ["submitted", "running", "interrupted"]
        assert backend.interrupts == 1

    async def test_timeout_stops_backend_and_cannot_later_publish_done(self) -> None:
        journal = FakeJournal()
        backend = FakeBackend()

        async def factory(
            backend_spec: BackendSpec,
            prompt: bytes,
            environ: Mapping[str, str],
        ) -> Backend:
            del backend_spec, prompt, environ
            return backend

        outcome = await run_agent(
            spec(timeout=0.001),
            journal=journal,
            backend_factory=factory,
            poll_seconds=0.001,
        )

        assert outcome.state == RunState.FAILED
        assert "TimeoutError" in (outcome.error or "")
        assert journal.states(outcome.task) == ["submitted", "running", "failed"]
        assert backend.interrupts == 1

    async def test_signal_stops_backend_and_records_interrupted(self) -> None:
        journal = FakeJournal()
        backend = FakeBackend()
        stop = asyncio.Event()

        async def factory(
            backend_spec: BackendSpec,
            prompt: bytes,
            environ: Mapping[str, str],
        ) -> Backend:
            del backend_spec, prompt, environ
            return backend

        task = asyncio.create_task(
            run_agent(
                spec(),
                journal=journal,
                backend_factory=factory,
                stop=stop,
                poll_seconds=0.001,
            )
        )
        await asyncio.sleep(0.01)
        stop.set()
        outcome = await task

        assert outcome.state == RunState.INTERRUPTED
        assert journal.states(outcome.task) == ["submitted", "running", "interrupted"]
        assert backend.interrupts == 1

    async def test_interrupt_watch_failure_stops_as_failed_not_interrupted(self) -> None:
        journal = FakeJournal()
        backend = FakeBackend()
        journal.latest_error = ConnectionError("weave unavailable")

        async def factory(
            backend_spec: BackendSpec,
            prompt: bytes,
            environ: Mapping[str, str],
        ) -> Backend:
            del backend_spec, prompt, environ
            return backend

        outcome = await run_agent(
            spec(),
            journal=journal,
            backend_factory=factory,
            poll_seconds=0.001,
        )

        assert outcome.state == RunState.FAILED
        assert "interrupt watch failed" in (outcome.error or "")
        assert journal.states(outcome.task) == ["submitted", "running", "failed"]
        assert backend.interrupts == 1

    async def test_start_timeout_is_terminal_without_claiming_running(self) -> None:
        journal = FakeJournal()

        async def factory(
            backend_spec: BackendSpec,
            prompt: bytes,
            environ: Mapping[str, str],
        ) -> Backend:
            del backend_spec, prompt, environ
            await asyncio.sleep(60)
            raise AssertionError("unreachable")

        outcome = await run_agent(
            spec(timeout=0.001),
            journal=journal,
            backend_factory=factory,
        )

        assert outcome.state == RunState.FAILED
        assert "did not start" in (outcome.error or "")
        assert journal.states(outcome.task) == ["submitted", "failed"]

    async def test_spawn_failure_is_terminal_and_never_claims_running(self) -> None:
        journal = FakeJournal()

        async def factory(
            backend_spec: BackendSpec,
            prompt: bytes,
            environ: Mapping[str, str],
        ) -> Backend:
            del backend_spec, prompt, environ
            raise FileNotFoundError("codex")

        outcome = await run_agent(spec(), journal=journal, backend_factory=factory)

        assert outcome.state == RunState.FAILED
        assert journal.states(outcome.task) == ["submitted", "failed"]
        assert "FileNotFoundError" in (outcome.error or "")


class BackendContractTests(unittest.TestCase):
    def test_prompt_never_rides_argv(self) -> None:
        cwd = Path.cwd()
        with tempfile.TemporaryDirectory() as raw:
            result = Path(raw) / "result"
            codex = argv_for(
                BackendSpec("codex", "gpt-5.6-sol", "medium", cwd, 1024),
                result,
            )
            claude = argv_for(BackendSpec("claude", "fable", None, cwd, 1024), result)
        assert codex[-1] == "-"
        assert claude == [
            "claude",
            "-p",
            "--no-session-persistence",
            "--model",
            "fable",
        ]
        assert "--ephemeral" in codex
        assert "fix it" not in codex + claude

    def test_child_environment_withholds_weave_authority_only(self) -> None:
        child = child_environment(
            {
                "WEAVE_URL": "http://weave",
                "WEAVE_TOKEN": "record-secret",
                "WEAVE_IDENTITY": "owner@example.com",
                "OPENAI_API_KEY": "provider-secret",
                "GH_TOKEN": "workflow-secret",
            }
        )
        assert "WEAVE_URL" not in child
        assert "WEAVE_TOKEN" not in child
        assert "WEAVE_IDENTITY" not in child
        assert child["OPENAI_API_KEY"] == "provider-secret"
        assert child["GH_TOKEN"] == "workflow-secret"  # noqa: S105 -- inert fixture


class ProcessBackendTests(unittest.IsolatedAsyncioTestCase):
    async def test_codex_prompt_and_result_cross_the_real_process_boundary(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            fake = root / "codex"
            fake.write_text(
                f"""#!{sys.executable}
import pathlib
import sys

prompt = sys.stdin.read()
output = pathlib.Path(sys.argv[sys.argv.index("--output-last-message") + 1])
output.write_text("reply: " + prompt)
print("activity log")
"""
            )
            fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
            backend = await ProcessBackend.spawn(
                BackendSpec("codex", "gpt-5.6-sol", "medium", root, 1024),
                b"from stdin",
                environ={
                    "PATH": os.fspath(root),
                    "OPENAI_API_KEY": "provider-secret",
                    "WEAVE_TOKEN": "record-secret",
                },
            )
            try:
                assert await backend.result() == "reply: from stdin"
            finally:
                await backend.close()


if __name__ == "__main__":
    unittest.main()
