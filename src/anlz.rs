// SPDX-License-Identifier: GPL-2.0-or-later
//! Edits to analysis files (`ANLZ0000.DAT`/`.EXT`/`.2EX`) through rbl-anlz:
//! swapping in a caller's beat grid, and moving cue lists between files.
//!
//! The cached analysis never carries cues. Cue lists are put in at export
//! time, either the caller's (through `SourceTrack.cues`, which rbl-export
//! renders) or the ones already on the device.

use rbl_anlz::cues::ExportCue;
use rbl_anlz::{AnlzBuilder, Beat, Section};

use crate::request::{CueInput, CueKind};

/// One analysis file as rbl-export takes it: (`DAT`/`EXT`/`2EX`, bytes).
pub type AnalysisFile = (String, Vec<u8>);

/// The extensions a device analysis directory holds, in rekordbox's order.
pub const EXTENSIONS: [&str; 3] = ["DAT", "EXT", "2EX"];

fn parse(bytes: &[u8]) -> Result<rbl_anlz::Anlz, String> {
    rbl_anlz::parse(bytes).map_err(|e| e.to_string())
}

/// Replaces the beat grid, leaving every other section (waveforms
/// included) byte for byte as it was. Mirrors rbxport's grid editor: the
/// `.DAT`'s `PQTZ` is replaced, and an `.EXT`'s extended grid (`PQT2`),
/// which describes the old beats and cannot be re-derived, is emptied.
pub fn replace_grid(files: &mut [AnalysisFile], beats: &[Beat]) -> Result<bool, String> {
    let mut replaced = false;
    for (extension, bytes) in files.iter_mut() {
        if extension.eq_ignore_ascii_case("DAT") {
            *bytes = parse(bytes)?.with_beat_grid(beats);
            replaced = true;
        } else if extension.eq_ignore_ascii_case("EXT") {
            if let Some(cleared) = parse(bytes)?.with_extended_grid_cleared() {
                *bytes = cleared;
            }
        }
    }
    Ok(replaced)
}

/// The grid a file set carries, if any.
pub fn grid_of(files: &[AnalysisFile]) -> Option<Vec<Beat>> {
    files
        .iter()
        .filter(|(e, _)| e.eq_ignore_ascii_case("DAT"))
        .find_map(|(_, bytes)| rbl_anlz::parse(bytes).ok()?.beat_grid())
}

/// The empty cue lists a freshly authored file carries (what rbl-anlz's
/// author writes when there is nothing to carry over).
fn empty_cue_lists(extended: bool) -> Vec<Section> {
    let bytes = AnlzBuilder::new()
        .empty_cue_list_of(extended, 0)
        .empty_cue_list_of(extended, 1)
        .finish();
    rbl_anlz::parse(&bytes)
        .map(|a| a.sections)
        .unwrap_or_default()
}

fn with_cue_lists(bytes: &[u8], lists: Vec<Section>) -> Result<Vec<u8>, String> {
    let mut parsed = parse(bytes)?;
    parsed.sections.retain(|s| !s.is_cue_list());
    parsed.sections.extend(lists);
    Ok(parsed.to_bytes())
}

/// Replaces every cue list with empty ones: what goes into the cache, so
/// cues never decide whether analysis is reused.
pub fn strip_cues(files: &mut [AnalysisFile]) -> Result<(), String> {
    for (extension, bytes) in files.iter_mut() {
        let extended = extension.eq_ignore_ascii_case("EXT");
        if extended || extension.eq_ignore_ascii_case("DAT") {
            *bytes = with_cue_lists(bytes, empty_cue_lists(extended))?;
        }
    }
    Ok(())
}

