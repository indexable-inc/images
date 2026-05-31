#[derive(Debug, Clone)]
pub struct Conflict {
    pub base: Option<Region>,
    pub left: Region,
    pub right: Region,
}

#[derive(Debug, Clone)]
pub struct Region {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Region {
    #[must_use]
    pub fn new(start: usize, end: usize, text: String) -> Self {
        Self { start, end, text }
    }
}

#[derive(Debug)]
pub struct Result {
    pub content: String,
    pub conflicts: Vec<Conflict>,
    pub success: bool,
}

impl Result {
    #[must_use]
    pub fn success(content: String) -> Self {
        Self {
            content,
            conflicts: Vec::new(),
            success: true,
        }
    }

    #[must_use]
    pub fn with_conflicts(content: String, conflicts: Vec<Conflict>) -> Self {
        Self {
            content,
            conflicts,
            success: false,
        }
    }
}
