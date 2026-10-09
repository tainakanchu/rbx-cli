// SPDX-License-Identifier: GPL-2.0-or-later
//! The `usb export` request document, and its validation.
//!
//! Everything a track carries is optional except its `path`. Unknown fields
//! are ignored so a newer caller can talk to an older CLI (check
//! `rbx-cli version --json` capabilities for features).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CliError, CliResult};
use crate::protocol::{ErrorCode, PROTOCOL_VERSION};

/// A complete description of what the device should hold.
///
/// An export is a sync: tracks and playlists written by an earlier rbx-cli or
/// rbxport export that are absent from this request are removed; content
/// written by other software (e.g. rekordbox) is kept.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    /// The protocol version the caller was written against. Refused when it
    /// is newer than this CLI's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    /// The device root (mount point), e.g. `/Volumes/DJ STICK` or `E:\`.
    /// `--to` overrides it. Must be an existing directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<PathBuf>,
    #[serde(default)]
    pub options: ExportOptions,
    /// Every track the device should hold, in or out of playlists.
    pub tracks: Vec<TrackInput>,
    /// The playlist tree, in display order.
    #[serde(default)]
    pub playlists: Vec<PlaylistInput>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportOptions {
    /// When to generate analysis (beat grid, waveforms, key) with rbxport's analyser.
    pub analyze: AnalyzeMode,
    /// Analyser settings (rbxport's Analysis Setting dialog).
    pub analysis: AnalysisSettings,
    /// Fill metadata the request leaves out from the file's own tags.
    pub read_tags: bool,
    /// Use the file's embedded cover when `artwork` is not given.
    pub embedded_artwork: bool,
    /// Convert audio a CDJ cannot play natively to this format (rbxport's
    /// compatibility conversion). Off when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub convert: Option<ConvertFormat>,
    /// `PIONEER` or `.PIONEER` on a device holding no library yet. `auto`
    /// picks the hidden root on HFS+ volumes, as rekordbox does. A device
    /// that already has a library keeps its root.
    pub root: RootPreference,
    /// Write even while rekordbox is running (it may write the same device).
    pub allow_rekordbox_running: bool,
    /// On an analysis-cache miss, reuse the analysis an earlier rbx-cli
    /// export left on the device for the same, unchanged audio.
    pub reuse_device_analysis: bool,
    /// Remove tracks an earlier export put on the device that this request
    /// no longer lists. With `false` they stay on the device, outside any
    /// playlist. Playlists absent from the request are always removed.
    pub prune: bool,
    /// The device name players show (`exportLibrary.db`). Absent keeps the
    /// device's current name (empty on a fresh device).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    /// What to do with a track whose cues or beat grid changed on the
    /// device since the last sync (e.g. cues saved on a player) when the
    /// request would replace them.
    pub on_device_changes: OnDeviceChanges,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            analyze: AnalyzeMode::Missing,
            analysis: AnalysisSettings::default(),
            read_tags: true,
            embedded_artwork: true,
            convert: None,
            root: RootPreference::Auto,
            allow_rekordbox_running: false,
            reuse_device_analysis: true,
            prune: true,
            device_name: None,
            on_device_changes: OnDeviceChanges::Fail,
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum AnalyzeMode {
    /// Use `analysisPath` when given; otherwise the analysis cache, then
    /// analysis left on the device, then generate.
    #[default]
    Missing,
    /// Ignore `analysisPath` and on-device analysis; take the analysis cache
    /// (identical output) or generate.
    Always,
    /// Never generate: tracks without `analysisPath` are exported without
    /// analysis (a player shows no waveform or grid for them).
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct AnalysisSettings {
    /// Lowest tempo the detector may choose (40–300).
    pub min_bpm: f64,
    /// Highest tempo the detector may choose (40–300, above `minBpm`).
    pub max_bpm: f64,
    /// Place beats on kick attacks (rbxport's "high precision"); otherwise on
    /// the onset envelope.
    pub high_precision: bool,
    /// Use the detected key when the track has no `key`.
    pub detect_key: bool,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            min_bpm: 70.0,
            max_bpm: 180.0,
            high_precision: true,
            detect_key: true,
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum ConvertFormat {
    Wav,
    Aiff,
    Mp3,
}

/// `options.onDeviceChanges`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum OnDeviceChanges {
    /// Refuse the export with `conflict` (`cues_or_grid_changed_on_device`).
    #[default]
    Fail,
    /// For each track whose analysis files changed on the device since the
    /// last sync, keep the device's cue lists and beat grid (ignoring the
    /// request's `cues` and `beatGrid` for it) and apply everything else.
    KeepDevice,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum RootPreference {
    #[default]
    Auto,
    Standard,
    Hidden,
}

/// One track.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackInput {
    /// The audio file. The only required field.
    pub path: PathBuf,
    /// A request-local name playlists can use to refer to this track.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// The caller's stable, non-zero id for this track (1 ..= 2^63-1). With
    /// it, a track keeps its device identity when its file moves; without
    /// it, the file path is the identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Musical key as the player shows it, e.g. `Am`, `F#m`, `8A`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Tempo in BPM (two decimals are kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f64>,
    /// Stars, 0–5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<u8>,
    /// Colour label: 0 none, 1 pink, 2 red, 3 orange, 4 yellow, 5 green,
    /// 6 aqua, 7 blue, 8 purple.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    /// `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    /// `YYYY-MM-DD`. Defaults to the date already on the device, else today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_added: Option<String>,
    /// `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_sec: Option<u32>,
    /// kbit/s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u32>,
    /// Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u16>,
    /// The size the databases record; defaults to the file's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
    /// The player's Hot Cue Auto Load flag for this track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_cue_auto_load: Option<bool>,
    /// A JPEG or PNG cover. Resized to the 80×80 and 240×240 JPEGs a player reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork: Option<PathBuf>,
    /// Existing rekordbox analysis: the `ANLZ0000.DAT`; `.EXT` and `.2EX`
    /// beside it are used when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis_path: Option<PathBuf>,
    /// A beat grid replacing the analysed (or supplied) one. Only the grid
    /// section of the analysis is replaced; waveforms are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat_grid: Option<BeatGridInput>,
    /// Hot cues, memory cues and loops. Absent keeps the cues the device
    /// already holds for this track (including ones a player saved); `[]`
    /// clears them; a list replaces them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cues: Option<Vec<CueInput>>,
}

/// A caller-supplied beat grid: either every beat, or tempo anchors the
/// grid is generated from. Exactly one of the two.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BeatGridInput {
    /// Every beat, in time order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beats: Option<Vec<BeatInput>>,
    /// Tempo anchors, in time order. Each starts a constant-tempo stretch
    /// that runs to the next anchor (or the end of the track); beats are
    /// also extended back from the first anchor towards 0 ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchors: Option<Vec<BeatInput>>,
}

/// One beat of a grid, or one tempo anchor.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BeatInput {
    /// Milliseconds from the start of the audio (fractions allowed; the
    /// device format keeps whole milliseconds).
    pub time_ms: f64,
    /// Tempo from this beat on, in BPM (0 < bpm ≤ 655.35).
    pub bpm: f64,
    /// Position in the bar, 1–4 (1 = downbeat). Beats: defaults to one
    /// after the previous beat's (the first defaults to 1). Anchors: the
    /// anchor's own position, default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beat_number: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CueKind {
    /// A memory cue (or memory loop).
    Memory,
    /// A hot cue (or hot loop) in slot `A`–`H` (`I`–`P` on players that have them).
    Hot,
}

