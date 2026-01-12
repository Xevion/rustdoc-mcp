//! Tracing initialization.

use std::sync::Once;
use tracing_subscriber::{EnvFilter, fmt::format::FmtSpan, util::SubscriberInitExt};

static INIT: Once = Once::new();

/// Initialize tracing. Safe to call multiple times.
pub fn init() {
    INIT.call_once(|| {
        let is_test =
            std::env::var("NEXTEST").is_ok() || std::env::var("CARGO_TARGET_TMPDIR").is_ok();
        let filter = EnvFilter::from_default_env().add_directive(
            if is_test {
                tracing::Level::DEBUG
            } else {
                tracing::Level::INFO
            }
            .into(),
        );

        let builder = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_target(true)
            .with_span_events(FmtSpan::NONE)
            .compact();

        if is_test {
            builder.with_test_writer().finish().set_default();
        } else {
            if let Err(e) = builder.with_writer(std::io::stderr).try_init() {
                eprintln!("Failed to initialize tracing: {}", e)
            }
        }
    });
}
