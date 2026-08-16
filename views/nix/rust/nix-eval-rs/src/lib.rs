//! Rust evaluator for the Nix expression language, reached from cppnix
//! through the C ABI in `capi`. Pipeline: rnix CST -> `compile` -> `ir`
//! module -> `vm` -> `print`. See ENG-12068.

/// Every Rust allocation in this crate goes through mimalloc, in whichever
/// process links us: the profile on ENG-13148 spends ~17% of a NixOS
/// toplevel eval in glibc malloc/free, and swapping the allocator alone
/// was measured at ~13% cpu (A/B/A/B, identical output; numbers beside the
/// dependency in Cargo.toml). What this does not cover is the C++ half of
/// the `nix` binary, which keeps its own allocator; the LD_PRELOAD
/// experiment covered both and scored the same 13%, so the Rust side is
/// where the traffic is.
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod abi_check;
pub mod builtins;
pub mod builtins_gen;
pub mod capi;
pub mod compile;
pub mod deepwalk;
pub mod drv;
pub mod drvpath;
pub mod drvstrict;
pub mod eval;
pub mod host;
pub mod ir;
pub mod modcache;
pub mod nixhash;
pub mod perf;
pub mod primops_host;
pub mod primops_pure;
pub mod print;
pub mod purity;
pub mod readset;
pub mod refusal;
pub mod session;
pub mod store;
pub mod storepath;
pub mod task;
pub mod value2;
pub mod vm;
