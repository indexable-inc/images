use crate::types::Profile;

pub(crate) static HASKELL: Profile = Profile {
    name: "Haskell",
    extensions: &["hs", "lhs"],
    file_names: &[],
    atomic_nodes: &["import", "pragma", "module"],
    commutative_parents: &[
        "class_body",
        "instance_body",
        "data_constructors",
        "record_body",
    ],
    comment_nodes: &["comment", "haddock"],
};

pub(crate) static ELIXIR: Profile = Profile {
    name: "Elixir",
    extensions: &["ex", "exs"],
    file_names: &["mix.exs"],
    atomic_nodes: &["alias", "import", "use", "require"],
    commutative_parents: &["map", "keyword_list", "do_block"],
    comment_nodes: &["comment"],
};

pub(crate) static OCAML: Profile = Profile {
    name: "OCaml",
    extensions: &["ml", "mli"],
    file_names: &[],
    atomic_nodes: &["open_statement", "include_statement"],
    commutative_parents: &[
        "record_expression",
        "record_declaration",
        "object_expression",
    ],
    comment_nodes: &["comment"],
};
