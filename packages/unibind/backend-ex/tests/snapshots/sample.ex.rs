// struct Row: # [derive (:: rustler :: NifStruct)] # [module = "Sample.Line"]
//   field id: 
//   field name: 
//   field weights: 
//   field home: 

#[doc(hidden)]
#[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
mod __unibind_ex_sample {
    mod __unibind_atoms_sample_error {
        ::rustler::atoms! {
            missing_store, invalid
        }
    }
    ///Carry `SampleError` across the boundary: variant atom plus `Display` text.
    #[derive(::rustler::NifStruct)]
    #[module = "Sample.SampleFault"]
    pub struct SampleErrorTerm {
        variant: ::rustler::types::atom::Atom,
        message: ::std::string::String,
    }
    impl ::std::convert::From<super::sample::SampleError> for SampleErrorTerm {
        fn from(error: super::sample::SampleError) -> Self {
            let message = ::std::string::ToString::to_string(&error);
            let variant = match &error {
                super::sample::SampleError::StoreGone { .. } => {
                    __unibind_atoms_sample_error::missing_store()
                }
                super::sample::SampleError::Invalid { .. } => {
                    __unibind_atoms_sample_error::invalid()
                }
            };
            Self { variant, message }
        }
    }
    #[::rustler::resource_impl]
    impl ::rustler::Resource for super::sample::Cursor {}
    #[::rustler::nif]
    fn cursor_open(
        store: &str,
    ) -> ::std::result::Result<
        ::rustler::ResourceArc<super::sample::Cursor>,
        SampleErrorTerm,
    > {
        super::sample::Cursor::open(store)
            .map(::rustler::ResourceArc::new)
            .map_err(SampleErrorTerm::from)
    }
    #[::rustler::nif]
    fn cursor_position(handle: ::rustler::ResourceArc<super::sample::Cursor>) -> u64 {
        handle.position()
    }
    #[::rustler::nif(schedule = "DirtyIo")]
    fn cursor_skip(
        handle: ::rustler::ResourceArc<super::sample::Cursor>,
        n: u64,
    ) -> u64 {
        handle.skip(n)
    }
    #[::rustler::nif]
    fn rows(
        store: &str,
        limit: usize,
        root: ::std::option::Option<&str>,
    ) -> ::std::result::Result<::std::vec::Vec<super::sample::Row>, SampleErrorTerm> {
        super::sample::rows(store, limit, root).map_err(SampleErrorTerm::from)
    }
    #[::rustler::nif(schedule = "DirtyIo")]
    fn recount(home: ::std::path::PathBuf) -> u64 {
        super::sample::recount(home)
    }
    #[::rustler::nif(name = "label_of")]
    fn label(
        env: ::rustler::Env,
        reference: ::rustler::Term,
        key: u64,
        prefix: ::std::string::String,
    ) -> ::rustler::NifResult<::rustler::ResourceArc<::unibind_ex_runtime::InFlight>> {
        let fut = async move {
            ::std::result::Result::<
                _,
                ::unibind_ex_runtime::Never,
            >::Ok(super::sample::label(key, prefix).await)
        };
        ::unibind_ex_runtime::spawn_reply(env, reference, fut)
    }
    #[::rustler::nif]
    fn store(
        env: ::rustler::Env,
        reference: ::rustler::Term,
        row: super::sample::Row,
    ) -> ::rustler::NifResult<::rustler::ResourceArc<::unibind_ex_runtime::InFlight>> {
        let fut = async move {
            super::sample::store(row).await.map_err(SampleErrorTerm::from)
        };
        ::unibind_ex_runtime::spawn_reply(env, reference, fut)
    }
    #[::rustler::nif]
    fn tags(
        env: ::rustler::Env,
        reference: ::rustler::Term,
        prefix: &str,
    ) -> ::rustler::NifResult<
        ::rustler::ResourceArc<::unibind_ex_runtime::StreamHandle>,
    > {
        ::unibind_ex_runtime::spawn_stream(env, reference, super::sample::tags(prefix))
    }
    #[::rustler::nif]
    fn scan(
        env: ::rustler::Env,
        reference: ::rustler::Term,
        store: &str,
    ) -> ::rustler::NifResult<
        ::std::result::Result<
            ::rustler::ResourceArc<::unibind_ex_runtime::StreamHandle>,
            SampleErrorTerm,
        >,
    > {
        match super::sample::scan(store) {
            ::std::result::Result::Ok(stream) => {
                ::unibind_ex_runtime::spawn_stream(env, reference, stream).map(Ok)
            }
            ::std::result::Result::Err(error) => Ok(Err(SampleErrorTerm::from(error))),
        }
    }
    #[::rustler::nif]
    fn unibind_demand(
        handle: ::rustler::ResourceArc<::unibind_ex_runtime::StreamHandle>,
        n: u64,
    ) {
        ::unibind_ex_runtime::grant(&handle, n);
    }
    ::rustler::init!("Elixir.Sample.Native");
    #[unsafe(no_mangle)]
    extern "C" fn nif_init() -> *const ::rustler::codegen_runtime::DEF_NIF_ENTRY {
        sample_nif_init()
    }
}
