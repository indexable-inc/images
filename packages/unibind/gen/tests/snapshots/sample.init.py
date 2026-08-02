"""A sample boundary for the emitter tests.

Everything the py generator renders appears here once.
"""

from ._sample import (
    Io,
    ParseError,
    Row,
    SampleError,
    Source,
    Store,
    StoreCursor,
    StoreWatchStream,
    TailStream,
    __version__,
    find,
    greet,
    ping,
    rows,
    tail,
    write_file,
)

__all__ = [
    "Io",
    "ParseError",
    "Row",
    "SampleError",
    "Source",
    "Store",
    "StoreCursor",
    "StoreWatchStream",
    "TailStream",
    "__version__",
    "find",
    "greet",
    "ping",
    "rows",
    "tail",
    "write_file",
]

import collections.abc as _collections_abc

for _record in (
    Row,
    Source,
):
    _collections_abc.Mapping.register(_record)
del _collections_abc, _record

