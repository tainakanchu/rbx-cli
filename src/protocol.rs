// SPDX-License-Identifier: GPL-2.0-or-later
//! The machine-readable output protocol: NDJSON envelopes on stdout.
//!
//! Every line printed in `--json` mode is one [`Message`]. All carry `type`
//! and `protocol`; exactly one `result` or `error` line ends a command. See
//! `docs/protocol.md` for the rules callers can rely on.

use schemars::JsonSchema;
use serde::Serialize;

/// Version of the wire protocol (envelope + request/response shapes).
///
/// Bump on any breaking change: a removed or renamed field, a changed type or
/// meaning, a removed command. Additive changes (new optional request
/// fields, new result fields, new event names, new capabilities) do not bump it.
pub const PROTOCOL_VERSION: u32 = 1;

/// One line of `--json` output.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Message {
    Progress(Progress),
    Event(Event),
    Log(Log),
    Result(ResultMessage),
    Error(ErrorMessage),
}

/// How far a long-running command has got.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub protocol: u32,
    /// The command reporting, e.g. `usb.export`.
    pub command: String,
    /// The current phase. For `usb.export`: `preparing`, `analyzing`,
    /// `checking`, `copying`, `database`, `verifying`, `publishing`.
    pub phase: String,
    /// Items completed so far in this phase.
    pub current: u64,
    /// Items in this phase (0 when unknown).
    pub total: u64,
    /// The item being worked on, when the phase is per item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<ProgressItem>,
}

/// The item a progress line is about.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProgressItem {
    /// Zero-based index into the request's `tracks`, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    /// The track's `ref` from the request, when it had one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// Human-readable name (usually the track title).
    pub title: String,
}

/// Something noteworthy that is not progress, e.g. a skipped track.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub protocol: u32,
    pub command: String,
    /// Dotted event name, e.g. `track.skipped`. See docs/protocol.md.
    pub event: String,
    /// Event-specific payload.
    pub data: serde_json::Value,
}

/// A diagnostic. Never needed to interpret the result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    pub protocol: u32,
    pub level: LogLevel,
    pub message: String,
    /// The Rust module that logged it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// The successful outcome of a command. Terminal.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResultMessage {
    pub protocol: u32,
    pub command: String,
    /// Command-specific result; see `schema/result.*.json`.
    pub data: serde_json::Value,
}

/// The failed outcome of a command. Terminal.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorMessage {
    pub protocol: u32,
    /// The command that failed; absent when the command line itself was invalid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub code: ErrorCode,
    pub message: String,
    /// The process exit code that follows this line.
    pub exit_code: i32,
    /// Structured context, when there is any (e.g. a verification report).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Stable, machine-matchable error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Bad command-line arguments.
    Usage,
    /// The request document is malformed or inconsistent.
    InvalidRequest,
    /// A path or device does not exist.
    NotFound,
    /// The device's existing library conflicts with the request (see message).
    Conflict,
    /// The device disappeared while being written.
    DeviceGone,
    /// rekordbox is running; it must not write the same device concurrently.
    RekordboxRunning,
    /// The destination volume has too little free space for the copy.
    InsufficientSpace,
    /// The written export did not read back correctly.
    VerificationFailed,
    /// SIGINT/SIGTERM/Ctrl+C/Ctrl+Break or a `cancel` control line.
    Cancelled,
    /// The request needs a newer protocol or an unsupported feature.
    Unsupported,
    /// An I/O error not covered above.
    Io,
    /// Anything else.
    Internal,
}

impl ErrorCode {
    /// The process exit code for this error.
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Usage | Self::InvalidRequest | Self::Unsupported => exit::USAGE,
            Self::VerificationFailed => exit::VERIFY_FAILED,
            Self::Cancelled => exit::CANCELLED,
            _ => exit::ERROR,
        }
    }
}

/// Process exit codes.
pub mod exit {
    pub const OK: i32 = 0;
    pub const ERROR: i32 = 1;
    pub const USAGE: i32 = 2;
    /// `usb verify` found problems, or an export failed its read-back check.
    pub const VERIFY_FAILED: i32 = 3;
    pub const CANCELLED: i32 = 130;
}
