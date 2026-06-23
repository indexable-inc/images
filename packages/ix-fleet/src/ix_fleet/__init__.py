#!/usr/bin/env python3
from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import shlex
import string
import subprocess
import sys
import tempfile
import typing
from pathlib import Path

from pydantic import BaseModel, ConfigDict, Field, ValidationError, model_validator

import ix_sdk


def empty_str_list() -> list[str]:
    return []


def empty_int_list() -> list[int]:
    return []


def empty_str_dict() -> dict[str, str]:
    return {}


class ReplacementImage(BaseModel):
    model_config = ConfigDict(extra="forbid")

    imageName: str = Field(min_length=1)
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
    # Names of secrets in the per-user store (`ix secret set NAME`) to attach to
    # this VM at create time. Resolved server-side and injected as env vars or
    # files exactly like `ix new --secret NAME`. References only, never values:
    # plaintext belongs in the store, not the fleet plan.
    secrets: list[str] = Field(default_factory=empty_str_list)
    # Skip auto-attach of secrets marked `--default` in the store, matching
    # `ix new --no-default-secrets`.
    noDefaultSecrets: bool = False
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


_client: ix_sdk.Client | None = None


def client() -> ix_sdk.Client:
    """Lazily construct the SDK client.

    `ix_sdk.Client()` resolves IX_TOKEN and the base URL from the environment,
    the same inputs the `ix` CLI used; constructing it lazily keeps `--dry-run`
    runs (which never touch the API) from requiring a token.
    """
    global _client
    if _client is None:
        _client = ix_sdk.Client()
    return _client


def status_str(status: ix_sdk.BranchStatus) -> str:
    """Render a BranchStatus as the lowercase string the `ix ls` JSON used, so
    host health-check env vars (IX_NODE_STATUS) keep their previous shape."""
    return {
        ix_sdk.BranchStatus.RUNNING: "running",
        ix_sdk.BranchStatus.STOPPED: "stopped",
        ix_sdk.BranchStatus.FAILED: "failed",
    }.get(status, str(status))


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


async def run_cli(
    command: list[str],
    *,
    dry_run: bool,
    timeout: int | None = None,
    cwd: Path | None = None,
) -> str:
    step("+ " + " ".join(command))
    if dry_run:
        return ""

    process = await asyncio.create_subprocess_exec(
        *command,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        cwd=str(cwd) if cwd is not None else None,
    )
    assert process.stdout is not None
    assert process.stderr is not None

    async def tee(reader: asyncio.StreamReader, target: typing.TextIO) -> str:
        chunks: list[str] = []
        while True:
            data = await reader.read(4096)
            if not data:
                return "".join(chunks)
            text = data.decode(errors="replace")
            chunks.append(text)
            print(text, end="", file=target, flush=True)

    # Drain both pipes concurrently so the child never blocks on a full buffer
    # while we await its exit.
    stdout_task = asyncio.ensure_future(tee(process.stdout, sys.stdout))
    stderr_task = asyncio.ensure_future(tee(process.stderr, sys.stderr))

    timed_out = False
    try:
        await asyncio.wait_for(process.wait(), timeout)
    except TimeoutError:
        timed_out = True
        process.kill()

    returncode = await process.wait()
    stdout = await stdout_task
    stderr = await stderr_task

    if timed_out:
        assert timeout is not None
        raise CliTimeoutError(command, timeout, stdout, stderr)
    if returncode != 0:
        raise CliError(command, returncode, stdout, stderr)
    return stdout


BOOTSTRAP_PROBE_SCRIPT = (
    "set -euo pipefail\n"
    "export PATH=/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:$PATH\n"
    "if command -v systemctl >/dev/null 2>&1; then\n"
    "  systemctl start nix-daemon.socket >/dev/null 2>&1 || true\n"
    "fi\n"
    "nix --extra-experimental-features nix-command store info >/dev/null"
)


