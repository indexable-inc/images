<!-- blast-radius -->
### Blast radius

`5` of `120` checks would rebuild between base `aaaaaaa` and head `bbbbbbb`.

1 added, 0 removed

```mermaid
flowchart LR
  c0["ix-rust-workspace"]
  c1["image-base (<1s)"]
  c2["lint"]
  c0 --> k2["mcp-serverTools (42s)"]
  c0 --> k3["rust-test-search_core (2m)"]
```

<details><summary>changed checks (4)</summary>

- mcp-serverTools (42s)
- rust-test-search_core (2m)
- image-base (<1s)
- lint

</details>
