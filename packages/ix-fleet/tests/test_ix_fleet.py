from __future__ import annotations

import asyncio
import contextlib
import io
import json
import types
import typing
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import pytest
from pydantic import ValidationError

import ix_fleet
import ix_sdk


def fleet_node(name: str, *, depends_on: list[str] | None = None) -> dict[str, typing.Any]:
    return {
        "name": name,
        "baseName": name,
        "system": f"/nix/store/{name}-system",
        "switch": {
            "target": f"/nix/store/{name}-system.drv",
            "sourceInstallable": f".#{name}",
        },
        "bootstrapImage": "registry.ix.dev/ix/base:latest",
        "replacementImage": {
            "imageName": name,
            "destination": f"registry.ix.dev/example/{name}:latest",
            "sourceInstallable": f".#{name}",
        },
        "region": "us-west-1",
        "ipv4": False,
        "snapshot": True,
        "dependsOn": depends_on or [],
    }


def fleet_plan(order: list[str], nodes: list[dict[str, typing.Any]]) -> dict[str, typing.Any]:
    return {
        "order": order,
        "nodes": {node["name"]: node for node in nodes},
    }


class FleetPlanValidationTests(unittest.TestCase):
    def test_rejects_nodes_missing_from_order(self) -> None:
        data = fleet_plan(["web"], [fleet_node("web"), fleet_node("db")])

        with pytest.raises(ValidationError, match="order is missing node 'db'"):
            ix_fleet.FleetPlan.model_validate(data)

    def test_rejects_duplicate_order_entries(self) -> None:
        data = fleet_plan(["web", "web"], [fleet_node("web")])

        with pytest.raises(ValidationError, match="order contains duplicate node 'web'"):
            ix_fleet.FleetPlan.model_validate(data)

    def test_selected_nodes_keeps_dependencies_before_selected_node(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(["db", "web"], [fleet_node("web", depends_on=["db"]), fleet_node("db")])
        )

        assert (
            [node.name for node in ix_fleet.selected_nodes(plan, ["web"])]
            == ["db", "web"]
        )

    def test_per_vm_secret_attachments_default_empty_and_round_trip(self) -> None:
        bare = ix_fleet.FleetNode.model_validate(fleet_node("web"))
        assert bare.secrets == []

        node = fleet_node("api")
        node["secrets"] = [
            {
                "name": "github_token",
                "target": {
                    "name": "GH_TOKEN",
                    "injectAs": "env",
                },
            },
            {
                "name": "hermes_env",
                "target": {
                    "name": "hermes.env",
                    "injectAs": "file",
                    "owner": "hermes",
                    "mode": "0400",
                },
            },
        ]
        parsed = ix_fleet.FleetNode.model_validate(node)
        assert [secret.name for secret in parsed.secrets] == ["github_token", "hermes_env"]
        assert parsed.secrets[0].target.name == "GH_TOKEN"
        assert parsed.secrets[1].target.owner == "hermes"


class VerifySecretsAvailableTests(unittest.TestCase):
    @staticmethod
    def _fake_client(names: list[str]) -> typing.Any:  # noqa: ANN401
        stored = [type("UserSecret", (), {"name": name})() for name in names]

        class FakeClient:
            async def list_secrets(self) -> list[typing.Any]:
                return stored

        return FakeClient

    def _plan(self, secrets: list[str]) -> typing.Any:  # noqa: ANN401
        node = fleet_node("web")
        node["secrets"] = [
            {
                "name": secret,
                "target": {
                    "name": secret.upper(),
                    "injectAs": "env",
                },
            }
            for secret in secrets
        ]
        return ix_fleet.FleetPlan.model_validate(fleet_plan(["web"], [node]))

    def test_passes_when_every_referenced_secret_exists(self) -> None:
        plan = self._plan(["github_token"])
        with patch.object(ix_fleet, "client", self._fake_client(["github_token", "other"])):
            asyncio.run(ix_fleet.verify_secrets_available(plan, [], dry_run=False))

    def test_raises_listing_missing_secrets(self) -> None:
        plan = self._plan(["github_token", "database_url"])
        with (
            patch.object(ix_fleet, "client", self._fake_client(["github_token"])),
            pytest.raises(RuntimeError, match=r"missing secret\(s\) in the store: database_url"),
        ):
            asyncio.run(ix_fleet.verify_secrets_available(plan, [], dry_run=False))

    def test_dry_run_makes_no_live_call(self) -> None:
        plan = self._plan(["missing"])

        def fail_client() -> typing.Any:  # noqa: ANN401
            raise AssertionError("dry-run preflight must not touch the store")

        with patch.object(ix_fleet, "client", fail_client):
            asyncio.run(ix_fleet.verify_secrets_available(plan, [], dry_run=True))

    def test_no_references_makes_no_live_call(self) -> None:
        plan = self._plan([])

        def fail_client() -> typing.Any:  # noqa: ANN401
            raise AssertionError("preflight must not query the store with no references")

        with patch.object(ix_fleet, "client", fail_client):
            asyncio.run(ix_fleet.verify_secrets_available(plan, [], dry_run=False))


