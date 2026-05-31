use crate::types::Profile;

pub(crate) static RUST: Profile = Profile {
    name: "Rust",
    extensions: &["rs"],
    file_names: &[],
    atomic_nodes: &["use_declaration", "attribute_item", "inner_attribute_item"],
    commutative_parents: &[
        "declaration_list",
        "use_list",
        "enum_variant_list",
        "field_declaration_list",
    ],
    comment_nodes: &["line_comment", "block_comment"],
};

pub(crate) static C: Profile = Profile {
    name: "C",
    extensions: &["c", "h"],
    file_names: &[],
    atomic_nodes: &[
        "preproc_include",
        "preproc_def",
        "preproc_ifdef",
        "preproc_ifndef",
        "preproc_if",
    ],
    commutative_parents: &[
        "field_declaration_list",
        "enumerator_list",
        "declaration_list",
    ],
    comment_nodes: &["comment"],
};

pub(crate) static CPP: Profile = Profile {
    name: "C++",
    extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx", "h++", "c++"],
    file_names: &[],
    atomic_nodes: &[
        "preproc_include",
        "preproc_def",
        "using_declaration",
        "namespace_definition",
        "template_declaration",
    ],
    commutative_parents: &[
        "field_declaration_list",
        "enumerator_list",
        "declaration_list",
        "base_class_clause",
    ],
    comment_nodes: &["comment"],
};

pub(crate) static CSHARP: Profile = Profile {
    name: "C#",
    extensions: &["cs"],
    file_names: &[],
    atomic_nodes: &[
        "using_directive",
        "namespace_declaration",
        "attribute",
        "attribute_list",
    ],
    commutative_parents: &[
        "class_declaration",
        "interface_declaration",
        "struct_declaration",
        "enum_declaration",
        "declaration_list",
    ],
    comment_nodes: &["comment", "multiline_comment"],
};

pub(crate) static SWIFT: Profile = Profile {
    name: "Swift",
    extensions: &["swift"],
    file_names: &[],
    atomic_nodes: &["import_declaration", "attribute"],
    commutative_parents: &[
        "class_body",
        "struct_body",
        "protocol_body",
        "enum_body",
        "extension_body",
    ],
    comment_nodes: &["comment", "multiline_comment"],
};

pub(crate) static GO: Profile = Profile {
    name: "Go",
    extensions: &["go"],
    file_names: &[],
    atomic_nodes: &["import_declaration", "import_spec"],
    commutative_parents: &[
        "import_spec_list",
        "field_declaration_list",
        "interface_type",
        "struct_type",
    ],
    comment_nodes: &["comment"],
};
