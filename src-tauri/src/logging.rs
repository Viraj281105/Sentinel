//! Structured logging.
//!
//! Logs go to a daily-rotated file in the app log directory (14 files kept) and, in
//! debug builds, to stderr. The level is controlled by `SENTINEL_LOG` (an
//! `EnvFilter` directive, default `info`). Never log secrets or file contents.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{self, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Keeps the background log writer alive; dropping it flushes pending records.
pub(crate) struct LogGuard(#[allow(dead_code)] WorkerGuard);

pub(crate) fn init(log_dir: &Path) -> Result<LogGuard, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(log_dir)?;
    let appender = rolling::Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix("sentinel")
        .filename_suffix("log")
        .max_log_files(14)
        .build(log_dir)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_env("SENTINEL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let file_layer = fmt::layer().with_writer(writer).with_ansi(false);
    let stderr_layer = cfg!(debug_assertions).then(|| fmt::layer().with_writer(std::io::stderr));

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()?;
    Ok(LogGuard(guard))
}
