// SPDX-License-Identifier: GPL-2.0-or-later
//! `usb export`: request → analysis (parallel, cached) → rbl-export → verify.
//!
//! rbl-export takes every track's analysis bytes before it starts copying,
//! so analysis cannot overlap the copy: all analysis is prepared first, in
//! parallel, then one rbl-export run copies, writes both databases,
//! verifies the staged generation and publishes it atomically.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use schemars::JsonSchema;
use serde::Serialize;

use crate::analyze::{self, AnalysisMeta};
use crate::anlz::{self, AnalysisFile};
use crate::artwork::ArtworkStore;
use crate::cache::{self, Cache, SourceStamp};
use crate::device::{self, AnalysisIndex, IndexedAnalysis};
use crate::error::{CliError, CliResult};
use crate::output;
use crate::protocol::{ErrorCode, ProgressItem};
use crate::request::{AnalyzeMode, ExportRequest, RootPreference, TrackInput};

/// Command-line settings of one export.
#[derive(Debug, Clone)]
pub struct ExportArgs {
    pub input: String,
    pub to: Option<PathBuf>,
    pub dry_run: bool,
    pub cache: Cache,
    pub jobs: usize,
    pub stdin_control: bool,
    /// The end of stdin cancels (`--cancel-on-stdin-eof`).
    pub cancel_on_stdin_eof: bool,
}

/// How many tracks rbxport analyses at once by default (its queue's slots).
pub const DEFAULT_JOBS: usize = 3;

// ---------------------------------------------------------------- results

/// Where a track's analysis came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AnalysisSource {
    /// The analysis cache.
    Cache,
    /// Generated in this run (and cached).
    Generated,
    /// Taken from the device, left there by an earlier rbx-cli export.
    Device,
    /// The caller's `analysisPath`.
    Supplied,
    /// Exported without analysis (`analyze: never`, or nothing to use).
    None,
    /// Analysis failed; exported without it (see `error`).
    Failed,
    /// `--dry-run`: would be generated.
    Generate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TrackStatus {
    /// Written to (or kept on) the device.
    Exported,
    /// Left out: the source file is missing.
    Skipped,
    /// `--dry-run`: would be exported.
    Planned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AudioPlan {
    /// The device copy is current and would be left alone (an estimate:
    /// rbl-export confirms by content when it runs).
    Reuse,
    Copy,
}

/// One requested track's outcome.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackResult {
    /// Index in the request's `tracks`.
    pub index: usize,
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    pub title: String,
    pub status: TrackStatus,
    pub analysis: AnalysisSource,
    /// Milliseconds spent decoding and analysing (generated only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_ms: Option<u64>,
    /// `--dry-run` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioPlan>,
    /// The caller's grid replaced the analysed one.
    pub grid_override: bool,
    /// The caller's cues were written (`cues` given).
    pub cues_override: bool,
    pub artwork: bool,
    /// The id players see (`export.pdb`/`exportLibrary.db`), stable across syncs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<u32>,
    /// The audio's path on the device, from its root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_path: Option<String>,
    /// The analysis directory on the device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_dir: Option<String>,
    /// Problems that did not stop the export (analysis, artwork, grid).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrackCounts {
    pub requested: usize,
    /// Tracks in the written databases (includes kept and device-only ones).
    pub exported: usize,
    pub copied: usize,
    pub reused: usize,
    pub skipped: usize,
    /// Tracks an earlier export wrote that this one removed.
    pub removed: usize,
    /// Tracks kept on the device although not requested (`prune: false`).
    pub kept: usize,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistCounts {
    /// Playlists and folders in the written databases.
    pub written: usize,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisCounts {
    pub generated: usize,
    pub cache_hits: usize,
    pub device_reuse: usize,
    pub supplied: usize,
    pub none: usize,
    pub failed: usize,
    /// Cache misses (generated + device reuse + failed).
    pub cache_misses: usize,
    pub grid_overrides: usize,
    pub cue_overrides: usize,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ByteCounts {
    pub copied: u64,
    pub reused: u64,
    /// Estimated audio bytes to copy (plan).
    pub to_copy: u64,
    /// Space available on the filesystem holding the destination (a volume
    /// root or any folder), when the OS reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Timings {
    pub plan_ms: u64,
    pub analyze_ms: u64,
    pub export_ms: u64,
    pub verify_ms: u64,
    pub total_ms: u64,
}

/// `usb export` result (`data` of the `result` line).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub destination: String,
    /// `PIONEER` or `.PIONEER`.
    pub root: String,
    pub dry_run: bool,
    pub tracks: TrackCounts,
    pub playlists: PlaylistCounts,
    pub analysis: AnalysisCounts,
    pub bytes: ByteCounts,
    pub analysis_files: usize,
    pub artwork_files: usize,
    pub pdb_bytes: usize,
    /// Both databases read back and agreed (absent for `--dry-run`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_dir: Option<String>,
    pub jobs: usize,
    pub timings: Timings,
    pub items: Vec<TrackResult>,
}

// ---------------------------------------------------------------- preparation

/// What the parallel stage works out for one track.
#[derive(Default)]
struct Prepared {
    source: PathBuf,
    stamp: Option<SourceStamp>,
    tags: Option<rbl_db::import::TrackTags>,
    analysis: Vec<AnalysisFile>,
    from: Option<AnalysisSource>,
    meta: Option<AnalysisMeta>,
    cache_key: Option<String>,
    analysis_ms: Option<u64>,
    artwork: Option<PathBuf>,
    warnings: Vec<String>,
}

/// Shared, read-only context of the parallel stage.
struct Context<'a> {
    request: &'a ExportRequest,
    destination: &'a Path,
    cache: &'a Cache,
    fingerprint: String,
    manifest: Option<&'a rbl_export::Manifest>,
    index: &'a AnalysisIndex,
    artwork: Option<&'a ArtworkStore>,
    dry_run: bool,
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
}

fn track_key(track: &TrackInput, source: &Path) -> String {
    rbl_export::track_key(track.id.unwrap_or(0), &source.to_string_lossy())
}

fn manifest_entry<'m>(
    manifest: Option<&'m rbl_export::Manifest>,
    key: &str,
) -> Option<&'m rbl_export::ManifestTrack> {
    manifest?.tracks.iter().find(|t| t.key() == key)
}

fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn prepare(ctx: &Context<'_>, index: usize) -> Prepared {
    let track = &ctx.request.tracks[index];
    let options = &ctx.request.options;
    let mut out = Prepared {
        source: absolute(&track.path),
        ..Prepared::default()
    };
    let stamp = match SourceStamp::read(&out.source) {
        Ok(stamp) => stamp,
        // A missing file is reported as skipped; anything else also says why.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            out.warnings.push(format!("source unreadable: {e}"));
            return out;
        }
    };
    let key = track_key(track, &out.source);
    if options.read_tags && !ctx.dry_run {
        out.tags = rbl_db::import::read_tags(&out.source)
            .map_err(|e| tracing::debug!(path = %out.source.display(), error = %e, "no tags read"))
            .ok();
    }

    // Analysis, cheapest source first.
    let supplied = track
        .analysis_path
        .as_ref()
        .filter(|_| options.analyze != AnalyzeMode::Always);
    if let Some(dat) = supplied {
        if ctx.dry_run {
            out.from = Some(AnalysisSource::Supplied);
        } else {
            match analyze::read_supplied(dat) {
                Ok(files) => {
                    out.analysis = files;
                    out.from = Some(AnalysisSource::Supplied);
                }
                Err(e) => {
                    out.warnings.push(format!("analysisPath: {e}"));
                    out.from = Some(AnalysisSource::Failed);
                }
            }
        }
    } else if options.analyze == AnalyzeMode::Never {
        out.from = Some(AnalysisSource::None);
    } else {
        let cache_key = cache::key(&stamp, &ctx.fingerprint);
        out.cache_key = Some(cache_key.clone());
        let device = (options.reuse_device_analysis && options.analyze == AnalyzeMode::Missing)
            .then(|| device_analysis(ctx, track, &key, &stamp, &cache_key))
            .flatten();
        if ctx.dry_run {
            out.from = Some(if ctx.cache.contains(&cache_key) {
                AnalysisSource::Cache
            } else if device.is_some() {
                AnalysisSource::Device
            } else {
                AnalysisSource::Generate
            });
        } else if let Some(hit) = ctx.cache.get(&cache_key) {
            out.analysis = hit.files;
            out.meta = Some(hit.meta);
            out.from = Some(AnalysisSource::Cache);
        } else if let Some((entry, mut files)) = device {
            match anlz::strip_cues(&mut files) {
                Ok(()) => {
                    if !entry.grid_override {
                        let generated = analyze::Generated {
                            files: files.clone(),
                            meta: entry.meta.clone(),
                        };
                        if let Err(e) = ctx.cache.put(&cache_key, &generated) {
                            tracing::warn!(error = %e, "could not store analysis in the cache");
                        }
                    }
                    out.analysis = files;
                    out.meta = Some(entry.meta);
                    out.from = Some(AnalysisSource::Device);
                }
                Err(e) => out.warnings.push(format!("device analysis unusable: {e}")),
            }
        }
        if out.from.is_none() {
            let started = Instant::now();
            match analyze::generate(&out.source, &options.analysis) {
                Ok(generated) => {
                    out.analysis_ms = Some(elapsed_ms(started));
                    if let Err(e) = ctx.cache.put(&cache_key, &generated) {
                        tracing::warn!(error = %e, "could not store analysis in the cache");
                    }
                    out.analysis = generated.files;
                    out.meta = Some(generated.meta);
                    out.from = Some(AnalysisSource::Generated);
                }
                Err(e) => {
                    out.warnings.push(format!("analysis failed: {e}"));
                    out.from = Some(AnalysisSource::Failed);
                }
            }
        }
    }

    if let Some(store) = ctx.artwork {
        let artwork = match &track.artwork {
            Some(image) => store.for_file(image).map(Some),
            None if options.embedded_artwork => store.embedded(&out.source, Some(&stamp)),
            None => Ok(None),
        };
        match artwork {
            Ok(path) => out.artwork = path,
            Err(e) => out.warnings.push(format!("artwork: {e}")),
        }
    }
    out.stamp = Some(stamp);
    out
}

/// Analysis an earlier rbx-cli export left on the device for this exact
/// source content and settings.
fn device_analysis(
    ctx: &Context<'_>,
    track: &TrackInput,
    key: &str,
    stamp: &SourceStamp,
    cache_key: &str,
) -> Option<(IndexedAnalysis, Vec<AnalysisFile>)> {
    let entry = manifest_entry(ctx.manifest, key)?;
    let indexed = ctx.index.tracks.get(key)?;
    let usable = indexed.cache_key == cache_key
        && entry.size == stamp.size
        && entry.modified == stamp.modified_ns
        && !entry.anlz_dir.is_empty()
        // A caller grid on the device hides the analysed one; usable only
        // when this request brings its own grid again.
        && (!indexed.grid_override || track.beat_grid.is_some());
    if !usable {
        return None;
    }
    let files = device::read_analysis(ctx.destination, &entry.anlz_dir);
    files
        .iter()
        .any(|(e, _)| e == "DAT")
        .then(|| (indexed.clone(), files))
}

