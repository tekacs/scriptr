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

# Tag a release (triggers CI build + formula update)
release version:
	#!/usr/bin/env bash
	set -euo pipefail
	CURRENT=$(cargo metadata --format-version 1 --no-deps | jq -r '.packages[0].version')
	if [ "$CURRENT" != "{{version}}" ]; then
		sed -i '' 's/^version *= *"[^"]*"/version = "{{version}}"/' Cargo.toml
		cargo generate-lockfile  # update Cargo.lock
		echo "Bumped Cargo.toml to {{version}}"
	fi
	jj commit -m $'v{{version}}\n\nBump version for release' Cargo.toml Cargo.lock
	jj bookmark set master -r @-
	jj git push
	git tag v{{version}} "$(jj log -r @- --no-graph -T 'commit_id')"
	git push origin v{{version}}
	@echo "Tagged v{{version}} — CI will build and create the release"
