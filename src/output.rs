// SPDX-License-Identifier: GPL-2.0-or-later
//! Where everything a command says goes.
//!
//! `--json`: one [`Message`] per line on stdout, flushed per line.
//! Human mode: the result on stdout, progress and diagnostics on stderr.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::error::CliError;
use crate::protocol::{
    ErrorMessage, Event, Log, LogLevel, Message, Progress, ProgressItem, ResultMessage,
    PROTOCOL_VERSION,
};

struct Sink {
    json: bool,
    quiet: bool,
    stderr_tty: bool,
    command: Mutex<Option<String>>,
    /// A `\r` progress line is on the terminal and must be cleared first.
    progress_open: AtomicBool,
    /// Serialises whole lines across threads (the tracing layer logs from any).
    lock: Mutex<()>,
}

static SINK: OnceLock<Sink> = OnceLock::new();

fn sink() -> &'static Sink {
    SINK.get_or_init(|| Sink {
        json: false,
        quiet: false,
        stderr_tty: false,
        command: Mutex::new(None),
        progress_open: AtomicBool::new(false),
        lock: Mutex::new(()),
    })
}

/// Configures output once, before anything is printed.
pub fn init(json: bool, quiet: bool) {
    let _ = SINK.set(Sink {
        json,
        quiet,
        stderr_tty: std::io::stderr().is_terminal(),
        command: Mutex::new(None),
        progress_open: AtomicBool::new(false),
        lock: Mutex::new(()),
    });
}

#[must_use]
pub fn is_json() -> bool {
    sink().json
}

/// Names the running command (`usb.export`, …) for every later line.
pub fn set_command(name: &str) {
    if let Ok(mut command) = sink().command.lock() {
        *command = Some(name.to_owned());
    }
}

fn command() -> Option<String> {
    sink().command.lock().ok().and_then(|c| c.clone())
}

fn write_json(message: &Message) {
    let Ok(line) = serde_json::to_string(message) else {
        return;
    };
    let _guard = sink().lock.lock();
    let mut out = std::io::stdout().lock();
    // A closed stdout (caller went away) is not worth crashing over.
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn clear_progress_line(err: &mut impl Write) {
    if sink().progress_open.swap(false, Ordering::Relaxed) {
        let _ = write!(err, "\r\x1b[2K");
    }
}

/// Writes one human-mode diagnostic line to stderr.
pub fn human_line(text: &str) {
    let _guard = sink().lock.lock();
    let mut err = std::io::stderr().lock();
    clear_progress_line(&mut err);
    let _ = writeln!(err, "{text}");
}

pub fn progress(phase: &str, current: u64, total: u64, item: Option<ProgressItem>) {
    let s = sink();
    if s.json {
        write_json(&Message::Progress(Progress {
            protocol: PROTOCOL_VERSION,
            command: command().unwrap_or_default(),
            phase: phase.to_owned(),
            current,
            total,
            item,
        }));
        return;
    }
    if s.quiet {
        return;
    }
    let counter = if total > 0 {
        format!(" {current}/{total}")
    } else {
        String::new()
    };
    let title = item.map(|i| format!(" {}", i.title)).unwrap_or_default();
    let text = format!("[{phase}{counter}]{title}");
    let _guard = s.lock.lock();
    let mut err = std::io::stderr().lock();
    if s.stderr_tty {
        // One rewritten line, cut so it never wraps on a narrow terminal.
        let shown: String = text.chars().take(100).collect();
        let _ = write!(err, "\r\x1b[2K{shown}");
        let _ = err.flush();
        s.progress_open.store(true, Ordering::Relaxed);
    } else {
        let _ = writeln!(err, "{text}");
    }
}

/// A named event; `human` is what a person sees on stderr instead.
pub fn event(name: &str, data: serde_json::Value, human: &str) {
    if sink().json {
        write_json(&Message::Event(Event {
            protocol: PROTOCOL_VERSION,
            command: command().unwrap_or_default(),
            event: name.to_owned(),
            data,
        }));
    } else if !sink().quiet {
        human_line(human);
    }
}

/// A log line in JSON mode (called by the tracing layer).
pub fn log(level: LogLevel, message: String, target: Option<String>) {
    write_json(&Message::Log(Log {
        protocol: PROTOCOL_VERSION,
        level,
        message,
        target,
    }));
}

/// The terminal success line. `human` renders the result for a person.
pub fn result<T: Serialize>(data: &T, human: impl FnOnce() -> String) {
    if sink().json {
        let data = serde_json::to_value(data).unwrap_or(serde_json::Value::Null);
        write_json(&Message::Result(ResultMessage {
            protocol: PROTOCOL_VERSION,
            command: command().unwrap_or_default(),
            data,
        }));
    } else {
        {
            let _guard = sink().lock.lock();
            clear_progress_line(&mut std::io::stderr().lock());
        }
        let text = human();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", text.trim_end());
        let _ = out.flush();
    }
}

/// The terminal failure line.
pub fn error(error: &CliError) {
    if sink().json {
        write_json(&Message::Error(ErrorMessage {
            protocol: PROTOCOL_VERSION,
            command: command(),
            code: error.code,
            message: error.message.clone(),
            exit_code: error.code.exit_code(),
            details: error.details.clone(),
        }));
    } else {
        human_line(&format!("error: {}", error.message));
        if let Some(details) = &error.details {
            if let Ok(pretty) = serde_json::to_string_pretty(details) {
                human_line(&pretty);
            }
        }
    }
}
