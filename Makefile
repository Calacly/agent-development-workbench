.PHONY: help run build test clean setup

help:
	@echo "MCP Client Development Commands"
	@echo "================================"
	@echo "make run        - Run the app (API and web UI) on http://localhost:3000"
	@echo "make build      - Build a release binary"
	@echo "make test       - Run the tests"
	@echo "make clean      - Clean build artifacts"
	@echo "make setup      - Initial setup (copy env file)"

setup:
	@echo "Setting up environment file..."
	@test -f .env || cp .env.example .env
	@echo "Environment file created"
	@echo "Please edit .env with your configuration"

run:
	cargo run

build:
	cargo build --release

test:
	cargo test

clean:
	cargo clean
	@echo "Clean complete"
