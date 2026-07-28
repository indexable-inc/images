fn main() {
    let args: Vec<String> = std::env::args().collect();
    let value = u64::try_from(args.len()).unwrap_or(1);
    println!("{}", scope_alpha::encode(value));
    println!("{}", scope_alpha::encode_float(f64::from(u32::try_from(value).unwrap_or(1))));
    println!("{}", scope_alpha::checked(value));
    // Monomorphizes panicking std generics here rather than in libstd, so the
    // reference gate also sees whether the toolchain's rust-src leaked in.
    let name = &args[0];
    println!("{}", &name[..name.len().min(3)]);
    let mut sorted: Vec<&str> = args.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    println!("{}", sorted.join(","));
}
