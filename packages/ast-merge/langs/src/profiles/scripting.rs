use crate::types::Profile;

pub(crate) static PYTHON: Profile = Profile {
    name: "Python",
    extensions: &["py", "pyi", "bzl", "bazel"],
    file_names: &["BUILD", "BUILD.bazel"],
    atomic_nodes: &[
        "import_statement",
        "import_from_statement",
        "future_import_statement",
    ],
    commutative_parents: &["dictionary", "set", "argument_list", "decorated_definition"],
    comment_nodes: &["comment"],
};

pub(crate) static RUBY: Profile = Profile {
    name: "Ruby",
    extensions: &["rb", "rake", "gemspec"],
    file_names: &["Rakefile", "Gemfile", "Guardfile", "Capfile"],
    atomic_nodes: &["require", "require_relative"],
    commutative_parents: &["hash", "class", "module", "block"],
    comment_nodes: &["comment"],
};

pub(crate) static PHP: Profile = Profile {
    name: "PHP",
    extensions: &["php", "phtml", "php3", "php4", "php5", "php7", "phps"],
    file_names: &[],
    atomic_nodes: &["namespace_use_declaration", "namespace_definition"],
    commutative_parents: &[
        "declaration_list",
        "class_declaration",
        "interface_declaration",
        "trait_declaration",
        "array_creation_expression",
    ],
    comment_nodes: &["comment"],
};

pub(crate) static BASH: Profile = Profile {
    name: "Bash",
    extensions: &["sh", "bash", "zsh"],
    file_names: &[".bashrc", ".bash_profile", ".zshrc", ".profile"],
    atomic_nodes: &["command"],
    commutative_parents: &["case_statement"],
    comment_nodes: &["comment"],
};

pub(crate) static LUA: Profile = Profile {
    name: "Lua",
    extensions: &["lua"],
    file_names: &[],
    atomic_nodes: &["require_call"],
    commutative_parents: &["table_constructor", "field_list"],
    comment_nodes: &["comment"],
};
