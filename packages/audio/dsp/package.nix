{
  id = "audio-dsp";
  inRustWorkspace = true;
  # Library crate: pure processing stages consumed by native pipelines here
  # and by the ix repo's wasm-bindgen wrapper (browser voice capture) as a
  # git dependency. No standalone artifact, so no flake/packageSet systems.
  passthruTests = true;
}
