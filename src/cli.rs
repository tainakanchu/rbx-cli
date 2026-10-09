// SPDX-License-Identifier: GPL-2.0-or-later
//! The command line.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// CLI for rekordbox-compatible DJ libraries (USB export and more), built on
/// rbxport crates. Not affiliated with AlphaTheta / Pioneer DJ.
#[derive(Debug, Parser)]
#[command(name = "rbx-cli", version, about, long_about = None, propagate_version = true)]
pub struct Cli {
    /// Machine-readable output: one JSON envelope per line on stdout
    /// (see docs/protocol.md).
    #[arg(long, global = true)]
    pub json: bool,
    /// Human mode: no progress lines.
    #[arg(long, short, global = true)]
    pub quiet: bool,
    /// Diagnostics level or filter (error, warn, info, debug, trace).
    /// Default: `RBX_CLI_LOG`, else `warn`.
    #[arg(long, global = true, value_name = "LEVEL")]
    pub log_level: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Write, read and check USB exports.
    #[command(subcommand)]
    Usb(UsbCommand),
    /// Volumes an export could go to.
    #[command(subcommand)]
    Devices(DevicesCommand),
    /// Version, pinned rbxport revision, protocol version and capabilities.
    Version,
    /// Print (or write) the JSON Schemas of requests and results.
    #[command(hide = true)]
    Schema {
        /// Write `*.json` files into this directory instead of printing.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum UsbCommand {
    /// Export (or incrementally sync) tracks and playlists to a USB root.
    Export(ExportCli),
    /// Show what a USB export holds.
    Inspect {
        /// The USB root (mount point).
        root: PathBuf,
        /// Counts only: no track or playlist lists.
        #[arg(long)]
        summary: bool,
        /// Include each track's cues and grid size, read from its analysis
        /// files (cues a player saved on the device included).
        #[arg(long)]
        cues: bool,
    },
    /// Read an export back with an independent parser and check it.
    Verify {
        /// The USB root (mount point).
        root: PathBuf,
    },
}

#[derive(Debug, Args)]
pub struct ExportCli {
    /// The request document (JSON); `-` reads stdin.
    #[arg(long, short, default_value = "-", value_name = "FILE")]
    pub input: String,
    /// The USB root; overrides the request's `destination`.
    #[arg(long, value_name = "DIR")]
    pub to: Option<PathBuf>,
    /// Plan only: report what would be copied, reused and analysed, and
    /// write nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Analysis cache directory [default: the platform cache dir +
    /// rbx-cli/analysis].
    #[arg(long, value_name = "DIR", conflicts_with = "no_cache")]
    pub cache_dir: Option<PathBuf>,
    /// Do not read or write the analysis cache.
    #[arg(long)]
    pub no_cache: bool,
    /// Tracks analysed at once [default: 3, as rbxport].
    #[arg(long, short, value_name = "N", value_parser = clap::value_parser!(u16).range(1..=64))]
    pub jobs: Option<u16>,
    /// Watch stdin for a `cancel` line even when the request comes from a
    /// file (a request read from stdin always enables this).
    #[arg(long)]
    pub stdin_control: bool,
    /// Treat the end of stdin as a cancellation (implies --stdin-control):
    /// when the parent process dies, its end of the pipe closes and the
    /// export stops. The parent must keep stdin open while it runs.
    #[arg(long)]
    pub cancel_on_stdin_eof: bool,
}

#[derive(Debug, Subcommand)]
pub enum DevicesCommand {
    /// List mounted volumes and the library each holds.
    List,
    /// Eject a listed volume (never forced).
    Eject {
        /// The volume's mount point, as `devices list` shows it.
        mount_point: PathBuf,
    },
}

impl Command {
    /// The protocol name of the command (`usb.export`, …).
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Usb(UsbCommand::Export(_)) => "usb.export",
            Self::Usb(UsbCommand::Inspect { .. }) => "usb.inspect",
            Self::Usb(UsbCommand::Verify { .. }) => "usb.verify",
            Self::Devices(DevicesCommand::List) => "devices.list",
            Self::Devices(DevicesCommand::Eject { .. }) => "devices.eject",
            Self::Version => "version",
            Self::Schema { .. } => "schema",
        }
    }
}