/// Puts the cue lists of `device` (the files the device holds for this
/// track) into `files`, extension by extension. A file the device lacks
/// keeps its own lists. Returns whether anything was carried.
pub fn carry_cues(files: &mut [AnalysisFile], device: &[AnalysisFile]) -> Result<bool, String> {
    let mut carried = false;
    for (extension, bytes) in files.iter_mut() {
        if !(extension.eq_ignore_ascii_case("DAT") || extension.eq_ignore_ascii_case("EXT")) {
            continue;
        }
        let Some((_, from)) = device
            .iter()
            .find(|(e, _)| e.eq_ignore_ascii_case(extension))
        else {
            continue;
        };
        let Ok(from) = rbl_anlz::parse(from) else {
            continue;
        };
        let lists: Vec<Section> = from
            .sections
            .into_iter()
            .filter(Section::is_cue_list)
            .collect();
        if lists.is_empty() {
            continue;
        }
        *bytes = with_cue_lists(bytes, lists)?;
        carried = true;
    }
    Ok(carried)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validated: finite, 0 ≤ ms < 2^32"
)]
fn ms(value: f64) -> u32 {
    value.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// The caller's cues as rbl-export takes them (`djmdCue`'s shape: kind
/// 0 memory, 1–3 hot A–C, 5–17 hot D–P).
pub fn export_cues(cues: &[CueInput]) -> Result<Vec<ExportCue>, String> {
    cues.iter()
        .map(|cue| {
            let kind = crate::request::cue_kind(cue)?;
            Ok(ExportCue {
                kind,
                time_ms: ms(cue.time_ms),
                loop_time_ms: cue.loop_end_ms.map(ms),
                color_id: if cue.kind == CueKind::Memory {
                    cue.color.unwrap_or(0)
                } else {
                    0
                },
                color_code: cue.color_index.unwrap_or(0),
                comment: cue.comment.clone().unwrap_or_default(),
                active_loop: cue.active_loop,
                loop_numerator: cue.loop_numerator.unwrap_or(0),
                loop_denominator: cue.loop_denominator.unwrap_or(0),
            })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn authored() -> Vec<AnalysisFile> {
        let beats = [
            Beat {
                beat_number: 1,
                tempo_x100: 12000,
                time_ms: 10,
            },
            Beat {
                beat_number: 2,
                tempo_x100: 12000,
                time_ms: 510,
            },
        ];
        let columns = vec![
            rbl_anlz::BandColumn {
                low: 10,
                mid: 20,
                high: 30,
                peak: 40
            };
            300
        ];
        let files = rbl_anlz::author("/a.wav", &beats, &columns, rbl_anlz::Existing::default());
        vec![
            ("DAT".into(), files.dat),
            ("EXT".into(), files.ext),
            ("2EX".into(), files.two_ex),
        ]
    }

    #[test]
    fn a_grid_swap_keeps_the_waveforms() {
        let mut files = authored();
        let before = files.clone();
        let grid = [Beat {
            beat_number: 1,
            tempo_x100: 13000,
            time_ms: 100,
        }];
        assert!(replace_grid(&mut files, &grid).unwrap());
        assert_eq!(grid_of(&files).unwrap(), grid.to_vec());
        let dat = rbl_anlz::parse(&files[0].1).unwrap();
        let old = rbl_anlz::parse(&before[0].1).unwrap();
        assert_eq!(dat.waveform(b"PWAV"), old.waveform(b"PWAV"));
        assert_eq!(files[1], before[1], "an EXT without PQT2 is untouched");
        assert_eq!(files[2], before[2]);
    }

    #[test]
    fn cues_go_in_and_come_out_again() {
        let clean = authored();
        let mut with_cues = clean.clone();
        let cue = ExportCue {
            kind: 1,
            time_ms: 1234,
            ..ExportCue::default()
        };
        for (extension, bytes) in &mut with_cues {
            if extension == "DAT" || extension == "EXT" {
                let lists =
                    rbl_anlz::cues::sections(std::slice::from_ref(&cue), extension == "EXT");
                *bytes = with_cue_lists(bytes, lists).unwrap();
            }
        }
        let ext = rbl_anlz::parse(&with_cues[1].1).unwrap();
        assert_eq!(ext.cue_entries()[0].time_ms, 1234);

        let mut stripped = with_cues.clone();
        strip_cues(&mut stripped).unwrap();
        assert_eq!(
            stripped, clean,
            "stripping restores the authored empty lists"
        );

        let mut carried = clean.clone();
        assert!(carry_cues(&mut carried, &with_cues).unwrap());
        assert_eq!(carried, with_cues);
    }

    #[test]
    fn caller_cues_map_to_rbxport_kinds_and_colours() {
        let cues: Vec<CueInput> = serde_json::from_str(
            r#"[{"type":"hot","slot":"E","timeMs":1000.4,"loopEndMs":2000,"colorIndex":9,"comment":"Drop"},
                {"type":"memory","timeMs":0,"color":3}]"#,
        )
        .unwrap();
        let mapped = export_cues(&cues).unwrap();
        assert_eq!(mapped[0].kind, 6);
        assert_eq!(mapped[0].time_ms, 1000);
        assert_eq!(mapped[0].loop_time_ms, Some(2000));
        assert_eq!(mapped[0].color_code, 9);
        assert_eq!(mapped[0].color_id, 0);
        assert_eq!(mapped[0].comment, "Drop");
        assert_eq!(mapped[1].kind, 0);
        assert_eq!(mapped[1].color_id, 3);
    }
}
