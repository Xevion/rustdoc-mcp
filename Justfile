default:
    just --list

# Install the MCP server (debug build)
install:
    cargo install --path . --debug

# Run type checking and linting
check:
    cargo check
    cargo clippy

# Run the MCP server
run:
    cargo run
