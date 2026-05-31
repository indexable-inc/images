#[derive(Debug, Clone)]
pub struct ParsedConflict {
    pub before: String,
    pub left: String,
    pub base: Option<String>,
    pub right: String,
    pub after: String,
    pub left_name: String,
    pub right_name: String,
    pub base_name: Option<String>,
}

#[derive(Debug)]
pub struct ParsedFile {
    pub conflicts: Vec<ParsedConflict>,
    pub has_conflicts: bool,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct DisplaySettings {
    pub marker_size: usize,
    pub diff3_style: bool,
    pub left_name: String,
    pub right_name: String,
    pub base_name: String,
}

/// Git's default conflict marker size (e.g. `<<<<<<<` is 7 characters).
const DEFAULT_MARKER_SIZE: usize = 7;

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            marker_size: DEFAULT_MARKER_SIZE,
            diff3_style: true,
            left_name: String::from("ours"),
            right_name: String::from("theirs"),
            base_name: String::from("base"),
        }
    }
}

impl DisplaySettings {
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            marker_size: std::env::var("GIT_MERGE_MARKER_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_MARKER_SIZE),
            diff3_style: std::env::var("GIT_DIFF3").map_or(true, |v| v != "0"),
            left_name: std::env::var("GIT_MERGE_OURS").unwrap_or_else(|_| String::from("ours")),
            right_name: std::env::var("GIT_MERGE_THEIRS")
                .unwrap_or_else(|_| String::from("theirs")),
            base_name: std::env::var("GIT_MERGE_BASE").unwrap_or_else(|_| String::from("base")),
        }
    }
}

#[derive(Debug)]
pub struct DriverResult {
    pub content: String,
    pub exit_code: i32,
    pub conflict_count: usize,
}

impl DriverResult {
    #[must_use]
    pub fn success(content: String) -> Self {
        Self {
            content,
            exit_code: 0,
            conflict_count: 0,
        }
    }

    #[must_use]
    pub fn with_conflicts(content: String, conflict_count: usize) -> Self {
        Self {
            content,
            exit_code: 1,
            conflict_count,
        }
    }
}
