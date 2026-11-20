# List available commands.
default:
    @just --list

# Build lkit and lflare for the host with Cargo's release profile.
build:
    cargo build --locked --release

# Install the existing release lkit on a test host over SSH.
push $host:
    sh scripts/push-lkit.sh "$host"

# Build static Linux distribution binaries and checksums in dist/.
dist:
    bash scripts/build-musl-static.sh

# Format the Rust workspace.
fmt:
    cargo fmt --all

# Run Clippy on all targets of the default workspace members.
lint:
    cargo clippy --all-targets -- -D warnings
