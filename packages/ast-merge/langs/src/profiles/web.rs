use crate::types::Profile;

pub static JAVASCRIPT: Profile = Profile {
    name: "JavaScript",
    extensions: &["js", "mjs", "cjs", "jsx"],
    file_names: &[],
    atomic_nodes: &["import_statement", "export_statement"],
    commutative_parents: &["object", "named_imports", "export_clause", "class_body"],
    comment_nodes: &["comment"],
};

pub static TYPESCRIPT: Profile = Profile {
    name: "TypeScript",
    extensions: &["ts", "mts", "cts"],
    file_names: &[],
    atomic_nodes: &[
        "import_statement",
        "export_statement",
        "type_alias_declaration",
        "interface_declaration",
    ],
    commutative_parents: &[
        "object",
        "object_type",
        "named_imports",
        "export_clause",
        "class_body",
        "interface_body",
        "enum_body",
    ],
    comment_nodes: &["comment"],
};

pub static TSX: Profile = Profile {
    name: "TypeScript TSX",
    extensions: &["tsx"],
    file_names: &[],
    atomic_nodes: &[
        "import_statement",
        "export_statement",
        "type_alias_declaration",
        "interface_declaration",
    ],
    commutative_parents: &[
        "object",
        "object_type",
        "named_imports",
        "export_clause",
        "class_body",
        "interface_body",
        "enum_body",
    ],
    comment_nodes: &["comment"],
};

pub static HTML: Profile = Profile {
    name: "HTML",
    extensions: &["html", "htm", "xhtml"],
    file_names: &["index.html"],
    atomic_nodes: &["script_element", "style_element", "doctype"],
    commutative_parents: &["start_tag"],
    comment_nodes: &["comment"],
};

pub static CSS: Profile = Profile {
    name: "CSS",
    extensions: &["css"],
    file_names: &[],
    atomic_nodes: &["import_statement", "charset_statement"],
    commutative_parents: &["declaration_list", "keyframe_block"],
    comment_nodes: &["comment"],
};

pub static SVELTE: Profile = Profile {
    name: "Svelte",
    extensions: &["svelte"],
    file_names: &[],
    atomic_nodes: &[
        "script_element",
        "style_element",
        "import_statement",
        "export_statement",
    ],
    commutative_parents: &["object", "start_tag"],
    comment_nodes: &["comment"],
};
