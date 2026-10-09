// SPDX-License-Identifier: GPL-2.0-or-later
//! What a device already holds, read without writing to it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::analyze::AnalysisMeta;
use crate::anlz::AnalysisFile;

/// The legacy database's track rows by device id (empty when unreadable).
pub fn pdb_tracks(destination: &Path) -> BTreeMap<u32, rbl_pdb::TrackRow> {
    let pdb = rbl_export::export_root(destination).join("rekordbox/export.pdb");
    let Ok(bytes) = std::fs::read(pdb) else {
        return BTreeMap::new();
    };
    let Ok(parsed) = rbl_pdb::Pdb::parse(&bytes) else {
        return BTreeMap::new();
    };
    parsed
        .table(rbl_pdb::PageType::Tracks)
        .map(|table| {
            parsed
                .track_rows(table)
                .into_iter()
                .map(|row| (row.id, row))
                .collect()
        })
        .unwrap_or_default()
}

/// The analysis files in a device analysis directory (`/PIONEER/USBANLZ/…`).
pub fn read_analysis(destination: &Path, anlz_dir: &str) -> Vec<AnalysisFile> {
    if anlz_dir.is_empty() {
        return Vec::new();
    }
    let dir = destination.join(anlz_dir.trim_start_matches('/'));
    crate::anlz::EXTENSIONS
        .iter()
        .filter_map(|extension| {
            let bytes = std::fs::read(dir.join(format!("ANLZ0000.{extension}"))).ok()?;
            rbl_anlz::parse(&bytes).ok()?;
            Some(((*extension).to_owned(), bytes))
        })
        .collect()
}

/// rbx-cli's own record on the device, beside rbl-export's manifest: which
/// cache entry each track's analysis came from, so analysis on the device
/// can stand in for a cache miss (another machine, a cleared cache).
///
/// Losing it costs re-analysis, never a wrong result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisIndex {
    pub version: u32,
    /// By rbl-export's track key (`#<id>` or the source path).
    pub tracks: BTreeMap<String, IndexedAnalysis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexedAnalysis {
    pub cache_key: String,
    /// The device grid is the caller's, not the analysed one.
    pub grid_override: bool,
    pub meta: AnalysisMeta,
}

const INDEX_VERSION: u32 = 1;

impl AnalysisIndex {
    fn path(destination: &Path, root_name: &str) -> PathBuf {
        destination
            .join(root_name)
            .join("rbx-cli")
            .join("analysis.json")
    }

    pub fn load(destination: &Path, root_name: &str) -> Self {
        std::fs::read(Self::path(destination, root_name))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|index| index.version == INDEX_VERSION)
            .unwrap_or_default()
    }

    pub fn save(&self, destination: &Path, root_name: &str) -> std::io::Result<()> {
        let path = Self::path(destination, root_name);
        if let Some(parent) = path.parent() {
            rbl_core::durable::create_dir_all(parent)?;
        }
        let mut index = self.clone();
        index.version = INDEX_VERSION;
        let bytes = serde_json::to_vec(&index).map_err(std::io::Error::other)?;
        rbl_core::durable::write(&path, &bytes)
    }
}

/// The ways `destination` may be spelled in the OS's mount list: as
/// given (absolute) and canonical (symlinks resolved; on Windows that adds
/// a `\\?\` prefix the mount list does not have).
fn spellings(destination: &Path) -> Vec<PathBuf> {
    let mut out = vec![std::path::absolute(destination).unwrap_or_else(|_| destination.to_owned())];
    if let Ok(canonical) = std::fs::canonicalize(destination) {
        out.push(canonical);
    }
    out
}

/// The free space of the volume holding `destination`, when the OS lists it.
pub fn free_bytes(destination: &Path) -> Option<u64> {
    let paths = spellings(destination);
    rbl_devices::list()
        .into_iter()
        .filter(|d| d.total_bytes > 0 && paths.iter().any(|p| p.starts_with(&d.mount_point)))
        .max_by_key(|d| d.mount_point.as_os_str().len())
        .map(|d| d.free_bytes)
}

/// The listed volume mounted exactly at `destination`.
pub fn volume_at(destination: &Path) -> Option<rbl_devices::Device> {
    let paths = spellings(destination);
    rbl_devices::list()
        .into_iter()
        .find(|d| paths.contains(&d.mount_point))
}
