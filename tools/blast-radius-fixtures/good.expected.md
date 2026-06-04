<!-- blast-radius -->
### Blast radius

`5` of `120` checks would rebuild between base `aaaaaaa` and head `bbbbbbb`.

1 added, 0 removed

```mermaid
pie showData title Rebuilt checks by category
  "rust" : 2
  "mcp" : 2
  "image" : 1
  "lint" : 1
```

```mermaid
flowchart LR
  c0["ix-rust-workspace"]
  c1["image-base (<1s)"]
  c2["lint"]
  c0 --> k2["mcp-serverTools (42s)"]
  c0 --> k3["rust-test-search_core (2m)"]
```

<details><summary>changed checks</summary>

- mcp-serverTools (42s)
- rust-test-search_core (2m)
- image-base (<1s)
- lint

</details>
