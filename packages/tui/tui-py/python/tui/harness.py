"""Playwright-style harnesses for interactive coding agents (Claude Code, Codex, Cursor).

`tui.Tui` gives you a raw PTY: send bytes, scrape the rendered screen. This
module is the layer every agent test rig re-invents on top of it, and it
deliberately borrows Playwright's vocabulary so it reads the way browser
automation does. The mapping is close enough to be a porting guide:

    Playwright (browser)                tui.harness (agent TUI)
    ------------------------            ---------------------------------
    browser = await chromium.launch()   agent = await Claude.launch()
    page.keyboard.type(text)            agent.keyboard.type(text)
    page.keyboard.press("Enter")        agent.keyboard.press(Key.ENTER)
    page.wait_for_selector(sel)         agent.wait_for(pattern)
    page.wait_for_load_state("idle")    agent.wait_for_idle()
    page.content() / inner_text()       agent.content() / agent.text()
    page.screenshot()                   agent.screenshot()        # a Snapshot
    page.on("dialog", auto-dismiss)     gates auto-cleared on launch
    expect(loc).to_contain_text(s)      await expect(agent).to_contain_text(s)

Quick start, exactly the shape of a Playwright test:

    from tui.harness import Claude, expect

    async with await Claude.launch(cwd="/repo") as agent:
        await agent.prompt("What does packages/foo do?")
        await expect(agent).to_be_idle(timeout=180)
        await expect(agent).to_contain_text("foo")
        print(await agent.last_reply())

    # or the one-liner: submit, wait for the turn to finish, return the reply
    answer = await agent.run("summarize CONTRIBUTING.md", timeout=180)

    # or fan out across mixed agents through the shared Agent interface
    claude, codex = await Claude.launch(cwd="/repo"), await Codex.launch(cwd="/repo")
    replies = await asyncio.gather(
        claude.run_to_completion("find one risk"),
        codex.run_to_completion("find one risk"),
    )

Why drive the real TUI instead of `claude -p`? A headless `-p` run is invisible
and uninterruptible. A harness drives the actual TUI in a PTY, so the session
shows up live on the `tui` web dashboard (`nix run .#dashboard`) just like a
human's. You watch the current state, attach, interrupt. For an *experiment*
that is the whole point: an agent you can observe beats a black box you diff.

## Auto-waiting and idle detection

Playwright's headline feature is auto-waiting: actions wait for actionability and
assertions retry until they pass. The same idea runs through here. `prompt`
waits for the box to accept the text before submitting; `run` waits for the turn
to finish; `expect(...)` retries until the deadline.

Knowing when a turn is *done* is the hard part: a PTY has no "done" event and the
obvious signals are brittle (spinners animate, footers reword between releases, a
submitted prompt stays on screen). So idle detection is **quiescence first,
marker second**: the viewport must stop changing for `settle` seconds, and a
`busy_marker` substring (if the agent has a reliable one) must be absent.
Quiescence is the version-independent backstop; the marker is a precise
fast-path. Claude Code's "esc to interrupt" footer is grounded and used.

Every I/O method is async, like `Tui`. Construction is synchronous; prefer the
async `launch()` classmethod, which also clears onboarding gates.
"""

from __future__ import annotations

import asyncio
import contextlib
import hashlib
import re
from collections.abc import Awaitable, Callable, Sequence
from types import TracebackType
from typing import ClassVar, Self

from . import Key, Pattern, Snapshot, Tui, WaitTimeout

__all__ = [
    "Agent",
    "AgentAssertions",
    "Claude",
    "Codex",
    "Cursor",
    "Gate",
    "Keyboard",
    "expect",
]


class Gate:
    """A one-time onboarding screen to clear before the agent is usable.

    The analog of a Playwright dialog handler: when `pattern` shows up while the
    agent is starting (a "trust this folder?" prompt, a changelog, a hooks
    review), the harness sends `response` to dismiss it. `name` is for logging.
    """

    __slots__ = ("name", "pattern", "response")

    def __init__(self, name: str, pattern: str | re.Pattern[str], response: str) -> None:
        self.name = name
        self.pattern = pattern
        self.response = response

    def __repr__(self) -> str:
        return f"Gate({self.name!r})"


