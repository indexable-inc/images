# Typst completions - uses nu-complete typ-files from config.nu

# Typst compile with .typ file completion
export extern "typst compile" [
    input: string@"nu-complete typ-files"   # Input .typ file
    output?: string                            # Output file (PDF, PNG, or SVG)
    --root: path                               # Root directory for file access
    --input(-i): string                        # Key-value pairs visible through sys.inputs
    --font-path: path                          # Additional directories to search for fonts
    --diagnostic-format: string                # Format for diagnostics (human, short)
    --format(-f): string                       # Output format (pdf, png, svg)
    --open                                     # Open output file after compilation
    --ppi: int                                 # Pixels per inch for PNG output
    --timings                                  # Show compilation timings
    --jobs(-j): int                            # Number of parallel jobs
    --package-path: path                       # Custom path to package cache
    --package-cache-path: path                 # Custom path for package cache
    --cert: path                               # Path to CA certificate for HTTPS
    --color: string                            # Colorize output (auto, always, never)
    --verbose(-v)                              # Enable verbose logging
    --quiet(-q)                                # Suppress output
    --help(-h)                                 # Print help
]

# Typst watch with .typ file completion
export extern "typst watch" [
    input: string@"nu-complete typ-files"   # Input .typ file
    output?: string                            # Output file (PDF, PNG, or SVG)
    --root: path                               # Root directory for file access
    --input(-i): string                        # Key-value pairs visible through sys.inputs
    --font-path: path                          # Additional directories to search for fonts
    --diagnostic-format: string                # Format for diagnostics (human, short)
    --format(-f): string                       # Output format (pdf, png, svg)
    --open                                     # Open output file after compilation
    --ppi: int                                 # Pixels per inch for PNG output
    --jobs(-j): int                            # Number of parallel jobs
    --package-path: path                       # Custom path to package cache
    --package-cache-path: path                 # Custom path for package cache
    --cert: path                               # Path to CA certificate for HTTPS
    --color: string                            # Colorize output (auto, always, never)
    --verbose(-v)                              # Enable verbose logging
    --quiet(-q)                                # Suppress output
    --help(-h)                                 # Print help
]
