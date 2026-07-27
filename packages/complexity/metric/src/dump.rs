//! Prints the tree-sitter node kinds each supported grammar actually produces
//! for the constructs the profiles classify. Run with
//! `cargo test -p complexity-metric dump -- --nocapture`; the profiles in
//! `kinds.rs` are transcribed from its output rather than guessed, because a
//! misspelled kind is silently scored zero rather than rejected.

use ast_merge_ast::tree;
use ast_merge_langs::Lang;

const SAMPLES: &[(Lang, &str)] = &[
    (
        Lang::Rust,
        r"
fn f(a: u32) -> u32 {
    let g = |x: u32| x + 1;
    if a > 1 && a < 9 || a == 4 { return 0; } else if a == 2 { return 1; } else { return 2; }
    while a > 0 { break; }
    loop { break; }
    for _ in 0..a { continue; }
    match a { 0 => 1, _ => 2 }
}
",
    ),
    (
        Lang::Python,
        r"
def f(a):
    g = lambda x: x + 1
    if a > 1 and a < 9 or a == 4:
        pass
    elif a == 2:
        pass
    else:
        pass
    while a: break
    for i in range(a): continue
    try: pass
    except ValueError: pass
    b = 1 if a else 2
    match a:
        case 1: pass
",
    ),
    (
        Lang::TypeScript,
        r"
function f(a: number): number {
  const g = (x: number) => x + 1;
  if (a > 1 && a < 9 || a === 4) { } else if (a === 2) { } else { }
  while (a) { break; }
  do { } while (a);
  for (const x of []) { }
  for (let i = 0; i < a; i++) { }
  try { } catch (e) { }
  switch (a) { case 1: break; default: break; }
  return a ? 1 : 2;
}
",
    ),
    (
        Lang::Go,
        "
func f(a int) int {
\tg := func(x int) int { return x + 1 }
\tif a > 1 && a < 9 || a == 4 {
\t} else if a == 2 {
\t} else {
\t}
\tfor i := 0; i < a; i++ {
\t}
\tswitch a {
\tcase 1:
\tdefault:
\t}
\tselect {}
\treturn g(a)
}
",
    ),
    (
        Lang::Nix,
        r"
{
  f = a: b:
    if a > 1 && a < 9 || a == 4
    then let c = 1; in c
    else with a; b;
}
",
    ),
    (
        Lang::Elixir,
        r"
defmodule M do
  def f(a) do
    g = fn x -> x + 1 end
    if a > 1 and a < 9 or a == 4, do: 1, else: 2
    case a do
      1 -> :one
      _ -> :other
    end
    cond do
      a > 0 -> :pos
      true -> :neg
    end
    g.(a)
  end
end
",
    ),
];

#[test]
fn dump_node_kinds() {
    for (lang, source) in SAMPLES {
        let parsed = tree(source, &lang.to_tree_sitter()).expect("sample parses");
        assert!(!parsed.has_errors, "{lang:?} sample has parse errors");
        let mut kinds: Vec<&str> = parsed
            .tree
            .preorder()
            .filter(tree_sitter::Node::is_named)
            .map(|node| node.kind())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        println!("\n=== {lang:?} ===\n{}", kinds.join("\n"));
    }
}