class Keyboard:
    """`agent.keyboard`, mirroring Playwright's `page.keyboard`.

    `type` sends literal text; `press` sends one or more `Key` sequences (or any
    raw string). Both are coroutines, like every `Tui` write.
    """

    __slots__ = ("_tui",)

    def __init__(self, tui: Tui) -> None:
        self._tui = tui

    async def type(self, text: str) -> None:
        """Type `text` into the focused input (no Enter)."""
        await self._tui.send(text)

    async def press(self, *keys: str) -> None:
        """Press one or more keys in order. `Key.ENTER`, `Key.ctrl('c')`, etc."""
        await self._tui.send(*keys)


class Agent:
    """A coding-agent TUI driven over a PTY, the Playwright way.

    Subclass with presets for a specific agent (`Claude`, `Codex`), or construct
    directly with explicit `binary` / `ready` / `busy_marker` / `gates`. The
    handle owns one `Tui`; reach it via `.tui` for anything not wrapped here.

    Prefer `agent = await Claude.launch(...)`, which spawns, clears onboarding
    gates, and waits for the input prompt. `async with await Claude.launch() as
    agent:` force-closes on exit. In Jupyter (loop already running) just
    `agent = await Claude.launch(...)` and drive it across cells.
    """

    #: Default command if `binary=` is not passed. Subclasses set this.
    binary: ClassVar[str] = ""
    #: Pattern present once the agent is spawned and awaiting input.
    ready: ClassVar[Pattern] = ""
    #: Substring present while the agent is working (a precise idle fast-path).
    #: `None` means rely on quiescence alone.
    busy_marker: ClassVar[str | None] = None
    #: Onboarding screens to auto-clear on launch, in priority order.
    gates: ClassVar[Sequence[Gate]] = ()

    def __init__(
        self,
        *args: str,
        binary: str | None = None,
        cwd: str | None = None,
        ready: Pattern | None = None,
        busy_marker: str | None = None,
        gates: Sequence[Gate] | None = None,
        size: tuple[int, int] = (40, 120),
        scrollback_lines: int = 50_000,
    ) -> None:
        cmd = binary if binary is not None else self.binary
        if not cmd:
            raise ValueError("no binary: set the class `binary` or pass binary=")
        # cwd is delivered by spawning through `sh -c 'cd … && exec …'` rather
        # than chdir of this process: several agents often run in different repos
        # at once, and a process-global chdir would race.
        if cwd is not None:
            quoted = " ".join(_shquote(a) for a in (cmd, *args))
            self._tui = Tui(
                "sh",
                "-c",
                f"cd {_shquote(cwd)} && exec {quoted}",
                size=size,
                scrollback_lines=scrollback_lines,
            )
        else:
            self._tui = Tui(cmd, *args, size=size, scrollback_lines=scrollback_lines)
        self._ready = ready if ready is not None else self.ready
        self._busy = busy_marker if busy_marker is not None else self.busy_marker
        self._gates = tuple(gates) if gates is not None else tuple(self.gates)
        self._started = False
        self.keyboard = Keyboard(self._tui)

    @classmethod
    async def launch(
        cls,
        *args: str,
        timeout: float = 45.0,
        **kwargs: object,
    ) -> Self:
        """Spawn the agent, clear onboarding gates, and wait for the prompt.

        The Playwright `await chromium.launch()` analog: the returned handle is
        ready to drive. `timeout` covers a cold start (a `nix run` of an uncached
        binary plus any onboarding screens). Extra kwargs go to the constructor.
        """
        # `**kwargs: object` keeps the public type honest without re-listing
        # every constructor arg; the constructor validates them.
        self = cls(*args, **kwargs)  # type: ignore[arg-type]
        return await self.start(timeout=timeout)

    # -- identity -----------------------------------------------------------

    @property
    def tui(self) -> Tui:
        """The underlying `Tui`. Escape hatch for anything not wrapped here."""
        return self._tui

    @property
    def is_alive(self) -> bool:
        return self._tui.is_alive

    # -- lifecycle ----------------------------------------------------------

    async def start(self, *, timeout: float = 45.0) -> Self:
        """Wait for the TUI to draw, clear onboarding gates, reach the prompt.

        Usually reached via `launch()`. Idempotent enough to call on an
        already-ready agent (the gate sweep no-ops and `ready` matches at once).
        """
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        await self._settle(timeout=timeout)
        # Clear any onboarding gates, re-sweeping until none match (one gate can
        # reveal the next), bounded so a misfiring pattern cannot spin forever.
        for _ in range(len(self._gates) + 2):
            txt = await self._tui.text()
            gate = next((g for g in self._gates if _gate_matches(g.pattern, txt)), None)
            if gate is None:
                break
            await self._tui.send(gate.response)
            await self._settle(timeout=max(1.0, deadline - loop.time()))
        if self._ready:
            await self._tui.wait_for(
                self._ready, timeout=max(1.0, deadline - loop.time())
            )
        self._started = True
        return self

    async def __aenter__(self) -> Self:
        # `await Claude.launch()` already started it; `async with Claude(...)`
        # has not. Start once, either way.
        return self if self._started else await self.start()

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self.close()

    async def close(self) -> None:
        """Force-kill the agent and drop it from the dashboard."""
        await self._tui.close()

    async def interrupt(self) -> None:
        """Stop the current turn without quitting: Esc, then Ctrl+C as fallback.

        Agent TUIs bind Esc to "interrupt the running turn"; neither key quits.
        """
        await self._tui.send(Key.ESC)
        await self._tui.interrupt()

    # -- actions (Playwright-style, auto-waiting) ---------------------------

    async def prompt(self, text: str) -> None:
        """Type `text` and submit it. The core action; auto-waits to submit.

        Submitting an agent TUI is racier than a shell: a bare `text + Enter`
        right after the previous turn can land mid-render, leaving the text typed
        but unsubmitted. So this types first, waits for the box to show the text,
        then presses Enter, and presses it once more if the turn has not started.
        Grounded against Claude Code, which drops the occasional fast Enter.
        """
        if not self._started:
            await self.start()
        await self.keyboard.type(text)
        # The box may wrap/scroll the text; submit anyway if it times out.
        with contextlib.suppress(WaitTimeout):
            await self._tui.wait_for(_submit_probe(text), timeout=5.0)
        await self.keyboard.press(Key.ENTER)
        if not await self._turn_started():
            await self.keyboard.press(Key.ENTER)

    async def send_message(self, text: str) -> None:
        """Submit `text` without waiting for the agent to finish the turn.

        High-level interface alias for `prompt`, shared by `Claude` and `Codex`
        so orchestrators can type against `AgentLike`.
        """
        await self.prompt(text)

    async def run(self, text: str, *, timeout: float = 180.0, settle: float = 0.6) -> str:
        """Submit `text`, wait for the turn to finish, return the agent's reply.

        The convenience wrapper: `prompt` + `wait_for_idle` + `parse_reply` over
        what appeared since the prompt. For finer control, drive `prompt` /
        `wait_for_idle` / `content` yourself.
        """
        before = await self._lines()
        await self.prompt(text)
        await self.wait_for_idle(timeout=timeout, settle=settle)
        delta = _tail_delta(before, await self._lines())
        return self.parse_reply(delta)

    async def run_to_completion(
        self,
        text: str,
        *,
        timeout: float = 180.0,
        settle: float = 0.6,
    ) -> str:
        """Submit `text`, wait for idle, and return the parsed reply.

        This is the agent-interface name for `run`, meant for mixed fan-out:
        `await asyncio.gather(claude.run_to_completion(q), codex.run_to_completion(q))`.
        """
        return await self.run(text, timeout=timeout, settle=settle)

    async def ask(self, text: str, *, timeout: float = 180.0, settle: float = 0.6) -> str:
        """Submit one question and return the parsed reply.

        `ask` is the high-level name for simple delegation; it keeps the visible
        resource open, unlike a headless one-shot command.
        """
        return await self.run_to_completion(text, timeout=timeout, settle=settle)

    # -- waiting (Playwright-style) -----------------------------------------

    async def wait_for(self, pattern: Pattern, *, timeout: float = 30.0) -> Snapshot:
        """Wait until the screen matches `pattern` (`page.wait_for_selector`).

        `pattern` is a substring, a compiled regex, or a `Snapshot -> bool`
        callable. Returns the matching snapshot. Raises `WaitTimeout`.
        """
        return await self._tui.wait_for(pattern, timeout=timeout)

    async def wait_for_idle(
        self,
        *,
        timeout: float = 180.0,
        settle: float = 0.6,
        poll: float = 0.1,
    ) -> Snapshot:
        """Wait until the turn finishes (`page.wait_for_load_state("idle")`).

        Idle means the `busy_marker` (if any) is absent AND the viewport has not
        changed for `settle` seconds. Returns the settled, styled snapshot.
        Raise `settle` for agents that pause mid-turn (a long tool call leaves
        the screen briefly static); lower it for snappier feedback.
        """
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        last: str | None = None
        stable_since: float | None = None
        while True:
            txt = await self._tui.text()
            digest = hashlib.md5(txt.encode(), usedforsecurity=False).hexdigest()
            busy = self._busy is not None and self._busy.lower() in txt.lower()
            if not busy and digest == last:
                stable_since = stable_since if stable_since is not None else loop.time()
                if loop.time() - stable_since >= settle:
                    return await self._tui.snapshot()
            else:
                stable_since = None
            last = digest
            if loop.time() >= deadline:
                raise WaitTimeout(f"{type(self).__name__} still busy after {timeout:.0f}s")
            await asyncio.sleep(poll)

    async def wait_for_timeout(self, seconds: float) -> None:
        """Sleep `seconds` (`page.wait_for_timeout`). A test smell, but handy."""
        await asyncio.sleep(seconds)

    # -- reading (Playwright-style) -----------------------------------------

    async def content(self) -> str:
        """The whole session: scrollback + viewport, joined (`page.content`)."""
        return (await self._tui.snapshot(styled=False)).full_text

    async def text(self) -> str:
        """The visible viewport text (`page.inner_text` of the screen)."""
        return await self._tui.text()

    async def screenshot(self) -> Snapshot:
        """A styled `Snapshot` (`page.screenshot`). Renders to color HTML in
        Jupyter and via `.to_html()`; this is the artifact to attach to a run."""
        return await self._tui.snapshot()

    async def last_reply(self) -> str:
        """The agent's most recent answer, via `parse_reply` over the viewport."""
        return self.parse_reply(await self.text())

    def parse_reply(self, transcript: str) -> str:
        """Turn a transcript (delta or screen) into the agent's answer.

        The base returns it stripped; subclasses drop TUI chrome and keep the
        answer block. Override for a custom agent.
        """
        return transcript.strip()

    # -- internals ----------------------------------------------------------

    async def _lines(self) -> list[str]:
        snap = await self._tui.snapshot(styled=False)
        return [*snap.scrollback, *snap.viewport]

    async def _settle(self, *, settle: float = 0.4, poll: float = 0.1, timeout: float = 30.0) -> None:
        """Wait for the screen to stop changing (drawn / done animating)."""
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        last: str | None = None
        stable_since: float | None = None
        while True:
            digest = await self._screen_hash()
            if digest == last:
                stable_since = stable_since if stable_since is not None else loop.time()
                if loop.time() - stable_since >= settle:
                    return
            else:
                stable_since = None
            last = digest
            if loop.time() >= deadline:
                return  # best-effort: caller's own wait_for surfaces a real miss
            await asyncio.sleep(poll)

    async def _turn_started(self, *, timeout: float = 2.0) -> bool:
        """Did a turn begin after Enter? True if the busy marker appeared, or
        (marker-less) the screen changed from the pre-Enter frame."""
        before = await self._screen_hash()
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while loop.time() < deadline:
            txt = await self._tui.text()
            if self._busy and self._busy.lower() in txt.lower():
                return True
            if not self._busy and hashlib.md5(txt.encode(), usedforsecurity=False).hexdigest() != before:
                return True
            await asyncio.sleep(0.1)
        return False

    async def _screen_hash(self) -> str:
        txt = await self._tui.text()
        return hashlib.md5(txt.encode(), usedforsecurity=False).hexdigest()

    def __repr__(self) -> str:
        return f"{type(self).__name__}(tui={self._tui!r})"