/// Runs `prepare` over every track on `jobs` threads, reporting progress
/// from this thread as tracks finish.
fn prepare_all(ctx: &Context<'_>, jobs: usize) -> CliResult<Vec<Prepared>> {
    let total = ctx.request.tracks.len();
    let next = AtomicUsize::new(0);
    let mut prepared: Vec<Option<Prepared>> = (0..total).map(|_| None).collect();
    let (send, receive) = std::sync::mpsc::channel::<(usize, Prepared)>();
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(total).max(1) {
            let send = send.clone();
            let next = &next;
            scope.spawn(move || loop {
                if crate::cancel::requested() {
                    break;
                }
                let index = next.fetch_add(1, Ordering::SeqCst);
                if index >= total {
                    break;
                }
                if send.send((index, prepare(ctx, index))).is_err() {
                    break;
                }
            });
        }
        drop(send);
        let mut done = 0_u64;
        for (index, item) in receive {
            done += 1;
            let track = &ctx.request.tracks[index];
            output::progress(
                "analyze",
                done,
                total as u64,
                Some(progress_item(index, track, &item.source)),
            );
            prepared[index] = Some(item);
        }
    });
    if crate::cancel::requested() {
        return Err(CliError::cancelled());
    }
    prepared
        .into_iter()
        .map(|p| p.ok_or_else(|| CliError::internal("a track was not prepared")))
        .collect()
}

fn progress_item(index: usize, track: &TrackInput, source: &Path) -> ProgressItem {
    ProgressItem {
        index: Some(index),
        reference: track.reference.clone(),
        title: track.title.clone().unwrap_or_else(|| file_stem(source)),
    }
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- source tracks

fn non_empty(value: Option<&String>, fallback: Option<&str>) -> String {
    value
        .filter(|v| !v.is_empty())
        .cloned()
        .or_else(|| fallback.filter(|v| !v.is_empty()).map(str::to_owned))
        .unwrap_or_default()
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validated 0..=999 BPM"
)]
fn bpm_x100(bpm: f64) -> u32 {
    (bpm * 100.0).round().clamp(0.0, 99_900.0) as u32
}

/// The key as rekordbox spells it when recognised (`F#m`, `Eb`; Camelot
/// codes are converted), otherwise as given.
fn normalize_key(key: &str) -> String {
    rbl_core::musickey::parse(key)
        .map_or_else(|| key.trim().to_owned(), rbl_core::musickey::Key::name)
}

struct Built {
    tracks: Vec<rbl_export::SourceTrack>,
    grid_override: Vec<bool>,
    warnings: Vec<Vec<String>>,
}

