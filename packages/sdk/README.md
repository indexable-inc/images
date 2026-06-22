# ix SDK

Public source for the ix SDKs (Rust, Python, TypeScript). Each links or bundles
the precompiled, proprietary `ix-sdk` libraries distributed by Indexable.

## License

Everything under `sdk/` is proprietary and source-available, governed by
[`sdk/LICENSE`](./LICENSE) (the Indexable SDK License), NOT the repository-root
MIT license. The SDK license supersedes the root MIT for this directory and its
subdirectories, including the compiled components the SDK fetches or bundles. In
short: you may use the SDK to build applications that access the hosted ix
service, but you may not reverse-engineer, modify, redistribute, or use it to
build a competing service. See `sdk/LICENSE` for the full terms.
