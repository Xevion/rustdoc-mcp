default:
    just --list

# Run all checks via tempo (fmt, clippy, machete, deny)
check *args:
    bunx @xevion/tempo check {{ args }}

# Format code
fmt:
    cargo fmt --all

# Check formatting without modifying files
fmt-check:
    cargo fmt --all --check

# Run tests
test *args:
    cargo nextest run --no-fail-fast {{ args }}

# Run the MCP server
run:
    cargo run

# Install the MCP server (debug build)
install:
    cargo install --path . --debug

# Install release build
install-release:
    cargo install --path .

# Supply-chain audit
deny:
    cargo deny check

# Full CI gate (ordered, fail-fast)
ci: fmt-check check deny test

# Clear generated rustdoc data (preserves binaries and caches)
clear:
    rm -rf target/doc
    @echo "Cleared rustdoc data from target/doc/"

# Clear cached search indexes (fixes stale cache issues)
clear-cache:
    rm -f target/doc/*.index
    @echo "Cleared search index cache"