async def wait_node_ready(node: FleetNode, *, dry_run: bool) -> None:
    if dry_run:
        step(f"+ wait until {node.name} bootstrap is ready (guest nix store probe)")
        return

    step(f"waiting for {node.name} bootstrap")
    c = client()
    deadline = asyncio.get_running_loop().time() + 180
    last_error = ""
    while asyncio.get_running_loop().time() < deadline:
        branch = await c.find_by_name(node.name)
        if branch is None:
            last_error = f"{node.name} not found"
        else:
            # check=False: a not-yet-ready store info is expected while we poll,
            # so inspect the exit code instead of raising CommandError.
            result = await branch.bash(BOOTSTRAP_PROBE_SCRIPT, check=False, quiet=True)
            if result.exit_code == 0:
                return
            last_error = (result.stderr or result.stdout).strip()
        await asyncio.sleep(2)

    raise RuntimeError(f"{node.name} bootstrap did not become ready: {last_error}")


async def push_replacement_image(node: FleetNode, *, dry_run: bool) -> str:
    image = node.replacementImage
    if dry_run:
        step(f"+ realise {image.sourceDrv} and image push -> {image.destination}")
        return image.destination

    # Realising the OCI image is host-side nix work; the push itself goes through
    # the SDK (sdk-core owns the chunk/dedup/upload pipeline).
    source = image.source
    out = await run_cli(["nix-store", "--realise", image.sourceDrv], dry_run=False)
    realised = [line.strip() for line in out.splitlines() if line.strip()]
    if realised:
        source = realised[-1]
    if not await asyncio.to_thread(Path(source).exists):
        raise RuntimeError(f"OCI image derivation did not realise to an existing path: {source}")

    step(f"pushing {image.destination} from {source}")
    return await client().image_push(source, image.destination, region=node.region)


async def list_nodes() -> list[ix_sdk.BranchInfo]:
    return await client().branches()


def find_node(rows: list[ix_sdk.BranchInfo], name: str) -> ix_sdk.BranchInfo | None:
    return next((row for row in rows if row.name == name), None)


async def verify_secrets_available(plan: FleetPlan, selectors: list[str], *, dry_run: bool) -> None:
    """Fail before doing any work if a selected node references a secret that is
    not in the user store, mirroring the `ix secret check` deploy-time bridge.

    Only explicitly named references are validated: secrets marked `--default`
    attach server-side and are not declared in the plan. Skipped on dry runs,
    which make no live calls.
    """
    referenced: set[str] = set()
    for node in selected_nodes(plan, selectors):
        referenced.update(node.secrets)
    if dry_run or not referenced:
        return
    stored = {secret.name for secret in await client().list_secrets()}
    missing = sorted(referenced - stored)
    if missing:
        raise RuntimeError(
            "missing secret(s) in the store: "
            + ", ".join(missing)
            + "; store them first with `ix secret set NAME`"
        )


def secrets_note(node: FleetNode) -> str:
    parts: list[str] = []
    if node.secrets:
        parts.append(f"secrets={sorted(node.secrets)}")
    if node.noDefaultSecrets:
        parts.append("no-default-secrets")
    return f", {', '.join(parts)}" if parts else ""


async def create_node(node: FleetNode, image: str, *, dry_run: bool) -> None:
    if dry_run:
        step(
            f"+ create {node.name} from {image} "
            f"(region={node.region}, ipv4={node.ipv4}, l7={list(node.l7ProxyPorts)}"
            f"{secrets_note(node)})"
        )
        return
    await client().create(
        image,
        region=node.region,
        name=node.name,
        env=dict(sorted(node.env.items())),
        l7_proxy_ports=list(node.l7ProxyPorts),
        ipv4=node.ipv4,
        secrets=sorted(node.secrets),
        no_default_secrets=node.noDefaultSecrets,
    )


async def recreate_node(node: FleetNode, image: str, *, dry_run: bool) -> None:
    """Delete the node if present, then create it on `image`.

    `client.create` (like `ix new --name`) inserts against a UNIQUE (owner,
    name) constraint and errors if the name is taken, so replacing a node's
    image is delete-then-create, not an in-place update. In-place updates are
    `switch`; this is the image-swap path used by `replace`/`up`/failed-node
    recovery.
    """
    await remove_node(node, dry_run=dry_run)
    await create_node(node, image, dry_run=dry_run)


