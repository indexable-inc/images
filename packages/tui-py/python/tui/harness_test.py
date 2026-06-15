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
    _shquote,
    _submit_probe,
    _tail_delta,
)


class TailDeltaTests(unittest.TestCase):
    def test_returns_appended_suffix(self) -> None:
        before = ["a", "b", "c"]
        after = ["a", "b", "c", "d", "e"]
        self.assertEqual(_tail_delta(before, after), "d\ne")

    def test_trims_blank_edges(self) -> None:
        before = ["a"]
        after = ["a", "", "  ", "x", "y", "", ""]
        self.assertEqual(_tail_delta(before, after), "x\ny")

    def test_redrawn_viewport_diverges_midway(self) -> None:
        before = ["p", "q", "OLD"]
        after = ["p", "q", "NEW", "z"]
        self.assertEqual(_tail_delta(before, after), "NEW\nz")

    def test_no_change_is_empty(self) -> None:
        self.assertEqual(_tail_delta(["a", "b"], ["a", "b"]), "")


class SubmitProbeTests(unittest.TestCase):
    def test_first_line_capped(self) -> None:
        self.assertEqual(_submit_probe("hello world\nsecond"), "hello world")

    def test_long_line_truncated_to_24(self) -> None:
        self.assertEqual(len(_submit_probe("x" * 100)), 24)


class ShQuoteTests(unittest.TestCase):
    def test_plain(self) -> None:
        self.assertEqual(_shquote("/tmp/repo"), "'/tmp/repo'")

    def test_embedded_quote(self) -> None:
        self.assertEqual(_shquote("a'b"), "'a'\\''b'")


class GateMatchTests(unittest.TestCase):
    def test_substring(self) -> None:
        self.assertTrue(_gate_matches("trust", "do you trust this folder"))
        self.assertFalse(_gate_matches("trust", "nothing here"))

    def test_regex(self) -> None:
        self.assertTrue(_gate_matches(re.compile(r"hooks?\b"), "review hooks"))


class ClaudeReplyTests(unittest.TestCase):
    def test_extracts_last_marker_block(self) -> None:
        transcript = "\n".join(
            [
                "❯ first question",
                "⏺ first answer",
                "✻ Churned for 1s",
                "❯ second question",
                "⏺ the real answer",
                "  spanning two lines",
                "✻ Baked for 2s",
            ]
        )
        self.assertEqual(
            _parse_claude_reply(transcript),
            "the real answer\n  spanning two lines",
        )

    def test_falls_back_when_no_marker(self) -> None:
        self.assertEqual(_parse_claude_reply("  plain text  "), "plain text")


if __name__ == "__main__":
    unittest.main()
