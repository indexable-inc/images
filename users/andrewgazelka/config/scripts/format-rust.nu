#!/usr/bin/env nu

# Format a Rust file using rustfmt
# Reads hook JSON from stdin: { tool_input: { file_path: "..." }, ... }

def main [] {
    try {
        let hook_data = $in | from json
        let file_path = $hook_data.tool_input?.file_path?

        if ($file_path != null) and ($file_path | str ends-with ".rs") {
            try {
                rustfmt $file_path
            }
        }
    }
}
