use crate::types::Profile;

pub static JAVA: Profile = Profile {
    name: "Java",
    extensions: &["java"],
    file_names: &[],
    atomic_nodes: &["import_declaration", "package_declaration", "annotation"],
    commutative_parents: &[
        "class_body",
        "interface_body",
        "enum_body",
        "annotation_type_body",
        "module_body",
    ],
    comment_nodes: &["line_comment", "block_comment"],
};

pub static KOTLIN: Profile = Profile {
    name: "Kotlin",
    extensions: &["kt", "kts"],
    file_names: &[],
    atomic_nodes: &["import_header", "package_header", "annotation"],
    commutative_parents: &[
        "class_body",
        "enum_class_body",
        "object_literal",
        "when_expression",
    ],
    comment_nodes: &["line_comment", "multiline_comment"],
};

pub static SCALA: Profile = Profile {
    name: "Scala",
    extensions: &["scala", "sc"],
    file_names: &[],
    atomic_nodes: &["import_declaration", "package_clause", "annotation"],
    commutative_parents: &["template_body", "block", "case_block"],
    comment_nodes: &["comment", "block_comment"],
};
