// struct Row: 
//   field id: 
//   field name: 
//   field weights: 
//   field home: 

#[doc(hidden)]
#[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
mod __unibind_jvm_sample {
    ///Decode a `Row` from the wire.
    fn __read_row(
        reader: &mut ::unibind_jvm_runtime::Reader<'_>,
    ) -> super::_sample::Row {
        super::_sample::Row {
            id: reader.read_u64(),
            name: reader.read_string(),
            weights: {
                let __count0 = reader.read_count();
                let mut __entries0 = ::std::collections::HashMap::with_capacity(
                    __count0,
                );
                for _ in 0..__count0 {
                    let __key0 = reader.read_string();
                    __entries0.insert(__key0, reader.read_f64());
                }
                __entries0
            },
            home: if reader.read_bool() {
                ::std::option::Option::Some(
                    ::std::path::PathBuf::from(reader.read_string()),
                )
            } else {
                ::std::option::Option::None
            },
        }
    }
    ///Encode a `Row` onto the wire.
    fn __write_row(
        writer: &mut ::unibind_jvm_runtime::Writer,
        value: &super::_sample::Row,
    ) {
        writer.write_u64(value.id);
        writer.write_str(&value.name);
        writer.write_count(value.weights.len());
        for (__key0, __value0) in &value.weights {
            writer.write_str(&(*__key0));
            writer.write_f64((*__value0));
        }
        match &value.home {
            ::std::option::Option::Some(__some0) => {
                writer.write_bool(true);
                writer
                    .write_str(
                        ::std::path::Path::to_str((*__some0).as_ref())
                            .expect("non-UTF-8 path crossing the jvm boundary"),
                    );
            }
            ::std::option::Option::None => {
                writer.write_bool(false);
            }
        }
    }
    ///Carry `SampleError` across the boundary: variant index plus `Display` text.
    fn __fail_sample_error(
        error: super::_sample::SampleError,
    ) -> ::unibind_jvm_runtime::Failure {
        let message = ::std::string::ToString::to_string(&error);
        let variant = match &error {
            super::_sample::SampleError::StoreGone { .. } => 0u32,
            super::_sample::SampleError::Invalid { .. } => 1u32,
        };
        ::unibind_jvm_runtime::Failure {
            variant,
            message,
        }
    }
    ///C-ABI shim for `rows`; called only by the generated Java.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn unibind_jvm__sample_rows(
        args: *const u8,
        len: usize,
        out: *mut ::unibind_jvm_runtime::RawBuf,
    ) {
        unsafe {
            ::unibind_jvm_runtime::invoke(
                args,
                len,
                out,
                |reader, writer| {
                    let store = reader.read_str();
                    let limit = reader.read_usize();
                    let root = if reader.read_bool() {
                        ::std::option::Option::Some(reader.read_str())
                    } else {
                        ::std::option::Option::None
                    };
                    reader.finish();
                    let __ret = super::_sample::rows(store, limit, root)
                        .map_err(__fail_sample_error)?;
                    writer.write_count(__ret.len());
                    for __item0 in &__ret {
                        __write_row(writer, &(*__item0));
                    }
                    ::std::result::Result::Ok(())
                },
            );
        }
    }
    ///C-ABI shim for `store`; called only by the generated Java.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn unibind_jvm__sample_store(
        args: *const u8,
        len: usize,
        out: *mut ::unibind_jvm_runtime::RawBuf,
    ) {
        unsafe {
            ::unibind_jvm_runtime::invoke(
                args,
                len,
                out,
                |reader, writer| {
                    let home = ::std::path::PathBuf::from(reader.read_string());
                    let row = __read_row(reader);
                    let payload = reader.read_byte_buf();
                    reader.finish();
                    let __ret = super::_sample::store(home, row, payload);
                    writer.write_u64(__ret);
                    ::std::result::Result::Ok(())
                },
            );
        }
    }
    ///C-ABI shim for `label`; called only by the generated Java.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn unibind_jvm__sample_label(
        args: *const u8,
        len: usize,
        out: *mut ::unibind_jvm_runtime::RawBuf,
    ) {
        unsafe {
            ::unibind_jvm_runtime::invoke(
                args,
                len,
                out,
                |reader, writer| {
                    let id = reader.read_u64();
                    let prefix = reader.read_string();
                    let trim = reader.read_bool();
                    reader.finish();
                    let __ret = super::_sample::label(id, prefix, trim);
                    writer.write_str(&__ret);
                    ::std::result::Result::Ok(())
                },
            );
        }
    }
    ///C-ABI shim for `clear`; called only by the generated Java.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn unibind_jvm__sample_clear(
        args: *const u8,
        len: usize,
        out: *mut ::unibind_jvm_runtime::RawBuf,
    ) {
        unsafe {
            ::unibind_jvm_runtime::invoke(
                args,
                len,
                out,
                |reader, writer| {
                    reader.finish();
                    let __ret = super::_sample::clear().map_err(__fail_sample_error)?;
                    let () = __ret;
                    ::std::result::Result::Ok(())
                },
            );
        }
    }
    /// Reclaim a reply buffer previously handed to Java.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn unibind_jvm__sample_free(ptr: *mut u8, len: usize, cap: usize) {
        unsafe { ::unibind_jvm_runtime::free(ptr, len, cap) }
    }
}

