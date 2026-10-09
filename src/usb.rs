// SPDX-License-Identifier: GPL-2.0-or-later
//! `usb inspect` and `usb verify`: reading an existing export back.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use crate::error::{CliError, CliResult};
use crate::protocol::ErrorCode;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Databases {
    /// `export.pdb` (Device Library, read by older players).
    pub device_library: bool,
    /// `exportLibrary.db` (Device Library Plus / OneLibrary).
    pub one_library: bool,
}

/// A cue as read from a device analysis file, in the request's shape.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InspectCue {
    /// `memory` or `hot`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    pub time_ms: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_end_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_index: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InspectTrack {
    /// The id players see.
    pub id: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub label: String,
    pub key: String,
    pub bpm: f64,
    pub rating: u32,
    pub color: u32,
    pub comment: String,
    /// The audio's path from the device root.
    pub path: String,
    /// The `.DAT` path from the device root; empty when none.
    pub analysis_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_sec: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_added: Option<String>,
    /// Where an rbx-cli/rbxport export copied it from (its manifest).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The caller's track `id` of that export, when it had one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<u64>,
    /// With `--cues`: the cues in the device analysis (including any a
    /// player saved there).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cues: Option<Vec<InspectCue>>,
    /// With `--cues`: beats in the device grid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beats: Option<usize>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InspectPlaylist {
    pub id: u32,
    /// 0 for the root.
    pub parent_id: u32,
    pub name: String,
    pub folder: bool,
    /// Device track ids, in order.
    pub track_ids: Vec<u32>,
}

/// `usb inspect` result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InspectResult {
    pub destination: String,
    /// `PIONEER` or `.PIONEER`; absent on a device with no library.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    pub databases: Databases,
    /// Written by rbx-cli or rbxport (a sync manifest is present).
    pub ours: bool,
    /// When that export ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub written: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    pub track_count: usize,
    pub playlist_count: usize,
    /// Play-history sessions players recorded on the device.
    pub history_count: usize,
    /// Absent with `--summary`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<InspectTrack>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playlists: Option<Vec<InspectPlaylist>>,
}

fn check_root(root: &Path) -> CliResult<PathBuf> {
    if !root.is_dir() {
        return Err(CliError::not_found(format!(
            "not a directory: {}",
            root.display()
        )));
    }
    Ok(std::path::absolute(root).unwrap_or_else(|_| root.to_owned()))
}

fn cues_of(destination: &Path, analysis_path: &str) -> (Vec<InspectCue>, Option<usize>) {
    if analysis_path.is_empty() {
        return (Vec::new(), None);
    }
    let dat = destination.join(analysis_path.trim_start_matches('/'));
    let beats = rbl_anlz::Anlz::read(&dat)
        .ok()
        .and_then(|a| a.beat_grid())
        .map(|g| g.len());
    // The extended list (`.EXT`'s PCO2) holds every cue with colour and name.
    let entries = rbl_anlz::Anlz::read(&dat.with_extension("EXT"))
        .map(|a| a.cue_entries())
        .unwrap_or_default();
    let cues = entries
        .into_iter()
        .map(|e| {
            let hot = e.hot_cue != 0;
            InspectCue {
                kind: if hot { "hot" } else { "memory" },
                slot: hot
                    .then(|| char::from_u32(u32::from('A') + e.hot_cue - 1).map(String::from))
                    .flatten(),
                time_ms: e.time_ms,
                loop_end_ms: (e.kind == 2 && e.loop_time_ms != u32::MAX).then_some(e.loop_time_ms),
                color: (!hot && e.color_id != 0).then_some(e.color_id),
                color_index: e.color_code.filter(|c| hot && *c != 0),
                comment: e.comment.filter(|c| !c.is_empty()),
            }
        })
        .collect();
    (cues, beats)
}

pub fn inspect(root: &Path, summary: bool, with_cues: bool) -> CliResult<InspectResult> {
    let destination = check_root(root)?;
    let root_name = rbl_export::export_root_name(&destination).map_err(CliError::from)?;
    let db_dir = destination.join(root_name).join("rekordbox");
    let databases = Databases {
        device_library: db_dir.join("export.pdb").is_file(),
        one_library: db_dir.join("exportLibrary.db").is_file(),
    };
    let has_library = databases.device_library || databases.one_library;
    let snapshot = if has_library {
        rbl_export::snapshot::Snapshot::read(&destination).map_err(CliError::from)?
    } else {
        rbl_export::snapshot::Snapshot::default()
    };
    let library = snapshot.merged_library().unwrap_or_default();
    let manifest = rbl_export::Manifest::load(&destination);
    let rows = crate::device::pdb_tracks(&destination);
    let device_name = databases
        .one_library
        .then(|| {
            rbl_onelibrary::settings::StickSettings::read(&db_dir.join("exportLibrary.db")).ok()
        })
        .flatten()
        .map(|s| s.device_name);
    let tracks = (!summary).then(|| {
        library
            .tracks
            .iter()
            .map(|t| {
                let row = rows.get(&t.id);
                let source = manifest
                    .as_ref()
                    .and_then(|m| m.tracks.iter().find(|e| e.export_id == t.id));
                let (cues, beats) = if with_cues {
                    let (c, b) = cues_of(&destination, &t.analysis);
                    (Some(c), b)
                } else {
                    (None, None)
                };
                InspectTrack {
                    id: t.id,
                    title: t.title.clone(),
                    artist: t.artist.clone(),
                    album: t.album.clone(),
                    genre: t.genre.clone(),
                    label: t.label.clone(),
                    key: t.key.clone(),
                    bpm: f64::from(t.bpm) / 100.0,
                    rating: t.rating,
                    color: t.color,
                    comment: t.comment.clone(),
                    path: t.path.clone(),
                    analysis_path: t.analysis.clone(),
                    duration_sec: row.map(|r| r.duration_sec),
                    date_added: row.map(|r| r.date_added.clone()).filter(|d| !d.is_empty()),
                    source: source.map(|s| s.source.clone()),
                    source_id: source.map(|s| s.library_id).filter(|id| *id != 0),
                    cues,
                    beats,
                }
            })
            .collect()
    });
    let playlists = (!summary).then(|| {
        library
            .playlists
            .iter()
            .map(|p| InspectPlaylist {
                id: p.id,
                parent_id: p.parent,
                name: p.name.clone(),
                folder: p.folder,
                track_ids: p.tracks.clone(),
            })
            .collect()
    });
    Ok(InspectResult {
        destination: destination.to_string_lossy().into_owned(),
        root: has_library.then(|| root_name.to_owned()),
        databases,
        ours: manifest.is_some(),
        written: manifest.as_ref().map(|m| m.written.clone()),
        device_name,
        track_count: library.tracks.len(),
        playlist_count: library.playlists.len(),
        history_count: snapshot.history.len(),
        tracks,
        playlists,
    })
}

