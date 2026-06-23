from __future__ import annotations

import asyncio
import typing
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import pytest
from pydantic import ValidationError

import ix_fleet


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
            "source": f"/nix/store/{name}-image.tar",
            "sourceDrv": f"/nix/store/{name}-image.drv",
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

    def test_accepts_declarative_secret_backend_and_refs(self) -> None:
        data = fleet_plan(["web"], [fleet_node("web")])
        data["secrets"] = {
            "provider": {
                "type": "vaultwarden",
                "mountRoot": "/run/secrets/fleet",
                "collection": "production",
            },
            "values": {
                "sessionKey": {
                    "key": "web/session-key",
                    "path": "/run/secrets/fleet/sessionKey",
                    "generate": True,
                },
            },
        }

        plan = ix_fleet.FleetPlan.model_validate(data)

        assert plan.secrets.provider.type == "vaultwarden"
        assert plan.secrets.provider.model_extra == {"collection": "production"}
        assert plan.secrets.values["sessionKey"].path == "/run/secrets/fleet/sessionKey"
        assert plan.secrets.values["sessionKey"].model_extra == {"generate": True}

    def test_per_vm_secret_refs_default_empty_and_round_trip(self) -> None:
        bare = ix_fleet.FleetNode.model_validate(fleet_node("web"))
        assert bare.secrets == []
        assert not bare.noDefaultSecrets

        node = fleet_node("api")
        node["secrets"] = ["GH_TOKEN", "DATABASE_URL"]
        node["noDefaultSecrets"] = True
        parsed = ix_fleet.FleetNode.model_validate(node)
        assert parsed.secrets == ["GH_TOKEN", "DATABASE_URL"]
        assert parsed.noDefaultSecrets


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
        node["secrets"] = secrets
        return ix_fleet.FleetPlan.model_validate(fleet_plan(["web"], [node]))

    def test_passes_when_every_referenced_secret_exists(self) -> None:
        plan = self._plan(["GH_TOKEN"])
        with patch.object(ix_fleet, "client", self._fake_client(["GH_TOKEN", "OTHER"])):
            asyncio.run(ix_fleet.verify_secrets_available(plan, [], dry_run=False))

    def test_raises_listing_missing_secrets(self) -> None:
        plan = self._plan(["GH_TOKEN", "DATABASE_URL"])
        with (
            patch.object(ix_fleet, "client", self._fake_client(["GH_TOKEN"])),
            pytest.raises(RuntimeError, match=r"missing secret\(s\) in the store: DATABASE_URL"),
        ):
            asyncio.run(ix_fleet.verify_secrets_available(plan, [], dry_run=False))

    def test_dry_run_makes_no_live_call(self) -> None:
        plan = self._plan(["MISSING"])

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
    def test_uses_image_subcommand(self) -> None:
        with TemporaryDirectory() as temporary_directory:
            source = Path(temporary_directory) / "image.tar"
            source.write_text("")
            calls: list[list[str]] = []

            async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
                del timeout
                calls.append(command)
                if command[0] == "nix-store":
                    return f"{source}\n"
                assert not dry_run
                return "registry.ix.dev/example/health-check-nginx:nginx-lifecycle\n"

            node = ix_fleet.FleetNode.model_validate(
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
                        "destination": "health-check-nginx:nginx-lifecycle",
                        "source": str(source),
                        "sourceDrv": "/nix/store/example-image.drv",
                    },
                    "region": "us-west-1",
                    "ipv4": False,
                    "snapshot": True,
                }
            )

            with patch.object(ix_fleet, "run_cli", fake_run_cli):
                image = asyncio.run(ix_fleet.push_replacement_image(node, dry_run=False))

            assert calls == [
                ["nix-store", "--realise", "/nix/store/example-image.drv"],
                ["ix", "image", "push", str(source), "health-check-nginx:nginx-lifecycle"],
            ]
            assert image == "registry.ix.dev/example/health-check-nginx:nginx-lifecycle"