#[allow(clippy::too_many_lines, reason = "one field mapping per request field")]
fn build_tracks(
    request: &ExportRequest,
    prepared: &mut [Prepared],
    destination: &Path,
    manifest: Option<&rbl_export::Manifest>,
    previous_dates: &BTreeMap<u32, String>,
) -> CliResult<Built> {
    let options = &request.options;
    let today = rbl_core::time::local_date();
    let mut built = Built {
        tracks: Vec::new(),
        grid_override: Vec::new(),
        warnings: Vec::new(),
    };
    for (index, (track, prep)) in request.tracks.iter().zip(prepared.iter_mut()).enumerate() {
        let mut warnings = std::mem::take(&mut prep.warnings);
        let tags = prep.tags.clone().unwrap_or_default();
        let meta = prep.meta.clone();
        let key = track_key(track, &prep.source);
        let previous = manifest_entry(manifest, &key);

        let mut analysis = std::mem::take(&mut prep.analysis);
        let duration_ms = track
            .duration_sec
            .map(|s| u64::from(s) * 1000)
            .or_else(|| {
                meta.as_ref()
                    .filter(|m| m.full_length)
                    .map(|m| m.duration_ms)
            })
            .or_else(|| (tags.duration_sec > 0).then(|| u64::from(tags.duration_sec) * 1000));

        // The caller's grid replaces the analysed one; waveforms stay.
        let mut grid_override = false;
        let mut grid_bpm = None;
        if let Some(grid) = &track.beat_grid {
            if analysis.iter().any(|(e, _)| e == "DAT") {
                match crate::grid::expand(grid, duration_ms) {
                    Ok(beats) if !beats.is_empty() => {
                        anlz::replace_grid(&mut analysis, &beats)
                            .map_err(|e| CliError::internal(format!("tracks[{index}]: {e}")))?;
                        grid_bpm = beats.first().map(|b| f64::from(b.tempo_x100) / 100.0);
                        grid_override = true;
                    }
                    Ok(_) => warnings.push("beatGrid: no beats fall inside the track".into()),
                    Err(e) => {
                        return Err(CliError::invalid(format!("tracks[{index}].beatGrid: {e}")))
                    }
                }
            } else {
                warnings.push("beatGrid ignored: the track has no analysis to put it in".into());
            }
        }

        // Cues: given → rbl-export writes them; absent → keep what the
        // device holds for this track, in the analysis files as in
        // exportLibrary.db (which rbl-export preserves on its own).
        let cues = match &track.cues {
            Some(cues) => Some(
                anlz::export_cues(cues)
                    .map_err(|e| CliError::invalid(format!("tracks[{index}].cues: {e}")))?,
            ),
            None => {
                if let Some(previous) = previous {
                    let on_device = device::read_analysis(destination, &previous.anlz_dir);
                    if !on_device.is_empty() {
                        anlz::carry_cues(&mut analysis, &on_device)
                            .map_err(|e| CliError::internal(format!("tracks[{index}]: {e}")))?;
                    }
                }
                None
            }
        };

        let title = non_empty(track.title.as_ref(), Some(&tags.title));
        let title = if title.is_empty() {
            file_stem(&prep.source)
        } else {
            title
        };
        let bpm = track
            .bpm
            .or(grid_bpm)
            .or_else(|| meta.as_ref().map(|m| m.bpm))
            .unwrap_or(0.0);
        let key_name = match &track.key {
            Some(k) if !k.trim().is_empty() => normalize_key(k),
            _ if options.analysis.detect_key => meta
                .as_ref()
                .and_then(|m| m.key.clone())
                .unwrap_or_default(),
            _ => String::new(),
        };
        let date_added = track
            .date_added
            .clone()
            .or_else(|| previous.and_then(|p| previous_dates.get(&p.export_id).cloned()))
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| today.clone());
        let duration_sec = track
            .duration_sec
            .or_else(|| (tags.duration_sec > 0).then_some(tags.duration_sec))
            .or_else(|| {
                meta.as_ref()
                    .filter(|m| m.full_length)
                    .and_then(|m| u32::try_from(m.duration_ms.div_ceil(1000)).ok())
            })
            .unwrap_or(0);
        let has_analysis = !analysis.is_empty();
        built.tracks.push(rbl_export::SourceTrack {
            metadata: rbl_core::ExportMetadata {
                track_number: track.track_number.unwrap_or(u32::from(tags.track_no)),
                disc_number: track.disc_number.unwrap_or(0),
                bit_depth: track.bit_depth.unwrap_or(u16::from(tags.bit_depth)),
                play_count: track.play_count.unwrap_or(0),
                analysed: if has_analysis {
                    u32::try_from(rbl_db::write::ANALYSED_FULL).unwrap_or(105)
                } else {
                    0
                },
                hot_cue_auto_load: track.hot_cue_auto_load.unwrap_or(false),
                date_created: track.date_created.clone().unwrap_or_default(),
                isrc: track.isrc.clone().unwrap_or_default(),
            },
            cues,
            device: None,
            id: track.id.unwrap_or(0),
            source_path: prep.source.clone(),
            title,
            artist: non_empty(track.artist.as_ref(), Some(&tags.artist)),
            album: non_empty(track.album.as_ref(), Some(&tags.album)),
            genre: non_empty(track.genre.as_ref(), Some(&tags.genre)),
            label: non_empty(track.label.as_ref(), Some(&tags.label)),
            key: key_name,
            comment: non_empty(track.comment.as_ref(), Some(&tags.comment)),
            date_added,
            release_date: track.release_date.clone().unwrap_or_default(),
            bpm_x100: bpm_x100(bpm),
            duration_sec: u16::try_from(duration_sec).unwrap_or(u16::MAX),
            rating: track.rating.unwrap_or(0),
            color_id: track.color.unwrap_or(0),
            year: track.year.unwrap_or(tags.year),
            bitrate: track.bitrate.unwrap_or(tags.bitrate),
            sample_rate: track.sample_rate.unwrap_or(tags.sample_rate),
            file_size: track.file_size.unwrap_or(0),
            analysis,
            artwork: prep.artwork.clone(),
            my_tags: Vec::new(),
        });
        built.grid_override.push(grid_override);
        built.warnings.push(warnings);
    }
    Ok(built)
}

