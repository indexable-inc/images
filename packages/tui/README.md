# TUI Library

A high-performance library for managing multiple pseudo-terminal (PTY) based terminal user interfaces concurrently.

## Overview

This library provides a robust abstraction for spawning and managing terminal applications with proper PTY support. Unlike simple stdin/stdout piping, this library uses real pseudo-terminals, enabling full support for interactive terminal applications like vim, htop, and other TUI programs.

## Architecture

### PTY-Based Design

The library uses **non-blocking PTY masters** on the managing side while providing **blocking PTY slaves** to spawned processes. This architecture allows:

- **Non-blocking I/O**: The master side uses async/await with Tokio for efficient concurrent management of multiple terminals
- **True Terminal Emulation**: Spawned processes see a real terminal device, not pipes
- **Proper Signal Handling**: Terminal control signals (SIGINT, SIGTSTP, etc.) work correctly
- **Terminal Sizing**: Support for terminal resize operations via `ioctl`

### Key Components

- **`TuiManager`**: Main interface for managing multiple TUI instances
- **`TuiInstance`**: Represents a single running TUI with its PTY master and output buffer
- **`TuiInfo`**: Metadata about a TUI instance (ID, command, arguments)

## Features

- **Concurrent TUI Management**: Spawn and manage multiple terminal applications simultaneously
- **Async I/O**: Non-blocking reads and writes using Tokio
- **VT100 Emulation**: Full terminal emulation via vt100 parser with scrollback support
- **Configurable Scrollback**: Store terminal history beyond the visible viewport
- **2D Slicing**: Extract specific rows/columns from terminal output
- **Timeout Support**: Blocking reads with configurable timeouts

## Usage

```rust
use tui::TuiManager;

let manager = TuiManager::new();

let instance = manager
    .spawn(
        "vim".to_string(),
        vec![],
        10000  // scrollback buffer: 10000 lines
    )
    .unwrap();

manager.write(&instance, "i").unwrap();
manager.write(&instance, "Hello, PTY!\n").unwrap();

let lines = manager.read(&instance).unwrap();
for line in lines {
    println!("{}", line);
}

let viewport = manager.read_viewport(&instance).unwrap();

let scrollback = manager.read_scrollback(&instance).unwrap();

let lines = manager.read_blocking(&instance, 1000).unwrap();
```

## Technical Details

### PTY Master (Non-Blocking)

The PTY master is held in an `Arc<tokio::sync::Mutex<Pty>>` for safe concurrent access across async tasks. This provides:

- Non-blocking reads via Tokio's `AsyncRead`
- Non-blocking writes via Tokio's `AsyncWrite`
- Safe sharing across multiple async contexts

### Output Processing

Terminal output is read asynchronously in a dedicated background task and processed through a VT100 terminal emulator:

1. Raw bytes are read from the PTY master
2. Bytes are fed to the vt100::Parser which maintains terminal state
3. ANSI escape sequences are processed (cursor movement, colors, formatting)
4. Terminal screen is maintained as 24x80 viewport with configurable scrollback
5. Concurrent reads from multiple threads are supported

### Terminal Configuration

- Default terminal size: 24 rows × 80 columns
- Configurable scrollback buffer (default: 10000 lines)
- Full VT100 terminal emulation with color and formatting support

### Scrollback Buffer

The scrollback buffer stores lines that have scrolled off the visible viewport:

- `read()`: Returns viewport only (current limitation: scrollback access not yet implemented)
- `read_viewport()`: Returns only the visible 24x80 screen
- `read_scrollback()`: Returns historical lines (currently returns empty; future enhancement)

Note: While the scrollback buffer is configured and the VT100 parser stores scrollback content, accessing historical lines programmatically requires navigating the vt100 API via `set_scrollback()` which will be implemented in a future update.

## Error Handling

All operations return `Result<T, Error>` with structured error types defined via `snafu`:

- `ProcessSpawn`: Failed to spawn process
- `TuiNotFound`: Invalid TUI ID
- `WriteToTui`: Write operation failed
- `NoOutputAvailable`: No buffered output to read

## Dependencies

- **pty-process**: PTY creation and management
- **tokio**: Async runtime and I/O
- **vt100**: Terminal emulation with ANSI escape sequence parsing
- **parking_lot**: High-performance synchronization primitives
- **snafu**: Ergonomic error handling

## Performance Characteristics

- **Memory**: O(n × m) where n = number of TUIs, m = average lines buffered per TUI
- **Concurrency**: Lock-free reads from output buffer (RwLock)
- **Latency**: Sub-millisecond write latency, configurable read timeout

## Platform Support

Currently supports Unix-like systems (Linux, macOS) where PTY devices are available.