class UpNodeTests(unittest.TestCase):
    def test_replaces_existing_running_node_with_uploaded_image(self) -> None:
        calls: list[list[str]] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("web"))

        async def fake_list_nodes() -> list[dict[str, typing.Any]]:
            return [{"name": "web", "status": "running"}]

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            assert not dry_run
            calls.append(command)
            return ""

        with (
            patch.object(ix_fleet, "list_nodes", fake_list_nodes),
            patch.object(ix_fleet, "run_cli", fake_run_cli),
        ):
            asyncio.run(ix_fleet.up_node(node, "registry.ix.dev/example/web:new", dry_run=False))

        assert calls == [
            [
                "ix",
                "new",
                "registry.ix.dev/example/web:new",
                "--name",
                "web",
                "--region",
                "us-west-1",
                "--no-shell",
            ]
        ]

    def test_replaces_existing_stopped_node_instead_of_starting_old_image(self) -> None:
        calls: list[list[str]] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("web"))

        async def fake_list_nodes() -> list[dict[str, typing.Any]]:
            return [{"name": "web", "status": "stopped"}]

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            assert not dry_run
            calls.append(command)
            return ""

        with (
            patch.object(ix_fleet, "list_nodes", fake_list_nodes),
            patch.object(ix_fleet, "run_cli", fake_run_cli),
        ):
            asyncio.run(ix_fleet.up_node(node, "registry.ix.dev/example/web:new", dry_run=False))

        assert calls[0][:3] == ["ix", "new", "registry.ix.dev/example/web:new"]
        assert ["ix", "start", "web"] not in calls

    def test_dry_run_shows_possible_node_replacement_without_live_lookup(self) -> None:
        calls: list[list[str]] = []
        steps: list[str] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("web"))

        async def fail_list_nodes() -> list[dict[str, typing.Any]]:
            self.fail("dry-run up should not require live node state")

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            assert dry_run
            calls.append(command)
            return ""

        with (
            patch.object(ix_fleet, "list_nodes", fail_list_nodes),
            patch.object(ix_fleet, "run_cli", fake_run_cli),
            patch.object(ix_fleet, "step", steps.append),
        ):
            asyncio.run(ix_fleet.up_node(node, "registry.ix.dev/example/web:new", dry_run=True))

        assert steps == ["create or replace web from uploaded image registry.ix.dev/example/web:new"]
        assert calls[0][:3] == ["ix", "new", "registry.ix.dev/example/web:new"]


