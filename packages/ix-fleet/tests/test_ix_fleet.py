from __future__ import annotations

import asyncio
import typing
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

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
            "imageTag": "latest",
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

        with self.assertRaisesRegex(ValidationError, "order is missing node 'db'"):
            ix_fleet.FleetPlan.model_validate(data)

    def test_rejects_duplicate_order_entries(self) -> None:
        data = fleet_plan(["web", "web"], [fleet_node("web")])

        with self.assertRaisesRegex(ValidationError, "order contains duplicate node 'web'"):
            ix_fleet.FleetPlan.model_validate(data)

    def test_selected_nodes_keeps_dependencies_before_selected_node(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(["db", "web"], [fleet_node("web", depends_on=["db"]), fleet_node("db")])
        )

        self.assertEqual(
            [node.name for node in ix_fleet.selected_nodes(plan, ["web"])],
            ["db", "web"],
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

        self.assertEqual(plan.secrets.provider.type, "vaultwarden")
        self.assertEqual(plan.secrets.provider.model_extra, {"collection": "production"})
        self.assertEqual(plan.secrets.values["sessionKey"].path, "/run/secrets/fleet/sessionKey")
        self.assertEqual(plan.secrets.values["sessionKey"].model_extra, {"generate": True})


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
                self.assertFalse(dry_run)
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
                        "imageTag": "nginx-lifecycle",
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

            self.assertEqual(
                calls,
                [
                    ["nix-store", "--realise", "/nix/store/example-image.drv"],
                    ["ix", "image", "push", str(source), "health-check-nginx:nginx-lifecycle"],
                ],
            )
            self.assertEqual(image, "registry.ix.dev/example/health-check-nginx:nginx-lifecycle")


class UpNodeTests(unittest.TestCase):
    def test_replaces_existing_running_node_with_uploaded_image(self) -> None:
        calls: list[list[str]] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("web"))

        async def fake_list_nodes() -> list[dict[str, typing.Any]]:
            return [{"name": "web", "status": "running"}]

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            self.assertFalse(dry_run)
            calls.append(command)
            return ""

        with (
            patch.object(ix_fleet, "list_nodes", fake_list_nodes),
            patch.object(ix_fleet, "run_cli", fake_run_cli),
        ):
            asyncio.run(ix_fleet.up_node(node, "registry.ix.dev/example/web:new", dry_run=False))

        self.assertEqual(
            calls,
            [
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
            ],
        )

    def test_replaces_existing_stopped_node_instead_of_starting_old_image(self) -> None:
        calls: list[list[str]] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("web"))

        async def fake_list_nodes() -> list[dict[str, typing.Any]]:
            return [{"name": "web", "status": "stopped"}]

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            self.assertFalse(dry_run)
            calls.append(command)
            return ""

        with (
            patch.object(ix_fleet, "list_nodes", fake_list_nodes),
            patch.object(ix_fleet, "run_cli", fake_run_cli),
        ):
            asyncio.run(ix_fleet.up_node(node, "registry.ix.dev/example/web:new", dry_run=False))

        self.assertEqual(calls[0][:3], ["ix", "new", "registry.ix.dev/example/web:new"])
        self.assertNotIn(["ix", "start", "web"], calls)

    def test_dry_run_shows_possible_node_replacement_without_live_lookup(self) -> None:
        calls: list[list[str]] = []
        steps: list[str] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("web"))

        async def fail_list_nodes() -> list[dict[str, typing.Any]]:
            self.fail("dry-run up should not require live node state")

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            self.assertTrue(dry_run)
            calls.append(command)
            return ""

        with (
            patch.object(ix_fleet, "list_nodes", fail_list_nodes),
            patch.object(ix_fleet, "run_cli", fake_run_cli),
            patch.object(ix_fleet, "step", steps.append),
        ):
            asyncio.run(ix_fleet.up_node(node, "registry.ix.dev/example/web:new", dry_run=True))

        self.assertEqual(steps, ["create or replace web from uploaded image registry.ix.dev/example/web:new"])
        self.assertEqual(calls[0][:3], ["ix", "new", "registry.ix.dev/example/web:new"])


