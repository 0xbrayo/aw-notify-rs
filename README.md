# aw-notify-rs

A Rust implementation of the ActivityWatch notification service. This is a port of the original Python [aw-notify](https://github.com/ActivityWatch/aw-notify) with improved performance and native system integration.

## Features

- **Time tracking notifications**: Get notified when you've spent a certain amount of time on different activities
- **Category-based alerts**: Configurable thresholds for categories like Work, Twitter, YouTube, etc.
- **Periodic check-ins**: Hourly summaries and daily reports
- **Server monitoring**: Notifications when ActivityWatch server goes offline
- **Native desktop notifications**: Cross-platform notification support
- **Efficient caching**: Reduces API calls with intelligent caching

## Installation

### Prerequisites

- Rust 1.70 or later
- ActivityWatch server running

### Build from source

```bash
git clone <repository-url>
cd aw-notify-rs
cargo build --release
```

The binary will be available at `target/release/aw-notify`.

## Usage

### Start the notification service

```bash
# Start with default settings
aw-notify start

# Start in testing mode (connects to port 5666)
aw-notify start --testing

# Start with custom port
aw-notify start --port 5600

# Enable verbose logging
aw-notify start --verbose
```

### Send a one-time check-in notification

```bash
# Send summary of today's activity
aw-notify checkin

# Send summary in testing mode
aw-notify checkin --testing
```

## Configuration

The notification service includes several pre-configured alerts:

- **All activities**: Notifications at 1h, 2h, 4h, 6h, 8h
- **Twitter**: Notifications at 15min, 30min, 1h
- **YouTube**: Notifications at 15min, 30min, 1h
- **Work**: Positive notifications at 15min, 30min, 1h, 2h, 4h (shown as "Goal reached!")

## Notification Types

1. **Threshold alerts**: When you reach time thresholds for specific categories
2. **Hourly check-ins**: Summary of activity every hour (when active)
3. **Daily summaries**: Report of yesterday's activity and today's progress
4. **New day notifications**: Welcome message when starting activity on a new day
5. **Server status**: Alerts when ActivityWatch server goes online/offline

## Architecture

The Rust implementation features:

- **Asynchronous processing**: Non-blocking notification delivery
- **Multi-threaded design**: Separate threads for different notification types
- **Intelligent caching**: TTL-based caching to minimize server requests
- **Error resilience**: Graceful handling of server disconnections
- **Cross-platform**: Works on macOS, Linux, and Windows

## Differences from Python version

- **Performance**: Significantly faster startup and lower memory usage
- **Native notifications**: Better integration with system notification centers
- **Simplified dependencies**: No Python runtime or pip packages required
- **Concurrent processing**: Multiple notification threads run simultaneously
- **Type safety**: Compile-time guarantees for data handling

## Development

### Dependencies

The project uses these key dependencies:

- `aw-client-rust`: ActivityWatch client library
- `clap`: Command-line argument parsing
- `chrono`: Date and time handling
- `notify-rust`: Cross-platform desktop notifications
- `tokio`: Async runtime
- `anyhow`: Error handling

### Building

```bash
# Check for compilation errors
cargo check

# Run tests
cargo test

# Build optimized release version
cargo build --release

# Run with logging
RUST_LOG=debug cargo run -- start --verbose
```

## Troubleshooting

### macOS Notifications

On macOS, you may need to grant notification permissions to your terminal or the built binary. If notifications aren't appearing:

1. Go to System Preferences → Notifications & Focus
2. Find your terminal app (Terminal, iTerm2, etc.) or the aw-notify binary
3. Enable "Allow Notifications"

### Server Connection Issues

If the service can't connect to ActivityWatch:

1. Ensure ActivityWatch server is running (`aw-qt` or `aw-server`)
2. Check the correct port (default: 5600, testing: 5666)
3. Verify the server is accessible at `http://localhost:5600`

### Missing Categories

If expected categories aren't showing up:

1. Ensure ActivityWatch watchers are running (`aw-watcher-window`, `aw-watcher-afk`)
2. Check that categorization rules are configured in ActivityWatch
3. Verify data exists by checking the ActivityWatch web interface

## License

This project follows the same license as the original aw-notify project (MPL-2.0).

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Submit a pull request

## Acknowledgments

This is a port of the original [aw-notify](https://github.com/ActivityWatch/aw-notify) Python implementation by Erik Bjäreholt and the ActivityWatch team.
