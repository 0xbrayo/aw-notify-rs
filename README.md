# aw-notify-rs

A simplified Rust implementation of [aw-notify](https://github.com/ActivityWatch/aw-notify) that matches the Python version's behavior while providing Rust's performance and safety benefits.

## Overview

This is a streamlined rewrite that consolidates functionality into a single file (~750 lines) similar to the Python version, while maintaining:

- ✅ **Identical behavior** to the Python implementation
- ✅ **Type safety** and memory safety of Rust
- ✅ **Zero runtime overhead** with native compilation
- ✅ **Simple architecture** that's easy to understand and maintain

## Features

- **Time Summaries**: Get daily and hourly summaries of your most-used categories
- **Threshold Alerts**: Receive notifications when you reach specific time thresholds
- **Server Monitoring**: Get notified if the ActivityWatch server goes down
- **New Day Greetings**: Start your day with a greeting showing the current date
- **Cross-platform**: Native desktop notifications on macOS, Linux, and Windows
- **Smart Caching**: 60-second TTL cache reduces server requests (matches Python's `@cache_ttl`)

## Installation

### Prerequisites

- Rust toolchain (1.70+)
- ActivityWatch server running (default: localhost:5600)

### Building from source

```bash
# Clone the repository
git clone <repository-url>
cd aw-notify-rs

# Build the application
cargo build --release

# The binary will be available at target/release/aw-notify
```

## Usage

### Starting the notification service

```bash
# Start with default settings
./target/release/aw-notify start

# Start in testing mode (connects to port 5666)
./target/release/aw-notify --testing start

# Start with custom port
./target/release/aw-notify --port 5678 start

# Enable verbose logging
./target/release/aw-notify --verbose start
```

### Sending a one-time summary notification

```bash
# Send summary of today's activity
./target/release/aw-notify checkin

# Send summary in testing mode
./target/release/aw-notify --testing checkin
```

### Command-line options

```
ActivityWatch notification service

Usage: aw-notify [OPTIONS] [COMMAND]

Commands:
  start    Start the notification service
  checkin  Send a summary notification
  help     Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose      Verbose logging
      --testing      Testing mode (port 5666)
      --port <PORT>  Port to connect to ActivityWatch server
  -h, --help         Print help
  -V, --version      Print version
```

## Architecture

This implementation uses a simplified architecture that mirrors the Python version:

### Single File Design
- **One main.rs file** (~750 lines) containing all functionality
- **Global state** using `once_cell::Lazy` (matches Python's globals)
- **Simple daemon threads** for background tasks (matches Python's threading)

### Core Components
- **CategoryAlert**: Tracks time thresholds (exact match to Python class)
- **Caching**: TTL cache with 60-second expiration (matches Python's `@cache_ttl`)
- **Notifications**: macOS terminal-notifier → notify-rust fallback (matches Python)
- **Query System**: Canonical events queries (identical to Python)

### Background Threads
- **Threshold monitoring**: Checks category time limits every 10 seconds
- **Hourly checkins**: Sends summaries at the top of each hour (if active)
- **New day notifications**: Greets user when they first become active each day
- **Server monitoring**: Alerts when ActivityWatch server goes up/down

## Default Alerts

The application includes these pre-configured threshold alerts:

- **All activities**: 1h, 2h, 4h, 6h, 8h notifications
- **Twitter**: 15min, 30min, 1h warnings
- **YouTube**: 15min, 30min, 1h warnings
- **Work**: 15min, 30min, 1h, 2h, 4h achievements (shown as "Goal reached!")

## Notification Types

1. **Threshold alerts**: "Time spent" or "Goal reached!" when limits hit
2. **Hourly summaries**: Top categories every hour (when active)
3. **Daily summaries**: "Time today" and "Time yesterday" reports
4. **New day greetings**: Welcome message with current date
5. **Server status**: Alerts when ActivityWatch server connectivity changes


## Compatibility
- **100% behavioral compatibility** with Python version
- **Identical queries** and time calculations
- **Same notification logic** and message formatting
- **Matching cache behavior** and error handling


### Building

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release

# Check for errors
cargo check

# Run with logging
RUST_LOG=debug cargo run -- --verbose checkin
```

## Troubleshooting

### Server Connection Issues

If the service can't connect to ActivityWatch:

1. Ensure ActivityWatch server is running
2. Check the correct port (default: 5600, testing: 5666)
3. Verify server accessibility: `curl http://localhost:5600/api/0/info`


## License

This project is licensed under the Mozilla Public License 2.0 (MPL-2.0), the same as the ActivityWatch project.

## Acknowledgments

This is a simplified rewrite of the original [aw-notify](https://github.com/ActivityWatch/aw-notify) Python implementation by Erik Bjäreholt and the ActivityWatch team, designed to match its behavior exactly while providing Rust's performance and safety benefits.
