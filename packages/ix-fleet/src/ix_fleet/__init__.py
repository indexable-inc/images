#!/usr/bin/env python3
from __future__ import annotations

import argparse
import asyncio
import json
import os
import select
import shlex
import string
import subprocess
import sys
import time
import typing
from pathlib import Path

from pydantic import BaseModel, ConfigDict, Field, ValidationError, model_validator


def empty_str_list() -> list[str]:
    return []


def empty_int_list() -> list[int]:
    return []


def empty_str_dict() -> dict[str, str]:
    return {}


class ReplacementImage(BaseModel):
    model_config = ConfigDict(extra="forbid")

    imageName: str = Field(min_length=1)
    imageTag: str = Field(min_length=1)
    destination: str = Field(min_length=1)
    source: str = Field(min_length=1)
    sourceDrv: str = Field(min_length=1)


class SwitchSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    target: str = Field(min_length=1)
    buildOn: typing.Literal["auto", "local", "remote"] = "auto"
    buildVm: str | None = None
    sourceInstallable: str = Field(min_length=1)
    overrideInputs: dict[str, str] = Field(default_factory=empty_str_dict)


class HealthCheck(BaseModel):
    model_config = ConfigDict(extra="forbid", populate_by_name=True)

    description: str = Field(min_length=1)
    command: list[str] = Field(min_length=1)
    timeoutSec: int = Field(ge=1)
    attempts: int = Field(ge=1)
    intervalSec: int = Field(ge=0)
    requiresIpv4: bool = False
    # `from` is a Python keyword, so accept it under an alias and store as
    # `from_` on the model.
    from_: typing.Literal["guest", "host"] = Field(alias="from")


class FleetNode(BaseModel):
    model_config = ConfigDict(extra="forbid")

    name: str = Field(min_length=1)
    baseName: str = Field(min_length=1)
    replicaIndex: int | None = None
    system: str = Field(min_length=1)
    switch: SwitchSpec
    bootstrapImage: str = Field(min_length=1)
    replacementImage: ReplacementImage
    region: str = Field(min_length=1)
    ipv4: bool
    snapshot: bool
    recreateOnUp: bool = False
    tags: list[str] = Field(default_factory=empty_str_list)
    groups: list[str] = Field(default_factory=empty_str_list)
    env: dict[str, str] = Field(default_factory=empty_str_dict)
    l7ProxyPorts: list[int] = Field(default_factory=empty_int_list)
    dependsOn: list[str] = Field(default_factory=empty_str_list)
    healthChecks: dict[str, HealthCheck] = Field(default_factory=dict)


class SecretProvider(BaseModel):
    model_config = ConfigDict(extra="allow")

    type: str = Field(min_length=1)
    mountRoot: str = Field(min_length=1)


class SecretSpec(BaseModel):
    model_config = ConfigDict(extra="allow")

    key: str = Field(min_length=1)
    path: str = Field(min_length=1)


class FleetSecrets(BaseModel):
    model_config = ConfigDict(extra="forbid")

    provider: SecretProvider
    values: dict[str, SecretSpec] = Field(default_factory=dict)


class FleetPlan(BaseModel):
    model_config = ConfigDict(extra="forbid")

    order: list[str]
    nodes: dict[str, FleetNode]
    secrets: FleetSecrets = Field(
        default_factory=lambda: FleetSecrets(
            provider=SecretProvider(type="runtime-directory", mountRoot="/run/secrets"),
        )
    )

    @model_validator(mode="after")
    def validate_graph(self) -> typing.Self:
        ordered: set[str] = set()
        for name in self.order:
            if name in ordered:
                raise ValueError(f"order contains duplicate node {name!r}")
            if name not in self.nodes:
                raise ValueError(f"order references missing node {name!r}")
            ordered.add(name)
        for key, node in self.nodes.items():
            if key != node.name:
                raise ValueError(f"node key {key!r} does not match name {node.name!r}")
            for dep in node.dependsOn:
                if dep not in self.nodes:
                    raise ValueError(f"node {key!r} depends on unknown node {dep!r}")
        missing_order = sorted(set(self.nodes) - ordered)
        if missing_order:
            label = "node" if len(missing_order) == 1 else "nodes"
            names = ", ".join(repr(name) for name in missing_order)
            raise ValueError(f"order is missing {label} {names}")
        return self


def load_plan(path: Path) -> FleetPlan:
    return FleetPlan.model_validate_json(path.read_text())