async def ensure_node(node: FleetNode, *, dry_run: bool) -> bool:
    if dry_run:
        step(f"ensure {node.name} exists from {node.bootstrapImage}")
        return False

    existing = find_node(await list_nodes(), node.name)
    if existing is None:
        await create_node(node, node.bootstrapImage, dry_run=dry_run)
        await wait_node_ready(node, dry_run=dry_run)
        return True

    if existing.status == ix_sdk.BranchStatus.FAILED:
        await recreate_node(node, node.bootstrapImage, dry_run=dry_run)
        await wait_node_ready(node, dry_run=dry_run)
        return True

    if existing.status == ix_sdk.BranchStatus.RUNNING:
        await wait_node_ready(node, dry_run=dry_run)
        return False

    branch = await client().find_by_name(node.name)
    if branch is not None:
        await branch.start()
    await wait_node_ready(node, dry_run=dry_run)
    return False


async def snapshot_node(node: FleetNode, *, dry_run: bool) -> None:
    if dry_run:
        step(f"+ snapshot create {node.name}")
        return
    await client().snapshot(name=node.name)


async def switch_node(node: FleetNode, *, dry_run: bool) -> None:
    if node.switch.buildOn == "local":
        # build-on=local expects the system out-path already in the local store;
        # the nix build stays a host-side step (it drives the local builder), and
        # the switch RPC itself goes through the SDK.
        await run_cli(
            ["nix", "build", "--no-link", "--print-out-paths", node.switch.sourceInstallable],
            dry_run=dry_run,
        )
    step(f"switching {node.name} (build-on={node.switch.buildOn})")
    if dry_run:
        step(f"+ switch {node.name} -> {node.switch.target} (build-on={node.switch.buildOn})")
        return
    # The SDK switch RPC has no deadline of its own; bound it like the old CLI
    # path (which passed timeout=1800) so a hung remote/local switch can't block
    # the fleet workflow forever.
    try:
        await asyncio.wait_for(
            client().switch_system(
                name=node.name,
                target=node.switch.target,
                build_on=node.switch.buildOn,
            ),
            SWITCH_TIMEOUT_SECS,
        )
    except TimeoutError as error:
        raise RuntimeError(
            f"switch of {node.name} timed out after {SWITCH_TIMEOUT_SECS}s"
        ) from error


async def ensure_group(group: str, *, dry_run: bool) -> None:
    if dry_run:
        step(f"ensure east-west group {group} exists")
        return

    try:
        await client().create_group(group)
    except ix_sdk.IxConflictError:
        step(f"east-west group {group} already exists")


async def ensure_node_groups(node: FleetNode, *, dry_run: bool) -> None:
    for group in sorted(node.groups):
        await ensure_group(group, dry_run=dry_run)
        if dry_run:
            step(f"+ add {node.name} to east-west group {group}")
            continue
        try:
            await client().add_group_member(group, node.name)
        except ix_sdk.IxConflictError:
            step(f"{node.name} is already in east-west group {group}")


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


async def remove_node(node: FleetNode, *, dry_run: bool) -> None:
    if dry_run:
        step(f"remove {node.name}")
        return
    branch = await client().find_by_name(node.name)
    if branch is None:
        step(f"{node.name} is already absent")
        return
    await branch.delete()