class Claude(Agent):
    """Claude Code (`claude`) in a PTY.

    Grounded against Claude Code 2.1.x: idle uses the "esc to interrupt" footer
    plus quiescence, launch auto-accepts the "trust this folder?" gate, and
    `parse_reply` returns the final assistant block (the `⏺`-led answer lines).
    """

    binary = "claude"
    #: The empty input prompt at the bottom of a ready Claude TUI.
    ready = re.compile(r"^❯\s*$", re.MULTILINE)
    busy_marker = "esc to interrupt"
    gates = (
        # A fresh / untrusted cwd opens on a trust prompt whose default
        # selection is "1. Yes, I trust this folder"; Enter accepts it.
        Gate("trust-folder", "Is this a project you created or one you trust", Key.ENTER),
    )

    def parse_reply(self, transcript: str) -> str:
        """Keep the last assistant block: the run of lines from the final `⏺`
        marker up to the next chrome line. Falls back to the stripped text."""
        return _parse_claude_reply(transcript)


class Codex(Agent):
    """OpenAI Codex (`codex`) in a PTY.

    Ships marker-less: idle is pure quiescence (no verified busy footer yet), so
    keep `settle` generous. Pass `busy_marker=`/`ready=` once you confirm Codex's
    indicators for your build. A fresh Codex may open on a "trust folder" or
    "review hooks" gate; pass matching `gates=` (the menus accept a digit + Enter,
    e.g. `Gate("trust", "Trust", "1\\r")`).
    """

    binary = "codex"
    ready = re.compile(r"[›❯>]\s*$", re.MULTILINE)
    busy_marker = None

    def __init__(
        self,
        *args: str,
        model: str = "gpt-5.5",
        reasoning_effort: str = "low",
        approval: str = "never",
        sandbox: str = "danger-full-access",
        **kwargs: object,
    ) -> None:
        defaults = (
            "-c",
            f"model_reasoning_effort={reasoning_effort!r}",
            "--model",
            model,
            "--ask-for-approval",
            approval,
            "--sandbox",
            sandbox,
        )
        super().__init__(*defaults, *args, **kwargs)  # type: ignore[arg-type]

    def parse_reply(self, transcript: str) -> str:
        """Keep the final Codex answer block from a TUI transcript."""
        lines = transcript.splitlines()
        starts = [i for i, line in enumerate(lines) if line.lstrip().startswith("•")]
        if not starts:
            return transcript.strip()
        out: list[str] = []
        for line in lines[starts[-1]:]:
            stripped = line.strip()
            if out and (stripped.startswith("›") or (" · " in stripped and "gpt-" in stripped)):
                break
            out.append(line.lstrip("• ").rstrip())
        return "\n".join(out).strip()


