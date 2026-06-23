from __future__ import annotations

import logging
import os
import socket
from collections import Counter
from dataclasses import dataclass

import ray
from pydantic import Field
from pydantic_settings import BaseSettings, CliSettingsSource, PydanticBaseSettingsSource, SettingsConfigDict


LOGGER = logging.getLogger("ray_demo")


class CliArgs(BaseSettings):
    model_config = SettingsConfigDict(cli_parse_args=True, cli_kebab_case=True)

    address: str = Field(
        default_factory=lambda: os.environ.get("RAY_ADDRESS", "auto"),
        description="Ray cluster address. Defaults to $RAY_ADDRESS, else 'auto'.",
    )
    tasks: int = Field(
        default=24,
        description="Number of remote tasks to fan out.",
    )
    min_nodes: int = Field(
        default=1,
        description="Fail unless the cluster has at least this many alive nodes.",
    )

    @classmethod
    def settings_customise_sources(
        cls,
        settings_cls: type[BaseSettings],
        init_settings: PydanticBaseSettingsSource,
        env_settings: PydanticBaseSettingsSource,
        dotenv_settings: PydanticBaseSettingsSource,
        file_secret_settings: PydanticBaseSettingsSource,
    ) -> tuple[PydanticBaseSettingsSource, ...]:
        # Return only init + CLI sources; drop env/dotenv/secrets so that
        # ambient env vars (e.g. ADDRESS, TASKS) cannot silently populate
        # settings (argparse parity, CWE-15).
        return (
            init_settings,
            CliSettingsSource(settings_cls, cli_parse_args=True),
        )


@dataclass(frozen=True)
class Probe:
    node_id: str
    hostname: str
    pid: int
    value: int


def parse_args() -> CliArgs:
    return CliArgs()


@ray.remote
def probe(value: int) -> Probe:
    """Run on some cluster node and report which one handled this task."""
    context = ray.get_runtime_context()
    # A trivial CPU spin so the scheduler has a reason to spread tasks across
    # nodes rather than packing them onto the first worker it sees.
    total = 0
    for index in range(200_000):
        total += index * value
    return Probe(
        node_id=context.get_node_id(),
        hostname=socket.gethostname(),
        pid=os.getpid(),
        value=total,
    )


def run(args: CliArgs) -> int:
    ray.init(address=args.address, log_to_driver=False)
    try:
        alive_nodes = [node for node in ray.nodes() if node.get("Alive", False)]
        cluster_cpus = int(ray.cluster_resources().get("CPU", 0))
        LOGGER.info(
            "cluster: %d alive node(s), %d CPU(s)",
            len(alive_nodes),
            cluster_cpus,
        )

        # Gate on cluster membership, which is deterministic: a worker either
        # joined the GCS or it did not. Task placement below is reported but
        # not asserted, since the scheduler may pack tasks onto fewer nodes.
        if len(alive_nodes) < args.min_nodes:
            LOGGER.error(
                "expected at least %d alive node(s), saw %d",
                args.min_nodes,
                len(alive_nodes),
            )
            return 1

        refs = [probe.remote(value) for value in range(args.tasks)]
        results = ray.get(refs)
    finally:
        ray.shutdown()

    placement = Counter(result.hostname for result in results)
    for hostname, count in sorted(placement.items()):
        LOGGER.info("  %-24s ran %d task(s)", hostname, count)

    LOGGER.info(
        "ran %d task(s) across %d distinct node(s)",
        len(results),
        len(placement),
    )
    return 0


def main() -> None:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
    raise SystemExit(run(parse_args()))