def selected_names(plan: FleetPlan, selectors: list[str]) -> set[str]:
    if not selectors:
        return set(plan.order)

    selected: set[str] = set()
    for selector in selectors:
        if selector.startswith("@"):
            tag = selector[1:]
            if not tag:
                raise ValueError("empty tag selector")
            selected.update(name for name, node in plan.nodes.items() if tag in node.tags)
        elif selector in plan.nodes:
            selected.add(selector)
        else:
            raise ValueError(f"unknown node {selector!r}")
    return selected


def selected_nodes(plan: FleetPlan, selectors: list[str]) -> list[FleetNode]:
    selected = selected_names(plan, selectors)
    ordered: list[FleetNode] = []
    visiting: set[str] = set()
    visited: set[str] = set()

    def visit(name: str) -> None:
        if name in visited:
            return
        if name in visiting:
            raise ValueError(f"dependency cycle at {name!r}")
        visiting.add(name)
        node = plan.nodes[name]
        for dep in node.dependsOn:
            visit(dep)
        visiting.remove(name)
        visited.add(name)
        ordered.append(node)

    for name in plan.order:
        if name in selected:
            visit(name)

    return ordered


def step(message: str) -> None:
    print(message, flush=True)


class CliError(RuntimeError):
    def __init__(self, command: list[str], returncode: int, stdout: str, stderr: str) -> None:
        self.command = command
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr
        self.output = stdout + stderr
        detail = self.output.strip()
        if len(detail) > 2000:
            detail = detail[-2000:]
        message = f"command failed with exit status {returncode}: {' '.join(command)}"
        if detail:
            message = f"{message}\n{detail}"
        super().__init__(message)


class CliTimeoutError(RuntimeError):
    def __init__(self, command: list[str], timeout: int, stdout: str, stderr: str) -> None:
        self.command = command
        self.timeout = timeout
        self.stdout = stdout
        self.stderr = stderr
        self.output = stdout + stderr
        super().__init__(f"command timed out after {timeout}s: {' '.join(command)}")


def run_cli(
    command: list[str],
    *,
    dry_run: bool,
    timeout: int | None = None,
) -> str:
    step("+ " + " ".join(command))
    if dry_run:
        return ""

    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    assert process.stdout is not None
    assert process.stderr is not None
    streams: dict[typing.IO[bytes], tuple[str, typing.TextIO]] = {
        process.stdout: ("stdout", sys.stdout),
        process.stderr: ("stderr", sys.stderr),
    }
    chunks = {
        "stdout": [],
        "stderr": [],
    }
    deadline = None if timeout is None else time.monotonic() + timeout

    while streams:
        wait = 0.2
        if deadline is not None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                assert timeout is not None
                process.kill()
                process.wait()
                raise CliTimeoutError(
                    command,
                    timeout,
                    "".join(chunks["stdout"]),
                    "".join(chunks["stderr"]),
                )
            wait = min(wait, remaining)

        readable, _, _ = select.select(list(streams), [], [], wait)
        if not readable and process.poll() is not None:
            readable = list(streams)

        for stream in readable:
            data = os.read(stream.fileno(), 4096)
            if data == b"":
                streams.pop(stream, None)
                continue
            name, target = streams[stream]
            text = data.decode(errors="replace")
            chunks[name].append(text)
            print(text, end="", file=target, flush=True)

    returncode = process.wait()
    stdout = "".join(chunks["stdout"])
    stderr = "".join(chunks["stderr"])
    if returncode != 0:
        raise CliError(command, returncode, stdout, stderr)
    return stdout


async def wait_node_ready(node: FleetNode, *, dry_run: bool) -> None:
    command = [
        "ix",
        "shell",
        node.name,
        "--",
        "/run/current-system/sw/bin/bash",
        "-lc",
        (
            "set -euo pipefail\n"
            "export PATH=/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:$PATH\n"
            "if command -v systemctl >/dev/null 2>&1; then\n"
            "  systemctl start nix-daemon.socket >/dev/null 2>&1 || true\n"
            "fi\n"
            "nix --extra-experimental-features nix-command store info >/dev/null"
        ),
    ]
    if dry_run:
        step("+ wait until bootstrap is ready: " + " ".join(command))
        return

    step(f"waiting for {node.name} bootstrap")
    deadline = asyncio.get_running_loop().time() + 180
    last_error = ""
    while asyncio.get_running_loop().time() < deadline:
        result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if result.returncode == 0:
            return
        last_error = (result.stderr or result.stdout).strip()
        await asyncio.sleep(2)

    raise RuntimeError(f"{node.name} bootstrap did not become ready: {last_error}")


