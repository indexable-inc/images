<!-- net-trace -->
### Client-side network during CI

| phase | wall | network wall | conns | failed | down | up |
| --- | --- | --- | --- | --- | --- | --- |
| eval | 863ms | 484ms | 1 | 0 | 5.7KiB | 2.8KiB |

**eval: top hosts**

| host | conns | time | down | up |
| --- | --- | --- | --- | --- |
| github.com:443 | 1 | 484ms | 5.7KiB | 2.8KiB |

```text
github.com:443 +72ms ######################## 484ms [eval]
```

<sub>Client-side connections only (proxy env): eval fetches, gh, git. Daemon-side substitutions and fixed-output builders are not visible here.</sub>
