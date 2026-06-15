"""Parse the Flecs Query Language without a flecs world.

The `flecs query language <https://github.com/SanderMertens/flecs/blob/master/docs/FlecsQueryLanguage.md>`_
is the string format flecs uses for queries. This module wraps a pure-Rust
parser: it answers "is this well-formed, and what is its structure" with no
ECS world attached (identifier *resolution* is inherently world-dependent and
out of scope)::

    import flecs_query

    ast = flecs_query.parse("Position, [in] Velocity, (ChildOf, $parent)")
    # {'terms': [{'access': None, 'oper': 'And', 'body': {'Id': {...}}}, ...]}

    flecs_query.canonicalize("Position , !  Velocity")   # 'Position, !Velocity'

    flecs_query.validate("Position,, Velocity")
    # {'valid': False, 'error': "expected term, found ','", 'rendered': ...}

``parse`` and ``canonicalize`` raise ``ValueError`` with a caret-rendered
message on syntax errors; ``validate`` never raises.
"""

from __future__ import annotations

from ._flecs_query import __version__, canonicalize, parse, validate

__all__ = ["__version__", "canonicalize", "parse", "validate"]
