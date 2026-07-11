//! Record codecs: one decode and one encode helper per record, shared by
//! every argument and return position the record appears in.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::{names, ty, RenderError, RenderedRecord};

/// Render `__read_<record>` and `__write_<record>` for one record. Fields
/// travel in declaration order with no framing, mirroring the generated
/// Java record's canonical constructor.
pub fn render_codecs(
    record: &ir::Record,
    interface: &ir::Interface,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    let rust_name = names::name_ident(&record.name)?;
    // Validate the Java side alongside the glue: one validator for both.
    names::checked(
        names::record_name(record).to_owned(),
        &format!("record `{}`", record.name),
    )?;
    let read = ty::read_record_ident(&record.name);
    let write = ty::write_record_ident(&record.name);

    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for field in &record.fields {
        ty::check_boundary(&field.ty, interface).map_err(|error| at_field(record, field, &error))?;
        // Validate the Java side too: one validator for both halves.
        names::component_name(record, field)?;
        let field_ident = names::name_ident(&field.name)?;
        let decode = ty::decode_expr(&field.ty, 0);
        reads.push(quote!(#field_ident: #decode,));
        writes.push(ty::encode_stmts(&field.ty, &quote!(value.#field_ident), 0));
    }

    let read_docs = format!("Decode a `{}` from the wire.", record.name);
    let write_docs = format!("Encode a `{}` onto the wire.", record.name);
    Ok(quote! {
        #[doc = #read_docs]
        fn #read(reader: &mut ::unibind_jvm_runtime::Reader<'_>) -> super::#user::#rust_name {
            super::#user::#rust_name {
                #(#reads)*
            }
        }
        #[doc = #write_docs]
        fn #write(writer: &mut ::unibind_jvm_runtime::Writer, value: &super::#user::#rust_name) {
            #(#writes)*
        }
    })
}

/// The attributes the exported struct gains: none. The codecs read and
/// build records with plain field access, so the struct crosses untouched.
pub fn record_attrs(record: &ir::Record) -> RenderedRecord {
    RenderedRecord {
        outer: Vec::new(),
        fields: record.fields.iter().map(|_| Vec::new()).collect(),
    }
}

fn at_field(record: &ir::Record, field: &ir::Field, error: &RenderError) -> RenderError {
    RenderError::new(format!(
        "field `{}` of record `{}`: {}",
        field.name, record.name, error.message
    ))
}