/// A cue point or loop.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CueInput {
    #[serde(rename = "type")]
    pub kind: CueKind,
    /// Hot cue slot letter `A`–`P`; required for hot cues, refused for memory cues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    /// Position in milliseconds (rounded to whole milliseconds).
    pub time_ms: f64,
    /// Loop end in milliseconds; makes the cue a loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_end_ms: Option<f64>,
    /// Memory cue colour, as the track `color` (0 none … 8 purple).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
    /// Hot cue colour: index into the players' hot cue palette (0 = the
    /// slot's default colour, 1–64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_index: Option<u8>,
    /// The cue's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// The loop is active (engaged on load).
    #[serde(default)]
    pub active_loop: bool,
    /// Beat-loop length as a fraction of beats (e.g. 1/2): numerator.
    /// Informational; `loopEndMs` sets the length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_numerator: Option<u16>,
    /// Beat-loop length denominator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_denominator: Option<u16>,
}

/// A playlist or a folder.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistInput {
    pub name: String,
    /// The caller's stable, non-zero id (1 ..= 2^63-1). Without it the
    /// node's path in the tree (folder names + name) is its identity, so a
    /// rename is a remove and an add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    /// A folder holds `children` and no tracks.
    #[serde(default)]
    pub folder: bool,
    /// Tracks, in order: a track's `ref`, or its zero-based index in `tracks`.
    #[serde(default)]
    pub tracks: Vec<TrackRef>,
    /// A folder's contents, in order.
    #[serde(default)]
    pub children: Vec<PlaylistInput>,
}