def node_env_vars(node: FleetNode, info: ix_sdk.BranchInfo | None) -> dict[str, str]:
    env = {"IX_NODE": node.name}
    if info is not None:
        env["IX_NODE_NAME"] = info.name
        env["IX_NODE_IMAGE"] = info.image
        env["IX_NODE_STATUS"] = status_str(info.status)
        env["IX_NODE_IPV6"] = info.ipv6
        if info.ipv4 is not None:
            env["IX_NODE_IPV4"] = info.ipv4
        if info.subdomain is not None:
            env["IX_NODE_SUBDOMAIN"] = info.subdomain
        if info.region is not None:
            env["IX_NODE_REGION"] = info.region.slug
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
    if dry_run:
        if check.from_ == "guest":
            step(f"+ health {node.name}/{check_name} (guest): exec {shlex.join(check.command)}")
        else:
            step(f"+ health {node.name}/{check_name} (host): {shlex.join(check.command)}")
        return

    step(f"checking {node.name}/{check_name} ({check.from_}): {check.description}")
    last_error = ""
    for attempt in range(1, check.attempts + 1):
        if check.from_ == "guest":
            # Run the check argv inside the VM through the SDK exec channel.
            branch = await client().find_by_name(node.name)
            if branch is None:
                last_error = f"{node.name} not found"
            else:
                try:
                    result = await asyncio.wait_for(
                        branch.exec(list(check.command), check=False, quiet=True),
                        check.timeoutSec,
                    )
                except TimeoutError:
                    last_error = f"timed out after {check.timeoutSec}s"
                else:
                    if result.exit_code == 0:
                        step(f"healthy {node.name}/{check_name}")
                        return
                    last_error = (result.stdout + result.stderr).strip()
        else:
            # Host check: run on the operator's machine with IX_NODE_* env so it
            # can probe the node from outside (public reachability, firewall).
            info = find_node(await list_nodes(), node.name)
            host_env = node_env_vars(node, info)
            if check.requiresIpv4 and not host_env.get("IX_NODE_IPV4"):
                last_error = "node has not reported IX_NODE_IPV4 yet"
                if attempt < check.attempts:
                    await asyncio.sleep(check.intervalSec)
                continue
            env = {**os.environ, **host_env}
            command = expand_host_command(check.command, host_env)
            process = await asyncio.create_subprocess_exec(
                *command,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=env,
            )
            try:
                stdout_b, stderr_b = await asyncio.wait_for(process.communicate(), check.timeoutSec)
            except TimeoutError:
                process.kill()
                await process.wait()
                last_error = f"timed out after {check.timeoutSec}s"
            else:
                if process.returncode == 0:
                    step(f"healthy {node.name}/{check_name}")
                    return
                last_error = (
                    stdout_b.decode(errors="replace") + stderr_b.decode(errors="replace")
                ).strip()

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
        return Path()


# Bound a target-based `switch` (matches the old CLI `timeout=1800`). The
# source-build switch below uses its own, longer deadline.
SWITCH_TIMEOUT_SECS = 1800
MAX_SWITCH_RETRIES = 3
RETRY_DELAY_SECS = 10


def relative_source_workdir(source_root: Path, source_workdir: Path) -> Path:
    # `ix up --workdir` is resolved relative to the uploaded source root, so an
    # absolute workdir outside that root has no valid mapping. Reject it instead
    # of forwarding a path `ix up` cannot interpret.
    workdir = source_workdir
    if workdir.is_absolute():
        try:
            workdir = workdir.relative_to(source_root)
        except ValueError:
            raise ValueError(
                f"source workdir {source_workdir} is outside source root {source_root}"
            ) from None
    return workdir


async def run_source_switch(command: list[str], source_root: Path, label: str, *, dry_run: bool) -> None:
    # `ix up` auto-uploads its working directory as the build source, so it runs
    # from `source_root`. A `stream framing error` is a transient upload/transport
    # hiccup, so retry it; anything else fails the switch.
    for attempt in range(1, MAX_SWITCH_RETRIES + 1):
        try:
            step(f"switching {label} from source (attempt {attempt}/{MAX_SWITCH_RETRIES})")
            await run_cli(command, dry_run=dry_run, timeout=3600, cwd=source_root)
            return
        except (CliError, CliTimeoutError) as e:
            error_msg = e.output or str(e)
            if "stream framing error" in error_msg and attempt < MAX_SWITCH_RETRIES:
                step(f"transient error, retrying in {RETRY_DELAY_SECS}s: {error_msg[:100]}")
                await asyncio.sleep(RETRY_DELAY_SECS)
            else:
                raise


async def switch_node_from_source(
    node: FleetNode,
    source_root: Path,
    source_workdir: Path,
    *,
    dry_run: bool,
) -> None:
    # `ix switch` was folded into `ix up` (indexable-inc/ix#4442): `ix up
    # <installable> --name <vm>` is the single-target converge path, used for a
    # node that cannot join a native multi-VM batch (see `switch_nodes_from_source`).
    workdir = relative_source_workdir(source_root, source_workdir)
    command = [
        "ix",
        "up",
        node.switch.sourceInstallable,
        "--name",
        node.name,
        "--workdir",
        str(workdir),
    ]
    if node.switch.buildVm is not None:
        command.extend(["--build-vm", node.switch.buildVm])
    for name, path in sorted(node.switch.overrideInputs.items()):
        command.extend(["--override-input", f"{name}={path}"])
    await run_source_switch(command, source_root, node.name, dry_run=dry_run)


