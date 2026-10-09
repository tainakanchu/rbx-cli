// SPDX-License-Identifier: GPL-2.0-or-later
//! End to end: real audio written in the test, exported with the binary,
//! read back with `usb inspect`/`usb verify` and with rbl-anlz.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_rbx-cli");

/// A 16-bit stereo WAV: a click every beat over a quiet tone.
fn write_wav(path: &Path, seconds: f64, bpm: f64) {
    let rate = 22_050_u32;
    let frames = (f64::from(rate) * seconds) as u32;
    let period = (60.0 / bpm * f64::from(rate)) as u32;
    let mut data = Vec::with_capacity(frames as usize * 4);
    for i in 0..frames {
        let since = i % period.max(1);
        let click = if since < 300 {
            20_000.0 * (1.0 - f64::from(since) / 300.0)
        } else {
            0.0
        };
        let tone =
            2_000.0 * (2.0 * std::f64::consts::PI * 220.0 * f64::from(i) / f64::from(rate)).sin();
        let sample = (click + tone).clamp(-32_768.0, 32_767.0) as i16;
        data.extend_from_slice(&sample.to_le_bytes());
        data.extend_from_slice(&sample.to_le_bytes());
    }
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    std::fs::write(path, out).unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    music: PathBuf,
    usb: PathBuf,
    cache: PathBuf,
}

fn fixture(tracks: &[(&str, f64, f64)]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let music = dir.path().join("music");
    let usb = dir.path().join("usb");
    let cache = dir.path().join("cache");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::create_dir_all(&usb).unwrap();
    for (name, seconds, bpm) in tracks {
        write_wav(&music.join(name), *seconds, *bpm);
    }
    Fixture {
        _dir: dir,
        music,
        usb,
        cache,
    }
}

struct Run {
    code: i32,
    lines: Vec<Value>,
}

impl Run {
    fn terminal(&self) -> &Value {
        self.lines.last().expect("output")
    }
    fn data(&self) -> &Value {
        let last = self.terminal();
        assert_eq!(last["type"], "result", "{last:#}");
        &last["data"]
    }
}

fn run(args: &[&str], stdin: Option<&str>) -> Run {
    let mut child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<Value> = stdout
        .lines()
        .map(|l| {
            serde_json::from_str(l).unwrap_or_else(|e| {
                panic!(
                    "not JSON ({e}): {l}\nstderr: {}",
                    String::from_utf8_lossy(&out.stderr)
                )
            })
        })
        .collect();
    Run {
        code: out.status.code().unwrap_or(-1),
        lines,
    }
}

fn export(f: &Fixture, request: &Value) -> Run {
    run(
        &[
            "usb",
            "export",
            "--json",
            "--to",
            f.usb.to_str().unwrap(),
            "--cache-dir",
            f.cache.to_str().unwrap(),
        ],
        Some(&request.to_string()),
    )
}

fn track(f: &Fixture, name: &str) -> String {
    f.music.join(name).to_string_lossy().into_owned()
}

/// The envelope rules: every line has type and protocol; exactly one
/// terminal line, and it is the last.
fn check_envelope(run: &Run) {
    for line in &run.lines {
        assert_eq!(line["protocol"], 1, "{line}");
        assert!(
            matches!(
                line["type"].as_str(),
                Some("progress" | "event" | "log" | "result" | "error")
            ),
            "{line}"
        );
    }
    let terminals = run
        .lines
        .iter()
        .filter(|l| matches!(l["type"].as_str(), Some("result" | "error")))
        .count();
    assert_eq!(terminals, 1);
    assert!(matches!(
        run.terminal()["type"].as_str(),
        Some("result" | "error")
    ));
}

fn anlz(usb: &Path, data: &Value, index: usize, extension: &str) -> rbl_anlz::Anlz {
    let dir = data["items"][index]["analysisDir"]
        .as_str()
        .expect("an analysis dir");
    rbl_anlz::Anlz::read(
        &usb.join(dir.trim_start_matches('/'))
            .join(format!("ANLZ0000.{extension}")),
    )
    .unwrap()
}

