_: {
  /**
    Return the Futhark compiler.

    Futhark is a purely functional, data-parallel array language whose
    compiler is written in Haskell and which emits code for one backend
    chosen per `futhark <backend>` invocation: `c` (single-threaded
    CPU), `multicore` (threaded CPU), `cuda`, `opencl`, `hip`, `ispc`,
    and `python` (CPU NumPy fallback) plus `pyopencl` (Python bindings
    around the OpenCL backend). nixpkgs ships one `pkgs.futhark`
    derivation that contains every backend; there is no per-backend
    attribute to pick between.

    Selection happens at the call site, not the closure: the helper
    only owns the compiler. The runtime story is the load-bearing part,
    so the image that consumes the generated code has to bring the
    matching driver:

    - `c` and `multicore` need no GPU runtime; the emitted C is plain
      libc.
    - `cuda` needs the CUDA runtime libraries and a CUDA-capable driver
      in the VM (`pkgs.cudatoolkit` or the split `pkgs.cudaPackages`
      derivations).
    - `opencl` needs an ICD loader (`pkgs.ocl-icd`) and a vendor ICD
      that exposes a runnable device; the host driver is what actually
      runs the kernel.
    - `hip` needs the ROCm runtime; pull in via the `pkgs.rocmPackages`
      set.
    - `ispc` needs `pkgs.ispc` available for the host build step that
      lowers to vector intrinsics.
    - `python` / `pyopencl` need a Python with `numpy` and, for the
      OpenCL flavor, `pyopencl` plus the same ICD setup as the native
      `opencl` backend.

    A follow-up `runtime` helper that bundles the matching ICD or CUDA
    closure per backend would be a reasonable extension; until that
    exists, the caller wires the GPU runtime through the image's own
    modules.

    Arguments:
    - `pkgs`: nixpkgs instance the compiler comes from.

    Example:
    ```nix
    { pkgs, ix, ... }:
    let futhark = ix.languages.futhark.compiler pkgs { };
    in { environment.systemPackages = [ futhark ]; }
    ```
  */
  compiler = pkgs: _: pkgs.futhark;
}
