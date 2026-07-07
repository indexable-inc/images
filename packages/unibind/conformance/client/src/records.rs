//!Plain-data records mirrored from the engine interface, with owned `std` types.
///A sample record. `flag` sits first on purpose: Rust would pack the
///struct tighter reordered, which exercises the generated mirror's
///layout-assert opt-out.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    ///Deliberately awkward leading bool.
    pub flag: bool,
    ///Identifier.
    pub id: u64,
    ///Display name.
    pub name: ::std::string::String,
    ///Optional note.
    pub note: ::std::option::Option<::std::string::String>,
    ///Plain values.
    pub values: ::std::vec::Vec<i64>,
    ///Keyed weights.
    pub weights: ::std::collections::HashMap<::std::string::String, i64>,
    ///A nested record.
    pub inner: Inner,
}
///The nested half of [`Sample`].
#[derive(Clone, Debug, PartialEq)]
pub struct Inner {
    ///A label.
    pub label: ::std::string::String,
    ///A ratio.
    pub ratio: f64,
}
