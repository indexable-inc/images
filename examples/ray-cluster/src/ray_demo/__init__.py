from __future__ import annotations

import argparse
import logging
import os
import socket
from collections import Counter
from dataclasses import dataclass
from typing import cast

import ray


LOGGER = logging.getLogger("ray_demo")


@dataclass(frozen=True)
class CliArgs:
    address: str
    tasks: int
    min_nodes: int


@dataclass(frozen=True)
class Probe:
    node_id: str
    hostname: str
    pid: int
    value: int


def parse_args() -> CliArgs:
    parser = argparse.ArgumentParser(
        prog="ray-demo",
        description="Fan out tasks across a Ray cluster and report node placement.",
    )
    _ = parser.add_argument(
        "--address",
        default=os.environ.get("RAY_ADDRESS", "auto"),
        help="Ray cluster address. Defaults to $RAY_ADDRESS, else 'auto'.",
    )
    _ = parser.add_argument(
        "--tasks",
        type=int,
        default=24,
        help="Number of remote tasks to fan out.",
    )
    _ = parser.add_argument(
        "--min-nodes",
        type=int,
        default=1,
        help="Fail unless the cluster has at least this many alive nodes.",
    )
    namespace = parser.parse_args()

    return CliArgs(
        address=cast(str, namespace.address),
        tasks=cast(int, namespace.tasks),
        min_nodes=cast(int, namespace.min_nodes),
    )


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
