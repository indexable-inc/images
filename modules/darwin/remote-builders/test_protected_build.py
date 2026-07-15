#!/usr/bin/env python3
"""Regression tests for the protected Nix build boundary."""

from __future__ import annotations

import json
import os
import stat
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest.mock import patch

import protected_build


def _builder() -> protected_build.Builder:
    return protected_build.Builder(
        name="cluster-builder",
        machine=(
            "ssh-ng://builder@cluster-builder x86_64-linux "
            "/etc/nix/cluster_ed25519 32 4 benchmark,kvm benchmark "
            "c3NoLWVkMjU1MTkgQUFBQQ=="
        ),
    )


def _policy() -> protected_build.Policy:
    builder = _builder()
    return protected_build.Policy(
        max_ttl_seconds=60,
        cancel_grace_seconds=1,
        verify_timeout_seconds=1,
        builders={builder.name: builder},
    )


def _write_executable(path: Path, source: str) -> None:
    path.write_text(textwrap.dedent(source).lstrip(), encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)


class ProtectedBuildTests(unittest.TestCase):
    def test_command_disables_local_fallback(self) -> None:
        command = protected_build.build_command(
            Path("/nix/bin/nix"), [_builder()], ["--no-link", ".#check"]
        )
        assert command == [
            "/nix/bin/nix",
            "--option",
            "builders",
            _builder().machine,
            "--option",
            "max-jobs",
            "0",
            "build",
            "--no-link",
            ".#check",
        ]

    def test_selected_builders_are_fanned_out_in_one_nix_invocation(self) -> None:
        second = protected_build.Builder(
            name="cluster-builder-2",
            machine="ssh-ng://builder@cluster-builder-2 x86_64-linux - 64 4 - - -",
        )
        command = protected_build.build_command(
            Path("/nix/bin/nix"), [_builder(), second], []
        )
        assert command[3] == f"{_builder().machine}\n{second.machine}"

    def test_blank_reason_is_rejected(self) -> None:
        message: str | None = None
        try:
            protected_build.parse_arguments(
                [
                    "/policy.json",
                    "/nix/bin/nix",
                    "/usr/bin/logger",
                    "--builder",
                    "cluster-builder",
                    "--reason",
                    "   ",
                    "--ttl-seconds",
                    "10",
                ]
            )
        except protected_build.AuthorizationError as error:
            message = str(error)
        assert message is not None
        assert "nonwhitespace" in message

    def test_build_arguments_cannot_replace_the_authorized_builder(self) -> None:
        message: str | None = None
        try:
            protected_build.parse_arguments(
                [
                    "/policy.json",
                    "/nix/bin/nix",
                    "/usr/bin/logger",
                    "--builder",
                    "cluster-builder",
                    "--reason",
                    "verify cluster fix",
                    "--ttl-seconds",
                    "10",
                    "--",
                    "--builders=ssh-ng://other",
                ]
            )
        except protected_build.AuthorizationError as error:
            message = str(error)
        assert message is not None
        assert "must not override --builders" in message

    def test_policy_loader_rejects_mismatched_builder_name(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            path.write_text(
                json.dumps(
                    {
                        "version": 1,
                        "maxTtlSeconds": 60,
                        "cancelGraceSeconds": 1,
                        "verifyTimeoutSeconds": 1,
                        "builders": {
                            "declared": {
                                "name": "different",
                                "machine": "ssh-ng://root@different x86_64-linux",
                            }
                        },
                    }
                ),
                encoding="utf-8",
            )
            message: str | None = None
            try:
                protected_build.load_policy(path)
            except protected_build.PolicyError as error:
                message = str(error)
            assert message is not None
            assert "does not match" in message

    def test_run_records_reason_and_uses_only_selected_builder(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            nix = root / "nix"
            logger = root / "logger"
            argv_record = root / "argv.json"
            audit_record = root / "audit.jsonl"
            _write_executable(
                nix,
                """
                #!/usr/bin/env python3
                import json
                import os
                import pathlib
                import sys

                if "store" in sys.argv and "builds" in sys.argv:
                    print("[]")
                else:
                    pathlib.Path(os.environ["FAKE_NIX_ARGV"]).write_text(
                        json.dumps(sys.argv), encoding="utf-8"
                    )
                """,
            )
            _write_executable(
                logger,
                """
                #!/usr/bin/env python3
                import os
                import pathlib
                import sys

                path = pathlib.Path(os.environ["FAKE_AUDIT"])
                with path.open("a", encoding="utf-8") as handle:
                    handle.write(sys.argv[-1] + "\\n")
                """,
            )
            arguments = protected_build.Arguments(
                policy_path=root / "policy.json",
                nix_path=nix,
                logger_path=logger,
                builder_names=("cluster-builder",),
                reason="verify cluster fix",
                ttl_seconds=10,
                build_args=("--no-link", ".#check"),
            )
            with patch.dict(
                os.environ,
                {
                    "FAKE_AUDIT": str(audit_record),
                    "FAKE_NIX_ARGV": str(argv_record),
                },
            ):
                assert protected_build.run(arguments, _policy()) == 0

            invoked: list[str] = json.loads(argv_record.read_text(encoding="utf-8"))
            assert (
                invoked[1:8]
                == protected_build.build_command(nix, [_builder()], [])[1:8]
            )
            audit = [
                json.loads(line)
                for line in audit_record.read_text(encoding="utf-8").splitlines()
            ]
            assert [record["event"] for record in audit] == ["authorized", "completed"]
            assert all(record["reason"] == "verify cluster fix" for record in audit)

    def test_ttl_terminates_the_client_and_verifies_daemon_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            nix = root / "nix"
            logger = root / "logger"
            audit_record = root / "audit.jsonl"
            _write_executable(
                nix,
                """
                #!/usr/bin/env python3
                import time
                import sys

                if "store" in sys.argv and "builds" in sys.argv:
                    print("[]")
                else:
                    time.sleep(60)
                """,
            )
            _write_executable(
                logger,
                """
                #!/usr/bin/env python3
                import os
                import pathlib
                import sys

                path = pathlib.Path(os.environ["FAKE_AUDIT"])
                with path.open("a", encoding="utf-8") as handle:
                    handle.write(sys.argv[-1] + "\\n")
                """,
            )
            arguments = protected_build.Arguments(
                policy_path=root / "policy.json",
                nix_path=nix,
                logger_path=logger,
                builder_names=("cluster-builder",),
                reason="bounded repro",
                ttl_seconds=1,
                build_args=(".#check",),
            )
            with patch.dict(os.environ, {"FAKE_AUDIT": str(audit_record)}):
                assert protected_build.run(arguments, _policy()) == 124

            audit = [
                json.loads(line)
                for line in audit_record.read_text(encoding="utf-8").splitlines()
            ]
            assert [record["event"] for record in audit] == ["authorized", "timed_out"]


if __name__ == "__main__":
    unittest.main()