class EastWestGroupTests(unittest.TestCase):
    def test_ensures_group_before_adding_node(self) -> None:
        calls: list[list[str]] = []

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            self.assertFalse(dry_run)
            calls.append(command)
            return ""

        node_data = fleet_node("api")
        node_data["groups"] = ["private-apps"]
        node = ix_fleet.FleetNode.model_validate(node_data)

        with patch.object(ix_fleet, "run_cli", fake_run_cli):
            asyncio.run(ix_fleet.ensure_node_groups(node, dry_run=False))

        self.assertEqual(
            calls,
            [
                ["ix", "group", "create", "private-apps"],
                ["ix", "group", "add", "private-apps", "api"],
            ],
        )

    def test_existing_group_membership_is_idempotent(self) -> None:
        calls: list[list[str]] = []

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            self.assertFalse(dry_run)
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

        self.assertEqual(
            calls,
            [
                ["ix", "group", "create", "private-apps"],
                ["ix", "group", "add", "private-apps", "api"],
            ],
        )


class BootstrapTests(unittest.TestCase):
    def test_bootstrap_waits_for_dependencies_before_selected_node(self) -> None:
        plan = ix_fleet.FleetPlan.model_validate(
            fleet_plan(["db", "web"], [fleet_node("web", depends_on=["db"]), fleet_node("db")])
        )
        calls: list[str] = []

        async def fake_bootstrap_node(node: ix_fleet.FleetNode, *, dry_run: bool) -> None:
            self.assertFalse(dry_run)
            calls.append(node.name)

        with patch.object(ix_fleet, "bootstrap_node", fake_bootstrap_node):
            args = argparse_namespace(on=["web"], dry_run=False)
            asyncio.run(ix_fleet.cmd_bootstrap(plan, args))

        self.assertEqual(calls, ["db", "web"])

    def test_bootstrap_uses_bootstrap_image_without_replacement_push(self) -> None:
        calls: list[list[str]] = []
        ready: list[str] = []
        node = ix_fleet.FleetNode.model_validate(fleet_node("api"))

        async def fake_list_nodes() -> list[dict[str, typing.Any]]:
            return []

        async def fake_wait_node_ready(node: ix_fleet.FleetNode, *, dry_run: bool) -> None:
            self.assertFalse(dry_run)
            ready.append(node.name)

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            self.assertFalse(dry_run)
            calls.append(command)
            return ""

        with (
            patch.object(ix_fleet, "list_nodes", fake_list_nodes),
            patch.object(ix_fleet, "run_cli", fake_run_cli),
            patch.object(ix_fleet, "wait_node_ready", fake_wait_node_ready),
        ):
            asyncio.run(ix_fleet.bootstrap_node(node, dry_run=False))

        self.assertEqual(
            calls,
            [
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
            ],
        )
        self.assertEqual(ready, ["api"])


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
            with self.assertRaisesRegex(RuntimeError, "web: command failed"):
                asyncio.run(ix_fleet.cmd_down(plan, args))

        self.assertEqual(
            calls,
            [
                ["ix", "rm", "--force", "web"],
                ["ix", "rm", "--force", "db"],
            ],
        )

    def test_down_treats_missing_nodes_as_absent(self) -> None:
        node = ix_fleet.FleetNode.model_validate(fleet_node("api"))

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del dry_run, timeout
            raise ix_fleet.CliError(command, 1, "", "VM not found")

        with patch.object(ix_fleet, "run_cli", fake_run_cli):
            asyncio.run(ix_fleet.remove_node(node, dry_run=False))


class SwitchSourceTests(unittest.TestCase):
    def test_override_inputs_are_separate_nix_flag_arguments(self) -> None:
        calls: list[list[str]] = []
        node_data = fleet_node("api")
        node_data["switch"]["buildVm"] = "builder"
        node_data["switch"]["overrideInputs"] = {
            "ix": "github:indexable-inc/ix",
            "ix-images": "path:/workspace/index",
        }
        node = ix_fleet.FleetNode.model_validate(node_data)

        async def fake_run_cli(command: list[str], *, dry_run: bool, timeout: int | None = None) -> str:
            del timeout
            self.assertFalse(dry_run)
            calls.append(command)
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

        self.assertEqual(
            calls,
            [
                [
                    "ix",
                    "switch",
                    "api",
                    ".#api",
                    "--build-on",
                    "remote",
                    "--source",
                    "/source",
                    "--source-workdir",
                    "/source/subdir",
                    "--build-vm",
                    "builder",
                    "--override-input",
                    "ix",
                    "github:indexable-inc/ix",
                    "--override-input",
                    "ix-images",
                    "path:/workspace/index",
                ],
            ],
        )


def argparse_namespace(**kwargs: typing.Any) -> typing.Any:
    return type("Args", (), kwargs)()


if __name__ == "__main__":
    unittest.main()
