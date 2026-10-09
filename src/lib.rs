// SPDX-License-Identifier: GPL-2.0-or-later
//! rbx-cli: a command-line front end to rbxport's rekordbox-compatible
//! library crates. Each command is a thin layer: request → rbxport types →
//! run → protocol envelope.

pub mod analyze;
pub mod anlz;
pub mod artwork;
pub mod cache;
pub mod cancel;
pub mod cli;
pub mod device;
pub mod devices;
pub mod error;
pub mod export;
pub mod grid;
pub mod logging;
pub mod output;
pub mod protocol;
pub mod request;
pub mod schema;
pub mod usb;
pub mod version;

use clap::Parser;

use crate::cli::{Cli, Command, DevicesCommand, UsbCommand};
use crate::error::{CliError, CliResult};
use crate::protocol::{exit, ErrorCode};

/// Runs the CLI with the process arguments; returns the exit code.
#[must_use]
pub fn main() -> i32 {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) => return usage_error(&args, &e),
    };
    output::init(cli.json, cli.quiet);
    logging::init(cli.json, cli.log_level.as_deref());
    output::set_command(cli.command.name());
    cancel::install_signal_handler();
    match dispatch(cli.command) {
        Ok(()) => exit::OK,
        Err(error) => {
            output::error(&error);
            error.code.exit_code()
        }
    }
}

/// Bad arguments: clap's message, or an `error` envelope with `--json`.
fn usage_error(args: &[std::ffi::OsString], error: &clap::Error) -> i32 {
    use clap::error::ErrorKind;
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        let _ = error.print();
        return if error.kind() == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand {
            exit::USAGE
        } else {
            exit::OK
        };
    }
    if args.iter().any(|a| a == "--json") {
        output::init(true, true);
        let message = error
            .kind()
            .as_str()
            .map_or_else(|| error.to_string(), str::to_owned);
        let detail = error.to_string();
        output::error(
            &CliError::new(ErrorCode::Usage, message)
                .with_details(serde_json::json!({ "usage": detail.trim_end() })),
        );
    } else {
        let _ = error.print();
    }
    exit::USAGE
}

fn dispatch(command: Command) -> CliResult<()> {
    match command {
        Command::Usb(UsbCommand::Export(args)) => {
            let cache = if args.no_cache {
                cache::Cache::disabled()
            } else {
                match args.cache_dir.or_else(cache::Cache::default_dir) {
                    Some(dir) => cache::Cache::at(dir),
                    None => {
                        tracing::warn!(
                            "no cache directory on this platform; analysis is not cached"
                        );
                        cache::Cache::disabled()
                    }
                }
            };
            let result = export::run(&export::ExportArgs {
                input: args.input,
                to: args.to,
                dry_run: args.dry_run,
                cache,
                jobs: args.jobs.map_or(export::DEFAULT_JOBS, usize::from),
                stdin_control: args.stdin_control,
            })?;
            output::result(&result, || export::human(&result));
        }
        Command::Usb(UsbCommand::Inspect {
            root,
            summary,
            cues,
        }) => {
            let result = usb::inspect(&root, summary, cues)?;
            output::result(&result, || usb::human_inspect(&result));
        }
        Command::Usb(UsbCommand::Verify { root }) => {
            let result = usb::verify(&root)?;
            output::result(&result, || usb::human_verify(&result));
        }
        Command::Devices(DevicesCommand::List) => {
            let result = devices::list();
            output::result(&result, || devices::human_list(&result));
        }
        Command::Devices(DevicesCommand::Eject { mount_point }) => {
            let result = devices::eject(&mount_point)?;
            output::result(&result, || format!("Ejected {}", result.mount_point));
        }
        Command::Version => {
            let info = version::info();
            output::result(&info, || version::human(&info));
        }
        Command::Schema { out } => {
            let schemas = schema::all();
            match out {
                Some(dir) => {
                    std::fs::create_dir_all(&dir)
                        .map_err(|e| CliError::io("cannot create the schema directory", &e))?;
                    for (name, text) in &schemas {
                        std::fs::write(dir.join(name), text)
                            .map_err(|e| CliError::io(&format!("cannot write {name}"), &e))?;
                    }
                    let names: Vec<&str> = schemas.iter().map(|(n, _)| *n).collect();
                    output::result(&names, || {
                        format!("Wrote {} schemas to {}", names.len(), dir.display())
                    });
                }
                None => {
                    let all: serde_json::Map<String, serde_json::Value> = schemas
                        .iter()
                        .map(|(name, text)| {
                            (
                                (*name).to_owned(),
                                serde_json::from_str(text).unwrap_or(serde_json::Value::Null),
                            )
                        })
                        .collect();
                    output::result(&all, || {
                        serde_json::to_string_pretty(&all).unwrap_or_default()
                    });
                }
            }
        }
    }
    Ok(())
}
