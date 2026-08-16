//! The only bin: links the whole chain. The check runs it and asserts the
//! computed value (37 * 3 + 3 = 114) so the convergence proof is about a
//! chain that demonstrably executes, not four crates that merely compile.

fn main() {
    println!("{}", chain_c::adjusted());
}
