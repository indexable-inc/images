"""Unit tests for the pure helpers in `tui.harness`.

These cover the parsing/diffing logic that has no PTY dependency, so they run
anywhere (`python -m unittest tui.harness_test`) without spawning an agent. The
live, agent-driven behavior is exercised separately against a real `claude`.
"""

from __future__ import annotations

import re
import unittest

from .harness import (
    _gate_matches,
    _parse_claude_reply,
    _parse_cursor_reply,
    _shquote,
    _submit_probe,
    _tail_delta,
)


class TailDeltaTests(unittest.TestCase):
    def test_returns_appended_suffix(self) -> None:
        before = ["a", "b", "c"]
        after = ["a", "b", "c", "d", "e"]
        assert _tail_delta(before, after) == "d\ne"

    def test_trims_blank_edges(self) -> None:
        before = ["a"]
        after = ["a", "", "  ", "x", "y", "", ""]
        assert _tail_delta(before, after) == "x\ny"

    def test_redrawn_viewport_diverges_midway(self) -> None:
        before = ["p", "q", "OLD"]
        after = ["p", "q", "NEW", "z"]
        assert _tail_delta(before, after) == "NEW\nz"

    def test_no_change_is_empty(self) -> None:
        assert _tail_delta(["a", "b"], ["a", "b"]) == ""


class SubmitProbeTests(unittest.TestCase):
    def test_first_line_capped(self) -> None:
        assert _submit_probe("hello world\nsecond") == "hello world"

    def test_long_line_truncated_to_24(self) -> None:
        assert len(_submit_probe("x" * 100)) == 24


class ShQuoteTests(unittest.TestCase):
    def test_plain(self) -> None:
        assert _shquote("/var/repo") == "'/var/repo'"

    def test_embedded_quote(self) -> None:
        assert _shquote("a'b") == "'a'\\''b'"


class GateMatchTests(unittest.TestCase):
    def test_substring(self) -> None:
        assert _gate_matches("trust", "do you trust this folder")
        assert not _gate_matches("trust", "nothing here")

    def test_regex(self) -> None:
        assert _gate_matches(re.compile(r"hooks?\b"), "review hooks")


class ClaudeReplyTests(unittest.TestCase):
    def test_extracts_last_marker_block(self) -> None:
        transcript = (
            "❯ first question\n"
            "⏺ first answer\n"
            "✻ Churned for 1s\n"
            "❯ second question\n"
            "⏺ the real answer\n"
            "  spanning two lines\n"
            "✻ Baked for 2s"
        )
        assert (
            _parse_claude_reply(transcript)
            == "the real answer\n  spanning two lines"
        )

    def test_falls_back_when_no_marker(self) -> None:
        assert _parse_claude_reply("  plain text  ") == "plain text"


class CursorReplyTests(unittest.TestCase):
    # Grounded against a real cursor-agent 2026.06 session (issue #1987): the
    # echoed prompt, a shell tool block, the answer, then the input-box footer.
    def test_extracts_answer_after_tool_block(self) -> None:
        transcript = (
            "  Run `ls` in this directory, then reply with just the count.\n"
            "\n"
            "  $ ls -1 /private/tmp | wc -l 24s\n"
            "    1894\n"
            "\n"
            "  1894\n"
            "\n"
            " ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\n"
            "  → Add a follow-up\n"
            " ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\n"
            "  Composer 2.5 Fast · 13%\n"
            "  /private/tmp"
        )
        assert _parse_cursor_reply(transcript) == "1894"

    def test_plain_turn_drops_prompt_echo(self) -> None:
        transcript = (
            "  Reply with exactly: hello from cursor.\n"
            "\n"
            " ⠘⠤ Composing\n"
            "  hello from cursor\n"
            "\n"
            " ▄▄▄▄▄▄▄▄\n"
            "  → Add a follow-up\n"
            " ▀▀▀▀▀▀▀▀"
        )
        assert _parse_cursor_reply(transcript) == "hello from cursor"

    def test_multi_paragraph_answer_survives(self) -> None:
        transcript = (
            "  question\n"
            "\n"
            "  first paragraph\n"
            "\n"
            "  second paragraph\n"
            "\n"
            " ▄▄▄▄▄▄▄▄"
        )
        assert _parse_cursor_reply(transcript) == "first paragraph\n\nsecond paragraph"

    def test_falls_back_when_nothing_survives(self) -> None:
        assert _parse_cursor_reply("  $ ls 1s\n    out\n") == "$ ls 1s\n    out"

if __name__ == "__main__":
    unittest.main()