async def switch_nodes_from_source(
    nodes: list[FleetNode],
    source_root: Path,
    source_workdir: Path,
    *,
    dry_run: bool,
) -> None:
    # The native multi-VM switch: `ix up .#a .#b .#c --build-vm <builder>` builds
    # every closure on one warm builder and activates each on its own VM. The CLI
    # rejects `--name` and derives each VM name from the installable's simple attr,
    # and shares one `--build-vm`/`--workdir`/`--override-input` set across the
    # batch, so `batch_groups` only ever passes nodes that agree on those.
    workdir = relative_source_workdir(source_root, source_workdir)
    build_vm = nodes[0].switch.buildVm
    assert build_vm is not None, "batched switch requires a shared build VM"
    command = [
        "ix",
        "up",
        *[node.switch.sourceInstallable for node in nodes],
        "--build-vm",
        build_vm,
        "--workdir",
        str(workdir),
    ]
    for name, path in sorted(nodes[0].switch.overrideInputs.items()):
        command.extend(["--override-input", f"{name}={path}"])
    await run_source_switch(command, source_root, ", ".join(node.name for node in nodes), dry_run=dry_run)


def is_batchable_switch(node: FleetNode) -> bool:
    # The native multi-VM `ix up` builds on one shared `--build-vm` and names each
    # VM from the installable's simple attr, so a node joins a batch only when it
    # builds remotely, names a build VM, and its installable is exactly
    # `.#<node-name>`. Anything else (local build, no build VM, a custom or dotted
    # installable) falls back to the single-target `ix up --name` path.
    switch = node.switch
    return (
        switch.buildOn == "remote"
        and switch.buildVm is not None
        and switch.sourceInstallable == f".#{node.name}"
    )


def batch_groups(nodes: list[FleetNode]) -> list[list[FleetNode]]:
    # One native multi-VM `ix up` per (build VM, region, override-input set). The
    # CLI shares one `--build-vm` and `--override-input` set across the batch, and
    # the server's multi-switch requires every target to share the builder's
    # region (CAS chunks are region-scoped). Grouping on region keeps a
    # cross-region fleet from failing a whole batch instead of just the
    # wrong-region nodes.
    groups: dict[tuple[str, str, tuple[tuple[str, str], ...]], list[FleetNode]] = {}
    order: list[tuple[str, str, tuple[tuple[str, str], ...]]] = []
    for node in nodes:
        assert node.switch.buildVm is not None
        key = (
            node.switch.buildVm,
            node.region,
            tuple(sorted(node.switch.overrideInputs.items())),
        )
        if key not in groups:
            groups[key] = []
            order.append(key)
        groups[key].append(node)
    return [groups[key] for key in order]


# Replacing a node's image is delete-then-create (see recreate_node): a new
# image cannot be applied to an existing VM in place.
async def replace_node(node: FleetNode, image: str, *, dry_run: bool) -> None:
    await recreate_node(node, image, dry_run=dry_run)


async def up_node(node: FleetNode, image: str, *, dry_run: bool) -> None:
    if dry_run:
        verb = "recreate" if node.recreateOnUp else "create or replace"
        step(f"{verb} {node.name} from uploaded image {image}")
        await recreate_node(node, image, dry_run=dry_run)
        return

    existing = find_node(await list_nodes(), node.name)
    if existing is None:
        await create_node(node, image, dry_run=dry_run)
        return

    # Any existing node (recreateOnUp, failed, or a plain image change) needs a
    # fresh VM on the uploaded image, since `up` swaps the image rather than
    # updating in place.
    await recreate_node(node, image, dry_run=dry_run)


async def cmd_diff(plan: FleetPlan, args: argparse.Namespace) -> None:
    for node in selected_nodes(plan, args.on):
        if node.switch.buildOn == "remote":
            print(f"{node.name}\twant {node.switch.sourceInstallable} (remote source)")
        else:
            print(f"{node.name}\twant {node.switch.target} ({node.switch.buildOn})")


