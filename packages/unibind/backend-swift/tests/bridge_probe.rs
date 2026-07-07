//! Prove the rendered bridge module is accepted by swift-bridge itself.
//!
//! The macro path hands these exact tokens to swift-bridge-macro, whose
//! codegen still has `todo!()` holes for unsupported constructs; a panic
//! there would surface as an opaque "custom attribute panicked" in the
//! consuming crate. This probe parses the bridge module with
//! swift-bridge-ir (the macro is a thin wrapper around it) and forces both
//! the Rust tokens and the Swift/C generation, bisecting per declaration on
//! failure so the offending construct is named in the test output.

use std::panic::{catch_unwind, AssertUnwindSafe};

use proc_macro2::TokenStream;
use quote::ToTokens as _;
use unibind_core::ir;

fn interface() -> ir::Interface {
    let file: syn::File =
        syn::parse_str(include_str!("fixtures/sample.rs")).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    unibind_core::lower_module(TokenStream::new(), module).expect("fixture lowers")
}

/// Force everything swift-bridge generates from a bridge module.
fn force_codegen(tokens: TokenStream) -> Result<(), String> {
    let module: swift_bridge_ir::SwiftBridgeModule = syn1::parse2(tokens)
        .map_err(|error| format!("swift-bridge failed to parse the bridge module: {error}"))?;
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let rust = module.to_token_stream();
        assert!(!rust.is_empty(), "no Rust tokens generated");
    }));
    outcome.map_err(|panic| {
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "non-string panic".to_owned());
        format!("swift-bridge codegen panicked: {message}")
    })
}

/// Rebuild a one-declaration bridge module, so a failure names its item.
fn single_decl_module(body: &str) -> TokenStream {
    format!("mod probe {{ {body} }}").parse().expect("probe module tokenizes")
}

#[test]
fn swift_bridge_accepts_the_bridge_module() {
    let rendered = unibind_backend_swift::render(&interface()).expect("renders");
    probe_bridge(rendered);
}

/// The conformance crate's exact surface, so the probe fails here (naming
/// the construct) rather than as an opaque panic in that crate's build.
#[test]
fn swift_bridge_accepts_the_conformance_bridge() {
    let file: syn::File =
        syn::parse_str(include_str!("fixtures/conformance_probe.rs")).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    let interface =
        unibind_core::lower_module(TokenStream::new(), module).expect("fixture lowers");
    let rendered = unibind_backend_swift::render(&interface).expect("renders");
    probe_bridge(rendered);
}

fn probe_bridge(rendered: unibind_backend_swift::RenderedInterface) {
    if force_codegen(rendered.bridge.clone()).is_ok() {
        return;
    }
    // Bisect: run every enum and extern declaration on its own and report
    // the failures, so the assertion below names the unsupported constructs.
    let file: syn::File = syn::parse2(rendered.bridge.clone()).expect("bridge parses as syn2");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("bridge is a module");
    };
    let items = &module.content.as_ref().expect("bridge is inline").1;
    let mut failures = Vec::new();
    for item in items {
        match item {
            syn::Item::Enum(_) => {
                let body = item.to_token_stream().to_string();
                if let Err(error) = force_codegen(single_decl_module(&body)) {
                    failures.push(format!("{body}\n  -> {error}"));
                }
            }
            syn::Item::ForeignMod(foreign) => {
                // Type declarations must stay in scope for the fns that use
                // them, so each fn probe carries every `type` declaration.
                let types: String = foreign
                    .items
                    .iter()
                    .filter(|foreign_item| {
                        matches!(foreign_item, syn::ForeignItem::Type(_) | syn::ForeignItem::Verbatim(_))
                    })
                    .map(|foreign_item| foreign_item.to_token_stream().to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                let enums: String = items
                    .iter()
                    .filter(|sibling| matches!(sibling, syn::Item::Enum(_)))
                    .map(|sibling| sibling.to_token_stream().to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                for foreign_item in &foreign.items {
                    if matches!(foreign_item, syn::ForeignItem::Type(_)) {
                        continue;
                    }
                    let decl = foreign_item.to_token_stream().to_string();
                    let body = format!("{enums} extern \"Rust\" {{ {types} {decl} }}");
                    if let Err(error) = force_codegen(single_decl_module(&body)) {
                        failures.push(format!("{decl}\n  -> {error}"));
                    }
                }
            }
            _ => {}
        }
    }
    panic!(
        "swift-bridge rejected the bridge module; failing declarations:\n{}",
        failures.join("\n")
    );
}