/// A reference to a track of the request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum TrackRef {
    Index(usize),
    Ref(String),
}

/// A playlist node flattened in display order, with references resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatPlaylist {
    pub id: u64,
    pub parent_id: u64,
    pub name: String,
    pub folder: bool,
    pub track_indices: Vec<usize>,
    /// Folder names from the root, then this node's name.
    pub path: Vec<String>,
}

/// Ids above this are reserved by rbxport for device-only playlists.
pub const MAX_ID: u64 = (1 << 63) - 1;

/// Reads a request from a file, or stdin for `-`. Stdin is read up to the
/// end of the first JSON document only, so the stream can carry control
/// lines afterwards; the remaining reader is returned for that.
pub fn read(input: &str) -> CliResult<(ExportRequest, Option<std::io::BufReader<std::io::Stdin>>)> {
    if input == "-" {
        let mut reader = std::io::BufReader::new(std::io::stdin());
        let request = {
            let mut stream =
                serde_json::Deserializer::from_reader(&mut reader).into_iter::<ExportRequest>();
            match stream.next() {
                Some(Ok(request)) => request,
                Some(Err(e)) => {
                    return Err(CliError::invalid(format!("invalid request on stdin: {e}")))
                }
                None => return Err(CliError::invalid("no request on stdin")),
            }
        };
        return Ok((request, Some(reader)));
    }
    let mut text = String::new();
    std::fs::File::open(input)
        .and_then(|mut f| f.read_to_string(&mut text))
        .map_err(|e| CliError::io(&format!("cannot read request {input}"), &e))?;
    Ok((parse(&text)?, None))
}

/// Parses a request document.
pub fn parse(text: &str) -> CliResult<ExportRequest> {
    serde_json::from_str(text).map_err(|e| CliError::invalid(format!("invalid request: {e}")))
}

impl ExportRequest {
    /// Checks everything that can be checked without touching the disk, and
    /// flattens the playlist tree.
    pub fn validate(&self) -> CliResult<Vec<FlatPlaylist>> {
        if let Some(protocol) = self.protocol {
            if protocol > PROTOCOL_VERSION {
                return Err(CliError::new(
                    ErrorCode::Unsupported,
                    format!("request is for protocol {protocol}; this rbx-cli speaks {PROTOCOL_VERSION}"),
                ));
            }
        }
        self.options.validate()?;
        let mut refs: BTreeMap<&str, usize> = BTreeMap::new();
        let mut ids = BTreeSet::new();
        for (index, track) in self.tracks.iter().enumerate() {
            let at = |message: String| CliError::invalid(format!("tracks[{index}]: {message}"));
            if track.path.as_os_str().is_empty() {
                return Err(at("`path` is empty".into()));
            }
            if let Some(reference) = &track.reference {
                if refs.insert(reference.as_str(), index).is_some() {
                    return Err(at(format!("duplicate ref {reference:?}")));
                }
            }
            if let Some(id) = track.id {
                if id == 0 || id > MAX_ID {
                    return Err(at(format!("`id` {id} is out of range 1..=2^63-1")));
                }
                if !ids.insert(id) {
                    return Err(at(format!("duplicate id {id}")));
                }
            }
            if let Some(bpm) = track.bpm {
                if !bpm.is_finite() || !(0.0..=999.0).contains(&bpm) {
                    return Err(at(format!("`bpm` {bpm} is out of range")));
                }
            }
            if track.rating.is_some_and(|r| r > 5) {
                return Err(at("`rating` must be 0–5".into()));
            }
            if track.color.is_some_and(|c| c > 8) {
                return Err(at("`color` must be 0–8".into()));
            }
            if let Some(grid) = &track.beat_grid {
                validate_grid(grid).map_err(|e| at(format!("beatGrid: {e}")))?;
            }
            if let Some(cues) = &track.cues {
                for (i, cue) in cues.iter().enumerate() {
                    validate_cue(cue).map_err(|e| at(format!("cues[{i}]: {e}")))?;
                }
            }
        }
        let mut flat = Vec::new();
        let mut playlist_ids = BTreeSet::new();
        flatten(
            &self.playlists,
            0,
            &[],
            self.tracks.len(),
            &refs,
            &mut playlist_ids,
            &mut flat,
        )?;
        Ok(flat)
    }
}

