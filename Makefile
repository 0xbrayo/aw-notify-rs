.PHONY: build release test clean install check fmt clippy run help

# Default target
help:
	@echo "Available targets:"
	@echo "  build    - Build debug version"
	@echo "  release  - Build optimized release version"
	@echo "  test     - Run tests"
	@echo "  check    - Check for compilation errors"
	@echo "  fmt      - Format code"
	@echo "  clippy   - Run clippy linter"
	@echo "  clean    - Clean build artifacts"
	@echo "  install  - Install to ~/.cargo/bin"
	@echo "  run      - Run in development mode"
	@echo "  run-test - Run in testing mode"

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

check:
	cargo check

fmt:
	cargo fmt

clippy:
	cargo clippy -- -D warnings

clean:
	cargo clean

# Install to ~/.cargo/bin
install:
	cargo install --path .

run:
	RUST_LOG=info cargo run -- start --verbose

# Run in testing mode
run-test:
	RUST_LOG=info cargo run -- start --testing --verbose

# Run checkin command
checkin:
	cargo run -- checkin --verbose

# Build and run all checks
ci: fmt clippy test check

# Build Docker image (if needed)
docker:
	docker build -t aw-notify-rs .