class PushReplacementImageTests(unittest.TestCase):
    @staticmethod
    def _node(destination: str) -> ix_fleet.FleetNode:
        return ix_fleet.FleetNode.model_validate(
            {
                "name": "health-check-nginx",
                "baseName": "nginx",
                "system": "/nix/store/example-system",
                "switch": {
                    "target": "/nix/store/example-system.drv",
                    "sourceInstallable": ".#health-check-nginx-system",
                },
                "bootstrapImage": "registry.ix.dev/ix/base:latest",
                "replacementImage": {
                    "imageName": "health-check-nginx",
                    "destination": destination,
                    "sourceInstallable": ".#health-check-nginx",
                },
                "region": "us-west-1",
                "ipv4": False,
                "snapshot": True,
            }
        )

    def test_builds_installable_and_pushes_manifest(self) -> None:
        with TemporaryDirectory() as temporary_directory:
            image_dir = Path(temporary_directory)
            (image_dir / "manifest.cas").write_text("")
            (image_dir / "locator.bin").write_text("")
            calls: list[list[str]] = []

            async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
                del timeout
                assert not dry_run
                calls.append(command)
                if command[0] == "nix":
                    return f"{image_dir}\n"
                return "registry.ix.dev/example/health-check-nginx:nginx-lifecycle\n"

            node = self._node("health-check-nginx:nginx-lifecycle")

            with patch.object(ix_fleet, "run_cli", fake_run_cli):
                image = asyncio.run(ix_fleet.push_replacement_image(node, dry_run=False))

            assert calls == [
                ["nix", "build", "--no-link", "--print-out-paths", ".#health-check-nginx"],
                [
                    "ix",
                    "image",
                    "push-manifest",
                    "--locator",
                    str(image_dir / "locator.bin"),
                    str(image_dir / "manifest.cas"),
                    "health-check-nginx:nginx-lifecycle",
                    "--region",
                    "us-west-1",
                ],
            ]
            assert image == "registry.ix.dev/example/health-check-nginx:nginx-lifecycle"

    def test_rejects_build_output_missing_manifest_files(self) -> None:
        with TemporaryDirectory() as temporary_directory:
            image_dir = Path(temporary_directory)

            async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
                del dry_run, timeout
                assert command[0] == "nix", "must fail before any push attempt"
                return f"{image_dir}\n"

            node = self._node("health-check-nginx:nginx-lifecycle")

            with (
                patch.object(ix_fleet, "run_cli", fake_run_cli),
                pytest.raises(RuntimeError, match=r"missing manifest\.cas"),
            ):
                asyncio.run(ix_fleet.push_replacement_image(node, dry_run=False))


def branch_info(
    name: str,
    *,
    status: typing.Any = None,  # noqa: ANN401
    image: str = "registry.ix.dev/example/old:latest",
    ipv4: str | None = None,
    region: str | None = "us-west-1",
) -> typing.Any:  # noqa: ANN401
    """A BranchInfo-shaped row for patched `list_nodes` results. SimpleNamespace
    (not the real SDK type) so tests don't depend on the cdylib's constructor."""
    return types.SimpleNamespace(
        name=name,
        image=image,
        status=status if status is not None else ix_sdk.BranchStatus.RUNNING,
        ipv6="fd00::1",
        ipv4=ipv4,
        subdomain=None,
        region=types.SimpleNamespace(slug=region) if region is not None else None,
    )


class FakeBranch:
    def __init__(self, client: RecordingClient, name: str) -> None:
        self._client = client
        self.name = name

    async def delete(self) -> None:
        error = self._client.delete_errors.get(self.name)
        if error is not None:
            raise error
        self._client.calls.append(("delete", self.name))

    async def start(self) -> None:
        self._client.calls.append(("start", self.name))

    async def exec(self, command: list[str], *, check: bool, quiet: bool) -> typing.Any:  # noqa: ANN401
        del check, quiet
        self._client.calls.append(("exec", self.name, *command))
        exit_code, stdout, stderr = self._client.exec_results.get(self.name, (0, "", ""))
        return types.SimpleNamespace(exit_code=exit_code, stdout=stdout, stderr=stderr)


class RecordingClient:
    """A fake `ix_sdk.Client` recording the SDK surface ix_fleet drives."""

    def __init__(self, present: list[str] | None = None) -> None:
        self.present = set(present or [])
        self.calls: list[tuple[str, ...]] = []
        self.delete_errors: dict[str, Exception] = {}
        self.exec_results: dict[str, tuple[int, str, str]] = {}

    async def find_by_name(self, name: str) -> FakeBranch | None:
        if name not in self.present:
            return None
        return FakeBranch(self, name)

    async def create(self, image: str, **kwargs: typing.Any) -> None:  # noqa: ANN401
        self.calls.append(("create", image, str(kwargs.get("name"))))

    async def apply_vm_groups(self, vm: str, groups: list[str]) -> None:
        self.calls.append(("apply_vm_groups", vm, *groups))


class UpNodeTests(unittest.TestCase):
    def test_replaces_existing_running_node_with_uploaded_image(self) -> None:
        fake = RecordingClient(present=["web"])
        node = ix_fleet.FleetNode.model_validate(fleet_node("web"))

        async def fake_list_nodes() -> list[typing.Any]:
            return [branch_info("web")]

        with (
            patch.object(ix_fleet, "list_nodes", fake_list_nodes),
            patch.object(ix_fleet, "client", lambda: fake),
        ):
            asyncio.run(ix_fleet.up_node(node, "registry.ix.dev/example/web:new", dry_run=False))

        # A new image cannot be applied in place: replace is delete-then-create.
        assert fake.calls == [
            ("delete", "web"),
            ("create", "registry.ix.dev/example/web:new", "web"),
        ]

    def test_replaces_existing_stopped_node_instead_of_starting_old_image(self) -> None:
        fake = RecordingClient(present=["web"])
        node = ix_fleet.FleetNode.model_validate(fleet_node("web"))

        async def fake_list_nodes() -> list[typing.Any]:
            return [branch_info("web", status=ix_sdk.BranchStatus.STOPPED)]

        with (
            patch.object(ix_fleet, "list_nodes", fake_list_nodes),
            patch.object(ix_fleet, "client", lambda: fake),
        ):
            asyncio.run(ix_fleet.up_node(node, "registry.ix.dev/example/web:new", dry_run=False))

        assert ("create", "registry.ix.dev/example/web:new", "web") in fake.calls
        assert ("start", "web") not in fake.calls

    def test_dry_run_shows_possible_node_replacement_without_live_lookup(self) -> None:
        steps: list[str] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("web"))

        async def fail_list_nodes() -> list[typing.Any]:
            self.fail("dry-run up should not require live node state")

        def fail_client() -> typing.NoReturn:
            self.fail("dry-run up should not touch the live client")

        with (
            patch.object(ix_fleet, "list_nodes", fail_list_nodes),
            patch.object(ix_fleet, "client", fail_client),
            patch.object(ix_fleet, "step", steps.append),
        ):
            asyncio.run(ix_fleet.up_node(node, "registry.ix.dev/example/web:new", dry_run=True))

        assert steps[0] == "create or replace web from uploaded image registry.ix.dev/example/web:new"
        assert steps[1] == "remove web"
        assert steps[2].startswith("+ create web from registry.ix.dev/example/web:new")


