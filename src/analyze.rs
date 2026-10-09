// SPDX-License-Identifier: GPL-2.0-or-later
//! Generating analysis the way rbxport's Analyze command does: decode to
//! mono, run rbl-analysis, author the three files with rbl-anlz.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::anlz::AnalysisFile;
use crate::request::AnalysisSettings;

/// How much of a file is decoded, as rbxport caps it: tempo and key settle
/// early, but the grid and waveforms must reach the end of the track, and
/// thirty minutes stops a long mix from becoming gigabytes of samples.
pub const DECODE_CAP_SECS: f64 = 1800.0;

/// What one analysis worked out besides the files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisMeta {
    /// The tempo at the start of the track.
    pub bpm: f64,
    /// Detected key, rekordbox spelling (`Ebm`); absent when undetermined.
    #[serde(default)]
    pub key: Option<String>,
    /// Decoded length, in milliseconds.
    pub duration_ms: u64,
    /// Whether the whole file was decoded (false when capped).
    pub full_length: bool,
    pub beats: u32,
}

#[derive(Debug, Clone)]
pub struct Generated {
    pub files: Vec<AnalysisFile>,
    pub meta: AnalysisMeta,
}

/// rbl-analysis options from the request, as rbxport's settings dialog
/// maps them onto its preset.
pub fn options(settings: &AnalysisSettings) -> rbl_analysis::AnalysisOptions {
    let mut options = rbl_analysis::AnalysisPreset::Rbxport.options();
    options.tempo.min_bpm = settings.min_bpm;
    options.tempo.max_bpm = settings.max_bpm;
    options.tempo.placement = if settings.high_precision {
        rbl_analysis::tempo::Placement::Attack
    } else {
        rbl_analysis::tempo::Placement::Envelope
    };
    options
}

/// The part of the settings that changes the generated files, for the
/// cache key. `detectKey` only decides whether the detected key is used.
pub fn fingerprint(settings: &AnalysisSettings) -> String {
    format!(
        "min={};max={};precision={};cap={DECODE_CAP_SECS}",
        settings.min_bpm, settings.max_bpm, settings.high_precision
    )
}

/// Decodes and analyses one file. The files carry the empty cue lists of a
/// fresh analysis; `PPTH` names `path` (rbl-export rewrites it on export).
pub fn generate(path: &Path, settings: &AnalysisSettings) -> Result<Generated, String> {
    let audio = rbl_audio::decode_mono(path, Some(DECODE_CAP_SECS))
        .map_err(|e| format!("could not decode: {e}"))?;
    let analysis = rbl_analysis::analyse_with(&audio.samples, audio.sample_rate, options(settings));
    let beats: Vec<rbl_anlz::Beat> = analysis
        .tempo
        .beats
        .iter()
        .map(|b| rbl_anlz::Beat {
            beat_number: b.beat_number,
            tempo_x100: b.tempo_x100,
            time_ms: b.time_ms,
        })
        .collect();
    let columns: Vec<rbl_anlz::BandColumn> = analysis
        .waveform
        .columns
        .iter()
        .map(|c| rbl_anlz::BandColumn {
            low: c.low,
            mid: c.mid,
            high: c.high,
            peak: c.peak,
        })
        .collect();
    let files = rbl_anlz::author_with_overview(
        &path.to_string_lossy(),
        &beats,
        &columns,
        analysis.waveform.overview.as_slice().try_into().ok(),
        rbl_anlz::Existing::default(),
    );
    let seconds = audio.duration_secs();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "non-negative and capped at 30 minutes"
    )]
    let duration_ms = (seconds * 1000.0).round().max(0.0) as u64;
    Ok(Generated {
        files: vec![
            ("DAT".into(), files.dat),
            ("EXT".into(), files.ext),
            ("2EX".into(), files.two_ex),
        ],
        meta: AnalysisMeta {
            bpm: analysis.tempo.bpm,
            key: analysis.key.map(|k| k.name),
            duration_ms,
            full_length: seconds < DECODE_CAP_SECS - 1.0,
            beats: u32::try_from(beats.len()).unwrap_or(u32::MAX),
        },
    })
}

/// Reads analysis files given by a caller (`analysisPath` names the
/// `.DAT`; `.EXT` and `.2EX` beside it are taken when present), checking
/// each parses.
pub fn read_supplied(dat: &Path) -> Result<Vec<AnalysisFile>, String> {
    let mut out = Vec::new();
    for extension in crate::anlz::EXTENSIONS {
        let path = dat.with_extension(extension);
        match std::fs::read(&path) {
            Ok(bytes) => {
                rbl_anlz::parse(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                out.push((extension.to_owned(), bytes));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && extension != "DAT" => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(out)
}
