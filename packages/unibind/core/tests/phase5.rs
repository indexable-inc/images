//! Lowering across the phase 5 additions: per-language `ex` renames,
//! `blocking`, async functions, `Stream<T>` returns, and objects with
//! constructors and methods, plus the positioned errors for the shapes that
//! stay out of scope.

use proc_macro2::TokenStream;
use unibind_core::ir;

fn lower(source: &str) -> Result<ir::Interface, unibind_core::LowerError> {
    lower_with(TokenStream::new(), source)
}

fn lower_with(args: TokenStream, source: &str) -> Result<ir::Interface, unibind_core::LowerError> {
    let file: syn::File = syn::parse_str(source).expect("fixture parses");
    let Some(syn::Item::Mod(module)) = file.items.first() else {
        panic!("fixture starts with a module");
    };
    unibind_core::lower_module(args, module)
}

fn error_message(source: &str) -> String {
    lower(source).expect_err("lowering should fail").message
}

#[test]
fn ex_renames_flow_into_names() {
    let args: TokenStream = r#"ex(name = "Other")"#.parse().expect("args parse");
    let interface = lower_with(
        args,
        r#"
        mod sample {
            #[unibind::record]
            #[derive(Clone)]
            pub struct Row {
                #[unibind(ex(name = "tag"))]
                pub name: String,
            }

            #[unibind(ex(name = "fetch_rows"))]
            pub fn rows() -> Vec<Row> {
                Vec::new()
            }
        }
        "#,
    )
    .expect("lowering succeeds");
    assert_eq!(interface.names.ex.as_deref(), Some("Other"));
    assert_eq!(
        interface.functions[0].names.ex.as_deref(),
        Some("fetch_rows")
    );
    assert_eq!(
        interface.records[0].fields[0].names.ex.as_deref(),
        Some("tag")
    );
}

#[test]
fn blocking_functions_carry_the_flag() {
    let interface = lower("mod m { #[unibind(blocking)] pub fn crunch() {} pub fn quick() {} }")
        .expect("lowering succeeds");
    assert!(interface.functions[0].blocking);
    assert!(!interface.functions[1].blocking);
}

#[test]
fn async_functions_lower_to_async() {
    let interface = lower("mod m { pub async fn go() {} }").expect("lowering succeeds");
    assert!(matches!(
        interface.functions[0].asyncness,
        ir::Asyncness::Async
    ));
}

#[test]
fn stream_returns_lower_to_stream() {
    let interface = lower("mod m { pub fn scan() -> Stream<u64> { unimplemented!() } }")
        .expect("lowering succeeds");
    let scan = &interface.functions[0];
    assert!(matches!(scan.asyncness, ir::Asyncness::Sync));
    let Some(ir::Type::Stream(item)) = &scan.ret else {
        panic!("stream return");
    };
    assert!(matches!(**item, ir::Type::Int(ir::IntKind::U64)));
}

#[test]
fn stream_results_keep_the_throws() {
    let interface = lower(
        r"
        mod m {
            #[unibind::error]
            pub enum ScanError {
                Gone { message: String },
            }

            pub fn scan() -> Result<Stream<String>, ScanError> {
                unimplemented!()
            }
        }
        ",
    )
    .expect("lowering succeeds");
    let scan = &interface.functions[0];
    assert_eq!(scan.throws.as_deref(), Some("ScanError"));
    assert!(matches!(scan.ret, Some(ir::Type::Stream(_))));
}

#[test]
fn objects_lower_constructors_and_methods() {
    let interface = lower(
        r#"
        mod m {
            #[unibind::error]
            pub enum CounterError {
                Overflow { message: String },
            }

            /// A stateful counter.
            #[unibind::object]
            pub struct Counter {
                value: u64,
            }

            impl Counter {
                /// Start from a value.
                pub fn new(start: u64) -> Self {
                    Self { value: start }
                }

                pub fn parse(text: &str) -> Result<Counter, CounterError> {
                    let _ = text;
                    unimplemented!()
                }

                /// Read the value.
                pub fn value(&self) -> u64 {
                    self.value
                }

                pub fn bump(&mut self, by: u64) {
                    self.value += by;
                }

                fn private_helper(&self) {}
            }

            impl std::fmt::Display for Counter {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "{}", self.value)
                }
            }
        }
        "#,
    )
    .expect("lowering succeeds");
    let [counter] = interface.objects.as_slice() else {
        panic!("one object");
    };
    assert_eq!(counter.name, "Counter");
    assert_eq!(counter.docs, vec!["A stateful counter.".to_owned()]);

    let [new, parse] = counter.constructors.as_slice() else {
        panic!("two constructors");
    };
    assert!(new.receiver.is_none());
    assert!(matches!(new.ret, Some(ir::Type::Named(ref name)) if name == "Counter"));
    assert_eq!(parse.throws.as_deref(), Some("CounterError"));

    let [value, bump] = counter.methods.as_slice() else {
        panic!("two methods (the private helper is skipped)");
    };
    assert!(matches!(value.receiver, Some(ir::Receiver::Ref)));
    assert!(matches!(bump.receiver, Some(ir::Receiver::Mut)));
    assert!(matches!(value.ret, Some(ir::Type::Int(ir::IntKind::U64))));
}

#[test]
fn streams_are_return_position_only() {
    let message = error_message("mod m { pub fn go(items: Stream<u64>) {} }");
    assert!(message.contains("return-position only"), "{message}");
}

#[test]
fn blocking_async_functions_are_rejected() {
    let message = error_message("mod m { #[unibind(blocking)] pub async fn go() {} }");
    assert!(message.contains("cannot be `blocking`"), "{message}");
}

#[test]
fn async_stream_returns_are_rejected() {
    let message = error_message("mod m { pub async fn go() -> Stream<u64> { unimplemented!() } }");
    assert!(message.contains("cannot return `Stream<T>`"), "{message}");
}

#[test]
fn object_impls_reject_plain_associated_functions() {
    let message = error_message(
        "mod m { #[unibind::object] pub struct H {} impl H { pub fn helper() -> u64 { 0 } } }",
    );
    assert!(
        message.contains("methods (&self) and constructors"),
        "{message}"
    );
}

#[test]
fn objects_cannot_be_record_fields() {
    let message = error_message(
        "mod m { #[unibind::object] pub struct H {} \
         #[unibind::record] pub struct R { pub handle: H } }",
    );
    assert!(message.contains("opaque handle"), "{message}");
}

#[test]
fn self_receivers_by_value_are_rejected() {
    let message =
        error_message("mod m { #[unibind::object] pub struct H {} impl H { pub fn go(self) {} } }");
    assert!(message.contains("`&self` or `&mut self`"), "{message}");
}