pub fn human_inspect(result: &InspectResult) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let Some(root) = &result.root else {
        return format!("{}: no rekordbox library\n", result.destination);
    };
    let _ = writeln!(s, "{} ({root})", result.destination);
    let formats: Vec<&str> = [
        result
            .databases
            .device_library
            .then_some("Device Library (export.pdb)"),
        result
            .databases
            .one_library
            .then_some("OneLibrary (exportLibrary.db)"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let _ = writeln!(s, "  databases: {}", formats.join(", "));
    if let Some(name) = result.device_name.as_ref().filter(|n| !n.is_empty()) {
        let _ = writeln!(s, "  name:      {name}");
    }
    let _ = writeln!(
        s,
        "  written:   {}",
        if result.ours {
            result.written.clone().unwrap_or_default()
        } else {
            "by other software".to_owned()
        }
    );
    let _ = writeln!(
        s,
        "  tracks: {}  playlists: {}  history: {}",
        result.track_count, result.playlist_count, result.history_count
    );
    if let Some(playlists) = &result.playlists {
        for p in playlists {
            let _ = writeln!(
                s,
                "  {} {} ({})",
                if p.folder { "[folder]" } else { "[list]  " },
                p.name,
                p.track_ids.len()
            );
        }
    }
    if let Some(tracks) = &result.tracks {
        for t in tracks {
            let _ = writeln!(
                s,
                "  {:>5}  {:6.2}  {:<4} {} - {}",
                t.id, t.bpm, t.key, t.artist, t.title
            );
        }
    }
    s
}

/// `usb verify` result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    pub destination: String,
    pub ok: bool,
    /// Both databases parsed.
    pub parsed: bool,
    pub tracks: usize,
    pub playlists: usize,
    pub playlist_entries: usize,
    pub audio_present: usize,
    pub analysis_present: usize,
    pub overview_waveforms: usize,
    pub detail_waveforms: usize,
    pub beat_grids: usize,
    pub hot_cues: usize,
    pub memory_cues: usize,
    pub missing_audio: Vec<String>,
    pub errors: Vec<String>,
}

fn verify_result(destination: &Path, r: &rbl_export::VerifyReport) -> VerifyResult {
    VerifyResult {
        destination: destination.to_string_lossy().into_owned(),
        ok: r.is_ok(),
        parsed: r.parsed,
        tracks: r.tracks,
        playlists: r.playlists,
        playlist_entries: r.playlist_entries,
        audio_present: r.audio_present,
        analysis_present: r.analysis_present,
        overview_waveforms: r.overview_waveforms,
        detail_waveforms: r.detail_waveforms,
        beat_grids: r.beat_grids,
        hot_cues: r.hot_cues,
        memory_cues: r.memory_cues,
        missing_audio: r.missing_audio.clone(),
        errors: r.errors.clone(),
    }
}

/// Error details for a failed read-back.
pub fn verify_details(r: &rbl_export::VerifyReport) -> serde_json::Value {
    serde_json::to_value(verify_result(Path::new(""), r)).unwrap_or(serde_json::Value::Null)
}

/// Reads both databases and every referenced file back with rbxport's
/// independent parser. Completes an interrupted publication first, as
/// rbxport does before reading a device.
pub fn verify(root: &Path) -> CliResult<VerifyResult> {
    let destination = check_root(root)?;
    let report = rbl_export::verify(&destination).map_err(CliError::from)?;
    let result = verify_result(&destination, &report);
    if !result.ok {
        let message = if report.parsed {
            format!(
                "{} problem(s): {}",
                result.errors.len() + result.missing_audio.len(),
                result
                    .errors
                    .first()
                    .or(result.missing_audio.first())
                    .cloned()
                    .unwrap_or_default()
            )
        } else {
            result
                .errors
                .first()
                .cloned()
                .unwrap_or_else(|| "no readable library".to_owned())
        };
        return Err(CliError::new(
            ErrorCode::VerificationFailed,
            format!("verification failed: {message}"),
        )
        .with_details(serde_json::to_value(&result).unwrap_or(serde_json::Value::Null)));
    }
    Ok(result)
}

pub fn human_verify(r: &VerifyResult) -> String {
    format!(
        "{}: OK\n  tracks {} (audio {}, analysis {}), playlists {} ({} entries)\n  grids {}, waveforms {}/{}, hot cues {}, memory cues {}\n",
        r.destination,
        r.tracks,
        r.audio_present,
        r.analysis_present,
        r.playlists,
        r.playlist_entries,
        r.beat_grids,
        r.overview_waveforms,
        r.detail_waveforms,
        r.hot_cues,
        r.memory_cues
    )
}
