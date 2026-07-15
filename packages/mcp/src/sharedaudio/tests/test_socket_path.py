from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

import sharedaudio


class SocketPathTest(unittest.TestCase):
    def test_macos_uses_application_support(self) -> None:
        with (
            patch.dict(os.environ, {"HOME": "/Users/test"}, clear=True),
            patch.object(sys, "platform", "darwin"),
        ):
            assert sharedaudio.socket_path() == Path(
                "/Users/test/Library/Application Support/shared-audio/control.sock"
            )

    def test_override_wins_on_macos(self) -> None:
        with (
            patch.dict(
                os.environ,
                {
                    "HOME": "/Users/test",
                    "SHARED_AUDIO_SOCKET": "/run/user/1000/audio.sock",
                },
                clear=True,
            ),
            patch.object(sys, "platform", "darwin"),
        ):
            assert sharedaudio.socket_path() == Path("/run/user/1000/audio.sock")