async def push_replacement_image(node: FleetNode, *, dry_run: bool) -> str:
    image = node.replacementImage
    source = image.source
    if not dry_run:
        out = run_cli(["nix-store", "--realise", image.sourceDrv], dry_run=False)
        realised = [line.strip() for line in out.splitlines() if line.strip()]
        if realised:
            source = realised[-1]
        if not Path(source).exists():
            raise RuntimeError(f"OCI image derivation did not realise to an existing path: {source}")

    out = run_cli(["ix", "image", "push", source, image.destination], dry_run=dry_run)
    refs = [line.strip() for line in out.splitlines() if line.strip()]
    return refs[-1] if refs else image.destination


async def list_nodes() -> list[dict[str, typing.Any]]:
    out = run_cli(["ix", "ls", "--output", "json"], dry_run=False)
    rows = json.loads(out)
    if not isinstance(rows, list):
        raise TypeError("ix ls --output json must return a list")
    return [row for row in rows if isinstance(row, dict)]


def find_node(rows: list[dict[str, typing.Any]], name: str) -> dict[str, typing.Any] | None:
    return next((row for row in rows if row.get("name") == name), None)


async def create_node(node: FleetNode, image: str, *, dry_run: bool) -> None:
    command = [
        "ix",
        "new",
        image,
        "--name",
        node.name,
        "--region",
        node.region,
        "--no-shell",
    ]
    for name, value in sorted(node.env.items()):
        command.extend(["--env", f"{name}={value}"])
    for port in node.l7ProxyPorts:
        command.extend(["--l7-proxy-port", str(port)])
    if node.ipv4:
        command.append("--ipv4")
    run_cli(command, dry_run=dry_run)


async def ensure_node(node: FleetNode, *, dry_run: bool) -> bool:
    if dry_run:
        step(f"ensure {node.name} exists from {node.bootstrapImage}")
        return False

    existing = find_node(await list_nodes(), node.name)
    if existing is None:
        await create_node(node, node.bootstrapImage, dry_run=dry_run)
        await wait_node_ready(node, dry_run=dry_run)
        return True

    if existing.get("status") == "failed":
        run_cli(["ix", "rm", "--force", node.name], dry_run=dry_run)
        await create_node(node, node.bootstrapImage, dry_run=dry_run)
        await wait_node_ready(node, dry_run=dry_run)
        return True

    if existing.get("status") == "running":
        await wait_node_ready(node, dry_run=dry_run)
        return False

    run_cli(["ix", "start", node.name], dry_run=dry_run)
    await wait_node_ready(node, dry_run=dry_run)
    return False


async def snapshot_node(node: FleetNode, *, dry_run: bool) -> None:
    run_cli(["ix", "snapshot", "create", node.name], dry_run=dry_run)


async def switch_node(node: FleetNode, *, dry_run: bool) -> None:
    if node.switch.buildOn == "local":
        # ix switch --build-on local expects the system out-path already in the
        # local store. Realize the flake installable first so the path is valid.
        run_cli(
            ["nix", "build", "--no-link", "--print-out-paths", node.switch.sourceInstallable],
            dry_run=dry_run,
        )
    step(f"switching {node.name} (build-on={node.switch.buildOn})")
    run_cli(
        ["ix", "switch", node.name, node.switch.target, "--build-on", node.switch.buildOn],
        dry_run=dry_run,
        timeout=1800,
    )


def is_existing_group_error(error: CliError) -> bool:
    return "already exists" in error.output.lower()


def is_existing_member_error(error: CliError) -> bool:
    output = error.output.lower()
    return "already" in output and ("member" in output or "group" in output)


async def ensure_group(group: str, *, dry_run: bool) -> None:
    if dry_run:
        step(f"ensure east-west group {group} exists")
        return

    try:
        run_cli(["ix", "group", "create", group], dry_run=dry_run)
    except CliError as error:
        if is_existing_group_error(error):
            step(f"east-west group {group} already exists")
            return
        raise


async def ensure_node_groups(node: FleetNode, *, dry_run: bool) -> None:
    for group in sorted(node.groups):
        await ensure_group(group, dry_run=dry_run)
        try:
            run_cli(["ix", "group", "add", group, node.name], dry_run=dry_run)
        except CliError as error:
            if is_existing_member_error(error):
                step(f"{node.name} is already in east-west group {group}")
                continue
            raise


