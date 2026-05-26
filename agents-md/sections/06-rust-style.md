## Rust style

Repo-owned crates, fixtures, examples, and generated manifests use Rust edition
2024. Fix compatibility issues directly and document unavoidable upstream
blockers next to the exception.

Prefer names that preserve the concept's path. Local aliases may shorten noisy
source paths only when the shape remains visible at the call site. Keep singular
names for single values and plural names for bags of constructors, helpers, or
registry entries.

Use local type annotations when they make the data shape clearer. Keep turbofish
for expression-local cases where an intermediate binding would add noise.

Use normal module layout. Move files so `mod` declarations follow the filesystem
instead of using `#[path = ...]`.

Avoid anonymous tuple-shaped domain data once a value crosses a function
boundary. Prefer named structs or full paths for values that carry real meaning.

Use blank lines as paragraph breaks inside functions: set up, act, then validate
or return. Keep tightly coupled statements together.

When parsing, normalizing, serializing, traversing graphs, handling archives, or
speaking protocols, start from a maintained crate. Hand-written logic is for the
thin glue around that crate unless the dependency boundary is measurably worse.

