use crate::types::Profile;

pub(crate) static JSON: Profile = Profile {
    name: "JSON",
    extensions: &["json", "jsonc"],
    file_names: &["package.json", "tsconfig.json", "composer.json"],
    atomic_nodes: &[],
    commutative_parents: &["object"],
    comment_nodes: &[],
};

pub(crate) static TOML: Profile = Profile {
    name: "TOML",
    extensions: &["toml"],
    file_names: &["Cargo.toml", "pyproject.toml"],
    atomic_nodes: &[],
    commutative_parents: &["table", "inline_table"],
    comment_nodes: &["comment"],
};

pub(crate) static YAML: Profile = Profile {
    name: "YAML",
    extensions: &["yaml", "yml"],
    file_names: &[],
    atomic_nodes: &[],
    commutative_parents: &["block_mapping", "flow_mapping"],
    comment_nodes: &["comment"],
};

pub(crate) static MARKDOWN: Profile = Profile {
    name: "Markdown",
    extensions: &["md", "markdown", "mdown", "mkd"],
    file_names: &["README.md", "CHANGELOG.md", "CONTRIBUTING.md"],
    atomic_nodes: &["code_block", "fenced_code_block", "html_block"],
    commutative_parents: &[],
    comment_nodes: &[],
};

pub(crate) static DOCKERFILE: Profile = Profile {
    name: "Dockerfile",
    extensions: &[],
    file_names: &["Dockerfile", "Containerfile"],
    atomic_nodes: &["comment_line"],
    commutative_parents: &[],
    comment_nodes: &["comment_line"],
};

pub(crate) static NIX: Profile = Profile {
    name: "Nix",
    extensions: &["nix"],
    file_names: &["flake.nix", "default.nix", "shell.nix"],
    atomic_nodes: &["inherit", "inherit_from"],
    commutative_parents: &["attrset_expression", "formals"],
    comment_nodes: &["comment"],
};
