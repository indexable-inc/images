pub fn encode(value: u64) -> String {
    let mut buffer = itoa::Buffer::new();
    format!("bravo:{}", buffer.format(value))
}

pub fn encode_float(value: f64) -> String {
    let mut buffer = ryu::Buffer::new();
    format!("bravo:{}", buffer.format(value))
}
