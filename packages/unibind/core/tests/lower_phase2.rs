//! Lowering across the phase 2 surface: async, blocking, streams, and
//! objects, plus the positioned errors that police their edges.

use proc_macro2::TokenStream;
use unibind_core::ir;

fn lower(source: &str) -> Result<ir::Interface, unibind_core::LowerError> {
    let file: syn::File = syn::parse_str(source).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    unibind_core::lower_module(TokenStream::new(), module)
}

fn error_message(source: &str) -> String {
    lower(source).expect_err("lowering should fail").message
}

const OBJECTS: &str = r"
/// A stateful boundary.
mod sample {
    /// Boundary failures.
    #[unibind::error]
    pub enum StoreError {
        Gone { message: String },
    }

    /// A live store handle.
    #[unibind::object(resource)]
    pub struct Store {
        rows: u64,
    }

    impl Store {
        /// Open a store.
        #[unibind(constructor)]
        pub fn open(path: &str) -> Result<Self, StoreError> {
            let _ = path;
            Err(StoreError::Gone { message: String::new() })
        }

        /// Stream rows out.
        pub async fn rows(&self, limit: usize) -> Result<UniStream<String>, StoreError> {
            let _ = limit;
            todo!()
        }

        /// Compact off the GIL.
        #[unibind(blocking)]
        pub fn compact(&self) -> u64 {
            self.rows
        }

        pub async fn close(&self) {}

        fn helper(&self) {}
    }

    /// The default store.
    pub fn default_store() -> Store {
        Store { rows: 0 }
    }

    /// Watch a counter.
    pub async fn watch(from: u64) -> UniStream<u64> {
        let _ = from;
        todo!()
    }
}
";

#[test]
fn objects_lower_with_constructor_methods_and_resource() {
    let interface = lower(OBJECTS).expect("lowering succeeds");
    let [store] = interface.objects.as_slice() else {
        panic!("one object");
    };
    assert_eq!(store.name, "Store");
    assert!(store.resource);
    assert_eq!(store.docs, vec!["A live store handle.".to_owned()]);

    let constructor = store.constructor.as_ref().expect("a constructor");
    assert_eq!(constructor.name, "open");
    assert!(matches!(constructor.asyncness, ir::Asyncness::Sync));
    assert!(constructor.ret.is_none(), "the object itself is implied");
    assert_eq!(constructor.throws.as_deref(), Some("StoreError"));
    assert!(matches!(constructor.args[0].ty, ir::Type::String { owned: false }));

    let [rows, compact, close] = store.methods.as_slice() else {
        panic!("three methods (the private helper is skipped)");
    };
    assert!(matches!(rows.asyncness, ir::Asyncness::Async));
    assert!(matches!(rows.ret, Some(ir::Type::Stream(_))));
    assert_eq!(rows.throws.as_deref(), Some("StoreError"));
    assert!(compact.blocking);
    assert!(matches!(close.asyncness, ir::Asyncness::Async));
    assert!(close.ret.is_none());
}

#[test]
fn object_names_return_as_named_types() {
    let interface = lower(OBJECTS).expect("lowering succeeds");
    let default_store = &interface.functions[0];
    let Some(ir::Type::Named(name)) = &default_store.ret else {
        panic!("object return lowers to Named");
    };
    assert_eq!(name, "Store");
}

#[test]
fn async_functions_lower_with_owned_args() {
    let interface = lower(OBJECTS).expect("lowering succeeds");
    let watch = &interface.functions[1];
    assert!(matches!(watch.asyncness, ir::Asyncness::Async));
    let Some(ir::Type::Stream(item)) = &watch.ret else {
        panic!("bare UniStream return lowers to Stream");
    };
    assert!(matches!(**item, ir::Type::Int(ir::IntKind::U64)));
}

#[test]
fn async_borrowed_args_are_rejected() {
    let message = error_message("mod m { pub async fn go(name: &str) {} }");
    assert!(message.contains("owned arguments"), "{message}");
}

#[test]
fn blocking_sets_the_flag_and_keeps_borrowed_bytes() {
    let interface =
        lower("mod m { #[unibind(blocking)] pub fn go(data: &[u8]) -> u64 { 0 } }")
            .expect("lowering succeeds");
    let go = &interface.functions[0];
    assert!(go.blocking);
    assert!(matches!(go.args[0].ty, ir::Type::Bytes { owned: false }));
}

#[test]
fn blocking_on_async_is_rejected() {
    let message = error_message("mod m { #[unibind(blocking)] pub async fn go() {} }");
    assert!(message.contains("sync exports"), "{message}");
}

#[test]
fn blocking_borrowed_strings_are_rejected() {
    let message = error_message("mod m { #[unibind(blocking)] pub fn go(name: &str) {} }");
    assert!(message.contains("releases the GIL"), "{message}");
}

#[test]
fn result_wrapped_streams_lower() {
    let interface = lower(
        "mod m { #[unibind::error] pub enum E { A { m: String } } \
         pub fn go() -> Result<UniStream<String>, E> { todo!() } }",
    )
    .expect("lowering succeeds");
    let go = &interface.functions[0];
    assert!(matches!(go.ret, Some(ir::Type::Stream(_))));
    assert_eq!(go.throws.as_deref(), Some("E"));
}

#[test]
fn streams_in_argument_position_are_rejected() {
    let message = error_message("mod m { pub fn go(stream: UniStream<u64>) {} }");
    assert!(message.contains("return type"), "{message}");
}

#[test]
fn resources_without_close_are_rejected() {
    let message =
        error_message("mod m { #[unibind::object(resource)] pub struct H { id: u64 } }");
    assert!(message.contains("close method"), "{message}");
}

#[test]
fn mut_receivers_are_rejected() {
    let message = error_message(
        "mod m { #[unibind::object] pub struct H { id: u64 } \
         impl H { pub fn poke(&mut self) {} } }",
    );
    assert!(message.contains("interior mutability"), "{message}");
}

#[test]
fn object_names_in_argument_position_are_rejected() {
    let message = error_message(
        "mod m { #[unibind::object] pub struct H { id: u64 } \
         pub fn take(handle: H) {} }",
    );
    assert!(message.contains("return values only"), "{message}");
}

#[test]
fn associated_fns_need_the_constructor_marker() {
    let message = error_message(
        "mod m { #[unibind::object] pub struct H { id: u64 } \
         impl H { pub fn make() -> Self { H { id: 0 } } } }",
    );
    assert!(message.contains("#[unibind(constructor)]"), "{message}");
}

#[test]
fn async_constructors_are_rejected() {
    let message = error_message(
        "mod m { #[unibind::object] pub struct H { id: u64 } \
         impl H { #[unibind(constructor)] pub async fn open() -> Self { H { id: 0 } } } }",
    );
    assert!(message.contains("synchronous"), "{message}");
}

#[test]
fn strip_removes_impl_block_attributes() {
    let file: syn::File = syn::parse_str(OBJECTS).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    let mut module = module.clone();
    unibind_core::strip_unibind_attrs(&mut module);
    let rendered = quote::quote!(#module).to_string();
    assert!(!rendered.contains("unibind"), "{rendered}");
}