class EastWestGroupTests(unittest.TestCase):
    def test_ensures_group_before_adding_node(self) -> None:
        calls: list[list[str]] = []

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            assert not dry_run
            calls.append(command)
            return ""

        node_data = fleet_node("api")
        node_data["groups"] = ["private-apps"]
        node = ix_fleet.FleetNode.model_validate(node_data)

        with patch.object(ix_fleet, "run_cli", fake_run_cli):
            asyncio.run(ix_fleet.ensure_node_groups(node, dry_run=False))

        assert calls == [
            ["ix", "group", "create", "private-apps"],
            ["ix", "group", "add", "private-apps", "api"],
        ]

    def test_existing_group_membership_is_idempotent(self) -> None:
        calls: list[list[str]] = []

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            assert not dry_run
            calls.append(command)
            if command[:3] == ["ix", "group", "create"]:
                raise ix_fleet.CliError(command, 1, "", "group already exists")
            if command[:3] == ["ix", "group", "add"]:
                raise ix_fleet.CliError(command, 1, "", "vm is already a member of group")
            return ""

        node_data = fleet_node("api")
        node_data["groups"] = ["private-apps"]
        node = ix_fleet.FleetNode.model_validate(node_data)

        with patch.object(ix_fleet, "run_cli", fake_run_cli):
            asyncio.run(ix_fleet.ensure_node_groups(node, dry_run=False))

        assert calls == [
            ["ix", "group", "create", "private-apps"],
            ["ix", "group", "add", "private-apps", "api"],
        ]


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
        calls: list[list[str]] = []
        ready: list[str] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("api"))

        async def fake_list_nodes() -> list[dict[str, typing.Any]]:
            return []

        async def fake_wait_node_ready(node: ix_fleet.FleetNode, *, dry_run: bool) -> None:
            assert not dry_run
            ready.append(node.name)

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            assert not dry_run
            calls.append(command)
            return ""

        with (
            patch.object(ix_fleet, "list_nodes", fake_list_nodes),
            patch.object(ix_fleet, "run_cli", fake_run_cli),
            patch.object(ix_fleet, "wait_node_ready", fake_wait_node_ready),
        ):
            asyncio.run(ix_fleet.bootstrap_node(node, dry_run=False))

        assert calls == [
            [
                "ix",
                "new",
                "registry.ix.dev/ix/base:latest",
                "--name",
                "api",
                "--region",
                "us-west-1",
                "--no-shell",
            ],
        ]
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
    def test_up_node_workflow_runs_the_existing_up_sequence(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api"], [fleet_node("api")]))
        args = argparse_namespace(
            node="api",
            dry_run=False,
            skip_push=False,
            skip_health=False,
        )
        calls: list[str] = []

        with (
            patch.object(
                ix_fleet,
                "push_replacement_image",
                async_recorder(calls, "push", "registry.ix.dev/example/api:pushed"),
            ),
            patch.object(ix_fleet, "up_node", async_recorder(calls, "up")),
            patch.object(ix_fleet, "ensure_node_groups", async_recorder(calls, "groups")),
            patch.object(ix_fleet, "run_node_health_checks", async_recorder(calls, "health")),
        ):
            asyncio.run(ix_fleet.cmd_up_node(plan, args))

        assert calls == ["push", "up", "groups", "health"]

    def test_replace_node_workflow_runs_the_existing_replace_sequence(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(fleet_plan(["api"], [fleet_node("api")]))
        args = argparse_namespace(
            node="api",
            dry_run=False,
            skip_push=False,
            skip_health=False,
        )
        calls: list[str] = []

        with (
            patch.object(
                ix_fleet,
                "push_replacement_image",
                async_recorder(calls, "push", "registry.ix.dev/example/api:pushed"),
            ),
            patch.object(ix_fleet, "replace_node", async_recorder(calls, "replace")),
            patch.object(ix_fleet, "ensure_node_groups", async_recorder(calls, "groups")),
            patch.object(ix_fleet, "run_node_health_checks", async_recorder(calls, "health")),
        ):
            asyncio.run(ix_fleet.cmd_replace_node(plan, args))

        assert calls == ["push", "replace", "groups", "health"]

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
        calls: list[list[str]] = []

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del dry_run, timeout
            calls.append(command)
            if command[-1] == "web":
                raise ix_fleet.CliError(command, 1, "", "permission denied")
            return ""

        with patch.object(ix_fleet, "run_cli", fake_run_cli):
            args = argparse_namespace(on=[], dry_run=False)
            with pytest.raises(RuntimeError, match="web: command failed"):
                asyncio.run(ix_fleet.cmd_down(plan, args))

        assert calls == [
            ["ix", "rm", "--force", "web"],
            ["ix", "rm", "--force", "db"],
        ]

    def test_down_treats_missing_nodes_as_absent(self) -> None:
        node = ix_fleet.FleetNode.model_validate(fleet_node("api"))

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del dry_run, timeout
            raise ix_fleet.CliError(command, 1, "", "VM not found")

        with patch.object(ix_fleet, "run_cli", fake_run_cli):
            asyncio.run(ix_fleet.remove_node(node, dry_run=False))


class SwitchSourceTests(unittest.TestCase):
    def test_source_switch_runs_ix_up_from_source_root(self) -> None:
        # `ix switch` was folded into `ix up` (indexable-inc/ix#4442): the source
        # switch now runs `ix up <installable> --name <vm>` from the source root
        # (which `ix up` auto-uploads), with `--workdir` relative to that root and
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

        async def fake_run_cli(
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

        with patch.object(ix_fleet, "run_cli", fake_run_cli):
            asyncio.run(
                ix_fleet.switch_node_from_source(
                    node,
                    Path("/source"),
                    Path("/source/subdir"),
                    dry_run=False,
                )
            )

        assert calls == [
            [
                "ix",
                "up",
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
        # loudly rather than forwarding a path `ix up` cannot interpret.
        node = ix_fleet.FleetNode.model_validate(fleet_node("api"))

        async def fail_run_cli(*args: typing.Any, **kwargs: typing.Any) -> str:  # noqa: ANN401
            del args, kwargs
            raise AssertionError("run_cli should not be reached")

        with (
            patch.object(ix_fleet, "run_cli", fail_run_cli),
            pytest.raises(ValueError, match="outside source root"),
        ):
            asyncio.run(
                ix_fleet.switch_node_from_source(
                    node,
                    Path("/source"),
                    Path("/elsewhere/subdir"),
                    dry_run=False,
                )
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


class SwitchBatchTests(unittest.TestCase):
    def _node(self, data: dict[str, typing.Any]) -> ix_fleet.FleetNode:
        return ix_fleet.FleetNode.model_validate(data)

    def test_is_batchable_switch(self) -> None:
        assert ix_fleet.is_batchable_switch(self._node(remote_node("api")))
        # local build: no shared builder to batch on.
        local = fleet_node("api")
        local["switch"]["buildOn"] = "local"
        assert not ix_fleet.is_batchable_switch(self._node(local))
        # remote but no build VM: multi `ix up` requires --build-vm.
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

    def test_switch_nodes_from_source_builds_one_multi_ix_up(self) -> None:
        nodes = [self._node(remote_node("web")), self._node(remote_node("worker"))]
        calls: list[list[str]] = []
        cwds: list[Path | None] = []

        async def fake_run_cli(
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

        with patch.object(ix_fleet, "run_cli", fake_run_cli):
            asyncio.run(
                ix_fleet.switch_nodes_from_source(
                    nodes,
                    Path("/source"),
                    Path("/source/subdir"),
                    dry_run=False,
                )
            )

        # One native multi-VM `ix up`: every installable, one shared --build-vm,
        # and no --name (multi derives each VM name from its installable attr).
        assert calls == [
            [
                "ix",
                "up",
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


if __name__ == "__main__":
    unittest.main()
