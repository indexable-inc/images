# Custom cargo completions using cargo metadata

def "nu-complete cargo packages" [] {
    let metadata = (cargo metadata --format-version 1 --no-deps | from json)
    $metadata.packages
    | where { $in.id in $metadata.workspace_members }
    | get name
}

def "nu-complete cargo run packages" [] {
    let metadata = (cargo metadata --format-version 1 --no-deps | from json)
    $metadata.packages
    | where { $in.id in $metadata.workspace_members }
    | where { $in.targets | any { "bin" in $in.kind } }
    | get name
}

def "nu-complete cargo binaries" [] {
    let metadata = (cargo metadata --format-version 1 --no-deps | from json)
    $metadata.packages
    | where { $in.id in $metadata.workspace_members }
    | each { $in.targets | where { "bin" in $in.kind } | get name }
    | flatten
}

def "nu-complete cargo examples" [] {
    let metadata = (cargo metadata --format-version 1 --no-deps | from json)
    $metadata.packages
    | where { $in.id in $metadata.workspace_members }
    | each { $in.targets | where { "example" in $in.kind } | get name }
    | flatten
}

def "nu-complete cargo test names" [] {
    let metadata = (cargo metadata --format-version 1 --no-deps | from json)
    $metadata.packages
    | where { $in.id in $metadata.workspace_members }
    | each { $in.targets | where { "test" in $in.kind } | get name }
    | flatten
}

def "nu-complete cargo packages with tests" [] {
    let metadata = (cargo metadata --format-version 1 --no-deps | from json)
    $metadata.packages
    | where { $in.id in $metadata.workspace_members }
    | where { $in.targets | any { "test" in $in.kind or "lib" in $in.kind or "bin" in $in.kind } }
    | get name
}

def "nu-complete cargo search crates" [] {
    let span = (commandline)
    let cmd = ($span | split row " " | last | str trim)

    # Return empty if less than 2 chars
    if ($cmd | str length) < 2 {
        return []
    }

    # Run cargo search and parse results
    let results = (do { ^cargo search $cmd --limit 10 } | complete)

    if $results.exit_code != 0 {
        return []
    }

    $results.stdout
    | lines
    | each { str trim }
    | where { |line| $line =~ '^[a-zA-Z0-9_-]+ = ' }
    | each { |line| $line | parse "{name} = {rest}" }
    | flatten
    | get name
}

def "nu-complete cargo binstall crates" [] {
    let span = (commandline)
    let cmd = ($span | split row " " | last | str trim)

    if ($cmd | str length) < 2 {
        return []
    }

    let results = (do { ^cargo search $cmd --limit 10 } | complete)

    if $results.exit_code != 0 {
        return []
    }

    $results.stdout
    | lines
    | each { str trim }
    | where { |line| $line =~ '^[a-zA-Z0-9_-]+ = ' }
    | each { |line| $line | parse "{name} = {rest}" }
    | flatten
    | get name
}

# Cargo run with package completion
export extern "cargo run" [
    --package(-p): string@"nu-complete cargo run packages"  # Package to run
    --release                                          # Build in release mode
    --bin: string@"nu-complete cargo binaries"        # Binary to run
    --example: string@"nu-complete cargo examples"    # Example to run
    --features: string                                 # Features to activate
    --all-features                                     # Activate all features
    --no-default-features                              # Do not activate default features
    --target: string                                   # Build for target triple
    --jobs(-j): int                                    # Number of parallel jobs
    --verbose(-v)                                      # Use verbose output
    --quiet(-q)                                        # No output printed to stdout
    --color: string                                    # Coloring: auto, always, never
    ...args: string                                    # Arguments for the binary
]

# Cargo build with package completion
export extern "cargo build" [
    --package(-p): string@"nu-complete cargo packages"  # Package to build
    --workspace                                         # Build all workspace members
    --exclude: string                                   # Exclude packages
    --all                                              # Alias for --workspace
    --release                                          # Build in release mode
    --features: string                                  # Features to activate
    --all-features                                      # Activate all features
    --no-default-features                               # Do not activate default features
    --target: string                                    # Build for target triple
    --target-dir: path                                  # Directory for build artifacts
    --jobs(-j): int                                     # Number of parallel jobs
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    --color: string                                     # Coloring: auto, always, never
    --frozen                                            # Require Cargo.lock is up to date
    --locked                                            # Require Cargo.lock is up to date
    --offline                                           # Run without network
]

# Cargo test with package completion
export extern "cargo test" [
    --package(-p): string@"nu-complete cargo packages"  # Package to test
    --workspace                                         # Test all workspace members
    --exclude: string                                   # Exclude packages
    --all                                              # Alias for --workspace
    --test: string@"nu-complete cargo test names"      # Test only specified test target
    --release                                          # Build in release mode
    --features: string                                  # Features to activate
    --all-features                                      # Activate all features
    --no-default-features                               # Do not activate default features
    --jobs(-j): int                                     # Number of parallel jobs
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    --color: string                                     # Coloring: auto, always, never
    --nocapture                                         # Don't capture stdout
    --exact                                            # Exactly match test names
    --test-threads: int                                 # Number of threads for tests
    ...args: string                                     # Test name patterns
]

