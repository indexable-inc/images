#!/usr/bin/env python3
import argparse
import os
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path


def run_text(args: list[str], cwd: Path | None = None) -> str:
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def git_root(path: Path) -> Path:
    probe = path if path.is_dir() else path.parent
    return Path(run_text(["git", "-C", str(probe), "rev-parse", "--show-toplevel"]))


def ignored_paths(root: Path, source: Path) -> list[str]:
    relative = os.path.relpath(source.resolve(), root)
    output = subprocess.check_output(
        [
            "git",
            "-C",
            str(root),
            "ls-files",
            "--ignored",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            relative,
        ]
    )
    paths = [item.decode() for item in output.split(b"\0") if item]
    unsafe = [path for path in paths if path.startswith("/") or ".." in Path(path).parts]
    if unsafe:
        raise SystemExit(f"refusing unsafe git path from ignored set: {unsafe[0]}")
    return paths


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Stream git-ignored files into an ix shell workspace."
    )
    parser.add_argument("vm", help="ix VM name passed to `ix shell`")
    parser.add_argument(
        "source",
        type=Path,
        help="Ignored file or directory to copy from the current Git checkout",
    )
    parser.add_argument(
        "--dest",
        default="/work/ix",
        help="Destination directory inside the VM, default: /work/ix",
    )
    args = parser.parse_args()

    source = args.source.resolve()
    if not source.exists():
        parser.error(f"source does not exist: {source}")

    root = git_root(source)
    paths = ignored_paths(root, source)
    if not paths:
        print(f"no git-ignored files found under {source}", file=sys.stderr)
        return 2

    with tempfile.NamedTemporaryFile("wb") as list_file:
        for path in paths:
            list_file.write(path.encode())
            list_file.write(b"\0")
        list_file.flush()

        tar = subprocess.Popen(
            ["tar", "-C", str(root), "--null", "-T", list_file.name, "-cpf", "-"],
            stdout=subprocess.PIPE,
        )
        assert tar.stdout is not None
        remote = subprocess.Popen(
            [
                "ix",
                "shell",
                args.vm,
                "--",
                "sh",
                "-lc",
                f"mkdir -p -- {shlex.quote(args.dest)} && tar -xpf - -C {shlex.quote(args.dest)}",
            ],
            stdin=tar.stdout,
        )
        tar.stdout.close()
        remote_status = remote.wait()
        tar_status = tar.wait()

    if tar_status != 0:
        return tar_status
    if remote_status != 0:
        return remote_status

    print(f"copied {len(paths)} git-ignored path(s) into {args.vm}:{args.dest}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
