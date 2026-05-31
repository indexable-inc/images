use snafu::ResultExt as _;

pub struct PreorderIterator<'a> {
    cursor: tree_sitter::TreeCursor<'a>,
    done: bool,
}

impl<'a> PreorderIterator<'a> {
    fn new(root: tree_sitter::Node<'a>) -> Self {
        Self {
            cursor: root.walk(),
            done: false,
        }
    }
}

impl<'a> Iterator for PreorderIterator<'a> {
    type Item = tree_sitter::Node<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        let node = self.cursor.node();

        if self.cursor.goto_first_child() {
            return Some(node);
        }

        if self.cursor.goto_next_sibling() {
            return Some(node);
        }

        loop {
            if !self.cursor.goto_parent() {
                self.done = true;
                return Some(node);
            }
            if self.cursor.goto_next_sibling() {
                return Some(node);
            }
        }
    }
}

#[derive(Debug)]
pub struct Tree {
    source: String,
    tree: tree_sitter::Tree,
}

impl Tree {
    #[must_use]
    pub fn new(source: String, tree: tree_sitter::Tree) -> Self {
        Self { source, tree }
    }

    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn root_node(&self) -> tree_sitter::Node<'_> {
        self.tree.root_node()
    }

    #[must_use]
    pub fn node_text(&self, node: tree_sitter::Node<'_>) -> &str {
        let range = node.byte_range();
        self.source.get(range).unwrap_or_default()
    }

    #[must_use]
    pub fn preorder(&self) -> PreorderIterator<'_> {
        PreorderIterator::new(self.tree.root_node())
    }
}

pub struct Output {
    pub tree: Tree,
    /// Whether there were parse errors.
    pub has_errors: bool,
}

#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("failed to set language: {source}"))]
    SetLanguage { source: tree_sitter::LanguageError },

    #[snafu(display("parser returned no tree (cancelled or out of memory)"))]
    NoTree,
}

pub fn tree(source: &str, language: &tree_sitter::Language) -> Result<Output, Error> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(language).context(SetLanguageSnafu)?;

    let tree = parser.parse(source, None).ok_or(Error::NoTree)?;

    let has_errors = tree.root_node().has_error();

    Ok(Output {
        tree: Tree::new(source.to_owned(), tree),
        has_errors,
    })
}