class EastWestGroupTests(unittest.TestCase):
    @staticmethod
    def _recording_client(calls: list[tuple[str, list[str]]]) -> typing.Any:  # noqa: ANN401
        class FakeClient:
            async def apply_vm_groups(self, vm: str, groups: list[str]) -> typing.Any:  # noqa: ANN401
                calls.append((vm, groups))
                return type("GroupApplySummary", (), {"added": groups, "removed": []})()

        return FakeClient

    def test_reconciles_membership_in_vm_region(self) -> None:
        # ensure_node_groups routes through vm.apply_groups, which get-or-creates
        # each slug in the VM's own region rather than the caller's local region
        # (ENG-2754), so a fleet driven from one region's leader no longer
        # strands a remote region's group. One set-based call keyed by VM name.
        calls: list[tuple[str, list[str]]] = []

        node_data = fleet_node("api")
        node_data["groups"] = ["shared-db", "private-apps"]
        node = ix_fleet.FleetNode.model_validate(node_data)

        with patch.object(ix_fleet, "client", self._recording_client(calls)):
            asyncio.run(ix_fleet.ensure_node_groups(node, dry_run=False))

        assert calls == [("api", ["private-apps", "shared-db"])]

    def test_no_groups_makes_no_live_call(self) -> None:
        node = ix_fleet.FleetNode.model_validate(fleet_node("api"))

        def fail_client() -> typing.Any:  # noqa: ANN401
            raise AssertionError("no declared groups: apply_vm_groups must not be called")

        with patch.object(ix_fleet, "client", fail_client):
            asyncio.run(ix_fleet.ensure_node_groups(node, dry_run=False))

    def test_dry_run_makes_no_live_call(self) -> None:
        node_data = fleet_node("api")
        node_data["groups"] = ["private-apps"]
        node = ix_fleet.FleetNode.model_validate(node_data)

        def fail_client() -> typing.Any:  # noqa: ANN401
            raise AssertionError("dry-run must not touch the group surface")

        with patch.object(ix_fleet, "client", fail_client):
            asyncio.run(ix_fleet.ensure_node_groups(node, dry_run=True))


class BootstrapTests(unittest.TestCase):
    def test_bootstrap_waits_for_dependencies_before_selected_node(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(["db", "web"], [fleet_node("web", depends_on=["db"]), fleet_node("db")])
        )
        calls: list[str] = []

        async def fake_bootstrap_node(node: ix_fleet.FleetNode, *, dry_run: bool) -> None:
            assert not dry_run
            calls.append(node.name)

        with patch.object(ix_fleet, "bootstrap_node", fake_bootstrap_node):
            args = argparse_namespace(on=["web"], dry_run=False)
            asyncio.run(ix_fleet.cmd_bootstrap(plan, args))

        assert calls == ["db", "web"]

    def test_bootstrap_uses_bootstrap_image_without_replacement_push(self) -> None:
        fake = RecordingClient()
        ready: list[str] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("api"))

        async def fake_list_nodes() -> list[typing.Any]:
            return []

        async def fake_wait_node_ready(node: ix_fleet.FleetNode, *, dry_run: bool) -> None:
            assert not dry_run
            ready.append(node.name)

        with (
            patch.object(ix_fleet, "list_nodes", fake_list_nodes),
            patch.object(ix_fleet, "client", lambda: fake),
            patch.object(ix_fleet, "wait_node_ready", fake_wait_node_ready),
        ):
            asyncio.run(ix_fleet.bootstrap_node(node, dry_run=False))

        assert fake.calls == [("create", "registry.ix.dev/ix/base:latest", "api")]
        assert ready == ["api"]


