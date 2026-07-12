//! Emits a small JSON document and declares, via linking metadata, that its
//! stdout is to be read through the `json` lens. A lens-aware shell parses
//! the output into structured data without being told to.

link_meta::stdout_lens!("json");

fn main() {
    println!(
        r#"{{"tool":"link-meta-demo","lens":"json","facts":[{{"name":"answer","value":42}},{{"name":"pi","value":3.14}}]}}"#
    );
}
