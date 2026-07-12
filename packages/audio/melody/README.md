# audio-melody

An example shared-audio instrument in plain Rust: a deterministic
pentatonic melody. Copy this crate to author your own.

## The one rule

An instrument is a **pure function of the absolute shared frame**:
`(controls, frame) -> sample`, no state between calls. That is what makes
every peer render bit-identical audio no matter when they joined or how
the host splits blocks. Keep your `render` free of statics, RNGs seeded at
load, and accumulating phase; derive everything from `start_frame`.

The `sa_*` exports at the bottom of `src/lib.rs` are the whole ABI (v1):

| export             | meaning                                            |
| ------------------ | -------------------------------------------------- |
| `memory`           | linear memory the host reads/writes                |
| `sa_abi_version`   | must return `1`                                    |
| `sa_channels`      | `1` (mono) or `2` (interleaved stereo)             |
| `sa_controls_ptr`  | where the host writes 64 `f32` controls            |
| `sa_out_ptr`       | where the host reads rendered `f32` samples        |
| `sa_render(start_frame: i64, frames: i32, sample_rate: i32)` | fill the out buffer |

## Build and publish

```sh
rustup target add wasm32-unknown-unknown   # once
cargo build --package audio-melody --release --target wasm32-unknown-unknown
shared-audio publish target/wasm32-unknown-unknown/release/audio_melody.wasm
```

Everyone in the session switches to your instrument at the same shared
frame, one second out by default (`--at <frame>` to pick the moment).

## Test on the host first

`render` is an ordinary Rust function, so determinism is unit-tested
natively (`cargo test --package audio-melody`): render a range whole, render
it again in odd-sized pieces, and compare `f32::to_bits`. Note the compiled
wasm and a native run may differ in the last bit (fused multiply-add
rounding); that is fine because peers share the *wasm bytes*, never native
builds. The block-split invariant must hold in both worlds.
