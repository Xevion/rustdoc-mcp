//! Tracing initialization with runtime format selection and custom formatting.
//!
//! Supports two output formats controlled by `LOG_FORMAT` env var:
//! - `pretty` (default): Human-readable compact output with ANSI colors when stderr is a TTY
//! - `json`: Machine-parseable JSON lines for log aggregation
//!
//! All output goes to stderr — stdout is reserved for the MCP JSON-RPC protocol.

use std::fmt;
use std::io::IsTerminal;
use std::sync::Once;
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{self, FormatEvent, FormatFields};
use tracing_subscriber::fmt::{FmtContext, FormattedFields};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt};

static INIT: Once = Once::new();

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogFormat {
    Pretty,
    Json,
}

impl LogFormat {
    fn from_env() -> Self {
        match std::env::var("LOG_FORMAT")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "json" => Self::Json,
            _ => Self::Pretty,
        }
    }
}

/// Build an `EnvFilter` with sensible defaults for an MCP server.
///
/// Suppresses noisy internal modules at `warn` while keeping
/// application modules at the specified default level.
fn build_filter(default_level: tracing::Level) -> EnvFilter {
    let base = format!(
        "warn,\
         rustdoc_mcp={level},\
         rmcp=warn,\
         hyper=warn,\
         tokio=warn,\
         h2=warn,\
         tower=warn",
        level = default_level,
    );

    // Allow RUST_LOG to override our defaults
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(base))
}

/// Custom compact event formatter.
///
/// Produces output like:
/// ```text
/// 2026-03-27T15:04:23.456Z  INFO rustdoc_mcp::server: Starting MCP server
/// 2026-03-27T15:04:23.789Z DEBUG rustdoc_mcp::worker: Cache hit crate_name="serde"
/// ```
///
/// Fields are rendered inline after the message. Span fields from parent
/// spans are included for context propagation.
struct CompactFormatter {
    ansi: bool,
}

impl<S, N> FormatEvent<S, N> for CompactFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: format::Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        use tracing_subscriber::fmt::time::FormatTime;

        // Timestamp
        let timer = tracing_subscriber::fmt::time::SystemTime;
        timer.format_time(&mut writer)?;

        // Level with optional color
        let level = *event.metadata().level();
        if self.ansi {
            let color = match level {
                tracing::Level::ERROR => "\x1b[31m",
                tracing::Level::WARN => "\x1b[33m",
                tracing::Level::INFO => "\x1b[32m",
                tracing::Level::DEBUG => "\x1b[36m",
                tracing::Level::TRACE => "\x1b[35m",
            };
            write!(writer, " {color}{level:>5}\x1b[0m")?;
        } else {
            write!(writer, " {level:>5}")?;
        }

        // Target (module path)
        if let Some(target) = event.metadata().target().strip_prefix("rustdoc_mcp::") {
            write!(writer, " {target}:")?;
        } else {
            write!(writer, " {}:", event.metadata().target())?;
        }

        // Collect span fields for context
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                let exts = span.extensions();
                if let Some(fields) = exts.get::<FormattedFields<N>>() {
                    if !fields.is_empty() {
                        write!(writer, " {{{fields}}}")?;
                    }
                }
            }
        }

        // Event message and fields
        write!(writer, " ")?;
        ctx.format_fields(writer.by_ref(), event)?;

        writeln!(writer)
    }
}

/// Initialize tracing. Safe to call multiple times (idempotent).
pub fn init() {
    INIT.call_once(|| {
        let is_test =
            std::env::var("NEXTEST").is_ok() || std::env::var("CARGO_TARGET_TMPDIR").is_ok();

        let default_level = if is_test {
            tracing::Level::DEBUG
        } else {
            tracing::Level::INFO
        };

        let filter = build_filter(default_level);
        let format = LogFormat::from_env();

        if is_test {
            // Test mode: always pretty, use test writer for cargo test capture
            let layer = tracing_subscriber::fmt::layer()
                .event_format(CompactFormatter { ansi: false })
                .with_test_writer();

            tracing_subscriber::registry()
                .with(filter)
                .with(layer)
                .set_default();
        } else {
            match format {
                LogFormat::Pretty => {
                    let ansi = std::io::stderr().is_terminal();
                    let layer = tracing_subscriber::fmt::layer()
                        .event_format(CompactFormatter { ansi })
                        .with_writer(std::io::stderr);

                    if let Err(e) = tracing_subscriber::registry()
                        .with(filter)
                        .with(layer)
                        .try_init()
                    {
                        eprintln!("Failed to initialize tracing: {e}");
                    }
                }
                LogFormat::Json => {
                    let layer = tracing_subscriber::fmt::layer()
                        .json()
                        .with_writer(std::io::stderr);

                    if let Err(e) = tracing_subscriber::registry()
                        .with(filter)
                        .with(layer)
                        .try_init()
                    {
                        eprintln!("Failed to initialize tracing: {e}");
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_format_from_env() {
        // Default is pretty
        assert_eq!(LogFormat::from_env(), LogFormat::Pretty);
    }

    #[test]
    fn test_build_filter_creates_filter() {
        let filter = build_filter(tracing::Level::INFO);
        // Should not panic and should create a valid filter
        drop(filter);
    }

    #[test]
    fn test_init_is_idempotent() {
        // Calling init multiple times should not panic
        init();
        init();
    }
}