# Cargo check with package completion
export extern "cargo check" [
    --package(-p): string@"nu-complete cargo packages"  # Package to check
    --workspace                                         # Check all workspace members
    --exclude: string                                   # Exclude packages
    --all                                              # Alias for --workspace
    --release                                          # Check in release mode
    --features: string                                  # Features to activate
    --all-features                                      # Activate all features
    --no-default-features                               # Do not activate default features
    --target: string                                    # Check for target triple
    --jobs(-j): int                                     # Number of parallel jobs
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    --color: string                                     # Coloring: auto, always, never
    --frozen                                            # Require Cargo.lock is up to date
    --locked                                            # Require Cargo.lock is up to date
    --offline                                           # Run without network
]

# Cargo clippy with package completion
export extern "cargo clippy" [
    --package(-p): string@"nu-complete cargo packages"  # Package to lint
    --workspace                                         # Lint all workspace members
    --exclude: string                                   # Exclude packages
    --all                                              # Alias for --workspace
    --fix                                              # Automatically apply suggestions
    --allow-dirty                                       # Allow fix with dirty working directory
    --allow-staged                                      # Allow fix with staged changes
    --release                                          # Check in release mode
    --features: string                                  # Features to activate
    --all-features                                      # Activate all features
    --no-default-features                               # Do not activate default features
    --jobs(-j): int                                     # Number of parallel jobs
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    --color: string                                     # Coloring: auto, always, never
    ...args: string                                     # Arguments passed to clippy
]

# Cargo doc with package completion
export extern "cargo doc" [
    --package(-p): string@"nu-complete cargo packages"  # Package to document
    --workspace                                         # Document all workspace members
    --exclude: string                                   # Exclude packages
    --all                                              # Alias for --workspace
    --no-deps                                           # Don't build documentation for dependencies
    --document-private-items                            # Document private items
    --open                                              # Open docs in browser after build
    --release                                          # Build in release mode
    --features: string                                  # Features to activate
    --all-features                                      # Activate all features
    --no-default-features                               # Do not activate default features
    --jobs(-j): int                                     # Number of parallel jobs
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    --color: string                                     # Coloring: auto, always, never
]

# Cargo clean
export extern "cargo clean" [
    --package(-p): string@"nu-complete cargo packages"  # Package to clean
    --release                                           # Clean release artifacts
    --target-dir: path                                  # Directory for build artifacts
    --target: string                                    # Clean for target triple
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    --color: string                                     # Coloring: auto, always, never
]

# Cargo fmt
export extern "cargo fmt" [
    --package(-p): string@"nu-complete cargo packages"  # Package to format
    --all                                              # Format all packages
    --check                                            # Check formatting without writing
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
]

# Cargo add
export extern "cargo add" [
    --package(-p): string@"nu-complete cargo packages"  # Package to add dependency to
    --dev                                              # Add as development dependency
    --build                                            # Add as build dependency
    --optional                                          # Mark as optional
    --no-optional                                       # Mark as required
    --no-default-features                               # Disable default features
    --default-features                                  # Re-enable default features
    --features: string                                  # Features to enable
    --rename: string                                    # Rename dependency
    --dry-run                                           # Don't write to Cargo.toml
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    --color: string                                     # Coloring: auto, always, never
    ...deps: string                                     # Dependencies to add
]

# Cargo nextest run
export extern "cargo nextest run" [
    --package(-p): string@"nu-complete cargo packages with tests"  # Package to test
    --workspace                                         # Test all workspace members
    --exclude: string                                   # Exclude packages
    --all                                              # Alias for --workspace
    --test: string@"nu-complete cargo test names"      # Test only specified test target
    --run-ignored: string                               # Run ignored tests (all, ignored-only, default)
    --partition: string                                 # Partition tests
    --release                                          # Build in release mode
    --features: string                                  # Features to activate
    --all-features                                      # Activate all features
    --no-default-features                               # Do not activate default features
    --jobs(-j): int                                     # Number of parallel jobs
    --test-threads: int                                 # Number of threads for tests
    --retries: int                                      # Number of retries for flaky tests
    --fail-fast                                        # Stop on first failure
    --no-fail-fast                                      # Run all tests even on failure
    --no-capture                                        # Don't capture output
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    --color: string                                     # Coloring: auto, always, never
    ...args: string                                     # Test name patterns
]

# Cargo search - search registry for crates
export extern "cargo search" [
    --limit: int                                        # Limit the number of results (default: 10)
    --index: string                                     # Registry index URL to use
    --registry: string                                  # Registry to use
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    --color: string                                     # Coloring: auto, always, never
    ...query: string@"nu-complete cargo search crates" # Search query
]

# Cargo binstall - install pre-compiled binaries
export extern "cargo binstall" [
    --no-confirm(-y)                                    # Skip confirmation prompt
    --force                                            # Force installation even if already installed
    --no-symlinks                                       # Don't create symlinks
    --install-path: path                                # Installation path for binaries
    --bin-dir: path                                     # Directory to install binaries
    --root: path                                        # Install to given path
    --locked                                            # Require Cargo.lock is up to date
    --offline                                           # Run without network access
    --no-track                                          # Don't track installation
    --strategies: string                                # Installation strategies (comma-separated)
    --only-signed                                       # Only install signed binaries
    --skip-signatures                                   # Skip signature verification
    --log-level: string                                 # Log level (off, error, warn, info, debug, trace)
    --version: string                                   # Version requirement
    --verbose(-v)                                       # Use verbose output
    --quiet(-q)                                         # No output printed to stdout
    ...crates: string@"nu-complete cargo binstall crates"  # Crates to install
]
