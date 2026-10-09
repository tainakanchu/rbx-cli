// SPDX-License-Identifier: GPL-2.0-or-later
//! `devices list` and `devices eject` (rbl-devices).

use std::path::Path;

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::{CliError, CliResult};
use crate::protocol::ErrorCode;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceExport {
    pub tracks: usize,
    pub playlists: usize,
    /// Written by rbx-cli or rbxport (incremental sync possible).
    pub ours: bool,
    /// When that export ran; empty otherwise.
    pub written: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub name: String,
    pub mount_point: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub file_system: String,
    /// What the OS says; external SSDs often say no.
    pub removable: bool,
    /// An identity that survives a rename while mounted.
    pub volume_id: String,
    /// The library on it, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export: Option<DeviceExport>,
}

/// `devices list` result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DevicesResult {
    pub devices: Vec<DeviceInfo>,
}

/// The volumes an export could be written to. Read-only: devices with an
/// interrupted export are reported as found, not recovered.
pub fn list() -> DevicesResult {
    DevicesResult {
        devices: rbl_devices::list()
            .into_iter()
            .map(|d| {
                let export = rbl_devices::inspect(&d.mount_point).map(|e| DeviceExport {
                    tracks: e.tracks,
                    playlists: e.playlists,
                    ours: e.ours,
                    written: e.written,
                });
                DeviceInfo {
                    name: d.name,
                    mount_point: d.mount_point.to_string_lossy().into_owned(),
                    total_bytes: d.total_bytes,
                    free_bytes: d.free_bytes,
                    file_system: d.file_system,
                    removable: d.removable,
                    volume_id: d.volume_id,
                    export,
                }
            })
            .collect(),
    }
}

pub fn human_list(result: &DevicesResult) -> String {
    use std::fmt::Write as _;
    if result.devices.is_empty() {
        return "No devices.\n".to_owned();
    }
    let mut s = String::new();
    for d in &result.devices {
        #[allow(clippy::cast_precision_loss, reason = "display only")]
        let gb = |b: u64| b as f64 / 1e9;
        let library = d.export.as_ref().map_or_else(
            || "no library".to_owned(),
            |e| {
                format!(
                    "{} tracks, {} playlists{}",
                    e.tracks,
                    e.playlists,
                    if e.ours { "" } else { " (other software)" }
                )
            },
        );
        let _ = writeln!(
            s,
            "{}  {}  {}  {:.1}/{:.1} GB free  {library}",
            d.mount_point,
            d.name,
            d.file_system,
            gb(d.free_bytes),
            gb(d.total_bytes)
        );
    }
    s
}

/// `devices eject` result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EjectResult {
    pub mount_point: String,
    pub ejected: bool,
}

/// Ejects a listed volume through the OS (never forced; refused for any
/// path that is not a listed volume's mount point).
pub fn eject(mount_point: &Path) -> CliResult<EjectResult> {
    rbl_devices::eject::eject(mount_point).map_err(|e| {
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            ErrorCode::NotFound
        } else {
            ErrorCode::Io
        };
        CliError::new(
            code,
            format!("could not eject {}: {e}", mount_point.display()),
        )
    })?;
    Ok(EjectResult {
        mount_point: mount_point.to_string_lossy().into_owned(),
        ejected: true,
    })
}