class Cursor(Agent):
    """Cursor CLI (`cursor-agent`) in a PTY.

    Grounded against cursor-agent 2026.06: idle uses the "ctrl+c to stop"
    footer plus quiescence, launch auto-accepts the "Workspace Trust Required"
    gate, and `parse_reply` returns the final plain answer block(s). The login
    gate is NOT auto-cleared: a logged-out cursor-agent opens a browser OAuth
    flow only a human can approve, so log in once interactively first.

    Defaults pin the fast Composer model and pass `--force` (skip per-command
    approval dialogs; explicit `cli-config.json` denies still apply), which
    makes `agent.ask(...)` a cheap smart-codebase-search delegate:

        async with await Cursor.launch(cwd="/repo") as cursor:
            where = await cursor.ask("where is retry backoff implemented?")
    """

    binary = "cursor-agent"
    #: The input caret of a ready cursor-agent box ("→ Plan, search, build
    #: anything" / "→ Add a follow-up").
    ready = re.compile(r"^\s*→ ", re.MULTILINE)
    busy_marker = "ctrl+c to stop"
    gates = (
        # A cwd outside the trusted-workspace list opens on "Workspace Trust
        # Required"; `a` selects "Trust this workspace".
        Gate("trust-workspace", "Do you trust the contents of this directory", "a"),
    )

    def __init__(
        self,
        *args: str,
        model: str = "composer-2.5-fast",
        force: bool = True,
        **kwargs: object,
    ) -> None:
        defaults = ("--model", model, *(("--force",) if force else ()))
        super().__init__(*defaults, *args, **kwargs)  # type: ignore[arg-type]

    def parse_reply(self, transcript: str) -> str:
        """Keep the final cursor-agent answer block(s) from a TUI transcript."""
        return _parse_cursor_reply(transcript)

