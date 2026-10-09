// SPDX-License-Identifier: GPL-2.0-or-later
//! Caller beat grids, expanded to the beats a device grid (`PQTZ`) holds.

use rbl_anlz::Beat;

use crate::request::{BeatGridInput, BeatInput};

/// More beats than any real track has (30 minutes at 655 BPM is ~20k);
/// stops a bad duration from allocating without bound.
const MAX_BEATS: usize = 200_000;

/// Expands a validated grid. `duration_ms` is where anchor grids stop; a
/// `beats` grid ignores it. Errors name what is missing.
pub fn expand(grid: &BeatGridInput, duration_ms: Option<u64>) -> Result<Vec<Beat>, String> {
    match (&grid.beats, &grid.anchors) {
        (Some(beats), None) => Ok(from_beats(beats)),
        (None, Some(anchors)) => {
            let end = duration_ms.filter(|d| *d > 0).ok_or(
                "a grid of `anchors` needs the track's length: give `durationSec`, or let the track be analysed",
            )?;
            Ok(from_anchors(anchors, end))
        }
        _ => Err("give exactly one of `beats` or `anchors`".into()),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validated: finite, 0 ≤ ms < 2^32, 0 < bpm ≤ 655.35"
)]
fn ms(value: f64) -> u32 {
    value.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "validated: 0 < bpm ≤ 655.35"
)]
fn tempo_x100(bpm: f64) -> u16 {
    (bpm * 100.0).round().clamp(1.0, f64::from(u16::MAX)) as u16
}

fn next_number(n: u16) -> u16 {
    n % 4 + 1
}

fn previous_number(n: u16) -> u16 {
    (n + 2) % 4 + 1
}

fn push(out: &mut Vec<Beat>, beat: Beat) {
    if out.last().is_none_or(|last| beat.time_ms > last.time_ms) && out.len() < MAX_BEATS {
        out.push(beat);
    }
}

fn from_beats(beats: &[BeatInput]) -> Vec<Beat> {
    let mut out = Vec::with_capacity(beats.len());
    let mut number = 0_u16;
    for beat in beats {
        number = beat
            .beat_number
            .unwrap_or(if number == 0 { 1 } else { next_number(number) });
        push(
            &mut out,
            Beat {
                beat_number: number,
                tempo_x100: tempo_x100(beat.bpm),
                time_ms: ms(beat.time_ms),
            },
        );
    }
    out
}

#[allow(clippy::cast_precision_loss, reason = "durations far below 2^52 ms")]
fn from_anchors(anchors: &[BeatInput], end_ms: u64) -> Vec<Beat> {
    let end = end_ms as f64;
    let mut out = Vec::new();
    let Some(first) = anchors.first() else {
        return out;
    };
    // Back from the first anchor towards the start, as an analysed grid
    // begins at the first beat in the audio.
    let period = 60_000.0 / first.bpm;
    let mut number = first.beat_number.unwrap_or(1);
    let mut before = Vec::new();
    let mut t = first.time_ms - period;
    while t >= 0.0 && before.len() < MAX_BEATS {
        number = previous_number(number);
        before.push(Beat {
            beat_number: number,
            tempo_x100: tempo_x100(first.bpm),
            time_ms: ms(t),
        });
        t -= period;
    }
    for beat in before.into_iter().rev() {
        push(&mut out, beat);
    }
    for (i, anchor) in anchors.iter().enumerate() {
        let stop = anchors.get(i + 1).map_or(end, |next| next.time_ms);
        let period = 60_000.0 / anchor.bpm;
        let mut number = anchor.beat_number.unwrap_or(1);
        let mut k = 0_u32;
        loop {
            let t = anchor.time_ms + f64::from(k) * period;
            if t >= stop || out.len() >= MAX_BEATS {
                break;
            }
            push(
                &mut out,
                Beat {
                    beat_number: number,
                    tempo_x100: tempo_x100(anchor.bpm),
                    time_ms: ms(t),
                },
            );
            number = next_number(number);
            k += 1;
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn beat(time_ms: f64, bpm: f64, beat_number: Option<u16>) -> BeatInput {
        BeatInput {
            time_ms,
            bpm,
            beat_number,
        }
    }

    #[test]
    fn listed_beats_number_themselves_from_one() {
        let grid = BeatGridInput {
            beats: Some(vec![
                beat(10.4, 120.0, None),
                beat(510.6, 120.0, None),
                beat(1010.0, 121.5, Some(1)),
                beat(1500.0, 121.5, None),
            ]),
            anchors: None,
        };
        let beats = expand(&grid, None).unwrap();
        let numbers: Vec<u16> = beats.iter().map(|b| b.beat_number).collect();
        assert_eq!(numbers, vec![1, 2, 1, 2]);
        assert_eq!(beats[0].time_ms, 10);
        assert_eq!(beats[1].time_ms, 511);
        assert_eq!(beats[2].tempo_x100, 12150);
    }

    #[test]
    fn anchors_fill_both_ways_and_change_tempo() {
        let grid = BeatGridInput {
            beats: None,
            anchors: Some(vec![
                beat(1000.0, 120.0, Some(1)),
                beat(5000.0, 60.0, Some(1)),
            ]),
        };
        let beats = expand(&grid, Some(8000)).unwrap();
        let times: Vec<u32> = beats.iter().map(|b| b.time_ms).collect();
        assert_eq!(
            times,
            vec![0, 500, 1000, 1500, 2000, 2500, 3000, 3500, 4000, 4500, 5000, 6000, 7000]
        );
        let numbers: Vec<u16> = beats.iter().map(|b| b.beat_number).collect();
        assert_eq!(numbers, vec![3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3]);
        assert_eq!(beats[0].tempo_x100, 12000);
        assert_eq!(beats[12].tempo_x100, 6000);
        assert!(expand(&grid, None).is_err(), "anchors need a length");
    }
}
