"""What is happening on every node, answered from the journal (index#3193).

The weave journal, not Ray's dashboard, is the system of record: Ray only
knows about actors and tasks that are currently alive, while the journal has
every run's full record (who asked, which function, which node, how it
ended). Ray's own dashboard stays a substrate debugging tool (raylet health,
object store); anything about WORK comes from one datalog query:

    ?- type(T, task), node(T, N), latest(T, fn, F), latest(T, state, S).

One row per fabric run: its task entity, the node it was placed on (``node``
is a journal bridge relation; ``latest`` is latest-wins per entity/attr, so
``state`` is each run's current state). Filter ``state`` to
:data:`fabric.reconcile.OPEN_STATES` for the live view; drop the filter for
history. ``await fabric.activity.frame()`` returns it as a Polars frame and
``fabric.activity.publish()`` keeps it on the live dashboard as a
self-refreshing pane.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

import weave

from .reconcile import OPEN_STATES

if TYPE_CHECKING:
    import polars

__all__ = ["QUERY", "frame", "publish"]

QUERY = "?- type(T, task), node(T, N), latest(T, fn, F), latest(T, state, S)."


async def frame(*, open_only: bool = True) -> polars.DataFrame:
    """The per-node run table: columns task, node, fn, state.

    ``open_only`` keeps only runs with no terminal fact yet (the "what is
    happening" view); pass ``open_only=False`` for the full history.
    """

    import polars as pl

    rows = (await weave.query(QUERY))["rows"]
    df = pl.DataFrame(
        [[str(v) for v in row] for row in rows],
        schema={"task": pl.String, "node": pl.String, "fn": pl.String, "state": pl.String},
        orient="row",
    )
    if open_only:
        df = df.filter(pl.col("state").is_in(sorted(OPEN_STATES)))
    return df.sort(["node", "task"])


def publish() -> object:
    """Pin the activity table to the live dashboard as a self-refreshing pane.

    Returns the resource handle (``.close()`` removes the pane). The id is
    stable, so calling this again replaces the pane instead of stacking a new
    one. Kernel-only: the dashboard runtime and ``view`` renderer exist in the
    kernel process, so this is imported lazily and not exercised by the
    sandboxed test suite.
    """

    from ix_notebook_mcp.runtime import register_resource
    from view import df_html

    async def render() -> str:
        return df_html(await frame())

    return register_resource(render=render, id="fabric-activity", title="fabric activity")
