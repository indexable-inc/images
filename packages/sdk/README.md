<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
    <img src="assets/hero.svg" width="720" alt="Rust, Python, and TypeScript SDKs link one precompiled ix-sdk core that talks to the hosted ix service">
  </picture>
</p>

# ix SDK

Building on ix and wondering which SDK to grab? The SDKs ship as compiled
artifacts only; there is no SDK source in this repository. Every artifact
bundles the same precompiled, proprietary `ix-sdk` core built in the ix
monorepo, so behavior is identical across languages.

| SDK | Get it |
| --- | --- |
| TypeScript | `npm install @indexable/sdk` |
| Python | `pip install ix-sdk` |
| Rust | prebuilt rlib boundary in [`rust/`](./rust), consumed in-repo via Nix |

This directory holds only the artifact-consumption boundary: [`rust/`](./rust)
pins and wraps the prebuilt `ix-sdk-wire` rlib published to R2 so in-repo
consumers can link it. The Python wheel consumer lives at
`packages/ix-sdk-python`.

## License

The npm and PyPI artifacts carry their license inside the package. Everything
under `packages/sdk/`, including the prebuilt components `rust/` fetches, is
proprietary and governed by [`LICENSE`](./LICENSE) (the Indexable SDK License),
NOT the repository-root MIT license. In short: you may use the SDK to build
applications that access the hosted ix service, but you may not
reverse-engineer, modify, redistribute, or use it to build a competing service.
