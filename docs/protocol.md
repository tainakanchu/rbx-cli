# rbx-cli protocol (version 1)

This is the contract between `rbx-cli` and programs that run it: the request
document `usb export` reads, the JSON lines every command writes with
`--json`, the exit codes, and what the export does with caches, cues and beat
grids. JSON Schemas generated from the implementation are in
[`schema/`](../schema) (`rbx-cli schema` prints them too).

- [Running a command](#running-a-command)
- [Output envelope](#output-envelope)
- [Exit codes and error codes](#exit-codes-and-error-codes)
- [Cancellation](#cancellation)
- [`usb export`](#usb-export)
- [Analysis, caching and performance](#analysis-caching-and-performance)
- [Cues](#cues)
- [Beat grids](#beat-grids)
- [`usb inspect`](#usb-inspect), [`usb verify`](#usb-verify),
  [`devices list` / `devices eject`](#devices), [`version`](#version)
- [Versioning policy](#versioning-policy)

## Running a command

```
rbx-cli [--json] [--quiet] [--log-level LEVEL] <command> [args]
```

| Command | Purpose | Result schema |
| --- | --- | --- |
| `usb export [--input FILE\|-] [--to DIR] [--dry-run] [--cache-dir DIR \| --no-cache] [--jobs N] [--stdin-control] [--cancel-on-stdin-eof]` | Export / incrementally sync tracks and playlists to a USB root | `result.usb-export.json` |
| `usb inspect ROOT [--summary] [--cues]` | What an export holds | `result.usb-inspect.json` |
| `usb verify ROOT` | Read an export back and check it | `result.usb-verify.json` |
| `devices list` | Mounted volumes and their libraries | `result.devices-list.json` |
| `devices eject MOUNT_POINT` | Eject a listed volume (never forced) | `result.devices-eject.json` |
| `version` | CLI version, pinned rbxport revision, protocol, capabilities | `result.version.json` |

Without `--json` the result is printed for a person on stdout and progress,
warnings and errors go to stderr.

## Output envelope

With `--json`, stdout carries **NDJSON**: one JSON object per line, UTF-8,
flushed per line. Nothing else is written to stdout. Every line has:

| Field | |
| --- | --- |
| `type` | `progress`, `event`, `log`, `result` or `error` |
| `protocol` | `1` |

Every run ends with **exactly one** terminal line, `result` or `error`, and
it is the last line. Readers must ignore unknown `type`s and unknown fields.

```jsonc
{"type":"progress","protocol":1,"command":"usb.export","phase":"analyze","current":3,"total":12,"item":{"index":2,"ref":"t3","title":"Track Three"}}
{"type":"event","protocol":1,"command":"usb.export","event":"track.skipped","data":{"index":5,"ref":null,"path":"/music/gone.mp3","reason":"missing"}}
{"type":"log","protocol":1,"level":"warn","message":"could not store analysis in the cache","target":"rbx_cli::export"}
{"type":"result","protocol":1,"command":"usb.export","data":{ ... }}
{"type":"error","protocol":1,"command":"usb.export","code":"conflict","message":"USB sync conflict: ...","exitCode":1,"details":{ ... }}
```

**progress** — `command`, `phase`, `current`, `total` (0 when unknown),
optional `item` (`index` into the request's `tracks`, `ref`, `title`).
`usb export` phases, in order:

| Phase | `current`/`total` | |
| --- | --- | --- |
| `plan` | 0 / tracks | Reading the request and the device |
| `analyze` | tracks done / tracks | Tags, analysis (cache, device, or generated) and artwork, in parallel; one line per finished track |
| `check` | track index / tracks | rbl-export checks each track (unchanged?) |
| `copy` | track index / tracks | rbl-export copies (or converts) a track's audio |
| `database` | tracks / tracks | Building `export.pdb` and `exportLibrary.db` |
| `verify` | — | Staged verification inside rbl-export, then the published databases are read back |
| `publish` | tracks / tracks | Atomic publication of the staged generation |

**event** — `command`, `event` (dotted name), `data`. Current events:

| Event | `data` |
| --- | --- |
| `track.skipped` | `index`, `ref`, `path`, `reason` (`missing`) |
| `track.warning` | `index`, `ref`, `message` (analysis failed, unreadable artwork, grid ignored…) |

**log** — diagnostics (`level`: `error`/`warn`/`info`/`debug`/`trace`,
`message`, `target`). Never needed to interpret a result. The level is
`--log-level`, else `RBX_CLI_LOG` (an `EnvFilter` directive), else `warn`.

**result** — `command` and `data`, the command's result (schemas above).

**error** — `command` (absent when the command line itself was invalid),
`code`, `message`, `exitCode` (the process exit code that follows) and
optional `details`.

## Exit codes and error codes

| Exit | Meaning |
| --- | --- |
| 0 | Success (`result` line) |
| 1 | Failure (`error` line) |
| 2 | Usage error or invalid request (`usage`, `invalid_request`, `unsupported`) |
| 3 | Verification failed (`verification_failed`) |
| 130 | Cancelled (`cancelled`) |

| `code` | |
| --- | --- |
| `usage` | Bad command-line arguments (`details.usage` holds clap's text) |
| `invalid_request` | The request is malformed or inconsistent (message names the field) |
| `unsupported` | The request's `protocol` is newer than this CLI's |
| `not_found` | A path or device does not exist |
| `conflict` | The device's library conflicts with the request; nothing was changed. `details.reason` says why (see [Conflict reasons](#usb.export.conflictReasons)) |
| `device_gone` | The device disappeared while being written |
| `rekordbox_running` | rekordbox is running (it may write the same device); set `options.allowRekordboxRunning` to override |
| `insufficient_space` | The filesystem holding the destination has less free space than the audio to copy (`details.freeBytes`, `details.bytesToCopy`) |
| `verification_failed` | The export did not read back correctly (`details` has the report) |
| `cancelled` | SIGINT/SIGTERM/SIGHUP, Ctrl+C/Ctrl+Break, or a `cancel` line |
| `io` | Other I/O errors |
| `internal` | Anything else (including `exportLibrary.db` failures) |

### Conflict reasons

A `conflict` error's `details` (schema `details.conflict.json`; capability
`usb.export.conflictReasons`) tells conflicts apart without reading `message`:

| Field | |
| --- | --- |
| `reason` | Always present; one of the strings below. New reasons may be added: treat an unknown one as `other`. |
| `database` | `deviceLibrary` (`export.pdb`) or `oneLibrary` (`exportLibrary.db`), when the conflict is about one |
| `name` | The track title, playlist or My Tag name rbxport named, when it named one |
| `deviceTrackId` | The device track id rbxport named, when it named one |
| `tracks` | `usb export` only, when rbx-cli can tell: request tracks concerned, each `{ index, ref, deviceId }` (`ref`/`deviceId` null when unknown) |

| `reason` | Meaning | `tracks` |
| --- | --- | --- |
| `cues_or_grid_changed_on_device` | A track's analysis files on the device (cue lists or beat grid; rbxport does not say which) changed since the last sync, e.g. cues saved on a player, and the request replaces them. See [Cues](#cues); `options.onDeviceChanges: "keepDevice"` keeps the device's instead. | Every listed track whose analysis files on the device differ from what the last sync wrote |
| `onelibrary_cues_changed_on_device` | `exportLibrary.db` cue records changed on the device since the last sync, and the export would replace them | — |
| `track_changed_on_device` | A track's metadata changed on the device (`database`, `name` = title) | Listed tracks with that title |
| `playlist_changed_on_device` | A playlist changed on the device (`database`, `name`) | — |
| `my_tags_changed_on_device` | A My Tag (`name`) or a track's My Tags (`deviceTrackId`) changed on the device | — |
| `deleted_on_device` | Tracks or playlists were deleted on the device since the last sync (`database`) | — |
| `device_only_track` | A track only the device has would be lost (`database`, `name`) | — |
| `device_only_playlist` | A playlist only the device has would be lost (`name`) | — |
| `history_references_track` | A track being removed is still in the device's history | — |
| `source_unavailable` | A track an earlier export put on the device has no readable source (missing, or gone while copying; `name` = the first one's title) | Every listed track the device holds whose source is not a file |
| `ownership` | The device belongs to a different (or older, unverified) library | — |
| `identities_changed` | rekordbox changed the device's track identities | — |
| `libraries_disagree` | The device's two existing libraries disagree | — |
| `both_roots` | Both `PIONEER` and `.PIONEER` hold a library | — |
| `unreadable_library` | A device database cannot be read or fails its integrity check (`database`) | — |
| `unsupported_onelibrary` | `exportLibrary.db` has a schema version rbxport does not support | — |
| `device_changed_during_sync` | Another program changed the device while the export was staging | — |
| `staged_verification_failed` | The staged generation did not verify before publication | — |
| `invalid_device_path` | A device library path is invalid or points outside the device | — |
| `inconsistent_device_library` | A device playlist or the history references a track or folder the device library lacks (`deviceTrackId` when named) | — |
| `inconsistent_request` | The tracks/playlists given to rbl-export are inconsistent (duplicate ids or audio paths, a missing parent folder) | — |
| `other` | Anything else | — |

rbl-export reports conflicts as text only, so rbx-cli derives `reason`,
`database`, `name` and `deviceTrackId` by matching rbxport's messages (at
the pinned revision) in one place (`src/conflict.rs`, with a test per
message); `tracks` is worked out by rbx-cli from the request, the device's
sync manifest and the files. `message` keeps rbxport's text.

## Cancellation

- **Signals**: SIGINT, SIGTERM and SIGHUP on Unix; Ctrl+C and Ctrl+Break on
  Windows. A second signal exits at once with 130.
- **stdin**: when the request is read from stdin, the rest of stdin is
  watched for control lines (with `--input FILE`, pass `--stdin-control`).
  A line `cancel` or `{"type":"cancel"}` cancels. End of input is not a
  cancellation (a request piped in ends with it). This is the portable way
  for a parent process to cancel on Windows.
- **stdin closing**: with `--cancel-on-stdin-eof` (which implies
  `--stdin-control`), the end of stdin (or a read error) also cancels. When
  the parent process dies, the OS closes its end of the pipe, so the export
  stops instead of running on unattended. The parent must keep stdin open
  for as long as the export runs (with the request on stdin: write it, then
  keep the pipe open). Capability `usb.export.stdinEofCancel`. It is a
  separate flag, rather than the behaviour of `--stdin-control`, because
  "end of input is not a cancellation" is what `--stdin-control` promises.

Cancellation is checked between tracks during analysis, between tracks while
rbl-export copies, and once more before publication. rbl-export stages the
whole new generation (audio, analysis, databases) beside the live one and
publishes it with a journalled, atomic commit: **a cancelled or interrupted
export leaves the previous library on the device intact**. Staged files are
discarded or completed (`rbl_export::recover`) by the next `usb export` or
`usb verify`. The terminal line is an `error` with code `cancelled`; exit 130.

## `usb export`

The request is a JSON document (`--input FILE`, or stdin). It describes the
**whole** desired state of the device: an export is a sync. Unknown fields
are ignored; check `version` capabilities for features.

```jsonc
{
  "protocol": 1,                     // optional; refused if newer than the CLI's
  "destination": "/Volumes/DJ STICK", // optional; `--to` overrides
  "options": {
    "analyze": "missing",            // missing | always | never
    "analysis": { "minBpm": 70, "maxBpm": 180, "highPrecision": true, "detectKey": true },
    "readTags": true,                // fill absent metadata from the file's tags
    "embeddedArtwork": true,         // use the embedded cover when `artwork` is absent
    "convert": "aiff",               // optional: wav | aiff | mp3 (rbxport compatibility conversion)
    "root": "auto",                  // auto | standard (PIONEER) | hidden (.PIONEER); fresh devices only
    "allowRekordboxRunning": false,
    "reuseDeviceAnalysis": true,
    "prune": true,
    "deviceName": "DJ STICK",        // optional
    "onDeviceChanges": "fail"        // fail | keepDevice
  },
  "tracks": [
    {
      "path": "/music/a.flac",       // required
      "ref": "a",                    // request-local name for playlists
      "id": 1234,                    // optional stable id (1..2^63-1)
      "title": "…", "artist": "…", "album": "…", "genre": "…", "label": "…",
      "key": "Am",                   // rekordbox spelling or Camelot (8A); normalised
      "comment": "…", "bpm": 128.0, "rating": 4, "color": 3,
      "year": 2024, "releaseDate": "2024-05-01", "dateAdded": "2026-10-01", "dateCreated": "…",
      "durationSec": 360, "bitrate": 320, "sampleRate": 44100, "bitDepth": 16, "fileSize": 1234,
      "trackNumber": 2, "discNumber": 1, "playCount": 0, "isrc": "…", "hotCueAutoLoad": true,
      "artwork": "/covers/a.jpg",    // JPEG/PNG; else the embedded cover
      "analysisPath": "/rb/share/…/ANLZ0000.DAT", // existing rekordbox analysis (EXT/2EX beside it)
      "beatGrid": { "anchors": [ { "timeMs": 120.5, "bpm": 128 } ] },
      "cues": [ { "type": "hot", "slot": "A", "timeMs": 120.5, "colorIndex": 3, "comment": "Drop" } ]
    }
  ],
  "playlists": [
    { "name": "Crates", "folder": true, "children": [
      { "name": "Peak", "id": 77, "tracks": ["a", 1] } ] }
  ]
}
```

### Tracks

Only `path` is required. Absent metadata is taken from the file's tags
(`readTags`), then from analysis (`bpm`: the caller grid's first tempo, else
the analysed tempo; `key`: the detected key when `detectKey`;
`durationSec`: the decoded length), else left empty. `dateAdded` defaults to
the date the device already has for the track, else today. `rating` 0–5;
`color` 0 none, 1 pink, 2 red, 3 orange, 4 yellow, 5 green, 6 aqua, 7 blue,
8 purple.

**Identity.** With `id`, a track is the same track across exports wherever
its file moves; without it, its absolute `path` is its identity. The device
id players see (`deviceId` in the result) is kept stable across syncs, as
rbxport does (decks cache artwork and waveforms by it).

**Missing files** are skipped (`track.skipped` event, `status: "skipped"`),
unless an earlier export put the track on the device: then the export is
refused with `conflict` (`details.reason`: `source_unavailable`,
`details.tracks`: those tracks) and the device is untouched (rbxport never deletes a
good device copy because the source went offline).

### Playlists

A tree in display order. A node is a playlist (`tracks`: `ref` strings or
zero-based indexes into `tracks`) or a folder (`folder: true`, `children`).
`id` (optional, 1..2^63-1) keeps a playlist's device identity across
renames; without it the path of names is the identity (a stable id is
derived from it). A track may be in several playlists or none.

### Options

| Option | Default | |
| --- | --- | --- |
| `analyze` | `missing` | `missing`: `analysisPath` if given, else cache, else device, else generate. `always`: ignore `analysisPath` and device analysis (the cache still applies: same input, same output). `never`: only `analysisPath`; other tracks go without analysis (no waveform/grid on players). |
| `analysis` | 70–180, true, true | rbxport's Analysis Setting dialog: BPM range (40–300), `highPrecision` (beats on kick attacks vs. the onset envelope), `detectKey`. |
| `readTags` | true | |
| `embeddedArtwork` | true | |
| `convert` | — | Convert audio a CDJ cannot play natively (rbxport's compatibility conversion) to WAV/AIFF/MP3 on the device. Sources are never changed. |
| `root` | `auto` | Library root on a device that has none yet; `auto` uses `.PIONEER` on HFS+ volumes like rekordbox. An existing library keeps its root; both present is a `conflict`. |
| `allowRekordboxRunning` | false | |
| `reuseDeviceAnalysis` | true | See caching. |
| `prune` | true | `false` keeps tracks an earlier export wrote that the request no longer lists (outside any playlist; metadata as on the device). Playlists absent from the request are always removed. |
| `deviceName` | — | The name players show (in `exportLibrary.db`). |
| `onDeviceChanges` | `fail` | A track whose cues or beat grid changed on the device since the last sync (e.g. cues saved on a player). `fail`: the export is refused with `conflict` (`cues_or_grid_changed_on_device`) when the request would replace them. `keepDevice`: for each such track, the device's cue lists and grid are kept and its `cues`/`beatGrid` ignored (a `warnings` entry says so); everything else is applied. See [Cues](#cues). Capability `usb.export.keepDeviceChanges`. |

**Formats.** Every export writes both the Device Library (`export.pdb`, for
older players) and Device Library Plus / OneLibrary (`exportLibrary.db`), as
rbxport does; rbl-export has no option to write only one, and verification
requires both to agree. A device's own settings, history, My Tags and
content written by other software (e.g. rekordbox) are preserved.

### Result (`data`)

```jsonc
{
  "destination": "/Volumes/DJ STICK", "root": "PIONEER", "dryRun": false,
  "tracks":   { "requested": 12, "exported": 12, "copied": 2, "reused": 10, "skipped": 0, "removed": 1, "kept": 0, "deviceChangesKept": 0 },
  "playlists":{ "written": 4, "added": 1, "removed": 0 },
  "analysis": { "generated": 2, "cacheHits": 10, "deviceReuse": 0, "supplied": 0, "none": 0, "failed": 0,
                "cacheMisses": 2, "gridOverrides": 1, "cueOverrides": 12 },
  "bytes":    { "copied": 24000000, "reused": 120000000, "toCopy": 24000000, "free": 3000000000 },
  "analysisFiles": 6, "artworkFiles": 4, "pdbBytes": 172032,
  "verified": true, "cacheDir": "…", "jobs": 3,
  "timings": { "planMs": 20, "analyzeMs": 1400, "exportMs": 2100, "verifyMs": 280, "totalMs": 3800 },
  "items": [
    { "index": 0, "ref": "a", "title": "…", "status": "exported", "analysis": "cache",
      "gridOverride": false, "cuesOverride": true, "deviceChangesKept": false, "artwork": true,
      "deviceId": 1, "devicePath": "/Contents/Artist/Album/a.flac",
      "analysisDir": "/PIONEER/USBANLZ/P051/0001470B", "analysisMs": 640, "warnings": [] }
  ]
}
```

`items[].analysis`: `cache`, `generated`, `device`, `supplied`, `none`,
`failed` (exported without analysis; see `warnings`), and with `--dry-run`
`generate`. `status`: `exported`, `skipped`, or `planned` (`--dry-run`).
`items[].deviceChangesKept` (count: `tracks.deviceChangesKept`): with
`onDeviceChanges: keepDevice`, the track's device cues and grid were kept
(with `--dry-run`: would be); `gridOverride`/`cuesOverride` are then false.
After the export both databases are read back (as rbxport does); a mismatch
is a `verification_failed` error rather than a result.

### `--dry-run`

Plans without writing anything (not even recovery of an interrupted export,
nor the cache): per track whether the audio would be copied or reused
(`items[].audio`: `copy`/`reuse`, an estimate from the device's sync
manifest by size and modification time — rbl-export confirms by content),
where analysis would come from (`cache`, `device`, `supplied`, `generate`,
`none`), the bytes to copy and the free space (`bytes.free`). A real export refuses
to start with `insufficient_space` when the estimate exceeds the free space
(not checked with `convert`, whose output size is unknown in advance).

`bytes.free` is the space available on the filesystem holding the
destination, whether the destination is a volume root or any folder: the
volume's figure when the OS lists it (as `devices list` does), else the
filesystem's own (`statvfs` on Unix, the volume's free space on Windows,
asked of the nearest existing ancestor). It is absent only when neither
can be read; the free-space check is then skipped.

## Analysis, caching and performance

Analysis is generated exactly as rbxport's Analyze command does it: decode
to mono (first 30 minutes at most) with rbl-audio, run rbl-analysis with the
`Rbxport` preset adjusted by `options.analysis`, and author `.DAT`, `.EXT`
and `.2EX` with rbl-anlz (grid, PWAV/PWV2–PWV7 waveforms, the measured PWV6
overview, empty cue lists). rbl-export then rewrites each file's `PPTH` for
the device path and adds the cue lists.

**Cache.** Default directory `<platform cache dir>/rbx-cli/analysis`
(`~/.cache` on Linux, `~/Library/Caches` on macOS, `%LOCALAPPDATA%` on
Windows); `--cache-dir` changes it, `--no-cache` disables it. The key is a
SHA-256 over: the canonical source path, size, modification time (ns), a
SHA-256 sample of the file (first, middle and last 64 KiB), the pinned
rbxport revision, the analysis settings that change the output (BPM range,
high precision, decode cap) and the cache format version. The value is the
three files **as generated** — never with caller cues or a caller grid — and
the analysed tempo, key and length. A changed file, a moved file, other
settings or another rbxport revision is a miss. Entries are written
atomically; there is no eviction (delete the directory to reclaim space).
Derived artwork (80×80 and 240×240 JPEGs) is cached beside it under
`v1/artwork`.

**Device reuse.** On a cache miss (`analyze: missing`,
`reuseDeviceAnalysis`), analysis an earlier rbx-cli export left on the
device is used when the device's sync manifest says the track's source had
the same size and modification time, and rbx-cli's record on the device
(`PIONEER/rbx-cli/analysis.json`) says it was made under the same cache key.
Its cue lists are stripped and it is put into the cache. Analysis that
carried a caller grid is only reused when the request brings a grid again.

**Parallelism.** Tracks are prepared (tags, cache lookup or analysis,
artwork) on `--jobs` threads (default 3, rbxport's default number of
concurrent analyses; rbl-analysis also runs its own stages in parallel per
track). rbl-export needs every track's analysis bytes before it starts, so
analysis cannot overlap the copy; it runs first, then one rbl-export pass
copies audio (its reader and writer overlap), writes the databases, verifies
the staged generation and publishes it.

**Unchanged audio** is not copied again: rbl-export compares the source's
size and modification time with its manifest and then hashes the source and
the device copy, so a re-export still reads every file once on each side.

## Cues

`cues` absent (field missing) and `cues: []` mean different things:

| Request | Effect on the device |
| --- | --- |
| field absent | **Keep** the cues the device holds for the track: the cue lists in its analysis files are carried into the new files, and `exportLibrary.db`'s cue rows are preserved by rbl-export. A first export has none. |
| `[]` | Clear all cues. |
| `[ … ]` | Replace with exactly these. |

A cue: `type` `hot` (with `slot` `A`–`P`; players show A–H, newer ones up
to P) or `memory` (no `slot`); `timeMs` (fractions are rounded; the format
holds whole milliseconds); `loopEndMs` makes it a loop (optionally
`loopNumerator`/`loopDenominator` for a beat loop's length, `activeLoop`);
`comment` is the name; colour: `color` 0–8 for memory cues (as track
colours), `colorIndex` 0–64 for hot cues (the players' hot cue palette; 0 is
the slot's default). Hot cues A–C and memory cues go to the `.DAT` lists
too; all go to the `.EXT` extended lists and `exportLibrary.db`.

Cues never affect the analysis cache: they are applied at export time.

**Cues saved on a player.** When a DJ stores memory (or hot) cues on the
stick, the player changes the track's analysis files on the device (and,
on OneLibrary players, possibly `exportLibrary.db`). On the next export:

- with `cues` **absent**, those cues are kept (the device's cue lists are
  carried over), so a routine sync does not lose them;
- with `cues` **given**, and the device's cues (or grid) changed since the
  last export, rbl-export refuses the whole export with `conflict`
  (`details.reason`: `cues_or_grid_changed_on_device`, `details.tracks`:
  the tracks whose analysis changed on the device) and changes nothing.
  Read them with `usb inspect --cues`, merge them into the caller's data,
  and export again with the merged list (sending exactly what the device
  holds is always accepted);
- with `options.onDeviceChanges: "keepDevice"`, a track whose analysis
  files on the device changed since the last sync keeps the device's cue
  lists **and** beat grid (`PCOB`/`PCO2`, `PQTZ`/`PQT2`; waveforms and
  everything else come from the cache as usual) and its `cues`/`beatGrid`
  are ignored; `exportLibrary.db`'s cue rows for it are preserved too.
  Other tracks get the request's cues, and the export succeeds. "Changed"
  means a file differs from what the last sync wrote (rbl-export records a
  hash of each), so the device wins only for this sync: once written, the
  next sync applies the request's `cues`/`beatGrid` (or the analysed grid)
  again unless the device changes again. Import what you want to keep
  (`usb inspect --cues`) before then. Only these analysis changes are
  covered: every other conflict (including cue rows changed only in
  `exportLibrary.db`, `onelibrary_cues_changed_on_device`) still fails.

## Beat grids

`beatGrid` replaces the analysed grid. Exactly one of:

- `beats`: every beat, in time order: `{ "timeMs": 10.5, "bpm": 128.0, "beatNumber": 1 }`.
  `beatNumber` (1–4, 1 = downbeat) defaults to one after the previous beat's.
- `anchors`: tempo anchors in time order, each `{ "timeMs", "bpm", "beatNumber" = 1 }`.
  Each anchor starts a constant-tempo stretch up to the next anchor (or the
  end of the track); beats are also extended back from the first anchor to
  0 ms. Variable tempo is a list of anchors. Needs the track's length
  (`durationSec`, or analysis).

Only the grid section is replaced (`PQTZ` in the `.DAT`; an `.EXT`'s
extended grid `PQT2`, which describes the old beats, is emptied, as
rbxport's grid editor does). Waveforms stay from the cache; nothing is
decoded again. `bpm` ≤ 655.35 (the format's limit). A track without
analysis (`analyze: never` and no `analysisPath`) gets no grid
(`track.warning`).

## `usb inspect`

`usb inspect ROOT [--summary] [--cues]` reads both databases (never writes)
and returns `destination`, `root`, `databases` (`deviceLibrary`,
`oneLibrary`), `ours` (written by rbx-cli/rbxport) and `written`,
`deviceName`, counts (`trackCount`, `playlistCount`, `historyCount`), and
unless `--summary`, `tracks` (device `id`, metadata, `bpm`, `path`,
`analysisPath`, `durationSec`, `dateAdded`, `source`/`sourceId` from the sync
manifest) and `playlists` (`id`, `parentId`, `name`, `folder`, `trackIds`).
`--cues` adds each track's `cues` (in the request's cue shape, read from the
`.EXT` extended list — including cues a player saved) and `beats` (grid size).
A device without a library is not an error: `root` is absent and counts are 0.

## `usb verify`

`usb verify ROOT` completes or discards an interrupted export first (the
only write it may make), then reads both databases and every referenced
audio and analysis file with rbxport's independent parser. `data`: `ok`,
`parsed`, `tracks`, `playlists`, `playlistEntries`, `audioPresent`,
`analysisPresent`, `overviewWaveforms`, `detailWaveforms`, `beatGrids`,
`hotCues`, `memoryCues`, `missingAudio`, `errors`. Problems are an `error`
with code `verification_failed` (exit 3) whose `details` is the same object.

## Devices

`devices list` → `{ "devices": [ { name, mountPoint, totalBytes, freeBytes,
fileSystem, removable, volumeId, export?: { tracks, playlists, ours, written } } ] }`.
`devices eject MOUNT_POINT` asks the OS to eject a volume that `devices
list` shows (never forced; any other path is `not_found`) →
`{ mountPoint, ejected: true }`.

## `version`

`{ name, version, protocol, rbxportRev, rbxportRepository, capabilities: [..], target }`.
Capabilities are stable strings (`usb.export`, `usb.export.cues`,
`usb.export.beatGrid.anchors`, `usb.export.analysisCache`, …); test for them
rather than comparing versions. Added in 0.1.1: `usb.export.conflictReasons`
(`details.reason` on `conflict` errors), `usb.export.stdinEofCancel`
(`--cancel-on-stdin-eof`) and `usb.export.keepDeviceChanges`
(`options.onDeviceChanges`).

## Versioning policy

- `protocol` (envelope and request/result shapes) is an integer, currently
  **1**, carried on every line and in `version`.
- **Additive** changes keep the number: new optional request fields, new
  result fields, new event names, new progress phases, new error codes, new
  commands, new capabilities. Readers must ignore what they do not know.
- **Breaking** changes bump it: removing or renaming a field, changing a
  type or a meaning, removing a command, changing an exit code.
- A request with a `protocol` newer than the CLI's is refused
  (`unsupported`); an older one is served.
- Later phases add a long-running `serve` mode (JSON-RPC over stdio) whose
  notifications reuse these envelopes (`progress`, `event`, `log`) and whose
  responses carry the same `result`/`error` payloads.