class NodeWorkflowDagTests(unittest.TestCase):
    def test_up_dag_includes_transitive_dependencies_and_forwards_flags(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(["db", "web"], [fleet_node("web", depends_on=["db"]), fleet_node("db")])
        )
        spec = captured_dag(
            ix_fleet.cmd_up,
            plan,
            argparse_namespace(
                plan=Path("fleet.json"),
                on=["web"],
                dry_run=False,
                skip_push=True,
                skip_health=True,
            ),
        )

        assert list(spec["nodes"]) == ["db", "web"]
        assert spec["nodes"]["db"]["depends_on"] == []
        assert spec["nodes"]["web"]["depends_on"] == ["db"]
        assert spec["nodes"]["web"]["command"] == [
            "/bin/ix-fleet",
            "--plan",
            "fleet.json",
            "_up-node",
            "web",
            "--skip-push",
            "--skip-health",
        ]

    def test_replace_dag_forwards_replace_flags(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api"], [fleet_node("api")]))
        spec = captured_dag(
            ix_fleet.cmd_replace,
            plan,
            argparse_namespace(
                plan=Path("/plans/fleet.json"),
                on=[],
                dry_run=False,
                skip_push=True,
                skip_health=True,
            ),
        )

        assert spec["nodes"]["api"]["command"] == [
            "/bin/ix-fleet",
            "--plan",
            "/plans/fleet.json",
            "_replace-node",
            "api",
            "--skip-push",
            "--skip-health",
        ]

    def test_push_dag_serializes_shared_image_destination(self) -> None:
        api = fleet_node("api")
        worker = fleet_node("worker")
        worker["replacementImage"]["destination"] = api["replacementImage"]["destination"]
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api", "worker"], [api, worker]))

        spec = captured_dag(
            ix_fleet.cmd_up,
            plan,
            argparse_namespace(plan=Path("fleet.json"), on=[], dry_run=False, skip_push=False, skip_health=True),
        )

        assert spec["nodes"]["worker"]["depends_on"] == ["api"]

    def test_dag_runner_exit_status_becomes_process_exit_status(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api"], [fleet_node("api")]))
        args = argparse_namespace(
            plan=Path("fleet.json"),
            on=[],
            dry_run=False,
            skip_push=True,
            skip_health=True,
        )

        with TemporaryDirectory() as temporary_directory:
            runner = Path(temporary_directory) / "dag-runner"
            runner.write_text("#!/bin/sh\nexit 17\n")
            runner.chmod(0o755)

            with (
                patch.dict(ix_fleet.os.environ, {"IX_FLEET_DAG_RUNNER": str(runner)}),
                pytest.raises(SystemExit) as raised,
            ):
                asyncio.run(ix_fleet.cmd_up(plan, args))

        assert raised.value.code == 17

    def test_dry_run_runs_inline_so_child_output_is_visible(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api"], [fleet_node("api")]))
        calls: list[str] = []

        async def fail_run_dag_runner(spec: dict[str, typing.Any]) -> None:
            self.fail("dry-run should not send child output through dag-runner")

        with (
            patch.object(ix_fleet, "run_dag_runner", fail_run_dag_runner),
            patch.object(ix_fleet, "run_up_node_workflow", async_recorder(calls, "api")),
        ):
            asyncio.run(
                ix_fleet.cmd_up(
                    plan,
                    argparse_namespace(plan=Path("fleet.json"), on=[], dry_run=True, skip_push=True, skip_health=True),
                )
            )

        assert calls == ["api"]


class SingleNodeWorkflowTests(unittest.TestCase):
    def test_image_node_workflows_run_their_existing_sequences(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api"], [fleet_node("api")]))
        args = argparse_namespace(
            node="api",
            dry_run=False,
            skip_push=False,
            skip_health=False,
        )
        for command, operation, expected_calls in [
            ("cmd_up_node", "up_node", ["push", "up", "ready", "groups", "health"]),
            ("cmd_replace_node", "replace_node", ["push", "replace", "groups", "health"]),
        ]:
            with self.subTest(command=command):
                calls: list[str] = []
                with (
                    patch.object(
                        ix_fleet,
                        "push_replacement_image",
                        async_recorder(calls, "push", "registry.ix.dev/example/api:pushed"),
                    ),
                    patch.object(ix_fleet, operation, async_recorder(calls, expected_calls[1])),
                    patch.object(ix_fleet, "wait_node_ready", async_recorder(calls, "ready")),
                    patch.object(ix_fleet, "ensure_node_groups", async_recorder(calls, "groups")),
                    patch.object(
                        ix_fleet, "run_node_health_checks", async_recorder(calls, "health")
                    ),
                ):
                    asyncio.run(getattr(ix_fleet, command)(plan, args))

                assert calls == expected_calls

    def test_switch_node_workflow_runs_the_existing_switch_sequence(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api"], [fleet_node("api")]))
        args = argparse_namespace(
            node="api",
            dry_run=False,
            no_snapshot=False,
            skip_health=False,
            source_root=Path("/source"),
            source_workdir=Path("subdir"),
        )
        calls: list[str] = []

        with (
            patch.object(ix_fleet, "ensure_node", async_recorder(calls, "ensure", result=False)),
            patch.object(ix_fleet, "ensure_node_groups", async_recorder(calls, "groups")),
            patch.object(ix_fleet, "snapshot_node", async_recorder(calls, "snapshot")),
            patch.object(ix_fleet, "switch_node", async_recorder(calls, "switch")),
            patch.object(ix_fleet, "run_node_health_checks", async_recorder(calls, "health")),
        ):
            asyncio.run(ix_fleet.run_switch_node_workflow(plan.nodes["api"], args))

        assert calls == ["ensure", "groups", "snapshot", "switch", "health"]


class DownTests(unittest.TestCase):
    def test_down_continues_after_node_failure(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(["db", "web"], [fleet_node("db"), fleet_node("web")])
        )
        fake = RecordingClient(present=["db", "web"])
        fake.delete_errors["web"] = ix_sdk.IxError("permission denied")

        with patch.object(ix_fleet, "client", lambda: fake):
            args = argparse_namespace(on=[], dry_run=False)
            with pytest.raises(RuntimeError, match="web: permission denied"):
                asyncio.run(ix_fleet.cmd_down(plan, args))

        # web's failure must not stop db (reverse plan order) from being removed.
        assert fake.calls == [("delete", "db")]

    def test_down_treats_missing_nodes_as_absent(self) -> None:
        node = ix_fleet.FleetNode.model_validate(fleet_node("api"))
        fake = RecordingClient(present=[])

        with patch.object(ix_fleet, "client", lambda: fake):
            asyncio.run(ix_fleet.remove_node(node, dry_run=False))

        assert fake.calls == []


class SwitchSourceTests(unittest.TestCase):
    def test_source_switch_runs_ix_apply_from_source_root(self) -> None:
        # `ix switch` was folded into `ix up` (indexable-inc/ix#4442), which merged
        # into `ix apply` (indexable-inc/ix#8134): the source switch now runs
        # `ix apply <installable> --name <vm>` from the source root (which
        # `ix apply` auto-uploads), with `--workdir` relative to that root and
        # `--override-input NAME=VALUE` single-token flags.
        calls: list[list[str]] = []
        cwds: list[Path | None] = []
        node_data = fleet_node("api")
        node_data["switch"]["buildVm"] = "builder"
        node_data["switch"]["overrideInputs"] = {
            "ix": "github:indexable-inc/ix",
            "ix-images": "path:/workspace/index",
        }
        node = ix_fleet.FleetNode.model_validate(node_data)

        run_with_cli(
            cli_recorder(calls, cwds),
            ix_fleet.switch_node_from_source(
                node, Path("/source"), Path("/source/subdir"), dry_run=False
            ),
        )

        assert calls == [
            [
                "ix",
                "apply",
                ".#api",
                "--name",
                "api",
                "--workdir",
                "subdir",
                "--build-vm",
                "builder",
                "--override-input",
                "ix=github:indexable-inc/ix",
                "--override-input",
                "ix-images=path:/workspace/index",
            ],
        ]
        assert cwds == [Path("/source")]

    def test_source_switch_rejects_workdir_outside_source_root(self) -> None:
        # `--workdir` is resolved relative to the uploaded source root, so an
        # absolute workdir outside that root has no valid mapping and must fail
        # loudly rather than forwarding a path `ix apply` cannot interpret.
        node = ix_fleet.FleetNode.model_validate(fleet_node("api"))

        async def fail_run_cli(*args: typing.Any, **kwargs: typing.Any) -> str:  # noqa: ANN401
            del args, kwargs
            raise AssertionError("run_cli should not be reached")

        with pytest.raises(ValueError, match="outside source root"):
            run_with_cli(
                fail_run_cli,
                ix_fleet.switch_node_from_source(
                    node,
                    Path("/source"),
                    Path("/elsewhere/subdir"),
                    dry_run=False,
                ),
            )


def remote_node(
    name: str,
    *,
    build_vm: str = "builder",
    depends_on: list[str] | None = None,
) -> dict[str, typing.Any]:
    node = fleet_node(name, depends_on=depends_on)
    node["switch"]["buildOn"] = "remote"
    node["switch"]["buildVm"] = build_vm
    return node


def cli_recorder(
    calls: list[list[str]], cwds: list[Path | None]
) -> typing.Callable[..., typing.Coroutine[typing.Any, typing.Any, str]]:
    async def record(
        command: list[str],
        *,
        dry_run: bool,
        timeout: int | None = None,
        cwd: Path | None = None,
    ) -> str:
        del timeout
        assert not dry_run
        calls.append(command)
        cwds.append(cwd)
        return ""

    return record


def run_with_cli(
    fake: typing.Callable[..., typing.Any],
    operation: typing.Coroutine[typing.Any, typing.Any, typing.Any],
) -> None:
    with patch.object(ix_fleet, "run_cli", fake):
        asyncio.run(operation)


class SwitchBatchTests(unittest.TestCase):
    def _node(self, data: dict[str, typing.Any]) -> ix_fleet.FleetNode:
        return ix_fleet.FleetNode.model_validate(data)

    def test_is_batchable_switch(self) -> None:
        assert ix_fleet.is_batchable_switch(self._node(remote_node("api")))
        # local build: no shared builder to batch on.
        local = fleet_node("api")
        local["switch"]["buildOn"] = "local"
        assert not ix_fleet.is_batchable_switch(self._node(local))
        # remote but no build VM: multi `ix apply` requires --build-vm.
        no_vm = fleet_node("api")
        no_vm["switch"]["buildOn"] = "remote"
        assert not ix_fleet.is_batchable_switch(self._node(no_vm))
        # installable attr must equal the node name (multi derives the VM name
        # from it and rejects --name).
        custom = remote_node("api")
        custom["switch"]["sourceInstallable"] = ".#api-system"
        assert not ix_fleet.is_batchable_switch(self._node(custom))

    def test_batch_groups_split_by_build_vm_region_and_overrides(self) -> None:
        a = self._node(remote_node("a", build_vm="b1"))
        b = self._node(remote_node("b", build_vm="b1"))
        c = self._node(remote_node("c", build_vm="b2"))
        d = remote_node("d", build_vm="b1")
        d["switch"]["overrideInputs"] = {"ix": "github:indexable-inc/ix"}
        d_node = self._node(d)
        # Same build VM as a/b, but a different region: the server's multi-switch
        # requires every target to share the builder's region, so it splits off.
        e = remote_node("e", build_vm="b1")
        e["region"] = "us-east-1"
        e_node = self._node(e)

        groups = ix_fleet.batch_groups([a, b, c, d_node, e_node])
        names = [[node.name for node in group] for group in groups]
        assert names == [["a", "b"], ["c"], ["d"], ["e"]]

    def test_switch_nodes_from_source_builds_one_multi_ix_apply(self) -> None:
        nodes = [self._node(remote_node("web")), self._node(remote_node("worker"))]
        calls: list[list[str]] = []
        cwds: list[Path | None] = []

        run_with_cli(
            cli_recorder(calls, cwds),
            ix_fleet.switch_nodes_from_source(
                nodes, Path("/source"), Path("/source/subdir"), dry_run=False
            ),
        )

        # One native multi-VM `ix apply`: every installable, one shared --build-vm,
        # and no --name (multi derives each VM name from its installable attr).
        assert calls == [
            [
                "ix",
                "apply",
                ".#web",
                ".#worker",
                "--build-vm",
                "builder",
                "--workdir",
                "subdir",
            ]
        ]
        assert "--name" not in calls[0]
        assert cwds == [Path("/source")]

    def test_cmd_switch_batches_remote_nodes_and_runs_singles(self) -> None:
        api = remote_node("api")
        web = remote_node("web")
        cache = fleet_node("cache")  # buildOn defaults to auto -> single fallback
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api", "web", "cache"], [api, web, cache]))
        groups: list[list[str]] = []
        singles: list[str] = []

        async def record_group(
            group: list[ix_fleet.FleetNode],
            source_root: Path,
            source_workdir: Path,
            args: typing.Any,  # noqa: ANN401
        ) -> None:
            del source_root, source_workdir, args
            groups.append([node.name for node in group])

        async def record_single(node: ix_fleet.FleetNode, args: typing.Any) -> None:  # noqa: ANN401
            del args
            singles.append(node.name)

        async def no_verify(*args: typing.Any, **kwargs: typing.Any) -> None:  # noqa: ANN401
            del args, kwargs

        with (
            patch.object(ix_fleet, "verify_secrets_available", no_verify),
            patch.object(ix_fleet, "switch_group_workflow", record_group),
            patch.object(ix_fleet, "run_switch_node_workflow", record_single),
        ):
            asyncio.run(
                ix_fleet.cmd_switch(
                    plan,
                    argparse_namespace(
                        on=[],
                        dry_run=False,
                        no_snapshot=False,
                        skip_health=False,
                        source_root=Path("/source"),
                        source_workdir=Path("subdir"),
                    ),
                )
            )

        assert groups == [["api", "web"]]
        assert singles == ["cache"]

    def test_cmd_switch_respects_dependency_layers(self) -> None:
        api = remote_node("api")
        worker = remote_node("worker", depends_on=["api"])
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api", "worker"], [api, worker]))
        switched: list[list[str]] = []

        async def record_group(
            group: list[ix_fleet.FleetNode],
            source_root: Path,
            source_workdir: Path,
            args: typing.Any,  # noqa: ANN401
        ) -> None:
            del source_root, source_workdir, args
            switched.append([node.name for node in group])

        with (
            patch.object(ix_fleet, "switch_group_workflow", record_group),
        ):
            asyncio.run(
                ix_fleet.cmd_switch(
                    plan,
                    argparse_namespace(
                        on=[],
                        dry_run=True,
                        no_snapshot=False,
                        skip_health=False,
                        source_root=Path("/source"),
                        source_workdir=Path("subdir"),
                    ),
                )
            )

        # `dependsOn` keeps the switch layered: api's batch runs before worker's.
        assert switched == [["api"], ["worker"]]


def argparse_namespace(**kwargs: typing.Any) -> typing.Any:  # noqa: ANN401
    return type("Args", (), kwargs)()


def captured_dag(
    command: typing.Callable[[ix_fleet.FleetPlan, typing.Any], typing.Coroutine[typing.Any, typing.Any, None]],
    plan: ix_fleet.FleetPlan,
    args: typing.Any,  # noqa: ANN401
) -> dict[str, typing.Any]:
    specs: list[dict[str, typing.Any]] = []

    async def fake_run_dag_runner(spec: dict[str, typing.Any]) -> None:
        specs.append(spec)

    with (
        patch.object(ix_fleet, "run_dag_runner", fake_run_dag_runner),
        patch.object(ix_fleet.sys, "argv", ["/bin/ix-fleet"]),
    ):
        asyncio.run(command(plan, args))

    return specs[0]


def async_recorder(
    calls: list[str],
    name: str,
    result: typing.Any = None,  # noqa: ANN401
) -> typing.Callable[..., typing.Coroutine[typing.Any, typing.Any, typing.Any]]:
    async def record(*args: typing.Any, **kwargs: typing.Any) -> typing.Any:  # noqa: ANN401
        del args, kwargs
        calls.append(name)
        return result

    return record


class WaitNodeReadyTests(unittest.TestCase):
    """`wait_node_ready` polls the server-side `system_ready` signal, treating
    `NotActivated` and transient SDK errors as keep-polling, not failure."""

    @staticmethod
    def _node() -> ix_fleet.FleetNode:
        return ix_fleet.FleetNode.model_validate(fleet_node("web"))

    @staticmethod
    def _fake_client(outcomes: list[object]) -> type:
        remaining = list(outcomes)

        class FakeBranch:
            async def system_ready(self) -> object:
                outcome = remaining.pop(0) if remaining else outcomes[-1]
                if isinstance(outcome, BaseException):
                    raise outcome
                return outcome

        class FakeClient:
            async def find_by_name(self, name: str) -> FakeBranch:
                del name
                return FakeBranch()

        return FakeClient

    async def _no_sleep(self, _seconds: float) -> None:
        return None

    def _run(self, outcomes: list[object]) -> None:
        with (
            patch.object(ix_fleet, "client", self._fake_client(outcomes)),
            patch.object(ix_fleet, "step", lambda _message: None),
            patch.object(asyncio, "sleep", self._no_sleep),
        ):
            asyncio.run(ix_fleet.wait_node_ready(self._node(), dry_run=False))

    def test_returns_once_system_ready(self) -> None:
        ready = types.SimpleNamespace(ready=True, at="2026-01-01T00:00:00Z", reason=None)
        self._run([ready])

    def test_polls_through_not_activated(self) -> None:
        not_yet = types.SimpleNamespace(ready=False, at=None, reason="not_activated")
        ready = types.SimpleNamespace(ready=True, at=None, reason=None)
        self._run([not_yet, not_yet, ready])

    def test_transient_ixerror_keeps_polling(self) -> None:
        err = ix_fleet.ix_sdk.IxError("upstream timeout")
        ready = types.SimpleNamespace(ready=True, at=None, reason=None)
        self._run([err, ready])

    def test_times_out_when_never_activated(self) -> None:
        not_yet = types.SimpleNamespace(ready=False, at=None, reason="not_activated")
        with (
            patch.object(ix_fleet, "_BOOTSTRAP_DEADLINE_SECONDS", 0.1),
            patch.object(ix_fleet, "client", self._fake_client([not_yet])),
            patch.object(ix_fleet, "step", lambda _message: None),
            patch.object(asyncio, "sleep", self._no_sleep),
            pytest.raises(RuntimeError) as caught,
        ):
            asyncio.run(ix_fleet.wait_node_ready(self._node(), dry_run=False))
        assert "not_activated" in str(caught.value)

    def test_dry_run_makes_no_live_call(self) -> None:
        def fail_client() -> typing.NoReturn:
            self.fail("dry-run readiness wait must not touch the live client")

        with patch.object(ix_fleet, "client", fail_client):
            asyncio.run(ix_fleet.wait_node_ready(self._node(), dry_run=True))


def guest_check(command: list[str], *, attempts: int = 3) -> dict[str, typing.Any]:
    return {
        "description": "service answers on loopback",
        "command": command,
        "timeoutSec": 5,
        "attempts": attempts,
        "intervalSec": 0,
        "from": "guest",
    }


def replica_node(base: str, index: int, *, max_unavailable: int) -> dict[str, typing.Any]:
    node = fleet_node(f"{base}-{index}")
    node["baseName"] = base
    node["replicaIndex"] = index
    node["updateStrategy"] = {"maxUnavailable": max_unavailable}
    return node


class RollingUpdateTests(unittest.TestCase):
    def test_rejects_non_positive_max_unavailable(self) -> None:
        with pytest.raises(ValidationError, match="maxUnavailable"):
            ix_fleet.FleetNode.model_validate(replica_node("api", 0, max_unavailable=0))

    def test_edges_form_a_sliding_window_per_base_name(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(
                ["db", "api-0", "api-1", "api-2", "api-3"],
                [fleet_node("db"), *(replica_node("api", i, max_unavailable=2) for i in range(4))],
            )
        )

        edges = ix_fleet.rolling_update_edges(ix_fleet.selected_nodes(plan, []))

        # Window of 2: replica i waits on replica i-2; the first window and
        # strategy-less nodes stay unconstrained.
        assert edges == {"api-2": "api-0", "api-3": "api-1"}

    def test_up_dag_serializes_replicas_by_update_strategy(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(
                ["api-0", "api-1", "api-2"],
                [replica_node("api", i, max_unavailable=1) for i in range(3)],
            )
        )

        spec = captured_dag(
            ix_fleet.cmd_up,
            plan,
            argparse_namespace(
                plan=Path("fleet.json"),
                on=[],
                dry_run=False,
                skip_push=True,
                skip_health=True,
            ),
        )

        assert spec["nodes"]["api-0"]["depends_on"] == []
        assert spec["nodes"]["api-1"]["depends_on"] == ["api-0"]
        assert spec["nodes"]["api-2"]["depends_on"] == ["api-1"]


def status_args(**overrides: typing.Any) -> typing.Any:  # noqa: ANN401
    defaults: dict[str, typing.Any] = {
        "on": [],
        "dry_run": False,
        "output": "json",
        "watch": False,
        "interval": 5,
        "no_checks": False,
    }
    defaults.update(overrides)
    return argparse_namespace(**defaults)


def run_status(
    plan: ix_fleet.FleetPlan,
    fake: RecordingClient,
    rows: list[typing.Any],
    args: typing.Any,  # noqa: ANN401
) -> tuple[list[dict[str, typing.Any]], int]:
    """Run cmd_status against a fake client, returning (parsed json, exit code)."""

    async def fake_list_nodes() -> list[typing.Any]:
        return rows

    stdout = io.StringIO()
    code = 0
    with (
        patch.object(ix_fleet, "list_nodes", fake_list_nodes),
        patch.object(ix_fleet, "client", lambda: fake),
        contextlib.redirect_stdout(stdout),
    ):
        try:
            asyncio.run(ix_fleet.cmd_status(plan, args))
        except SystemExit as raised:
            code = int(raised.code or 0)
    reports = json.loads(stdout.getvalue())
    assert isinstance(reports, list)
    return reports, code


class StatusTests(unittest.TestCase):
    def _plan(self, *, checks: bool) -> ix_fleet.FleetPlan:
        node = fleet_node("web")
        if checks:
            node["healthChecks"] = {"http": guest_check(["curl", "--fail", "http://127.0.0.1:8080/"])}
        return ix_fleet.FleetPlan.model_validate(fleet_plan(["web"], [node]))

    def test_missing_node_reports_missing_and_exits_nonzero(self) -> None:
        reports, code = run_status(self._plan(checks=False), RecordingClient(), [], status_args())

        assert code == 1
        assert reports[0]["status"] == "missing"
        assert not reports[0]["healthy"]

    def test_healthy_node_runs_each_check_exactly_once(self) -> None:
        fake = RecordingClient(present=["web"])
        reports, code = run_status(
            self._plan(checks=True), fake, [branch_info("web", ipv4="192.0.2.7")], status_args()
        )

        assert code == 0
        assert reports[0]["status"] == "running"
        assert reports[0]["ready"] == "1/1"
        assert reports[0]["address"] == "192.0.2.7"
        assert reports[0]["healthy"]
        # A status snapshot runs one attempt per check, never the deploy-time
        # retry loop (attempts=3 in the plan).
        assert fake.calls.count(("exec", "web", "curl", "--fail", "http://127.0.0.1:8080/")) == 1

    def test_failing_check_carries_detail_and_exits_nonzero(self) -> None:
        fake = RecordingClient(present=["web"])
        fake.exec_results["web"] = (7, "", "connection refused")
        reports, code = run_status(
            self._plan(checks=True), fake, [branch_info("web")], status_args()
        )

        assert code == 1
        assert reports[0]["ready"] == "0/1"
        assert reports[0]["checks"][0]["detail"] == "connection refused"

    def test_stopped_node_is_not_probed(self) -> None:
        fake = RecordingClient(present=["web"])
        rows = [branch_info("web", status=ix_sdk.BranchStatus.STOPPED)]
        reports, code = run_status(self._plan(checks=True), fake, rows, status_args())

        assert code == 1
        assert reports[0]["checks"][0]["detail"] == "node is stopped"
        assert all(call[0] != "exec" for call in fake.calls)

    def test_no_checks_skips_probes_but_keeps_liveness(self) -> None:
        fake = RecordingClient(present=["web"])
        reports, code = run_status(
            self._plan(checks=True), fake, [branch_info("web")], status_args(no_checks=True)
        )

        assert code == 0
        assert reports[0]["ready"] == "-"
        assert all(call[0] != "exec" for call in fake.calls)

    def test_missing_host_check_binary_marks_check_unhealthy(self) -> None:
        # A host check whose binary is absent raises OSError from
        # create_subprocess_exec; status must report one unhealthy check, not
        # abort the whole table.
        node = fleet_node("web")
        node["healthChecks"] = {
            "reach": {
                "description": "host-side probe",
                "command": ["/nonexistent/ix-fleet-test-probe"],
                "timeoutSec": 5,
                "attempts": 1,
                "intervalSec": 0,
                "from": "host",
            }
        }
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["web"], [node]))
        reports, code = run_status(
            plan, RecordingClient(present=["web"]), [branch_info("web")], status_args()
        )

        assert code == 1
        assert reports[0]["ready"] == "0/1"
        assert "No such file" in (reports[0]["checks"][0]["detail"] or "")

    def test_status_interval_rejects_non_positive_values(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()), pytest.raises(SystemExit):
            ix_fleet.parser().parse_args(["--plan", "plan.json", "status", "--interval", "0"])
        args = ix_fleet.parser().parse_args(["--plan", "plan.json", "status", "--interval", "3"])
        assert args.interval == 3

    def test_dry_run_reports_desired_state_without_live_calls(self) -> None:
        steps: list[str] = []

        def fail_client() -> typing.NoReturn:
            self.fail("status --dry-run must not touch the live client")

        with (
            patch.object(ix_fleet, "client", fail_client),
            patch.object(ix_fleet, "step", steps.append),
        ):
            asyncio.run(ix_fleet.cmd_status(self._plan(checks=True), status_args(dry_run=True)))

        assert steps == [
            "+ status web: image=registry.ix.dev/example/web:latest region=us-west-1 checks=http"
        ]

    def test_table_output_renders_kubectl_style_columns(self) -> None:
        table = ix_fleet.render_status_table(
            [
                ix_fleet.NodeStatus(
                    name="web",
                    status="running",
                    ready="1/1",
                    address="192.0.2.7",
                    region="us-west-1",
                    image="registry.ix.dev/example/web:latest",
                    desiredImage="registry.ix.dev/example/web:latest",
                    checks=[ix_fleet.CheckResult(name="http", healthy=True)],
                    healthy=True,
                )
            ],
            wide=True,
        )

        lines = table.splitlines()
        assert lines[0].split() == ["NODE", "STATUS", "READY", "ADDRESS", "REGION", "IMAGE", "DESIRED-IMAGE"]
        assert lines[1].split() == [
            "web",
            "running",
            "1/1",
            "192.0.2.7",
            "us-west-1",
            "registry.ix.dev/example/web:latest",
            "registry.ix.dev/example/web:latest",
        ]


class LogsTests(unittest.TestCase):
    def test_logs_prefix_multi_node_output_and_forward_flags(self) -> None:
        fake = RecordingClient(present=["db", "web"])
        fake.exec_results["web"] = (0, "GET / 200\n", "")
        fake.exec_results["db"] = (0, "ready to accept connections\n", "")
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(["db", "web"], [fleet_node("db"), fleet_node("web")])
        )
        args = argparse_namespace(on=[], dry_run=False, unit="app.service", lines=7, since=None)

        stdout = io.StringIO()
        with (
            patch.object(ix_fleet, "client", lambda: fake),
            contextlib.redirect_stdout(stdout),
        ):
            asyncio.run(ix_fleet.cmd_logs(plan, args))

        expected_exec = ("exec", "web", "journalctl", "--no-pager", "-n", "7", "-u", "app.service")
        assert expected_exec in fake.calls
        assert stdout.getvalue().splitlines() == [
            "[db] ready to accept connections",
            "[web] GET / 200",
        ]

    def test_single_node_logs_are_unprefixed(self) -> None:
        fake = RecordingClient(present=["web"])
        fake.exec_results["web"] = (0, "GET / 200\n", "")
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["web"], [fleet_node("web")]))
        args = argparse_namespace(on=["web"], dry_run=False, unit=None, lines=100, since=None)

        stdout = io.StringIO()
        with (
            patch.object(ix_fleet, "client", lambda: fake),
            contextlib.redirect_stdout(stdout),
        ):
            asyncio.run(ix_fleet.cmd_logs(plan, args))

        assert stdout.getvalue() == "GET / 200\n"

    def test_one_bad_node_still_prints_other_nodes_logs(self) -> None:
        fake = RecordingClient(present=["web"])  # db was deleted out-of-band
        fake.exec_results["web"] = (0, "GET / 200\n", "")
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(["db", "web"], [fleet_node("db"), fleet_node("web")])
        )
        args = argparse_namespace(on=[], dry_run=False, unit=None, lines=100, since=None)

        stdout = io.StringIO()
        with (
            patch.object(ix_fleet, "client", lambda: fake),
            contextlib.redirect_stdout(stdout),
            pytest.raises(RuntimeError, match="db: db not found"),
        ):
            asyncio.run(ix_fleet.cmd_logs(plan, args))

        assert stdout.getvalue() == "[web] GET / 200\n"

    def test_logs_selection_does_not_pull_in_dependencies(self) -> None:
        # `logs --on worker` must not also fetch web's logs the way deploy
        # selection pulls in the dependency closure.
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(["web", "worker"], [fleet_node("web"), fleet_node("worker", depends_on=["web"])])
        )
        assert [node.name for node in ix_fleet.selected_in_order(plan, ["worker"])] == ["worker"]


if __name__ == "__main__":
    unittest.main()