async def run_switch_node_workflow(node: FleetNode, args: argparse.Namespace) -> None:
    source_root = (args.source_root or default_source_root(Path.cwd())).resolve()
    source_workdir = args.source_workdir or default_source_workdir(Path.cwd(), source_root)
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


async def run_replace_node_workflow(node: FleetNode, args: argparse.Namespace) -> None:
    image = node.replacementImage.destination
    if not args.skip_push:
        image = await push_replacement_image(node, dry_run=args.dry_run)
    await replace_node(node, image, dry_run=args.dry_run)
    await ensure_node_groups(node, dry_run=args.dry_run)
    if not args.skip_health:
        await run_node_health_checks(node, dry_run=args.dry_run)


async def run_up_node_workflow(node: FleetNode, args: argparse.Namespace) -> None:
    image = node.replacementImage.destination
    if not args.skip_push:
        image = await push_replacement_image(node, dry_run=args.dry_run)
    await up_node(node, image, dry_run=args.dry_run)
    # `up_node` recreates the VM on a fresh image and returns once `client.create`
    # has issued the boot, not once the guest is reachable. Wait for the guest to
    # answer an exec before health-checking, the same gate `ensure_node` gives the
    # switch path: a `from: guest` check against a still-booting guest raises an
    # exec RPC error (not a TimeoutError or non-zero exit), which escapes the
    # retry loop in `run_health_check` and fails the whole `up` on the first
    # attempt instead of waiting the boot out.
    await wait_node_ready(node, dry_run=args.dry_run)
    await ensure_node_groups(node, dry_run=args.dry_run)
    if not args.skip_health:
        await run_node_health_checks(node, dry_run=args.dry_run)


async def run_node_workflow_dag(
    plan: FleetPlan,
    args: argparse.Namespace,
    subcommand: str,
    extra_args: list[str],
) -> None:
    pushes_images = subcommand in {"_up-node", "_replace-node"} and "--skip-push" not in extra_args
    last_push_by_destination: dict[str, str] = {}
    nodes: dict[str, dict[str, typing.Any]] = {}
    for node in selected_nodes(plan, args.on):
        depends_on = list(node.dependsOn)
        if pushes_images:
            previous = last_push_by_destination.get(node.replacementImage.destination)
            if previous is not None and previous not in depends_on:
                depends_on.append(previous)
            last_push_by_destination[node.replacementImage.destination] = node.name
        nodes[node.name] = {
            "command": [
                sys.argv[0],
                "--plan",
                str(args.plan),
                subcommand,
                node.name,
                *extra_args,
            ],
            "depends_on": depends_on,
        }
    await run_dag_runner({"nodes": nodes})


async def run_dag_runner(spec: dict[str, typing.Any]) -> None:
    dag_runner = os.environ.get("IX_FLEET_DAG_RUNNER") or shutil.which("dag-runner")
    if dag_runner is None:
        raise RuntimeError("dag-runner is unavailable; set IX_FLEET_DAG_RUNNER or add dag-runner to PATH")

    with tempfile.TemporaryDirectory(prefix="ix-fleet-dag-") as temporary_directory:
        spec_path = Path(temporary_directory) / "spec.json"
        spec_path.write_text(json.dumps(spec, indent=2))
        process = await asyncio.create_subprocess_exec(dag_runner, str(spec_path))
        returncode = await process.wait()
    if returncode != 0:
        raise SystemExit(returncode)


async def switch_group_workflow(
    group: list[FleetNode],
    source_root: Path,
    source_workdir: Path,
    args: argparse.Namespace,
) -> None:
    # Pre-create every VM with its full fleet config (groups, region, secrets,
    # ...) and snapshot it first, so the native multi-VM `ix up` only switches
    # existing VMs instead of creating them with bare defaults.
    for node in group:
        created = await ensure_node(node, dry_run=args.dry_run)
        await ensure_node_groups(node, dry_run=args.dry_run)
        if not created and node.snapshot and not args.no_snapshot:
            await snapshot_node(node, dry_run=args.dry_run)
    await switch_nodes_from_source(group, source_root, source_workdir, dry_run=args.dry_run)
    if not args.skip_health:
        for node in group:
            await run_node_health_checks(node, dry_run=args.dry_run)


