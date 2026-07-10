#!/usr/bin/env nu

# Run clippy on a Rust file and output lints inline
# Reads hook JSON from stdin: { tool_input: { file_path: "..." }, ... }

def main [] {
    try {
        let hook_data = $in | from json
        let file_path = $hook_data.tool_input?.file_path?

        if ($file_path != null) and ($file_path | str ends-with ".rs") {
            try {
                # Run clippy with JSON output
                let clippy_output = (cargo clippy --message-format=json --all-targets --all-features -- -D warnings
                    | lines
                    | where $it != ""
                    | each { |line| $line | from json })

                # Filter for actual diagnostics related to the edited file
                let diagnostics = ($clippy_output
                    | where reason == "compiler-message"
                    | get message
                    | where spans.file_name.0? == $file_path)

                # Display lints inline
                for diag in $diagnostics {
                    let span = $diag.spans.0
                    let level = $diag.level
                    let message = $diag.message
                    let line = $span.line_start
                    let col = $span.column_start

                    print $"($file_path):($line):($col): ($level): ($message)"

                    # Show code snippet if available
                    if ($diag.code?.code? | is-not-empty) {
                        print $"  (ansi grey)help: see `rustc --explain ($diag.code.code)`(ansi reset)"
                    }
                }
            }
        }
    }
}
