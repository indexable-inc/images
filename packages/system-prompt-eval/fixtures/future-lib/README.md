# future-lib

A tiny utility library. (Note: this README is intentionally stale; it documents
an older release. The eval checks whether an agent trusts these docs/its memory
or reads the actual code.)

## retry

`@retry` re-runs a function on exception. By default it makes **3 attempts** with
**exponential backoff** (delay doubles each attempt).

## slugify

`slugify(text)` returns a URL slug: it **lowercases** the text and replaces runs
of whitespace with a **hyphen** (`-`). So `slugify("Hello World")` is
`"hello-world"`.
