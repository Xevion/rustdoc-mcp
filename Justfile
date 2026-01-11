default:
    just --list

# Install the MCP server (debug build)
install:
    cargo install --path . --debug

# Clear generated rustdoc data (preserves binaries and caches)
clear:
    rm -rf target/doc
    @echo "Cleared rustdoc data from target/doc/"

# Clear cached search indexes (fixes stale cache issues)
clear-cache:
    rm -f target/doc/*.index
    @echo "Cleared search index cache"

# Run type checking and linting
check:
    cargo check --workspace --all-targets
    cargo clippy --workspace --all-targets
    cargo machete --with-metadata

# Run the MCP server
run:
    cargo run

# Run tests in parallel
test:
    cargo nextest run --no-fail-fast
