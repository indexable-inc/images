//!Idiomatic error types: one enum per engine error enum (variants carry the engine-side `Display` text) plus the load-time `LoadError`.
/// Everything [`crate::Engine::load`] can fail with. Loading never
/// falls back: a mismatch is a hard error naming both sides.
#[derive(Debug)]
pub enum LoadError {
    /// The engine library could not be opened.
    Dlopen {
        /// The loader's error text.
        message: ::std::string::String,
    },
    /// An expected `#[stabby::export]` symbol is missing; the engine
    /// was probably built without the `rs` unibind feature or with a
    /// different stabby major version.
    MissingSymbol {
        /// The symbol that failed to resolve.
        symbol: ::std::string::String,
        /// The loader's error text.
        message: ::std::string::String,
    },
    /// A symbol resolved, but stabby's structural type report does
    /// not match this client's expected signature.
    SignatureMismatch {
        /// The symbol whose report mismatched.
        symbol: ::std::string::String,
        /// Both type reports, as rendered by stabby.
        message: ::std::string::String,
    },
    /// The engine was generated from a different interface than this
    /// client: the IR hashes disagree.
    IrHashMismatch {
        /// The hex SHA-256 this client was generated from.
        expected: ::std::string::String,
        /// The hex SHA-256 the engine reported.
        actual: ::std::string::String,
    },
}
impl ::core::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        match self {
            Self::Dlopen { message } => {
                write!(formatter, "opening the engine library failed: {message}")
            }
            Self::MissingSymbol { symbol, message } => {
                write!(formatter, "symbol `{symbol}` did not resolve: {message}")
            }
            Self::SignatureMismatch { symbol, message } => {
                write!(formatter, "symbol `{symbol}` has a mismatching ABI: {message}")
            }
            Self::IrHashMismatch { expected, actual } => {
                write!(
                    formatter,
                    "engine/client interface mismatch: client was generated from IR \
                             {expected}, engine reports {actual}; regenerate the client"
                )
            }
        }
    }
}
impl ::std::error::Error for LoadError {}
///Boundary failures.
#[derive(Clone, Debug)]
pub enum SampleError {
    ///The store is gone.
    StoreGone {
        /// The engine-side variant's `Display` text.
        message: ::std::string::String,
    },
    ///Bad input.
    Invalid {
        /// The engine-side variant's `Display` text.
        message: ::std::string::String,
    },
}
impl ::core::fmt::Display for SampleError {
    fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        match self {
            Self::StoreGone { message } | Self::Invalid { message } => {
                formatter.write_str(message)
            }
        }
    }
}
impl ::std::error::Error for SampleError {}
impl ::core::convert::From<crate::abi::SampleErrorStable> for SampleError {
    fn from(raw: crate::abi::SampleErrorStable) -> Self {
        let message = ::std::string::String::from(raw.message);
        match raw.variant {
            0u32 => Self::StoreGone { message },
            1u32 => Self::Invalid { message },
            other => {
                unreachable!("the IR-hash handshake pins variant indices; got {other}")
            }
        }
    }
}