# --------------------------------------------------------------------------- #
# Assertions (Playwright `expect`)
# --------------------------------------------------------------------------- #


class AgentAssertions:
    """Auto-retrying assertions, like Playwright's `expect(locator)`.

    Each method polls until it passes or `timeout` expires, so you assert the
    state you want without sprinkling sleeps. Raises `AssertionError` on expiry.
    """

    __slots__ = ("_agent",)

    def __init__(self, agent: Agent) -> None:
        self._agent = agent

    async def _poll(
        self,
        predicate: Callable[[], Awaitable[bool]],
        describe: str,
        *,
        timeout: float,
        poll: float,
    ) -> None:
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while True:
            if await predicate():
                return
            if loop.time() >= deadline:
                raise AssertionError(f"expect(agent).{describe} not met within {timeout:.0f}s")
            await asyncio.sleep(poll)

    async def to_contain_text(self, needle: str, *, timeout: float = 30.0, poll: float = 0.2) -> None:
        """Assert the session transcript contains `needle` before `timeout`."""
        async def pred() -> bool:
            return needle in await self._agent.content()

        await self._poll(pred, f"to_contain_text({needle!r})", timeout=timeout, poll=poll)

    async def not_to_contain_text(self, needle: str, *, timeout: float = 5.0, poll: float = 0.2) -> None:
        """Assert `needle` stays absent for the whole `timeout` window."""
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while loop.time() < deadline:
            if needle in await self._agent.content():
                raise AssertionError(f"expect(agent).not_to_contain_text({needle!r}) failed")
            await asyncio.sleep(poll)

    async def to_be_idle(self, *, timeout: float = 180.0, settle: float = 0.6) -> None:
        """Assert the agent reaches an idle (turn-finished) state."""
        try:
            await self._agent.wait_for_idle(timeout=timeout, settle=settle)
        except WaitTimeout as exc:
            raise AssertionError(str(exc)) from exc