impl ExportOptions {
    fn validate(&self) -> CliResult<()> {
        let a = &self.analysis;
        if !a.min_bpm.is_finite()
            || !a.max_bpm.is_finite()
            || a.min_bpm < 40.0
            || a.max_bpm > 300.0
            || a.min_bpm >= a.max_bpm
        {
            return Err(CliError::invalid(
                "options.analysis: choose a BPM range between 40 and 300 with minBpm < maxBpm",
            ));
        }
        Ok(())
    }
}

/// The highest tempo the device grid format holds (BPM × 100 in a `u16`).
pub const MAX_GRID_BPM: f64 = 655.35;
/// Positions beyond this are refused (about 49 days; `u32` milliseconds).
const MAX_MS: f64 = 4_000_000_000.0;

fn valid_ms(ms: f64) -> bool {
    ms.is_finite() && (0.0..MAX_MS).contains(&ms)
}

fn validate_grid(grid: &BeatGridInput) -> Result<(), String> {
    let (list, name) = match (&grid.beats, &grid.anchors) {
        (Some(beats), None) => (beats, "beats"),
        (None, Some(anchors)) => (anchors, "anchors"),
        _ => return Err("give exactly one of `beats` or `anchors`".into()),
    };
    if list.is_empty() {
        return Err(format!("`{name}` is empty"));
    }
    for (i, beat) in list.iter().enumerate() {
        if !valid_ms(beat.time_ms) {
            return Err(format!("{name}[{i}].timeMs is out of range"));
        }
        if !beat.bpm.is_finite() || beat.bpm <= 0.0 || beat.bpm > MAX_GRID_BPM {
            return Err(format!(
                "{name}[{i}].bpm must be above 0 and at most {MAX_GRID_BPM}"
            ));
        }
        if beat.beat_number.is_some_and(|n| !(1..=4).contains(&n)) {
            return Err(format!("{name}[{i}].beatNumber must be 1–4"));
        }
        if i > 0 && list[i - 1].time_ms.round() >= beat.time_ms.round() {
            return Err(format!("{name}[{i}] is not after the one before it"));
        }
    }
    Ok(())
}

fn validate_cue(cue: &CueInput) -> Result<(), String> {
    cue_kind(cue)?;
    if !valid_ms(cue.time_ms) {
        return Err("`timeMs` is out of range".into());
    }
    if cue.color.is_some_and(|c| c > 8) {
        return Err("`color` must be 0–8".into());
    }
    if cue.color_index.is_some_and(|c| c > 64) {
        return Err("`colorIndex` must be 0–64".into());
    }
    if let Some(end) = cue.loop_end_ms {
        if !valid_ms(end) || end.round() <= cue.time_ms.round() {
            return Err("`loopEndMs` must be after `timeMs`".into());
        }
    }
    if cue.loop_numerator.is_some() != cue.loop_denominator.is_some()
        || cue.loop_denominator == Some(0)
    {
        return Err(
            "give both `loopNumerator` and a non-zero `loopDenominator`, or neither".into(),
        );
    }
    if (cue.active_loop || cue.loop_numerator.is_some()) && cue.loop_end_ms.is_none() {
        return Err("loop fields need `loopEndMs`".into());
    }
    Ok(())
}

