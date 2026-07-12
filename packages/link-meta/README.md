# link-meta

Let a compiled binary tell the shell how to parse its output — metadata
embedded at link time, readable without executing anything.

A binary that emits JSON declares it once, in code. The declaration is a
small, versioned JSON blob (`{"v":1,"stdout":{"lens":"json"}}`) placed in a
dedicated section of the object file: `.ix.link` in ELF, `__DATA,__ix_link`
in Mach-O. A lens-aware shell reads the section when resolving the external
command and applies the named *lens* — a parsing strategy the shell owns — to
the command's stdout, so `^tool` behaves like `^tool | from json` with no
per-tool wrapper. The blob names the lens; the shell implements it. Unknown
sections, unknown versions, and unknown lens names all degrade to today's
raw-bytes behavior.

## Quickstart

In a binary crate:

```rust
link_meta::stdout_lens!("json");

fn main() {
    println!(r#"{"answer":42}"#);
}
```

Build it and the section is there:

```console
$ readelf -p .ix.link target/debug/my-tool
String dump of section '.ix.link':
  [     0]  {"v":1,"stdout":{"lens":"json"}}
```

In the patched nushell from this repo (`nix run .#nushell`), running
`^my-tool` now returns a record with `answer: 42` instead of a raw string.
[`./demo`](demo) is exactly this binary, kept as the working example.

## Pointers

- Consumer side: the nushell patch series in
  [`packages/nushell/patches`](../nushell/patches) reads the section in
  `run-external` and applies the `json` lens.
- Reading from Rust: `link_meta::read::stdout_lens(path)`.