async def bootstrap_node(node: FleetNode, *, dry_run: bool) -> None:
    await ensure_node(node, dry_run=dry_run)
    await ensure_node_groups(node, dry_run=dry_run)


def dependency_batches(plan: FleetPlan, selectors: list[str]) -> list[list[FleetNode]]:
    remaining = {node.name for node in selected_nodes(plan, selectors)}
    completed: set[str] = set()
    batches: list[list[FleetNode]] = []
    while remaining:
        batch = [
            plan.nodes[name]
            for name in plan.order
            if name in remaining and all(dep not in remaining or dep in completed for dep in plan.nodes[name].dependsOn)
        ]
        if not batch:
            names = ", ".join(sorted(remaining))
            raise ValueError(f"dependency cycle among selected nodes: {names}")
        batches.append(batch)
        for node in batch:
            remaining.remove(node.name)
            completed.add(node.name)
    return batches


def is_missing_node_error(error: CliError) -> bool:
    return "not found" in error.output.lower()


async def remove_node(node: FleetNode, *, dry_run: bool) -> None:
    if dry_run:
        step(f"remove {node.name}")
        return
    try:
        run_cli(["ix", "rm", "--force", node.name], dry_run=dry_run)
    except CliError as error:
        if is_missing_node_error(error):
            step(f"{node.name} is already absent")
            return
        raise


