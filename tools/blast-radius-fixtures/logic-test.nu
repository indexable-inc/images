# Unit test for the report-building logic in tools/blast-radius.nu.
# `nix-store` is stubbed by the caller (blast-radius-test.sh) so `causes-for`
# runs its real reference-diff code path without a live store.
use std assert
source ../blast-radius.nu

assert equal (category "image-foo") "image"
assert equal (category "rust-test-bar") "rust"
assert equal (category "lint") "lint"
assert equal (drv-name "/nix/store/abcdefghijklmnopqrstuvwxyz012345-ix-rust-workspace.drv") "ix-rust-workspace"

let b = [{attr: "rust-a", drvPath: "/nix/store/base-rust-a.drv"} {attr: "rust-b", drvPath: "/nix/store/base-rust-b.drv"}]
let h = [{attr: "rust-a", drvPath: "/nix/store/head-rust-a.drv"} {attr: "rust-b", drvPath: "/nix/store/head-rust-b.drv"}]
# Both checks' direct refs gain a fresh ix-rust-workspace hash (glibc is
# unchanged), so it is the single root cause fanning out to both.
assert equal (causes-for $b $h ["rust-a" "rust-b"]) [{name: "ix-rust-workspace", checks: ["rust-a" "rust-b"]}]

print "logic-test: ok"
