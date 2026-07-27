//! Binary payloads across the BEAM boundary.

use rustler::{Binary, Decoder, Encoder, Env, NewBinary, NifResult, Term};

/// A byte buffer that crosses as an Elixir binary rather than a list of
/// integers.
///
/// rustler ships `Encoder`/`Decoder` for `Vec<T>` and applies them
/// element-wise, so a plain `Vec<u8>` reaches the BEAM as `[104, 105]`
/// instead of `"hi"`. Both the traits and `Vec` are foreign to this crate,
/// so the orphan rule forbids a competing `impl Encoder for Vec<u8>`; this
/// newtype is the one spelling a generated NIF signature can name instead.
///
/// User code never sees it. `unibind-backend-ex` spells it in the wrapper
/// signature and converts at the call site (`ty::forward` on the way in,
/// `ty::to_wire` on the way out), so an exported `fn(&[u8]) -> Vec<u8>`
/// stays exactly that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl Encoder for Bytes {
    fn encode<'a>(&self, env: Env<'a>) -> Term<'a> {
        // `NewBinary` allocates inside the caller's env, so the term is
        // BEAM-owned the moment it exists and needs no release step.
        let mut binary = NewBinary::new(env, self.0.len());
        binary.as_mut_slice().copy_from_slice(&self.0);
        binary.into()
    }
}

impl<'a> Decoder<'a> for Bytes {
    fn decode(term: Term<'a>) -> NifResult<Self> {
        // Strictly a binary. `Binary::from_iolist` would also accept a
        // charlist, which is precisely the list-of-integers shape this type
        // exists to keep off the boundary.
        //
        // The copy is deliberate: the decoded slice borrows the calling
        // NIF env, while async wrappers move their arguments into a
        // `'static` future and stream producers outlive the call
        // altogether. Threading an env lifetime through every generated
        // signature to save it is a later optimisation, not a correctness
        // question.
        Ok(Self(Binary::from_term(term)?.as_slice().to_vec()))
    }
}
