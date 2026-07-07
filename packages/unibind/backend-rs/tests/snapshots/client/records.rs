//!Plain-data records mirrored from the engine interface, with owned `std` types.
///A row. The `flag`-first field order is deliberate: Rust would pack
///this struct tighter reordered, which exercises the mirror's layout
///opt-out.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    ///Awkwardly-placed bool.
    pub flag: bool,
    ///Identifier.
    pub id: u64,
    ///The `name` field.
    pub name: ::std::string::String,
    ///The `tags` field.
    pub tags: ::std::vec::Vec<::std::string::String>,
    ///The `weights` field.
    pub weights: ::std::collections::HashMap<::std::string::String, f64>,
    ///The `blob` field.
    pub blob: ::std::vec::Vec<u8>,
    ///The `home` field.
    pub home: ::std::option::Option<::std::path::PathBuf>,
    ///The `nested` field.
    pub nested: ::std::option::Option<::std::vec::Vec<Inner>>,
    ///The `inner` field.
    pub inner: Inner,
}
///A nested record.
#[derive(Clone, Debug, PartialEq)]
pub struct Inner {
    ///The `label` field.
    pub label: ::std::string::String,
    ///The `ratio` field.
    pub ratio: f64,
}