#[test]
fn export_inspect_verify_round_trip() {
    let f = fixture(&[("one.wav", 12.0, 120.0), ("two.wav", 10.0, 128.0)]);
    let request = json!({
        "protocol": 1,
        "tracks": [
            { "path": track(&f, "one.wav"), "ref": "one", "title": "One", "artist": "Artist", "album": "LP", "genre": "House", "key": "8A", "rating": 4, "color": 2 },
            { "path": track(&f, "two.wav"), "ref": "two", "title": "Two", "artist": "Artist", "bpm": 128.0 },
            { "path": track(&f, "missing.wav"), "title": "Missing" }
        ],
        "playlists": [
            { "name": "Crates", "folder": true, "children": [ { "name": "Warmup", "tracks": ["one", "two"] } ] },
            { "name": "Single", "tracks": [1] }
        ]
    });
    let out = export(&f, &request);
    check_envelope(&out);
    assert_eq!(out.code, 0, "{:#}", out.terminal());
    let phases: Vec<&str> = out
        .lines
        .iter()
        .filter(|l| l["type"] == "progress")
        .filter_map(|l| l["phase"].as_str())
        .collect();
    for phase in [
        "plan", "analyze", "check", "copy", "database", "publish", "verify",
    ] {
        assert!(phases.contains(&phase), "{phase} in {phases:?}");
    }
    assert!(out
        .lines
        .iter()
        .any(|l| l["type"] == "event" && l["event"] == "track.skipped"));
    let data = out.data();
    assert_eq!(data["verified"], true);
    assert_eq!(data["tracks"]["exported"], 2);
    assert_eq!(data["tracks"]["skipped"], 1);
    assert_eq!(data["analysis"]["generated"], 2);
    assert_eq!(data["items"][2]["status"], "skipped");
    assert_eq!(data["playlists"]["written"], 3);

    let inspect = run(
        &[
            "usb",
            "inspect",
            "--json",
            "--cues",
            f.usb.to_str().unwrap(),
        ],
        None,
    );
    check_envelope(&inspect);
    let held = inspect.data();
    assert_eq!(held["trackCount"], 2);
    assert_eq!(held["databases"]["deviceLibrary"], true);
    assert_eq!(held["databases"]["oneLibrary"], true);
    assert_eq!(held["ours"], true);
    let one = held["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "One")
        .unwrap();
    assert_eq!(
        one["key"], "Am",
        "Camelot 8A is written as rekordbox spells it"
    );
    assert_eq!(one["artist"], "Artist");
    assert_eq!(one["rating"], 4);
    assert!(one["beats"].as_u64().unwrap() > 10, "an analysed grid");
    let two = held["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "Two")
        .unwrap();
    assert!((two["bpm"].as_f64().unwrap() - 128.0).abs() < 0.01);
    let names: Vec<&str> = held["playlists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 3);
    for name in ["Crates", "Warmup", "Single"] {
        assert!(names.contains(&name), "{names:?}");
    }
    let warmup = held["playlists"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Warmup")
        .unwrap();
    let crates = held["playlists"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Crates")
        .unwrap();
    assert_eq!(warmup["parentId"], crates["id"]);
    assert_eq!(warmup["trackIds"].as_array().unwrap().len(), 2);

    let verify = run(&["usb", "verify", "--json", f.usb.to_str().unwrap()], None);
    check_envelope(&verify);
    assert_eq!(verify.code, 0);
    let verified = verify.data();
    assert_eq!(verified["ok"], true);
    assert_eq!(verified["tracks"], 2);
    assert_eq!(verified["beatGrids"], 2);
    assert_eq!(verified["audioPresent"], 2);
}

#[test]
fn a_second_export_hits_the_cache_and_changing_only_cues_keeps_it() {
    let f = fixture(&[("a.wav", 10.0, 124.0), ("b.wav", 8.0, 126.0)]);
    let with_cues = |cues: Value| {
        json!({ "tracks": [
            { "path": track(&f, "a.wav"), "title": "A", "cues": cues },
            { "path": track(&f, "b.wav"), "title": "B" }
        ]})
    };
    let first = export(
        &f,
        &with_cues(json!([{ "type": "hot", "slot": "A", "timeMs": 1000 }])),
    );
    assert_eq!(first.code, 0, "{:#}", first.terminal());
    assert_eq!(first.data()["analysis"]["generated"], 2);
    assert_eq!(first.data()["analysis"]["cacheHits"], 0);
    let dat_before = anlz(&f.usb, first.data(), 1, "DAT").to_bytes();

    // Same request: every track from the cache, nothing regenerated, no audio copied.
    let second = export(
        &f,
        &with_cues(json!([{ "type": "hot", "slot": "A", "timeMs": 1000 }])),
    );
    assert_eq!(second.code, 0, "{:#}", second.terminal());
    let data = second.data();
    assert_eq!(data["analysis"]["cacheHits"], 2);
    assert_eq!(data["analysis"]["generated"], 0);
    assert_eq!(data["analysis"]["cacheMisses"], 0);
    assert_eq!(data["tracks"]["reused"], 2);
    assert_eq!(data["bytes"]["copied"], 0);
    assert!(data["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["analysis"] == "cache" && i.get("analysisMs").is_none()));
    assert_eq!(anlz(&f.usb, data, 1, "DAT").to_bytes(), dat_before);

    // Only the cues change: still all cache hits, and the new cues are written.
    let third = export(
        &f,
        &with_cues(json!([
            { "type": "hot", "slot": "C", "timeMs": 2500, "colorIndex": 5, "comment": "Drop" },
            { "type": "memory", "timeMs": 400, "loopEndMs": 2400, "color": 4 }
        ])),
    );
    assert_eq!(third.code, 0, "{:#}", third.terminal());
    let data = third.data();
    assert_eq!(data["analysis"]["cacheHits"], 2);
    assert_eq!(data["analysis"]["generated"], 0);
    let entries = anlz(&f.usb, data, 0, "EXT").cue_entries();
    assert_eq!(entries.len(), 2, "{entries:?}");
    let hot = entries.iter().find(|e| e.hot_cue != 0).unwrap();
    assert_eq!(
        (
            hot.hot_cue,
            hot.time_ms,
            hot.color_code,
            hot.comment.as_deref()
        ),
        (3, 2500, Some(5), Some("Drop"))
    );
    let memory = entries.iter().find(|e| e.hot_cue == 0).unwrap();
    assert_eq!(
        (
            memory.time_ms,
            memory.loop_time_ms,
            memory.kind,
            memory.color_id
        ),
        (400, 2400, 2, 4)
    );
}

#[test]
fn caller_cues_and_grid_are_written_into_the_analysis() {
    let f = fixture(&[("g.wav", 12.0, 120.0)]);
    let request = json!({ "tracks": [{
        "path": track(&f, "g.wav"),
        "beatGrid": { "anchors": [ { "timeMs": 250, "bpm": 120, "beatNumber": 1 }, { "timeMs": 6250, "bpm": 125 } ] },
        "cues": [
            { "type": "hot", "slot": "B", "timeMs": 250 },
            { "type": "hot", "slot": "E", "timeMs": 4250, "loopEndMs": 6250, "loopNumerator": 4, "loopDenominator": 1, "colorIndex": 9 },
            { "type": "memory", "timeMs": 750, "color": 6, "comment": "Intro" }
        ]
    }]});
    let out = export(&f, &request);
    assert_eq!(out.code, 0, "{:#}", out.terminal());
    let data = out.data();
    assert_eq!(data["items"][0]["gridOverride"], true);
    assert_eq!(data["items"][0]["cuesOverride"], true);

    let dat = anlz(&f.usb, data, 0, "DAT");
    let grid = dat.beat_grid().unwrap();
    assert_eq!(grid[0].time_ms, 250);
    assert_eq!(grid[0].beat_number, 1);
    assert_eq!(grid[0].tempo_x100, 12_000);
    assert_eq!(grid[1].time_ms, 750);
    let second_anchor = grid.iter().position(|b| b.time_ms == 6250).unwrap();
    assert_eq!(grid[second_anchor].tempo_x100, 12_500);
    assert_eq!(grid[second_anchor].beat_number, 1);
    assert_eq!(grid[second_anchor + 1].time_ms, 6730);
    assert!(grid.last().unwrap().time_ms < 12_000);
    assert!(
        dat.waveform(b"PWAV").is_some(),
        "the waveform survives the grid swap"
    );

    // The extended list carries all three; the legacy DAT list carries
    // memory cues and hot cues A–C only.
    let ext = anlz(&f.usb, data, 0, "EXT").cue_entries();
    assert_eq!(ext.len(), 3, "{ext:?}");
    let e = ext.iter().find(|c| c.hot_cue == 5).unwrap();
    assert_eq!(
        (e.time_ms, e.loop_time_ms, e.color_code),
        (4250, 6250, Some(9))
    );
    let memory = ext.iter().find(|c| c.hot_cue == 0).unwrap();
    assert_eq!(
        (memory.time_ms, memory.color_id, memory.comment.as_deref()),
        (750, 6, Some("Intro"))
    );
    let dat_lists: usize = dat
        .sections
        .iter()
        .filter(|s| s.is_cue_list())
        .map(|s| s.payload.len())
        .sum();
    assert!(dat_lists > 0);

    let inspect = run(
        &[
            "usb",
            "inspect",
            "--json",
            "--cues",
            f.usb.to_str().unwrap(),
        ],
        None,
    );
    let cues = &inspect.data()["tracks"][0]["cues"];
    assert_eq!(cues.as_array().unwrap().len(), 3);
    assert!(
        (inspect.data()["tracks"][0]["bpm"].as_f64().unwrap() - 120.0).abs() < 0.01,
        "BPM follows the caller grid"
    );
}

/// What a player does when a DJ stores a memory cue on the stick: the
/// cue lists in the track's analysis files change on the device.
fn add_memory_cue_on_device(usb: &Path, data: &Value, time_ms: u32) {
    let dir = usb.join(
        data["items"][0]["analysisDir"]
            .as_str()
            .unwrap()
            .trim_start_matches('/'),
    );
    let existing = anlz(usb, data, 0, "EXT").cue_entries();
    let mut cues: Vec<rbl_anlz::cues::ExportCue> = existing
        .iter()
        .map(|e| rbl_anlz::cues::ExportCue {
            kind: if e.hot_cue == 0 {
                0
            } else if e.hot_cue <= 3 {
                e.hot_cue as u8
            } else {
                e.hot_cue as u8 + 1
            },
            time_ms: e.time_ms,
            loop_time_ms: (e.kind == 2).then_some(e.loop_time_ms),
            color_code: e.color_code.unwrap_or(0),
            ..Default::default()
        })
        .collect();
    cues.push(rbl_anlz::cues::ExportCue {
        kind: 0,
        time_ms,
        ..Default::default()
    });
    for extension in ["DAT", "EXT"] {
        let path = dir.join(format!("ANLZ0000.{extension}"));
        let mut file = rbl_anlz::Anlz::read(&path).unwrap();
        file.sections.retain(|s| !s.is_cue_list());
        file.sections
            .extend(rbl_anlz::cues::sections(&cues, extension == "EXT"));
        std::fs::write(&path, file.to_bytes()).unwrap();
    }
}

#[test]
fn absent_cues_keep_the_device_cues_and_an_empty_list_clears_them() {
    let f = fixture(&[("c.wav", 8.0, 122.0)]);
    let request = |cues: Option<Value>| {
        let mut t = json!({ "path": track(&f, "c.wav"), "title": "C" });
        if let Some(cues) = cues {
            t["cues"] = cues;
        }
        json!({ "tracks": [t] })
    };
    let first = export(
        &f,
        &request(Some(
            json!([{ "type": "hot", "slot": "A", "timeMs": 1000 }]),
        )),
    );
    assert_eq!(first.code, 0, "{:#}", first.terminal());

    // A player saves a memory cue; a sync without `cues` keeps both.
    add_memory_cue_on_device(&f.usb, first.data(), 3000);
    let second = export(&f, &request(None));
    assert_eq!(second.code, 0, "{:#}", second.terminal());
    assert_eq!(second.data()["analysis"]["cacheHits"], 1);
    let kept: Vec<u32> = anlz(&f.usb, second.data(), 0, "EXT")
        .cue_entries()
        .iter()
        .map(|e| e.time_ms)
        .collect();
    assert!(kept.contains(&1000) && kept.contains(&3000), "{kept:?}");

    // A player saves another; a sync that replaces the cues is refused
    // rather than losing it.
    add_memory_cue_on_device(&f.usb, second.data(), 5000);
    let refused = export(
        &f,
        &request(Some(
            json!([{ "type": "hot", "slot": "B", "timeMs": 2000 }]),
        )),
    );
    assert_eq!(refused.code, 1);
    assert_eq!(refused.terminal()["type"], "error");
    assert_eq!(
        refused.terminal()["code"],
        "conflict",
        "{:#}",
        refused.terminal()
    );
    let details = &refused.terminal()["details"];
    assert_eq!(
        details["reason"], "cues_or_grid_changed_on_device",
        "{details:#}"
    );
    assert_eq!(details["tracks"][0]["index"], 0, "{details:#}");
    assert!(details["tracks"][0]["deviceId"].is_u64(), "{details:#}");

    // Adopting the device's cues (sending them back) is accepted; then `[]` clears.
    let adopt = export(
        &f,
        &request(Some(json!([
            { "type": "hot", "slot": "A", "timeMs": 1000 },
            { "type": "memory", "timeMs": 3000 },
            { "type": "memory", "timeMs": 5000 }
        ]))),
    );
    assert_eq!(adopt.code, 0, "{:#}", adopt.terminal());
    let cleared = export(&f, &request(Some(json!([]))));
    assert_eq!(cleared.code, 0, "{:#}", cleared.terminal());
    assert!(anlz(&f.usb, cleared.data(), 0, "EXT")
        .cue_entries()
        .is_empty());
    let verify = run(&["usb", "verify", "--json", f.usb.to_str().unwrap()], None);
    assert_eq!(verify.code, 0, "{:#}", verify.terminal());
}

#[test]
fn a_missing_source_the_device_holds_is_a_source_unavailable_conflict() {
    let f = fixture(&[("m1.wav", 6.0, 120.0), ("m2.wav", 6.0, 124.0)]);
    let request = json!({ "tracks": [
        { "path": track(&f, "m1.wav"), "ref": "one", "title": "M1" },
        { "path": track(&f, "m2.wav"), "ref": "two", "title": "M2" }
    ] });
    assert_eq!(export(&f, &request).code, 0);
    std::fs::remove_file(f.music.join("m2.wav")).unwrap();
    let refused = export(&f, &request);
    check_envelope(&refused);
    assert_eq!(refused.code, 1);
    let error = refused.terminal();
    assert_eq!(error["code"], "conflict", "{error:#}");
    assert_eq!(
        error["details"]["reason"], "source_unavailable",
        "{error:#}"
    );
    assert_eq!(error["details"]["name"], "M2", "{error:#}");
    let tracks = error["details"]["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1, "{error:#}");
    assert_eq!(tracks[0]["index"], 1);
    assert_eq!(tracks[0]["ref"], "two");
}

#[test]
fn analysis_on_the_device_stands_in_for_an_empty_cache() {
    let f = fixture(&[("d.wav", 8.0, 120.0)]);
    let request = json!({ "tracks": [{ "path": track(&f, "d.wav") }] });
    assert_eq!(export(&f, &request).code, 0);
    let other_cache = f.cache.with_file_name("other-cache");
    let out = run(
        &[
            "usb",
            "export",
            "--json",
            "--to",
            f.usb.to_str().unwrap(),
            "--cache-dir",
            other_cache.to_str().unwrap(),
        ],
        Some(&request.to_string()),
    );
    assert_eq!(out.code, 0, "{:#}", out.terminal());
    assert_eq!(out.data()["analysis"]["deviceReuse"], 1);
    assert_eq!(out.data()["analysis"]["generated"], 0);
    // …and was put in that cache.
    let again = run(
        &[
            "usb",
            "export",
            "--json",
            "--to",
            f.usb.to_str().unwrap(),
            "--cache-dir",
            other_cache.to_str().unwrap(),
        ],
        Some(&request.to_string()),
    );
    assert_eq!(again.data()["analysis"]["cacheHits"], 1);
}

#[test]
fn dry_run_plans_without_writing() {
    let f = fixture(&[("p.wav", 6.0, 120.0)]);
    let request =
        json!({ "tracks": [{ "path": track(&f, "p.wav") }, { "path": track(&f, "gone.wav") }] });
    let out = run(
        &[
            "usb",
            "export",
            "--json",
            "--dry-run",
            "--to",
            f.usb.to_str().unwrap(),
            "--cache-dir",
            f.cache.to_str().unwrap(),
        ],
        Some(&request.to_string()),
    );
    assert_eq!(out.code, 0, "{:#}", out.terminal());
    let data = out.data();
    assert_eq!(data["dryRun"], true);
    assert_eq!(data["items"][0]["analysis"], "generate");
    assert_eq!(data["items"][0]["audio"], "copy");
    assert_eq!(data["items"][1]["status"], "skipped");
    assert!(data["bytes"]["toCopy"].as_u64().unwrap() > 0);
    assert!(
        data["bytes"]["free"].as_u64().is_some_and(|free| free > 0),
        "a plain folder reports the free space of its filesystem: {data:#}"
    );
    assert_eq!(
        std::fs::read_dir(&f.usb).unwrap().count(),
        0,
        "nothing written"
    );
    assert!(!f.cache.exists(), "no cache written");

    assert_eq!(
        export(&f, &json!({ "tracks": [{ "path": track(&f, "p.wav") }] })).code,
        0
    );
    let planned = run(
        &[
            "usb",
            "export",
            "--json",
            "--dry-run",
            "--to",
            f.usb.to_str().unwrap(),
            "--cache-dir",
            f.cache.to_str().unwrap(),
        ],
        Some(&request.to_string()),
    );
    assert_eq!(planned.data()["items"][0]["analysis"], "cache");
    assert_eq!(planned.data()["items"][0]["audio"], "reuse");
}

#[test]
fn prune_false_keeps_tracks_the_request_no_longer_lists() {
    let f = fixture(&[("k1.wav", 6.0, 120.0), ("k2.wav", 6.0, 124.0)]);
    let both = json!({ "tracks": [{ "path": track(&f, "k1.wav"), "title": "K1" }, { "path": track(&f, "k2.wav"), "title": "K2" }] });
    assert_eq!(export(&f, &both).code, 0);
    let one = |prune: bool| json!({ "options": { "prune": prune }, "tracks": [{ "path": track(&f, "k1.wav"), "title": "K1" }] });
    let kept = export(&f, &one(false));
    assert_eq!(kept.code, 0, "{:#}", kept.terminal());
    assert_eq!(kept.data()["tracks"]["kept"], 1);
    assert_eq!(kept.data()["tracks"]["exported"], 2);
    let pruned = export(&f, &one(true));
    assert_eq!(pruned.code, 0, "{:#}", pruned.terminal());
    assert_eq!(pruned.data()["tracks"]["exported"], 1);
    assert_eq!(pruned.data()["tracks"]["removed"], 1);
    let verify = run(&["usb", "verify", "--json", f.usb.to_str().unwrap()], None);
    assert_eq!(verify.code, 0, "{:#}", verify.terminal());
}

#[test]
fn device_name_and_artwork_are_written() {
    let f = fixture(&[("n.wav", 6.0, 120.0)]);
    let cover = f.music.join("cover.png");
    image::RgbImage::from_pixel(500, 400, image::Rgb([10, 200, 10]))
        .save(&cover)
        .unwrap();
    let request = |name: &str| json!({ "options": { "deviceName": name }, "tracks": [{ "path": track(&f, "n.wav"), "artwork": cover }] });
    let out = export(&f, &request("FIRST"));
    assert_eq!(out.code, 0, "{:#}", out.terminal());
    assert_eq!(out.data()["artworkFiles"], 4);
    assert_eq!(out.data()["items"][0]["artwork"], true);
    let renamed = export(&f, &request("SECOND"));
    assert_eq!(renamed.code, 0, "{:#}", renamed.terminal());
    assert_eq!(
        renamed.data()["artworkFiles"],
        0,
        "unchanged artwork is not rewritten"
    );
    let inspect = run(
        &[
            "usb",
            "inspect",
            "--json",
            "--summary",
            f.usb.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(inspect.data()["deviceName"], "SECOND");
    assert!(inspect.data().get("tracks").is_none());
}

#[test]
fn errors_are_envelopes_with_exit_codes() {
    let usage = run(&["usb", "export", "--json", "--jobs", "0"], Some("{}"));
    check_envelope(&usage);
    assert_eq!(usage.code, 2);
    assert_eq!(usage.terminal()["code"], "usage");

    let dir = tempfile::tempdir().unwrap();
    let invalid = run(
        &[
            "usb",
            "export",
            "--json",
            "--to",
            dir.path().to_str().unwrap(),
        ],
        Some(r#"{"tracks":[{"title":"x"}]}"#),
    );
    check_envelope(&invalid);
    assert_eq!(invalid.code, 2);
    assert_eq!(invalid.terminal()["code"], "invalid_request");
    assert_eq!(invalid.terminal()["exitCode"], 2);

    let missing = run(
        &["usb", "export", "--json", "--to", "/no/such/dir"],
        Some(r#"{"tracks":[]}"#),
    );
    assert_eq!(missing.code, 1);
    assert_eq!(missing.terminal()["code"], "not_found");

    let unverifiable = run(
        &["usb", "verify", "--json", dir.path().to_str().unwrap()],
        None,
    );
    assert_eq!(unverifiable.code, 3);
    assert_eq!(unverifiable.terminal()["code"], "verification_failed");

    let version = run(&["version", "--json"], None);
    let data = version.data();
    assert_eq!(data["protocol"], 1);
    assert_eq!(data["rbxportRev"].as_str().unwrap().len(), 40);
    assert!(data["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "usb.export.cues"));
}

#[test]
fn a_cancel_line_on_stdin_stops_the_export_and_leaves_the_device_as_it_was() {
    let names: Vec<String> = (0..6).map(|i| format!("x{i}.wav")).collect();
    let specs: Vec<(&str, f64, f64)> = names.iter().map(|n| (n.as_str(), 20.0, 120.0)).collect();
    let f = fixture(&specs);
    assert_eq!(
        export(&f, &json!({ "tracks": [{ "path": track(&f, "x0.wav") }] })).code,
        0
    );
    let tracks: Vec<Value> = names
        .iter()
        .map(|n| json!({ "path": track(&f, n) }))
        .collect();
    let request = json!({ "tracks": tracks });
    let out = run(
        &[
            "usb",
            "export",
            "--json",
            "--jobs",
            "1",
            "--no-cache",
            "--to",
            f.usb.to_str().unwrap(),
        ],
        Some(&format!("{request}\n{{\"type\":\"cancel\"}}\n")),
    );
    check_envelope(&out);
    assert_eq!(out.code, 130, "{:#}", out.terminal());
    assert_eq!(out.terminal()["code"], "cancelled");
    let inspect = run(
        &[
            "usb",
            "inspect",
            "--json",
            "--summary",
            f.usb.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(
        inspect.data()["trackCount"],
        1,
        "the earlier export is intact"
    );
    assert_eq!(
        run(&["usb", "verify", "--json", f.usb.to_str().unwrap()], None).code,
        0
    );
}

#[test]
fn the_end_of_stdin_cancels_only_with_cancel_on_stdin_eof() {
    let names: Vec<String> = (0..6).map(|i| format!("e{i}.wav")).collect();
    let specs: Vec<(&str, f64, f64)> = names.iter().map(|n| (n.as_str(), 20.0, 120.0)).collect();
    let f = fixture(&specs);
    let write_request = |names: &[String]| {
        let tracks: Vec<Value> = names
            .iter()
            .map(|n| json!({ "path": track(&f, n) }))
            .collect();
        let path = f.music.join("request.json");
        std::fs::write(&path, json!({ "tracks": tracks }).to_string()).unwrap();
        path
    };
    let args = |request: &Path, flag: &'static str| {
        vec![
            "usb".to_owned(),
            "export".into(),
            "--json".into(),
            "--jobs".into(),
            "1".into(),
            "--no-cache".into(),
            flag.into(),
            "--input".into(),
            request.to_string_lossy().into_owned(),
            "--to".into(),
            f.usb.to_string_lossy().into_owned(),
        ]
    };
    let run_with = |args: &[String]| {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        // `run` closes stdin at once, as a parent that died would.
        run(&args, None)
    };

    // `--stdin-control` alone: the end of stdin is not a cancellation.
    let one = write_request(&names[..1]);
    let kept = run_with(&args(&one, "--stdin-control"));
    check_envelope(&kept);
    assert_eq!(kept.code, 0, "{:#}", kept.terminal());

    // `--cancel-on-stdin-eof`: it is, and the device keeps its library.
    let all = write_request(&names);
    let cancelled = run_with(&args(&all, "--cancel-on-stdin-eof"));
    check_envelope(&cancelled);
    assert_eq!(cancelled.code, 130, "{:#}", cancelled.terminal());
    assert_eq!(cancelled.terminal()["code"], "cancelled");
    let inspect = run(
        &[
            "usb",
            "inspect",
            "--json",
            "--summary",
            f.usb.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(
        inspect.data()["trackCount"],
        1,
        "the earlier export is intact"
    );
}

#[cfg(unix)]
#[test]
fn sigint_cancels() {
    use std::io::BufRead as _;
    let names: Vec<String> = (0..4).map(|i| format!("s{i}.wav")).collect();
    let specs: Vec<(&str, f64, f64)> = names.iter().map(|n| (n.as_str(), 30.0, 120.0)).collect();
    let f = fixture(&specs);
    let tracks: Vec<Value> = names
        .iter()
        .map(|n| json!({ "path": track(&f, n) }))
        .collect();
    let request = f.music.join("request.json");
    std::fs::write(&request, json!({ "tracks": tracks }).to_string()).unwrap();
    let mut child = Command::new(BIN)
        .args([
            "usb",
            "export",
            "--json",
            "--jobs",
            "1",
            "--no-cache",
            "--input",
            request.to_str().unwrap(),
            "--to",
            f.usb.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut lines = Vec::new();
    let mut signalled = false;
    for line in stdout.lines() {
        let line: Value = serde_json::from_str(&line.unwrap()).unwrap();
        if !signalled && line["type"] == "progress" && line["phase"] == "plan" {
            Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .status()
                .unwrap();
            signalled = true;
        }
        lines.push(line);
    }
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130));
    assert_eq!(lines.last().unwrap()["code"], "cancelled");
    assert!(
        !f.usb.join("PIONEER/rekordbox/export.pdb").exists(),
        "nothing published"
    );
}
