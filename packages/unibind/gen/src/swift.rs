//! Emit the Swift host files for one interface.
//!
//! Six files land under `<package>/`: the low-level `<package>.swift` that
//! swift-bridge-build derives from the same bridge module the macro
//! expanded, the `SwiftBridgeCore.swift` support file, the ergonomic
//! `Bindings.swift` overlay, and the C headers (plus a `bridging-header.h`
//! aggregating them for `swiftc -import-objc-header`).
//!
//! The bridge module is re-rendered from the embedded IR by the same
//! `unibind-backend-swift` code the macro runs, so the Swift output matches
//! the compiled artifact's FFI symbols by construction. swift-bridge-build
//! only reads top-level items of a source file, which is why the bridge
//! module is written to a scratch file on its own rather than parsed out of
//! the expanded crate.

use anyhow::Context as _;
use unibind_core::ir::Interface;

use crate::host::{EmitError, HostEmitter, HostFile};

/// The Swift emitter; writes into `<package>/` under the output root.
pub struct SwiftEmitter {
    /// Directory (and Swift module name) the files land under.
    pub package: String,
}

impl HostEmitter for SwiftEmitter {
    fn target(&self) -> &'static str {
        "swift"
    }

    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError> {
        let rendered = unibind_backend_swift::render(interface).map_err(|error| EmitError {
            message: error.message,
        })?;
        let package = &self.package;
        let generated = generate_low_level(&rendered.bridge, package).map_err(|error| {
            EmitError {
                message: format!("{error:#}"),
            }
        })?;

        let bridging_header = format!(
            "#include \"SwiftBridgeCore.h\"\n#include \"{package}.h\"\n"
        );
        Ok(vec![
            HostFile {
                path: format!("{package}/{package}.swift"),
                contents: generated.swift,
            },
            HostFile {
                path: format!("{package}/Bindings.swift"),
                contents: rendered.overlay,
            },
            HostFile {
                path: format!("{package}/SwiftBridgeCore.swift"),
                contents: generated.core_swift,
            },
            HostFile {
                path: format!("{package}/include/{package}.h"),
                contents: generated.header,
            },
            HostFile {
                path: format!("{package}/include/SwiftBridgeCore.h"),
                contents: generated.core_header,
            },
            HostFile {
                path: format!("{package}/include/bridging-header.h"),
                contents: bridging_header,
            },
        ])
    }
}

/// swift-bridge-build's output for one bridge module.
struct LowLevel {
    /// The generated `<package>.swift`.
    swift: String,
    /// The generated `<package>.h`.
    header: String,
    /// The `SwiftBridgeCore.swift` support file.
    core_swift: String,
    /// The `SwiftBridgeCore.h` support header.
    core_header: String,
}

/// Run swift-bridge-build over the rendered bridge module in a scratch
/// directory and collect the generated sources.
fn generate_low_level(
    bridge: &proc_macro2::TokenStream,
    package: &str,
) -> anyhow::Result<LowLevel> {
    let scratch = tempfile::tempdir().context("creating a scratch directory")?;
    let bridge_path = scratch.path().join("bridge.rs");
    std::fs::write(&bridge_path, bridge.to_string()).context("writing the bridge module")?;

    let out_dir = scratch.path().join("generated");
    swift_bridge_build::parse_bridges([&bridge_path]).write_all_concatenated(&out_dir, package);

    let read = |path: std::path::PathBuf| {
        std::fs::read_to_string(&path)
            .with_context(|| format!("reading swift-bridge output {}", path.display()))
    };
    Ok(LowLevel {
        swift: read(out_dir.join(package).join(format!("{package}.swift")))?,
        header: read(out_dir.join(package).join(format!("{package}.h")))?,
        core_swift: read(out_dir.join("SwiftBridgeCore.swift"))?,
        core_header: read(out_dir.join("SwiftBridgeCore.h"))?,
    })
}