def expect(agent: Agent) -> AgentAssertions:
    """Auto-retrying assertions for `agent` (Playwright's `expect`)."""
    return AgentAssertions(agent)


# --------------------------------------------------------------------------- #
# Helpers
# --------------------------------------------------------------------------- #


#: Lines that are Claude Code TUI chrome, not answer content.
_CLAUDE_CHROME = re.compile(r"^\s*(✻|✶|✳|·|⏵⏵|░|─|╭|╰|│|▎|❯|Debug mode|\+\d+ more)")


def _parse_claude_reply(transcript: str) -> str:
    """The last assistant block in a Claude Code transcript.

    The run of lines from the final `⏺` answer marker up to the next chrome
    line, with the marker stripped. Falls back to the stripped transcript when
    no marker is present.
    """
    lines = transcript.splitlines()
    starts = [i for i, ln in enumerate(lines) if ln.lstrip().startswith("⏺")]
    if not starts:
        return transcript.strip()
    out: list[str] = []
    for raw in lines[starts[-1] :]:
        ln = raw.rstrip()
        if ln.lstrip().startswith("⏺"):
            ln = ln.replace("⏺", " ", 1)
        elif _CLAUDE_CHROME.match(ln):
            break
        out.append(ln)
    return "\n".join(out).strip()


