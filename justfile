# Justfile for scriptr

# Show help by default
default:
	@just --list

# Build the scriptr binary (debug)
build:
	cargo build

# Run scriptr from source with arbitrary args
# Usage: just run -- --help
run *args:
	cargo run -- {{args}}

# Install scriptr from the current workspace (overwrite existing)
install:
	cargo install --path . --locked --force