/// `prune: false`: the tracks an earlier export wrote that this request
/// does not list, rebuilt from the device so they stay, outside playlists.
fn kept_tracks(
    destination: &Path,
    manifest: &rbl_export::Manifest,
    requested: &BTreeSet<String>,
    previous_dates: &BTreeMap<u32, String>,
    store: Option<&ArtworkStore>,
) -> CliResult<Vec<rbl_export::SourceTrack>> {
    let snapshot = rbl_export::snapshot::Snapshot::read(destination).map_err(CliError::from)?;
    let library = snapshot.merged_library().unwrap_or_default();
    let rows = device::pdb_tracks(destination);
    let mut out = Vec::new();
    for entry in manifest
        .tracks
        .iter()
        .filter(|t| !requested.contains(&t.key()))
    {
        let Some(device) = library.tracks.iter().find(|t| t.id == entry.export_id) else {
            continue;
        };
        let row = rows.get(&entry.export_id);
        let source = PathBuf::from(&entry.source);
        let analysis = device::read_analysis(destination, &entry.anlz_dir);
        // A source on the device itself is an earlier kept copy.
        let on_device = source.starts_with(destination);
        let (source_path, preserve) =
            if source.is_file() && entry.conversion.is_empty() && !on_device {
                (source, None)
            } else {
                // The source is gone (or was converted): keep the device copy as it is.
                let analysis_dir = Path::new(&device.analysis)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (
                    destination.join(device.path.trim_start_matches('/')),
                    Some(rbl_export::DeviceTrack {
                        id: entry.export_id,
                        master_db_id: manifest.db_id,
                        master_content_id: entry.library_id,
                        audio: device.path.clone(),
                        analysis_dir,
                        preserve: true,
                    }),
                )
            };
        let artwork = match (store, entry.artwork.is_empty()) {
            (Some(store), false) => {
                let small = destination.join(entry.artwork.trim_start_matches('/'));
                let medium = small.with_file_name(format!(
                    "{}_m.jpg",
                    small
                        .file_stem()
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default()
                ));
                store.for_device(&small, &medium).ok()
            }
            _ => None,
        };
        out.push(rbl_export::SourceTrack {
            metadata: rbl_core::ExportMetadata {
                track_number: row.map_or(0, |r| r.track_number),
                disc_number: row.map_or(0, |r| r.disc_number),
                bit_depth: row.map_or(0, |r| r.sample_depth),
                play_count: row.map_or(0, |r| u32::from(r.play_count)),
                analysed: if analysis.is_empty() {
                    0
                } else {
                    u32::try_from(rbl_db::write::ANALYSED_FULL).unwrap_or(105)
                },
                hot_cue_auto_load: row.is_some_and(|r| r.hot_cue_auto_load),
                ..rbl_core::ExportMetadata::default()
            },
            cues: None,
            device: preserve,
            id: entry.library_id,
            source_path,
            title: device.title.clone(),
            artist: device.artist.clone(),
            album: device.album.clone(),
            genre: device.genre.clone(),
            label: device.label.clone(),
            key: device.key.clone(),
            comment: device.comment.clone(),
            date_added: previous_dates
                .get(&entry.export_id)
                .cloned()
                .unwrap_or_default(),
            release_date: row.map(|r| r.release_date.clone()).unwrap_or_default(),
            bpm_x100: device.bpm,
            duration_sec: row.map_or(0, |r| r.duration_sec),
            rating: u8::try_from(device.rating).unwrap_or(0),
            color_id: u8::try_from(device.color).unwrap_or(0),
            year: row.map_or(0, |r| r.year),
            bitrate: row.map_or(0, |r| r.bitrate),
            sample_rate: row.map_or(0, |r| r.sample_rate),
            file_size: 0,
            analysis,
            artwork,
            my_tags: Vec::new(),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- the command

fn stage_phase(stage: &str) -> &'static str {
    match stage {
        "checking" => "check",
        "copying" => "copy",
        "database" => "database",
        "verifying" => "verify",
        "publishing" => "publish",
        _ => "export",
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the export's steps in the order they run"
)]
pub fn run(args: &ExportArgs) -> CliResult<ExportResult> {
    let started = Instant::now();
    let (request, rest) = crate::request::read(&args.input)?;
    if let Some(rest) = rest {
        crate::cancel::watch_stdin(rest, args.cancel_on_stdin_eof);
    } else if args.stdin_control {
        crate::cancel::watch_stdin(
            std::io::BufReader::new(std::io::stdin()),
            args.cancel_on_stdin_eof,
        );
    }
    let playlists_flat = request.validate()?;
    let destination = args
        .to
        .clone()
        .or_else(|| request.destination.clone())
        .ok_or_else(|| {
            CliError::new(
                ErrorCode::Usage,
                "no destination: give --to <usb-root> or `destination` in the request",
            )
        })?;
    if !destination.is_dir() {
        return Err(CliError::not_found(format!(
            "the destination is not a directory: {}",
            destination.display()
        )));
    }
    let destination = absolute(&destination);
    let options = &request.options;
    if !args.dry_run && !options.allow_rekordbox_running && rbl_db::is_rekordbox_running() {
        return Err(CliError::new(
            ErrorCode::RekordboxRunning,
            "rekordbox is running. Quit it so only one application writes the device's libraries (or set options.allowRekordboxRunning).",
        ));
    }
    output::progress("plan", 0, request.tracks.len() as u64, None);
    if !args.dry_run {
        // Finish an interrupted publication before reading the device.
        rbl_export::recover(&destination)
            .map_err(|e| CliError::io("could not recover the device", &e))?;
    }
    let preferred_root = match options.root {
        RootPreference::Standard => Some(rbl_export::ExportRoot::Standard),
        RootPreference::Hidden => Some(rbl_export::ExportRoot::Hidden),
        RootPreference::Auto => device::volume_at(&destination)
            .and_then(|v| rbl_export::ExportRoot::for_file_system(&v.file_system)),
    };
    let root_name =
        rbl_export::export_root_name_with(&destination, preferred_root).map_err(CliError::from)?;
    let manifest = rbl_export::Manifest::load(&destination);
    let index = AnalysisIndex::load(&destination, root_name);
    let previous_dates: BTreeMap<u32, String> = device::pdb_tracks(&destination)
        .into_iter()
        .map(|(id, row)| (id, row.date_added))
        .collect();
    let store = if args.dry_run {
        None
    } else {
        Some(
            ArtworkStore::new(args.cache.artwork_dir())
                .map_err(|e| CliError::io("could not prepare artwork", &e))?,
        )
    };
    let plan_ms = elapsed_ms(started);

    // Analysis, artwork and tags, in parallel.
    let analyze_started = Instant::now();
    let ctx = Context {
        request: &request,
        destination: &destination,
        cache: &args.cache,
        fingerprint: analyze::fingerprint(&options.analysis),
        manifest: manifest.as_ref(),
        index: &index,
        artwork: store.as_ref(),
        dry_run: args.dry_run,
    };
    let mut prepared = prepare_all(&ctx, args.jobs)?;
    let analyze_ms = elapsed_ms(analyze_started);

    // What the plan says about the audio, and the free-space check.
    let mut to_copy = 0_u64;
    let mut audio_plan = Vec::with_capacity(prepared.len());
    for (track, prep) in request.tracks.iter().zip(&prepared) {
        let plan = prep.stamp.as_ref().map(|stamp| {
            let key = track_key(track, &prep.source);
            let reusable = manifest_entry(manifest.as_ref(), &key).is_some_and(|entry| {
                entry.size == stamp.size
                    && entry.modified == stamp.modified_ns
                    && (options.convert.is_none() == entry.conversion.is_empty())
                    && destination
                        .join(entry.audio.trim_start_matches('/'))
                        .is_file()
            });
            if !reusable {
                to_copy += stamp.size;
            }
            if reusable {
                AudioPlan::Reuse
            } else {
                AudioPlan::Copy
            }
        });
        audio_plan.push(plan);
    }
    let free = device::free_bytes(&destination);
    if let Some(free) = free {
        if !args.dry_run && options.convert.is_none() && to_copy > free {
            return Err(CliError::new(
                ErrorCode::InsufficientSpace,
                format!("the device has {free} bytes free; this export copies about {to_copy}"),
            )
            .with_details(serde_json::json!({ "freeBytes": free, "bytesToCopy": to_copy })));
        }
    }

    let mut result = ExportResult {
        destination: destination.to_string_lossy().into_owned(),
        root: root_name.to_owned(),
        dry_run: args.dry_run,
        tracks: TrackCounts {
            requested: request.tracks.len(),
            ..TrackCounts::default()
        },
        playlists: PlaylistCounts::default(),
        analysis: AnalysisCounts::default(),
        bytes: ByteCounts {
            to_copy,
            free,
            ..ByteCounts::default()
        },
        analysis_files: 0,
        artwork_files: 0,
        pdb_bytes: 0,
        verified: None,
        cache_dir: args.cache.dir().map(|d| d.to_string_lossy().into_owned()),
        jobs: args.jobs,
        timings: Timings {
            plan_ms,
            analyze_ms,
            ..Timings::default()
        },
        items: Vec::with_capacity(request.tracks.len()),
    };

    for (index, (track, prep)) in request.tracks.iter().zip(&prepared).enumerate() {
        let missing = prep.stamp.is_none();
        if missing {
            output::event(
                "track.skipped",
                serde_json::json!({ "index": index, "ref": track.reference, "path": prep.source, "reason": "missing" }),
                &format!("skipped (missing): {}", prep.source.display()),
            );
        }
        for warning in &prep.warnings {
            output::event(
                "track.warning",
                serde_json::json!({ "index": index, "ref": track.reference, "message": warning }),
                &format!("warning: tracks[{index}]: {warning}"),
            );
        }
    }

    if args.dry_run {
        for (index, (track, prep)) in request.tracks.iter().zip(&prepared).enumerate() {
            let from = prep.from.unwrap_or(AnalysisSource::None);
            count_analysis(&mut result.analysis, from);
            result.tracks.skipped += usize::from(prep.stamp.is_none());
            result.items.push(TrackResult {
                index,
                reference: track.reference.clone(),
                title: track
                    .title
                    .clone()
                    .unwrap_or_else(|| file_stem(&prep.source)),
                status: if prep.stamp.is_some() {
                    TrackStatus::Planned
                } else {
                    TrackStatus::Skipped
                },
                analysis: from,
                analysis_ms: None,
                audio: audio_plan[index],
                grid_override: track.beat_grid.is_some(),
                cues_override: track.cues.is_some(),
                artwork: track.artwork.is_some(),
                device_id: None,
                device_path: None,
                analysis_dir: None,
                warnings: prep.warnings.clone(),
            });
        }
        result.tracks.copied = audio_plan
            .iter()
            .filter(|p| **p == Some(AudioPlan::Copy))
            .count();
        result.tracks.reused = audio_plan
            .iter()
            .filter(|p| **p == Some(AudioPlan::Reuse))
            .count();
        result.playlists.written = playlists_flat.len();
        result.timings.total_ms = elapsed_ms(started);
        return Ok(result);
    }

    // The export itself.
    let built = build_tracks(
        &request,
        &mut prepared,
        &destination,
        manifest.as_ref(),
        &previous_dates,
    )?;
    let mut tracks = built.tracks;
    let requested_keys: BTreeSet<String> = request
        .tracks
        .iter()
        .zip(&prepared)
        .map(|(t, p)| track_key(t, &p.source))
        .collect();
    if !options.prune {
        if let Some(manifest) = &manifest {
            let kept = kept_tracks(
                &destination,
                manifest,
                &requested_keys,
                &previous_dates,
                store.as_ref(),
            )?;
            result.tracks.kept = kept.len();
            tracks.extend(kept);
        }
    }
    let playlists: Vec<rbl_export::SourcePlaylist> = playlists_flat
        .iter()
        .map(|p| rbl_export::SourcePlaylist {
            device_id: 0,
            device_only: false,
            id: p.id,
            name: p.name.clone(),
            parent_id: p.parent_id,
            folder: p.folder,
            track_indices: p.track_indices.clone(),
        })
        .collect();
    let defaults =
        options
            .device_name
            .as_ref()
            .map(|name| rbl_onelibrary::settings::StickSettings {
                device_name: name.clone(),
                ..rbl_onelibrary::settings::StickSettings::default()
            });
    let export_started = Instant::now();
    let requested = request.tracks.len();
    let report = rbl_export::export_cancellable(
        &destination,
        &tracks,
        &playlists,
        &[],
        &rbl_export::ExportOptions {
            defaults: defaults.as_ref(),
            sync: None,
            compatibility: options.convert.map(|c| match c {
                crate::request::ConvertFormat::Wav => rbl_export::CompatibilityFormat::Wav,
                crate::request::ConvertFormat::Aiff => rbl_export::CompatibilityFormat::Aiff,
                crate::request::ConvertFormat::Mp3 => rbl_export::CompatibilityFormat::Mp3,
            }),
            root: preferred_root,
        },
        &mut |p| {
            let item = (p.done < requested && !p.title.is_empty()).then(|| ProgressItem {
                index: Some(p.done),
                reference: request.tracks[p.done].reference.clone(),
                title: p.title.clone(),
            });
            let current = if matches!(p.stage, "checking" | "copying") {
                p.done
            } else {
                p.total
            };
            output::progress(stage_phase(p.stage), current as u64, p.total as u64, item);
        },
        &crate::cancel::requested,
    )
    .map_err(|e| {
        conflict_error(
            e,
            &ConflictContext {
                request: &request,
                prepared: &prepared,
                titles: &tracks,
                manifest: manifest.as_ref(),
                destination: &destination,
            },
        )
    })?;
    result.timings.export_ms = elapsed_ms(export_started);

    // The device name, on a device that already had a library.
    if let Some(name) = &options.device_name {
        let current = rbl_devices::settings::read(&destination);
        if let Some(library) = current.library.as_ref().filter(|l| l.device_name != *name) {
            let mut next = current.clone();
            next.library = Some(rbl_onelibrary::settings::StickSettings {
                device_name: name.clone(),
                ..library.clone()
            });
            rbl_devices::settings::write_changes(&destination, &current, &next).map_err(|e| {
                CliError::new(ErrorCode::Io, format!("could not set the device name: {e}"))
            })?;
        }
    }

    // Read both databases back, as rbxport does after every export.
    let verify_started = Instant::now();
    output::progress("verify", 0, 1, None);
    let check = rbl_export::verify_databases(&destination).map_err(CliError::from)?;
    result.timings.verify_ms = elapsed_ms(verify_started);
    let verified = check.is_ok() && check.tracks == report.tracks;
    output::progress("verify", 1, 1, None);
    if !verified {
        return Err(CliError::new(
            ErrorCode::VerificationFailed,
            format!(
                "the written export did not read back: {} of {} tracks; missing audio {:?}; {}",
                check.tracks,
                report.tracks,
                check.missing_audio,
                check.errors.join("; ")
            ),
        )
        .with_details(crate::usb::verify_details(&check)));
    }

    // Record where each track's analysis came from, for the next export.
    let after = rbl_export::Manifest::load(&destination);
    let mut next_index = AnalysisIndex::default();
    for (track, (prep, grid)) in request
        .tracks
        .iter()
        .zip(prepared.iter().zip(&built.grid_override))
    {
        let (Some(cache_key), Some(meta)) = (&prep.cache_key, &prep.meta) else {
            continue;
        };
        if matches!(
            prep.from,
            Some(AnalysisSource::Cache | AnalysisSource::Generated | AnalysisSource::Device)
        ) {
            next_index.tracks.insert(
                track_key(track, &prep.source),
                IndexedAnalysis {
                    cache_key: cache_key.clone(),
                    grid_override: *grid,
                    meta: meta.clone(),
                },
            );
        }
    }
    if let Err(e) = next_index.save(&destination, root_name) {
        tracing::warn!(error = %e, "could not record analysis provenance on the device");
    }

    for (index, (track, prep)) in request.tracks.iter().zip(&prepared).enumerate() {
        let key = track_key(track, &prep.source);
        let written = after
            .as_ref()
            .and_then(|m| m.tracks.iter().find(|t| t.key() == key));
        let from = if prep.stamp.is_none() {
            AnalysisSource::None
        } else {
            prep.from.unwrap_or(AnalysisSource::None)
        };
        if prep.stamp.is_some() {
            count_analysis(&mut result.analysis, from);
        }
        let item = TrackResult {
            index,
            reference: track.reference.clone(),
            title: tracks[index].title.clone(),
            status: if written.is_some() {
                TrackStatus::Exported
            } else {
                TrackStatus::Skipped
            },
            analysis: from,
            analysis_ms: prep.analysis_ms,
            audio: None,
            grid_override: built.grid_override[index],
            cues_override: track.cues.is_some(),
            artwork: tracks[index].artwork.is_some(),
            device_id: written.map(|w| w.export_id),
            device_path: written.map(|w| w.audio.clone()),
            analysis_dir: written
                .map(|w| w.anlz_dir.clone())
                .filter(|d| !d.is_empty()),
            warnings: built.warnings[index].clone(),
        };
        result.analysis.grid_overrides += usize::from(item.grid_override);
        result.analysis.cue_overrides +=
            usize::from(item.cues_override && item.status == TrackStatus::Exported);
        result.tracks.skipped += usize::from(item.status == TrackStatus::Skipped);
        result.items.push(item);
    }
    result.tracks.exported = report.tracks;
    result.tracks.copied = report.tracks.saturating_sub(report.reused);
    result.tracks.reused = report.reused;
    result.tracks.removed = report.removed;
    result.playlists = PlaylistCounts {
        written: report.playlists,
        added: report.playlists_added,
        removed: report.playlists_removed,
    };
    result.bytes.copied = report.bytes_copied;
    result.bytes.reused = report.bytes_reused;
    result.analysis_files = report.analysis_files;
    result.artwork_files = report.artwork_files;
    result.pdb_bytes = report.pdb_bytes;
    result.verified = Some(verified);
    result.timings.total_ms = elapsed_ms(started);
    Ok(result)
}

/// What a failed export knows about the request, to name the tracks a
/// conflict concerns.
struct ConflictContext<'a> {
    request: &'a ExportRequest,
    prepared: &'a [Prepared],
    /// The tracks handed to rbl-export (request tracks first).
    titles: &'a [rbl_export::SourceTrack],
    manifest: Option<&'a rbl_export::Manifest>,
    destination: &'a Path,
}

/// An rbl-export error, with the request tracks a conflict concerns added
/// to its `details` when rbx-cli can tell which they are.
fn conflict_error(error: rbl_export::ExportError, ctx: &ConflictContext<'_>) -> CliError {
    use crate::conflict::{ConflictReason as R, ConflictTrack};
    let rbl_export::ExportError::Conflict(message) = &error else {
        return CliError::from(error);
    };
    let mut details = crate::conflict::classify(message);
    let entry = |index: usize| {
        let track = &ctx.request.tracks[index];
        manifest_entry(ctx.manifest, &track_key(track, &ctx.prepared[index].source))
    };
    let concerned = |index: usize| ConflictTrack {
        index,
        reference: ctx.request.tracks[index].reference.clone(),
        device_id: entry(index).map(|e| e.export_id),
    };
    let listed = 0..ctx.request.tracks.len().min(ctx.prepared.len());
    let by_name = |name: &Option<String>| -> Vec<ConflictTrack> {
        name.as_ref().map_or_else(Vec::new, |name| {
            listed
                .clone()
                .filter(|i| ctx.titles.get(*i).is_some_and(|t| t.title == *name))
                .map(concerned)
                .collect()
        })
    };
    details.tracks = match details.reason {
        R::SourceUnavailable => {
            let missing: Vec<ConflictTrack> = listed
                .clone()
                .filter(|i| entry(*i).is_some() && !ctx.prepared[*i].source.is_file())
                .map(concerned)
                .collect();
            if missing.is_empty() {
                by_name(&details.name)
            } else {
                missing
            }
        }
        R::TrackChangedOnDevice => by_name(&details.name),
        R::CuesOrGridChangedOnDevice => listed
            .clone()
            .filter(|i| entry(*i).is_some_and(|e| analysis_changed_on_device(ctx.destination, e)))
            .map(concerned)
            .collect(),
        _ => Vec::new(),
    };
    let text = error.to_string();
    CliError::new(ErrorCode::Conflict, text).with_details(details.to_value())
}

/// Whether any of a track's analysis files on the device differs from what
/// the last sync wrote (rbl-export records a hash of each).
fn analysis_changed_on_device(destination: &Path, entry: &rbl_export::ManifestTrack) -> bool {
    !entry.anlz_dir.is_empty()
        && entry.analysis_hashes.iter().any(|(extension, recorded)| {
            let path = destination
                .join(entry.anlz_dir.trim_start_matches('/'))
                .join(format!("ANLZ0000.{extension}"));
            std::fs::read(path).is_ok_and(|bytes| rbl_export::manifest::hash(&bytes) != *recorded)
        })
}

fn count_analysis(counts: &mut AnalysisCounts, from: AnalysisSource) {
    match from {
        AnalysisSource::Cache => counts.cache_hits += 1,
        AnalysisSource::Generated | AnalysisSource::Generate => {
            counts.generated += 1;
            counts.cache_misses += 1;
        }
        AnalysisSource::Device => {
            counts.device_reuse += 1;
            counts.cache_misses += 1;
        }
        AnalysisSource::Supplied => counts.supplied += 1,
        AnalysisSource::None => counts.none += 1,
        AnalysisSource::Failed => {
            counts.failed += 1;
            counts.cache_misses += 1;
        }
    }
}

/// The human rendering of a result.
pub fn human(result: &ExportResult) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let mb = |b: u64| {
        #[allow(clippy::cast_precision_loss, reason = "display only")]
        let v = b as f64 / 1_048_576.0;
        format!("{v:.1} MB")
    };
    let a = &result.analysis;
    let t = &result.tracks;
    if result.dry_run {
        let _ = writeln!(s, "Dry run for {} ({})", result.destination, result.root);
        let _ = writeln!(
            s,
            "  tracks:   {} requested, {} to copy, {} to reuse, {} missing",
            t.requested, t.copied, t.reused, t.skipped
        );
        let _ = writeln!(
            s,
            "  analysis: {} cached, {} from device, {} to generate, {} supplied, {} none",
            a.cache_hits, a.device_reuse, a.generated, a.supplied, a.none
        );
        let free = result.bytes.free.map_or_else(|| "unknown".to_owned(), mb);
        let _ = writeln!(
            s,
            "  bytes:    {} to copy, {free} free",
            mb(result.bytes.to_copy)
        );
        let _ = writeln!(s, "  playlists: {}", result.playlists.written);
    } else {
        let _ = writeln!(s, "Exported to {} ({})", result.destination, result.root);
        let _ = writeln!(
            s,
            "  tracks:    {} on device, {} copied, {} reused, {} removed, {} skipped, {} kept",
            t.exported, t.copied, t.reused, t.removed, t.skipped, t.kept
        );
        let _ = writeln!(
            s,
            "  playlists: {} ({} added, {} removed)",
            result.playlists.written, result.playlists.added, result.playlists.removed
        );
        let _ = writeln!(s, "  analysis:  {} cache hits, {} generated, {} from device, {} supplied, {} none, {} failed", a.cache_hits, a.generated, a.device_reuse, a.supplied, a.none, a.failed);
        let _ = writeln!(
            s,
            "  bytes:     {} copied, {} reused",
            mb(result.bytes.copied),
            mb(result.bytes.reused)
        );
        let _ = writeln!(
            s,
            "  verified:  {}",
            if result.verified == Some(true) {
                "yes"
            } else {
                "NO"
            }
        );
    }
    let tm = &result.timings;
    let _ = writeln!(
        s,
        "  time:      {} ms (analyze {} ms, export {} ms, verify {} ms)",
        tm.total_ms, tm.analyze_ms, tm.export_ms, tm.verify_ms
    );
    for item in result
        .items
        .iter()
        .filter(|i| !i.warnings.is_empty() || i.status == TrackStatus::Skipped)
    {
        let _ = writeln!(
            s,
            "  ! [{}] {}: {}",
            item.index,
            item.title,
            if item.warnings.is_empty() {
                "skipped".to_owned()
            } else {
                item.warnings.join("; ")
            }
        );
    }
    s
}
