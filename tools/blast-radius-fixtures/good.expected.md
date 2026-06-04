<!-- blast-radius -->
### Blast radius

`4` of `120` checks would rebuild between base `aaaaaaa` and head `bbbbbbb`.

1 added, 0 removed

```mermaid
pie showData title Rebuilt checks by category
  "rust" : 2
  "mcp" : 2
  "image" : 1
```

```mermaid
flowchart LR
  c0["ix-rust-workspace"]
  c1["image-base-layer"]
  c0 --> k1["mcp-serverTools"]
  c0 --> k2["rust-test-search_core"]
  c1 --> k0["image-base"]
```

<details><summary>changed checks</summary>

- mcp-serverTools
- rust-test-search_core
- image-base

</details>
