#![warn(clippy::fallible_int_fallback)]
#![allow(
    clippy::useless_conversion,
    clippy::unnecessary_fallible_conversions,
    clippy::unnecessary_lazy_evaluations,
    unused
)]

fn fires(big: u64, x: i64) {
    // `unwrap_or` with a literal default.
    let _ = u8::try_from(big).unwrap_or(0);
    //~^ fallible_int_fallback

    // `unwrap_or` with `T::MAX`.
    let _ = i16::try_from(x).unwrap_or(i16::MAX);
    //~^ fallible_int_fallback

    // `unwrap_or_default`.
    let _ = i32::try_from(x).unwrap_or_default();
    //~^ fallible_int_fallback

    // `unwrap_or_else`.
    let _ = usize::try_from(x).unwrap_or_else(|_| 0);
    //~^ fallible_int_fallback

    // The same conversion expressed with `try_into` on the value.
    let _: u8 = big.try_into().unwrap_or(0);
    //~^ fallible_int_fallback
}

fn does_not_fire(maybe: Option<u8>, big: u64) {
    // `Option::unwrap_or` is not a fallible integer conversion.
    let _ = maybe.unwrap_or(0);

    // A `Result` whose error is not `TryFromIntError`.
    let parsed: Result<u8, std::num::ParseIntError> = "5".parse();
    let _ = parsed.unwrap_or(0);

    // An explicit, documented clamp via `min` then `as`: intent is visible.
    let _ = big.min(u8::MAX as u64) as u8;

    // Infallible conversion: no `Result`, no fallback.
    let _ = u64::from(7u8);
}

fn main() {
    fires(300, -1);
    does_not_fire(None, 300);
}