async def cmd_switch(plan: FleetPlan, args: argparse.Namespace) -> None:
    if not args.dry_run:
        await verify_secrets_available(plan, args.on, dry_run=args.dry_run)
    source_root = (args.source_root or default_source_root(Path.cwd())).resolve()
    source_workdir = args.source_workdir or default_source_workdir(Path.cwd(), source_root)
    # `dependency_batches` yields dependency-ordered layers; switch them in order
    # so `dependsOn` still gates the switch. Within a layer the nodes are
    # independent, so each native multi-VM batch (one `ix up` per build VM /
    # override set) and each single-node fallback run concurrently.
    for layer in dependency_batches(plan, args.on):
        batchable = [node for node in layer if is_batchable_switch(node)]
        singles = [node for node in layer if not is_batchable_switch(node)]
        tasks = [
            switch_group_workflow(group, source_root, source_workdir, args)
            for group in batch_groups(batchable)
        ]
        tasks.extend(run_switch_node_workflow(node, args) for node in singles)
        await asyncio.gather(*tasks)


async def cmd_replace(plan: FleetPlan, args: argparse.Namespace) -> None:
    if args.dry_run:
        for node in selected_nodes(plan, args.on):
            await run_replace_node_workflow(node, args)
        return

    await verify_secrets_available(plan, args.on, dry_run=args.dry_run)
    extra_args: list[str] = []
    if args.skip_push:
        extra_args.append("--skip-push")
    if args.skip_health:
        extra_args.append("--skip-health")
    await run_node_workflow_dag(plan, args, "_replace-node", extra_args)


async def cmd_up(plan: FleetPlan, args: argparse.Namespace) -> None:
    if args.dry_run:
        for node in selected_nodes(plan, args.on):
            await run_up_node_workflow(node, args)
        return

    await verify_secrets_available(plan, args.on, dry_run=args.dry_run)
    extra_args: list[str] = []
    if args.skip_push:
        extra_args.append("--skip-push")
    if args.skip_health:
        extra_args.append("--skip-health")
    await run_node_workflow_dag(plan, args, "_up-node", extra_args)


async def cmd_replace_node(plan: FleetPlan, args: argparse.Namespace) -> None:
    await run_replace_node_workflow(plan.nodes[args.node], args)


async def cmd_up_node(plan: FleetPlan, args: argparse.Namespace) -> None:
    await run_up_node_workflow(plan.nodes[args.node], args)


async def cmd_health(plan: FleetPlan, args: argparse.Namespace) -> None:
    for node in selected_nodes(plan, args.on):
        await run_node_health_checks(node, dry_run=args.dry_run)


async def cmd_bootstrap(plan: FleetPlan, args: argparse.Namespace) -> None:
    await verify_secrets_available(plan, args.on, dry_run=args.dry_run)
    for batch in dependency_batches(plan, args.on):
        await asyncio.gather(*(bootstrap_node(node, dry_run=args.dry_run) for node in batch))


async def cmd_down(plan: FleetPlan, args: argparse.Namespace) -> None:
    failures: list[str] = []
    for node in reversed(selected_nodes(plan, args.on)):
        try:
            await remove_node(node, dry_run=args.dry_run)
        except (ix_sdk.IxError, OSError) as error:
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

    def add_dry_run_option(target: argparse.ArgumentParser) -> None:
        target.add_argument("--dry-run", action="store_true", default=argparse.SUPPRESS)

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
    replace_node_parser = sub.add_parser("_replace-node", help=argparse.SUPPRESS)
    replace_node_parser.add_argument("node")
    add_dry_run_option(replace_node_parser)
    replace_node_parser.add_argument("--skip-push", action="store_true")
    replace_node_parser.add_argument("--skip-health", action="store_true")
    up_node_parser = sub.add_parser("_up-node", help=argparse.SUPPRESS)
    up_node_parser.add_argument("node")
    add_dry_run_option(up_node_parser)
    up_node_parser.add_argument("--skip-push", action="store_true")
    up_node_parser.add_argument("--skip-health", action="store_true")
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
    elif args.command == "_replace-node":
        await cmd_replace_node(plan, args)
    elif args.command == "_up-node":
        await cmd_up_node(plan, args)
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