#: cursor-agent input-box top border: a run of `▄` (the footer starts here).
_CURSOR_FOOTER = re.compile(r"^\s*▄+\s*$")
#: Spinner/status lines like "⠘⠤ Composing" / "⠠⠛ Running  55 tokens".
_CURSOR_SPINNER = re.compile(r"^\s*[\u2800-\u28ff]")


def _parse_cursor_reply(transcript: str) -> str:
    """The last answer block(s) in a cursor-agent transcript.

    cursor-agent prints no answer marker (unlike Claude's `⏺` or Codex's `•`):
    a turn renders as the echoed prompt, tool blocks (a `$ cmd` line with its
    output indented beneath), and plain answer paragraphs, then the input-box
    footer. So: cut at the footer border, drop spinner lines, split into
    blank-line-separated blocks, drop `$`-led tool blocks and the leading
    echoed-prompt block, and return what remains. Falls back to the stripped
    transcript when nothing survives.
    """
    body: list[str] = []
    for line in transcript.splitlines():
        if _CURSOR_FOOTER.match(line):
            break
        if _CURSOR_SPINNER.match(line):
            continue
        body.append(line)
    blocks: list[list[str]] = []
    current: list[str] = []
    for line in body:
        if line.strip():
            current.append(line)
        elif current:
            blocks.append(current)
            current = []
    if current:
        blocks.append(current)
    answers = [b for b in blocks if not b[0].lstrip().startswith("$ ")]
    if len(answers) >= 2:
        # The first surviving block of a turn is the echoed user prompt.
        answers = answers[1:]
    if not answers:
        return transcript.strip()
    return "\n\n".join("\n".join(line.strip() for line in block) for block in answers).strip()

def _shquote(s: str) -> str:
    """Single-quote `s` for a POSIX shell."""
    return "'" + s.replace("'", "'\\''") + "'"


def _gate_matches(pattern: str | re.Pattern[str], text: str) -> bool:
    """True if a gate `pattern` (substring or regex) is on screen."""
    if isinstance(pattern, str):
        return pattern in text
    return pattern.search(text) is not None


def _submit_probe(text: str) -> str:
    """A short prefix of `text` to confirm it landed in the input box.

    The first line, capped: agent boxes wrap or soft-truncate long input, so
    matching the whole prompt is unreliable.
    """
    first = text.strip().splitlines()[0] if text.strip() else text
    return first[:24]


def _tail_delta(before: Sequence[str], after: Sequence[str]) -> str:
    """The lines in `after` not already the tail of `before`.

    Both are scrollback+viewport line lists. A new turn appends lines (and may
    redraw the viewport), so return the suffix of `after` past the longest shared
    prefix, with blank edges trimmed.
    """
    n = 0
    for a, b in zip(before, after, strict=False):
        if a != b:
            break
        n += 1
    tail = list(after[n:])
    while tail and not tail[-1].strip():
        tail.pop()
    while tail and not tail[0].strip():
        tail.pop(0)
    return "\n".join(tail)
