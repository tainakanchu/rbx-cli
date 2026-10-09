// SPDX-License-Identifier: GPL-2.0-or-later
//! Diagnostics from this crate and the rbxport crates (`tracing`).
//!
//! Human mode: formatted lines on stderr. `--json`: `log` envelopes on stdout.
//! The level comes from `--log-level`, else `RBX_CLI_LOG`, else `warn`.

use std::fmt::Write as _;

use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::protocol::LogLevel;

pub fn init(json: bool, level: Option<&str>) {
    let directive = level
        .map(str::to_owned)
        .or_else(|| std::env::var("RBX_CLI_LOG").ok())
        .unwrap_or_else(|| "warn".to_owned());
    let filter = EnvFilter::try_new(&directive).unwrap_or_else(|_| EnvFilter::new("warn"));
    let registry = tracing_subscriber::registry().with(filter);
    let result = if json {
        registry.with(JsonLogLayer).try_init()
    } else {
        registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(HumanWriter)
                    .without_time()
                    .with_target(false),
            )
            .try_init()
    };
    // Already initialised (tests): keep the first subscriber.
    let _ = result;
}

/// Routes formatted human logs through the output module, so they do not
/// tear a progress line.
#[derive(Clone, Copy)]
struct HumanWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for HumanWriter {
    type Writer = HumanLine;
    fn make_writer(&'a self) -> Self::Writer {
        HumanLine(Vec::new())
    }
}

struct HumanLine(Vec<u8>);

impl std::io::Write for HumanLine {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for HumanLine {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.0);
        let text = text.trim_end();
        if !text.is_empty() {
            crate::output::human_line(text);
        }
    }
}

struct JsonLogLayer;

#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.rest, " {}={value:?}", field.name());
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.rest, " {}={value}", field.name());
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for JsonLogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let metadata = event.metadata();
        let level = match *metadata.level() {
            tracing::Level::ERROR => LogLevel::Error,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::DEBUG => LogLevel::Debug,
            tracing::Level::TRACE => LogLevel::Trace,
        };
        let message = format!("{}{}", fields.message, fields.rest);
        crate::output::log(level, message, Some(metadata.target().to_owned()));
    }
}