/// rbxport's cue kind: memory 0, hot A–C 1–3, D–P 5–17.
pub fn cue_kind(cue: &CueInput) -> Result<u8, String> {
    match cue.kind {
        CueKind::Memory => {
            if cue.slot.is_some() {
                return Err("memory cues take no `slot`".into());
            }
            Ok(0)
        }
        CueKind::Hot => {
            let slot = cue.slot.as_deref().ok_or("hot cues need a `slot` (A–P)")?;
            let mut chars = slot.chars();
            let (Some(letter), None) = (chars.next(), chars.next()) else {
                return Err(format!("`slot` {slot:?} must be one letter A–P"));
            };
            let letter = letter.to_ascii_uppercase();
            if !('A'..='P').contains(&letter) {
                return Err(format!("`slot` {slot:?} must be one letter A–P"));
            }
            let index =
                u8::try_from(u32::from(letter) - u32::from('A')).map_err(|e| e.to_string())?;
            Ok(if index < 3 { index + 1 } else { index + 2 })
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn flatten(
    nodes: &[PlaylistInput],
    parent_id: u64,
    parent_path: &[String],
    track_count: usize,
    refs: &BTreeMap<&str, usize>,
    ids: &mut BTreeSet<u64>,
    out: &mut Vec<FlatPlaylist>,
) -> CliResult<()> {
    for node in nodes {
        let mut path = parent_path.to_vec();
        path.push(node.name.clone());
        let at = |message: String| {
            CliError::invalid(format!("playlist {:?}: {message}", path.join(" / ")))
        };
        if node.name.trim().is_empty() {
            return Err(at("`name` is empty".into()));
        }
        if node.folder && !node.tracks.is_empty() {
            return Err(at("a folder cannot hold tracks".into()));
        }
        if !node.folder && !node.children.is_empty() {
            return Err(at("only a folder (`folder: true`) can have children".into()));
        }
        let id = match node.id {
            Some(id) if id == 0 || id > MAX_ID => {
                return Err(at(format!("`id` {id} is out of range 1..=2^63-1")))
            }
            Some(id) => id,
            None => derived_playlist_id(&path, node.folder, ids),
        };
        if !ids.insert(id) {
            return Err(at(format!("duplicate playlist id {id}")));
        }
        let mut track_indices = Vec::with_capacity(node.tracks.len());
        for reference in &node.tracks {
            let index = match reference {
                TrackRef::Index(i) if *i < track_count => *i,
                TrackRef::Index(i) => return Err(at(format!("track index {i} is out of range"))),
                TrackRef::Ref(r) => *refs
                    .get(r.as_str())
                    .ok_or_else(|| at(format!("no track has ref {r:?}")))?,
            };
            track_indices.push(index);
        }
        out.push(FlatPlaylist {
            id,
            parent_id,
            name: node.name.clone(),
            folder: node.folder,
            track_indices,
            path: path.clone(),
        });
        flatten(&node.children, id, &path, track_count, refs, ids, out)?;
    }
    Ok(())
}

/// A stable id from the node's path, so the same tree maps to the same ids
/// on every run. 53 bits, so JavaScript callers can hold it exactly; a
/// clash (two same-named siblings) takes the next free value.
fn derived_playlist_id(path: &[String], folder: bool, taken: &BTreeSet<u64>) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            hash = (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    feed(if folder { b"folder" } else { b"playlist" });
    for part in path {
        feed(&[0]);
        feed(part.as_bytes());
    }
    let mut id = (hash & ((1 << 53) - 1)).max(1);
    while taken.contains(&id) {
        id = if id >= (1 << 53) - 1 { 1 } else { id + 1 };
    }
    id
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const FULL: &str = r#"{
      "protocol": 1,
      "destination": "/Volumes/STICK",
      "options": { "analyze": "never", "convert": "aiff", "root": "hidden", "prune": false, "deviceName": "STICK",
                   "analysis": { "minBpm": 90, "maxBpm": 160 } },
      "tracks": [
        { "path": "/music/a.mp3", "ref": "a", "id": 7, "title": "A", "bpm": 128.5, "rating": 4,
          "color": 3, "cues": [ { "type": "hot", "slot": "B", "timeMs": 1000, "colorIndex": 5 },
                                 { "type": "memory", "timeMs": 0, "loopEndMs": 4000, "color": 2 } ],
          "beatGrid": { "beats": [ { "timeMs": 10, "beatNumber": 1, "bpm": 128 }, { "timeMs": 478.75, "bpm": 128 } ] },
          "someFutureField": true },
        { "path": "/music/b.flac" }
      ],
      "playlists": [
        { "name": "Crates", "folder": true, "children": [
          { "name": "Peak", "tracks": ["a", 1] } ] },
        { "name": "Loose", "id": 99, "tracks": [1] }
      ]
    }"#;

    #[test]
    fn a_full_request_parses_and_flattens() {
        let request = parse(FULL).unwrap();
        assert_eq!(
            request.destination.as_deref(),
            Some(std::path::Path::new("/Volumes/STICK"))
        );
        assert_eq!(request.options.analyze, AnalyzeMode::Never);
        assert_eq!(request.options.convert, Some(ConvertFormat::Aiff));
        assert_eq!(request.options.root, RootPreference::Hidden);
        assert!((request.options.analysis.min_bpm - 90.0).abs() < f64::EPSILON);
        assert!(
            request.options.analysis.detect_key,
            "unspecified settings keep defaults"
        );
        assert!(request.options.read_tags);
        assert_eq!(request.tracks.len(), 2);
        assert_eq!(request.tracks[0].reference.as_deref(), Some("a"));
        assert_eq!(request.tracks[0].cues.as_ref().unwrap().len(), 2);
        assert!(request.tracks[1].title.is_none());
        assert!(
            request.tracks[1].cues.is_none(),
            "absent cues stay distinguishable from []"
        );
        assert!(!request.options.prune);
        assert_eq!(request.options.device_name.as_deref(), Some("STICK"));
        let grid = request.tracks[0].beat_grid.as_ref().unwrap();
        assert_eq!(grid.beats.as_ref().unwrap()[1].beat_number, None);
        assert!((grid.beats.as_ref().unwrap()[1].time_ms - 478.75).abs() < 1e-9);

        let flat = request.validate().unwrap();
        assert_eq!(flat.len(), 3);
        assert!(flat[0].folder);
        assert_eq!(flat[1].parent_id, flat[0].id);
        assert_eq!(flat[1].track_indices, vec![0, 1]);
        assert_eq!(flat[1].path, vec!["Crates".to_owned(), "Peak".to_owned()]);
        assert_eq!(flat[2].id, 99);
        assert_eq!(flat[2].parent_id, 0);
    }

    #[test]
    fn minimal_request_takes_defaults() {
        let request = parse(r#"{"tracks":[{"path":"x.wav"}]}"#).unwrap();
        assert_eq!(request.options.analyze, AnalyzeMode::Missing);
        assert!(request.options.reuse_device_analysis);
        assert!(request.options.prune);
        assert!(request.options.embedded_artwork);
        assert!(request.playlists.is_empty());
        let cleared = parse(r#"{"tracks":[{"path":"x.wav","cues":[]}]}"#).unwrap();
        assert_eq!(cleared.tracks[0].cues.as_deref(), Some(&[][..]));
        assert!(request.validate().unwrap().is_empty());
    }

    #[test]
    fn derived_ids_are_stable_and_distinct() {
        let a = parse(FULL).unwrap().validate().unwrap();
        let b = parse(FULL).unwrap().validate().unwrap();
        assert_eq!(a, b);
        assert_ne!(a[0].id, a[1].id);
        assert!(a.iter().all(|p| p.id > 0 && p.id < (1 << 53) || p.id == 99));
        let twins = parse(r#"{"tracks":[],"playlists":[{"name":"X"},{"name":"X"}]}"#)
            .unwrap()
            .validate()
            .unwrap();
        assert_ne!(twins[0].id, twins[1].id);
    }

    #[test]
    fn hot_cue_slots_map_to_rbxport_kinds() {
        let cue = |slot: &str| CueInput {
            kind: CueKind::Hot,
            slot: Some(slot.into()),
            time_ms: 0.0,
            loop_end_ms: None,
            color: None,
            color_index: None,
            comment: None,
            active_loop: false,
            loop_numerator: None,
            loop_denominator: None,
        };
        assert_eq!(cue_kind(&cue("A")), Ok(1));
        assert_eq!(cue_kind(&cue("c")), Ok(3));
        assert_eq!(cue_kind(&cue("D")), Ok(5));
        assert_eq!(cue_kind(&cue("P")), Ok(17));
        assert!(cue_kind(&cue("Q")).is_err());
        assert!(cue_kind(&cue("AB")).is_err());
        let memory = CueInput {
            kind: CueKind::Memory,
            slot: None,
            ..cue("A")
        };
        assert_eq!(cue_kind(&memory), Ok(0));
    }

    #[test]
    fn invalid_requests_are_refused_with_a_reason() {
        for (text, needle) in [
            (r#"{}"#, "tracks"),
            (r#"{"tracks":[{"title":"no path"}]}"#, "path"),
            (
                r#"{"tracks":[{"path":"a","ref":"x"},{"path":"b","ref":"x"}]}"#,
                "duplicate ref",
            ),
            (r#"{"tracks":[{"path":"a","id":0}]}"#, "out of range"),
            (r#"{"tracks":[{"path":"a","rating":6}]}"#, "rating"),
            (r#"{"tracks":[{"path":"a","color":9}]}"#, "color"),
            (
                r#"{"tracks":[{"path":"a","cues":[{"type":"hot","timeMs":1}]}]}"#,
                "slot",
            ),
            (
                r#"{"tracks":[{"path":"a","cues":[{"type":"memory","timeMs":5,"loopEndMs":5}]}]}"#,
                "loopEndMs",
            ),
            (
                r#"{"tracks":[{"path":"a"}],"playlists":[{"name":"P","tracks":["nope"]}]}"#,
                "no track has ref",
            ),
            (
                r#"{"tracks":[{"path":"a"}],"playlists":[{"name":"P","tracks":[3]}]}"#,
                "out of range",
            ),
            (
                r#"{"tracks":[{"path":"a"}],"playlists":[{"name":"F","folder":true,"tracks":[0]}]}"#,
                "folder cannot",
            ),
            (
                r#"{"tracks":[],"playlists":[{"name":"P","children":[{"name":"C"}]}]}"#,
                "only a folder",
            ),
            (
                r#"{"tracks":[],"playlists":[{"name":"A","id":5},{"name":"B","id":5}]}"#,
                "duplicate playlist id",
            ),
            (
                r#"{"tracks":[],"options":{"analysis":{"minBpm":200,"maxBpm":100}}}"#,
                "BPM range",
            ),
            (r#"{"protocol":999,"tracks":[]}"#, "protocol"),
            (
                r#"{"tracks":[{"path":"a","beatGrid":{"beats":[{"timeMs":5,"beatNumber":5,"bpm":120}]}}]}"#,
                "beatNumber",
            ),
            (
                r#"{"tracks":[{"path":"a","beatGrid":{"beats":[{"timeMs":5,"bpm":0}]}}]}"#,
                "bpm",
            ),
            (
                r#"{"tracks":[{"path":"a","beatGrid":{"beats":[{"timeMs":5,"bpm":700}]}}]}"#,
                "bpm",
            ),
            (
                r#"{"tracks":[{"path":"a","beatGrid":{"beats":[{"timeMs":5,"bpm":120},{"timeMs":5,"bpm":120}]}}]}"#,
                "not after",
            ),
            (
                r#"{"tracks":[{"path":"a","beatGrid":{"beats":[{"timeMs":-1,"bpm":120}]}}]}"#,
                "timeMs",
            ),
            (r#"{"tracks":[{"path":"a","beatGrid":{}}]}"#, "exactly one"),
            (
                r#"{"tracks":[{"path":"a","beatGrid":{"beats":[],"anchors":[]}}]}"#,
                "exactly one",
            ),
            (
                r#"{"tracks":[{"path":"a","beatGrid":{"anchors":[]}}]}"#,
                "empty",
            ),
            (
                r#"{"tracks":[{"path":"a","cues":[{"type":"memory","slot":"A","timeMs":1}]}]}"#,
                "no `slot`",
            ),
            (
                r#"{"tracks":[{"path":"a","cues":[{"type":"hot","slot":"A","timeMs":1,"colorIndex":65}]}]}"#,
                "colorIndex",
            ),
            (
                r#"{"tracks":[{"path":"a","cues":[{"type":"hot","slot":"A","timeMs":1,"activeLoop":true}]}]}"#,
                "loopEndMs",
            ),
            (
                r#"{"tracks":[{"path":"a","cues":[{"type":"hot","slot":"A","timeMs":1,"loopEndMs":9,"loopNumerator":1}]}]}"#,
                "loopDenominator",
            ),
            (
                r#"{"tracks":[{"path":"a","cues":[{"type":"memory","timeMs":"x"}]}]}"#,
                "invalid",
            ),
        ] {
            let error = parse(text)
                .and_then(|r| r.validate().map(|_| ()))
                .expect_err(text);
            assert!(error.message.contains(needle), "{text}: {}", error.message);
        }
        let error = parse(r#"{"protocol":999,"tracks":[]}"#)
            .unwrap()
            .validate()
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Unsupported);
    }
}