def _stringify_env_value(value: typing.Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return ""
    if isinstance(value, (str, int, float)):
        return str(value)
    return json.dumps(value)


def node_env_vars(node: FleetNode, row: dict[str, typing.Any] | None) -> dict[str, str]:
    env = {"IX_NODE": node.name}
    if row is not None:
        for key, value in row.items():
            env[f"IX_NODE_{key.upper()}"] = _stringify_env_value(value)
    return env


def expand_host_command(command: list[str], env: dict[str, str]) -> list[str]:
    return [string.Template(arg).safe_substitute(env) for arg in command]


async def run_health_check(
    node: FleetNode,
    check_name: str,
    check: HealthCheck,
    *,
    dry_run: bool,
) -> None:
    command = ["ix", "shell", node.name, "--", *check.command] if check.from_ == "guest" else check.command

    if dry_run:
        step(f"+ health {node.name}/{check_name} ({check.from_}): {shlex.join(command)}")
        return

    step(f"checking {node.name}/{check_name} ({check.from_}): {check.description}")
    last_error = ""
    for attempt in range(1, check.attempts + 1):
        env: dict[str, str] | None = None
        if check.from_ == "host":
            row = find_node(await list_nodes(), node.name)
            host_env = node_env_vars(node, row)
            if check.requiresIpv4 and not host_env.get("IX_NODE_IPV4"):
                last_error = "ix ls did not report IX_NODE_IPV4 yet"
                if attempt < check.attempts:
                    await asyncio.sleep(check.intervalSec)
                continue
            env = {**os.environ, **host_env}
            command = expand_host_command(check.command, host_env)

        try:
            result = subprocess.run(
                command,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=check.timeoutSec,
                env=env,
            )
        except subprocess.TimeoutExpired as error:
            stdout = error.stdout or ""
            stderr = error.stderr or ""
            last_error = f"timed out after {check.timeoutSec}s\n{stdout}{stderr}".strip()
        else:
            if result.returncode == 0:
                step(f"healthy {node.name}/{check_name}")
                return
            last_error = (result.stdout + result.stderr).strip()

        if attempt < check.attempts:
            await asyncio.sleep(check.intervalSec)

    detail = f": {last_error}" if last_error else ""
    raise RuntimeError(
        f"{node.name}/{check_name} health check failed after {check.attempts} attempts{detail}"
    )


async def run_node_health_checks(node: FleetNode, *, dry_run: bool) -> None:
    for check_name in sorted(node.healthChecks):
        await run_health_check(
            node,
            check_name,
            node.healthChecks[check_name],
            dry_run=dry_run,
        )


def default_source_root(cwd: Path) -> Path:
    try:
        out = subprocess.check_output(
            ["git", "-C", str(cwd), "rev-parse", "--show-toplevel"],
            text=True,
        )
        return Path(out.strip()).resolve()
    except (OSError, subprocess.CalledProcessError):
        return cwd.resolve()


def default_source_workdir(cwd: Path, source_root: Path) -> Path:
    try:
        return cwd.resolve().relative_to(source_root.resolve())
    except ValueError:
        return Path(".")


MAX_SWITCH_RETRIES = 3
RETRY_DELAY_SECS = 10


async def switch_node_from_source(
    node: FleetNode,
    source_root: Path,
    source_workdir: Path,
    *,
    dry_run: bool,
) -> None:
    command = [
        "ix",
        "switch",
        node.name,
        node.switch.sourceInstallable,
        "--build-on",
        "remote",
        "--source",
        str(source_root),
        "--source-workdir",
        str(source_workdir),
    ]
    if node.switch.buildVm is not None:
        command.extend(["--build-vm", node.switch.buildVm])
    for name, path in sorted(node.switch.overrideInputs.items()):
        command.extend(["--override-input", f"{name}={path}"])

    for attempt in range(1, MAX_SWITCH_RETRIES + 1):
        try:
            step(f"switching {node.name} from source (attempt {attempt}/{MAX_SWITCH_RETRIES})")
            run_cli(command, dry_run=dry_run, timeout=3600)
            return
        except (CliError, CliTimeoutError) as e:
            error_msg = e.output or str(e)
            if "stream framing error" in error_msg and attempt < MAX_SWITCH_RETRIES:
                step(f"transient error, retrying in {RETRY_DELAY_SECS}s: {error_msg[:100]}")
                await asyncio.sleep(RETRY_DELAY_SECS)
            else:
                raise


async def replace_node(node: FleetNode, image: str, *, dry_run: bool) -> None:
    command = [
        "ix",
        "new",
        image,
        "--name",
        node.name,
        "--region",
        node.region,
        "--no-shell",
    ]
    for name, value in sorted(node.env.items()):
        command.extend(["--env", f"{name}={value}"])
    for port in node.l7ProxyPorts:
        command.extend(["--l7-proxy-port", str(port)])
    if node.ipv4:
        command.append("--ipv4")
    run_cli(command, dry_run=dry_run)


async def up_node(node: FleetNode, image: str, *, dry_run: bool) -> None:
    if dry_run:
        verb = "recreate" if node.recreateOnUp else "ensure"
        step(f"{verb} {node.name} from uploaded image {image}")
        return

    existing = find_node(await list_nodes(), node.name)
    if existing is not None and node.recreateOnUp:
        run_cli(["ix", "rm", "--force", node.name], dry_run=dry_run)
        await create_node(node, image, dry_run=dry_run)
        return

    if existing is None:
        await create_node(node, image, dry_run=dry_run)
        return

    if existing.get("status") == "failed":
        run_cli(["ix", "rm", "--force", node.name], dry_run=dry_run)
        await create_node(node, image, dry_run=dry_run)
        return

    if existing.get("status") != "running":
        run_cli(["ix", "start", node.name], dry_run=dry_run)


async def cmd_diff(plan: FleetPlan, args: argparse.Namespace) -> None:
    for node in selected_nodes(plan, args.on):
        if node.switch.buildOn == "remote":
            print(f"{node.name}\twant {node.switch.sourceInstallable} (remote source)")
        else:
            print(f"{node.name}\twant {node.switch.target} ({node.switch.buildOn})")


async def cmd_switch(plan: FleetPlan, args: argparse.Namespace) -> None:
    source_root = (args.source_root or default_source_root(Path.cwd())).resolve()
    source_workdir = args.source_workdir or default_source_workdir(Path.cwd(), source_root)
    for node in selected_nodes(plan, args.on):
        created = await ensure_node(node, dry_run=args.dry_run)
        await ensure_node_groups(node, dry_run=args.dry_run)
        if not created and node.snapshot and not args.no_snapshot:
            await snapshot_node(node, dry_run=args.dry_run)
        if node.switch.buildOn == "remote":
            await switch_node_from_source(
                node,
                source_root,
                source_workdir,
                dry_run=args.dry_run,
            )
        else:
            await switch_node(node, dry_run=args.dry_run)
        if not args.skip_health:
            await run_node_health_checks(node, dry_run=args.dry_run)


async def cmd_replace(plan: FleetPlan, args: argparse.Namespace) -> None:
    for node in selected_nodes(plan, args.on):
        image = node.replacementImage.destination
        if not args.skip_push:
            image = await push_replacement_image(node, dry_run=args.dry_run)
        await replace_node(node, image, dry_run=args.dry_run)
        await ensure_node_groups(node, dry_run=args.dry_run)
        if not args.skip_health:
            await run_node_health_checks(node, dry_run=args.dry_run)


async def cmd_up(plan: FleetPlan, args: argparse.Namespace) -> None:
    for node in selected_nodes(plan, args.on):
        image = node.replacementImage.destination
        if not args.skip_push:
            image = await push_replacement_image(node, dry_run=args.dry_run)
        await up_node(node, image, dry_run=args.dry_run)
        await ensure_node_groups(node, dry_run=args.dry_run)
        if not args.skip_health:
            await run_node_health_checks(node, dry_run=args.dry_run)


async def cmd_health(plan: FleetPlan, args: argparse.Namespace) -> None:
    for node in selected_nodes(plan, args.on):
        await run_node_health_checks(node, dry_run=args.dry_run)


async def cmd_bootstrap(plan: FleetPlan, args: argparse.Namespace) -> None:
    for batch in dependency_batches(plan, args.on):
        await asyncio.gather(*(bootstrap_node(node, dry_run=args.dry_run) for node in batch))


async def cmd_down(plan: FleetPlan, args: argparse.Namespace) -> None:
    failures: list[str] = []
    for node in reversed(selected_nodes(plan, args.on)):
        try:
            await remove_node(node, dry_run=args.dry_run)
        except (CliError, OSError) as error:
            failures.append(f"{node.name}: {error}")
    if failures:
        raise RuntimeError("failed to remove fleet nodes: " + "; ".join(failures))


def parser() -> argparse.ArgumentParser:
    def add_common_options(target: argparse.ArgumentParser, *, defaults: bool) -> None:
        target.add_argument(
            "--on",
            action="append",
            default=[] if defaults else argparse.SUPPRESS,
            metavar="NODE_OR_@TAG",
        )
        target.add_argument(
            "--dry-run",
            action="store_true",
            default=False if defaults else argparse.SUPPRESS,
        )

    p = argparse.ArgumentParser(prog="ix-fleet")
    p.add_argument("--plan", required=True, type=Path)
    add_common_options(p, defaults=True)

    sub = p.add_subparsers(dest="command", required=True)
    bootstrap = sub.add_parser("bootstrap")
    add_common_options(bootstrap, defaults=False)
    down = sub.add_parser("down")
    add_common_options(down, defaults=False)
    plan = sub.add_parser("plan")
    add_common_options(plan, defaults=False)
    diff = sub.add_parser("diff")
    add_common_options(diff, defaults=False)
    health = sub.add_parser("health")
    add_common_options(health, defaults=False)
    switch = sub.add_parser("switch")
    add_common_options(switch, defaults=False)
    switch.add_argument("--no-snapshot", action="store_true")
    switch.add_argument("--skip-health", action="store_true")
    switch.add_argument("--source-root", type=Path)
    switch.add_argument("--source-workdir", type=Path)
    replace = sub.add_parser("replace")
    add_common_options(replace, defaults=False)
    replace.add_argument("--skip-push", action="store_true")
    replace.add_argument("--skip-health", action="store_true")
    up = sub.add_parser("up")
    add_common_options(up, defaults=False)
    up.add_argument("--skip-push", action="store_true")
    up.add_argument("--skip-health", action="store_true")
    return p


async def main() -> None:
    args = parser().parse_args()
    plan = load_plan(args.plan)
    if args.command == "plan":
        nodes = [node.model_dump() for node in selected_nodes(plan, args.on)]
        print(json.dumps({"nodes": nodes}, indent=2))
    elif args.command == "bootstrap":
        await cmd_bootstrap(plan, args)
    elif args.command == "down":
        await cmd_down(plan, args)
    elif args.command == "diff":
        await cmd_diff(plan, args)
    elif args.command == "health":
        await cmd_health(plan, args)
    elif args.command == "switch":
        await cmd_switch(plan, args)
    elif args.command == "replace":
        await cmd_replace(plan, args)
    elif args.command == "up":
        await cmd_up(plan, args)
    else:
        raise AssertionError(args.command)


def run() -> None:
    try:
        asyncio.run(main())
    except (
        OSError,
        ValidationError,
        ValueError,
        TypeError,
        RuntimeError,
        subprocess.CalledProcessError,
    ) as error:
        print(f"ix-fleet: {error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    run()
