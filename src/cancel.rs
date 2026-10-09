// SPDX-License-Identifier: GPL-2.0-or-later
//! Cancellation: SIGINT/SIGTERM/SIGHUP (Unix), Ctrl+C/Ctrl+Break (Windows),
//! or a `cancel` line on stdin when `--stdin-control` is given, and the end
//! of stdin with `--cancel-on-stdin-eof` (the parent process went away).
//!
//! The first request sets a flag that rbxport polls between tracks and before
//! publishing (its publication is atomic once started, so the device is never
//! left half-written). A second signal exits immediately with 130; anything
//! staged is then recovered or discarded by the next run.

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::protocol::exit;

static CANCELLED: AtomicBool = AtomicBool::new(false);
static SIGNALS: AtomicUsize = AtomicUsize::new(0);

/// Whether cancellation was requested.
#[must_use]
pub fn requested() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}

/// Requests cancellation.
pub fn request() {
    CANCELLED.store(true, Ordering::Relaxed);
}

/// Installs the signal handlers.
pub fn install_signal_handler() {
    let installed = ctrlc::set_handler(|| {
        if SIGNALS.fetch_add(1, Ordering::SeqCst) >= 1 {
            std::process::exit(exit::CANCELLED);
        }
        request();
    });
    if let Err(e) = installed {
        tracing::warn!("could not install the signal handler: {e}");
    }
}

/// Whether a stdin control line asks for cancellation.
#[must_use]
pub fn is_cancel_line(line: &str) -> bool {
    let line = line.trim();
    if line.eq_ignore_ascii_case("cancel") {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "cancel")
        })
        .unwrap_or(false)
}

/// Watches the rest of stdin for control lines on a background thread.
/// End of input (or a read error) is a cancellation only when
/// `eof_cancels`; by default it is not (a request piped in ends with it).
pub fn watch_stdin<R: BufRead + Send + 'static>(reader: R, eof_cancels: bool) {
    let spawned = std::thread::Builder::new()
        .name("stdin-control".into())
        .spawn(move || {
            if watch(reader) {
                request();
            } else if eof_cancels {
                tracing::info!("stdin closed: cancelling (--cancel-on-stdin-eof)");
                request();
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("could not watch stdin for control lines: {e}");
    }
}

/// Reads control lines until a cancel line (true) or the end of input or
/// a read error (false).
fn watch<R: BufRead>(reader: R) -> bool {
    for line in reader.lines() {
        let Ok(line) = line else { return false };
        if is_cancel_line(&line) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_line_ends_the_watch_and_the_end_of_input_does_not_count() {
        assert!(watch(&b"ping\ncancel\nmore\n"[..]));
        assert!(!watch(&b"ping\n"[..]));
        assert!(!watch(&b""[..]));
    }

    #[test]
    fn cancel_lines_are_recognised() {
        assert!(is_cancel_line("cancel"));
        assert!(is_cancel_line("  CANCEL \r"));
        assert!(is_cancel_line(r#"{"type":"cancel"}"#));
        assert!(!is_cancel_line(r#"{"type":"ping"}"#));
        assert!(!is_cancel_line(""));
        assert!(!is_cancel_line("cancelled"));
    }
}
