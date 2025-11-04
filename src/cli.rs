use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "cargo-doc-mcp")]
#[command(about = "Query Rust documentation for AI assistants", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    Search {
        query: String,
        #[arg(short = 'c', long = "crate")]
        crate_override: Option<String>,
        #[arg(short, long)]
        kind: Option<String>,
        #[arg(short = 'n', long, default_value = "25")]
        limit: usize,
    },
    Paths {
        type_name: String,
        #[arg(short = 'c', long = "crate")]
        crate_override: Option<String>,
    },
    Signature {
        function_name: String,
        #[arg(short = 'c', long = "crate")]
        crate_override: Option<String>,
        #[arg(short = 'n', long, default_value = "5")]
        limit: usize,
    },
}
