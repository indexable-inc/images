---
name: python-style
description: "Python conventions: uv projects, buildUvApplication, writePythonApplication, type-checking (ty/zuban/mypy) and ruff ANN. Use when writing or packaging Python."
---

## Python style

Default repo-owned Python apps to uv: `pyproject.toml`, committed `uv.lock`,
normal `src/<package>/` files, and Nix packaging through
[`ix.buildUvApplication`](lib/build-uv-application.nix).

Use [`ix.writePythonApplication`](lib/default.nix) for tiny single-file commands
without PyPI dependencies or multiple source files. Once a script needs
dependencies, console entry points, or a package layout, give it the uv project
shape.

The Python helpers run a type checker at build time, selected with `pyChecker`,
which **defaults to `"zuban"`**: `zuban check --strict` (correctness, including
disallow-untyped-defs) plus `ruff check` with the shared
[`lib/build/ruff-ann.nix`](lib/build/ruff-ann.nix) selector. So a new uv app or
`writePythonApplication` script must be fully annotated and pass the lints out of
the box. Set `pyChecker = "ty"` (the older gradual checker) or `"mypy"` for a
deliberate reason; disable entirely with `check = false` only when justified.

The ruff selector is a high-signal "really good lints" set (one source of truth
in `ruff-ann.nix`, consumed by every gate): the bug-catchers and modernizers
`B`/`ASYNC`/`SIM`/`RET`/`C4`/`PIE`/`UP`/`RUF`/`PERF`/`FURB`/`PLE`/`PLW`/`LOG`/`G`/
`DTZ`/`FLY`/`ISC`, the security family `S` (bandit), `PTH` (pathlib), `PT`
(pytest), `FBT` (boolean-trap), plus `ANN` (explicit annotations; `ANN401` bans
bare `typing.Any`) and `TID251` (no `typing.cast`). Contextual/noisy members are
globally ignored (`S101` asserts, `S603`/`S607` fixed-arg subprocess, `PLW0603`
caches, `ISC001`, `RUF001/2/3`); pure-stylistic families (`TRY003`, `EM`, `T201`,
`N`, `ARG`, `D`) are not selected. A genuine false positive opts out per line with
an annotated `# noqa: <rule> -- <reason>`, never a blanket silence.

Enforcement is two-layer, sharing that one selector: each package's build gate
runs it over the package, and a repo-wide `ruff` lint stage
(`lib/per-system.nix`, in `nix run .#lint` + the CI `lint` check) runs it over
**every** tracked `.py` — including `tools/`, `users/`, `skills/`, `sdk/`,
`examples/`, `lib/` that no package gate covers.

`typing.Any` in annotations is already banned everywhere by `ruff ANN401`, and
`typing.cast` is banned by `ruff TID251`: a cast lies to the type checker at zero
runtime cost, so a wrong one is a latent bug no checker can catch. For external or
untrusted JSON (an HTTP API response, a config file, a GraphQL reply), parse it
into a [pydantic](https://docs.pydantic.dev) model at the boundary rather than
casting or threading `dict[str, Any]`/`object` through the code: the model
validates the shape once and fails with a path-precise error when upstream drifts,
and every downstream access is typed. Worked examples: `packages/update-loaders` (a
`TypeAdapter` over the PaperMC response) and the bundled `linear`/`google_auth`
modules (typed models returned from the GraphQL/CLI boundary). For an untyped
stdlib boundary that genuinely returns the type you expect (e.g. an HTTP
`response.read()` that is `bytes`), prefer an explicit local annotation
(`body: bytes = response.read()`) over a cast. The rare genuinely-unavoidable cast
— casting a test double to the interface it stands in for — opts out per file with
`# noqa: TID251` plus a one-line reason. Reserve a bare `object` annotation for
genuinely opaque values where narrowing is the caller's job
(`def __eq__(self, other: object)`, `*args: object`), and leave a comment saying
why; do not use it as a shortcut to skip modeling data you actually read.
