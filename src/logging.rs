//! Log setup: daily rotating files in the log directory, plus stderr in
//! debug builds. Level filter from `NTFY_NATIVE_LOG` (default `info`).

use anyhow::Context;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::paths::{APP_NAME, AppPaths};

pub const LOG_ENV: &str = "NTFY_NATIVE_LOG";
const KEEP_FILES: usize = 7;

/// Installs the global subscriber. Keep the returned guard alive until the
/// process ends; dropping it flushes buffered lines.
pub fn init(paths: &AppPaths) -> anyhow::Result<WorkerGuard> {
    let appender = Builder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix(APP_NAME)
        .filename_suffix("log")
        .max_log_files(KEEP_FILES)
        .build(&paths.log_dir)
        .context("creating log file appender")?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("info"));
    let file = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(writer);
    let stderr = cfg!(debug_assertions)
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    tracing_subscriber::registry()
        .with(filter)
        .with(file)
        .with(stderr)
        .try_init()
        .context("installing log subscriber")?;

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(thread = ?std::thread::current().name(), "panic: {info}");
        default_hook(info);
    }));
    Ok(guard)
}
