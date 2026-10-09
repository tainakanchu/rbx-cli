# rbx-cli

CLI for rekordbox-compatible DJ libraries (USB export and more), built on
[rbxport](https://github.com/chrisle/rbxport) crates.

`rbx-cli` writes USB sticks that CDJ/XDJ players read — audio, analysis
(beat grid, waveforms, key), cue points, playlists and artwork in both the
Device Library (`export.pdb`) and Device Library Plus / OneLibrary
(`exportLibrary.db`) formats — and reads and verifies existing exports. It is
a thin, general-purpose layer over rbxport's `rbl-*` crates, so the sticks it
writes are the ones rbxport writes. It speaks plain text to people and NDJSON
to programs (`--json`), so any application can drive it as a separate
process.

> **Not affiliated with AlphaTheta Corporation or Pioneer DJ.** rekordbox,
> CDJ and XDJ are trademarks of AlphaTheta Corporation. This project is
> independent and not endorsed by them.

## Install

Download an archive for your platform from the
[releases](https://github.com/tainakanchu/rbx-cli/releases) (Linux x86-64,
macOS arm64/x86-64, Windows x86-64), check it against `SHA256SUMS`, and put
`rbx-cli` on your `PATH`. The binaries are self-contained (SQLCipher and
OpenSSL are compiled in).

From source (Rust 1.99, see `rust-toolchain.toml`; a C compiler is needed
for SQLCipher):

```sh
cargo install --locked --git https://github.com/tainakanchu/rbx-cli
```

## Usage

### `usb export`

Describe what the stick should hold in a JSON request and pass it with
`--input` (or on stdin):

```json
{
  "tracks": [
    { "path": "/music/Intro.flac", "ref": "intro", "title": "Intro", "artist": "Someone",
      "cues": [ { "type": "hot", "slot": "A", "timeMs": 1200, "comment": "Start" },
                { "type": "memory", "timeMs": 64000, "color": 5 } ] },
    { "path": "/music/Peak.mp3", "ref": "peak",
      "beatGrid": { "anchors": [ { "timeMs": 84.5, "bpm": 126 } ] } }
  ],
  "playlists": [
    { "name": "Gigs", "folder": true, "children": [
      { "name": "Friday", "tracks": ["intro", "peak"] } ] }
  ],
  "options": { "deviceName": "FRIDAY" }
}
```

```sh
rbx-cli usb export --input set.json --to "/Volumes/DJ STICK"
rbx-cli usb export --input set.json --to E:\ --dry-run     # plan only
cat set.json | rbx-cli usb export --to /media/stick --json  # NDJSON progress + result
```

An export is a sync: a second run copies only changed audio, keeps device
ids stable, and removes what the request no longer lists (`"prune": false`
keeps tracks). Tracks without analysis are analysed with rbxport's analyser;
caller-supplied beat grids and cues are written into the device analysis.
Everything a track can carry is in [docs/protocol.md](docs/protocol.md#usb-export).

### `usb inspect`, `usb verify`

```sh
rbx-cli usb inspect "/Volumes/DJ STICK"           # libraries, playlists, tracks
rbx-cli usb inspect "/Volumes/DJ STICK" --cues    # plus cues (incl. ones saved on a player)
rbx-cli usb verify  "/Volumes/DJ STICK"           # read everything back; exit 3 on problems
```

### `devices`, `version`

```sh
rbx-cli devices list
rbx-cli devices eject "/Volumes/DJ STICK"
rbx-cli version --json      # version, pinned rbxport revision, protocol, capabilities
```

## JSON mode

With `--json`, every line on stdout is one JSON envelope
(`{"type": "progress"|"event"|"log"|"result"|"error", "protocol": 1, ...}`),
ending with exactly one `result` or `error`. Exit codes: 0 ok, 1 error,
2 usage/invalid request, 3 verification failed, 130 cancelled. Cancel with
SIGINT/SIGTERM (Ctrl+C/Ctrl+Break on Windows) or by writing `cancel` to
stdin (`--cancel-on-stdin-eof` also cancels when stdin closes, e.g. because
the parent process died); the device keeps its previous library. The full contract — request
and result shapes, events, error codes, cue and grid semantics, versioning
— is in **[docs/protocol.md](docs/protocol.md)**, with JSON Schemas in
[`schema/`](schema).

## Caching and performance

- **Analysis cache.** Generated analysis is cached by source content
  (path, size, mtime and a sampled hash), settings and the pinned rbxport
  revision, in `<platform cache dir>/rbx-cli/analysis` (`--cache-dir`,
  `--no-cache`). Unchanged audio is never decoded twice; cues and caller
  grids are applied on top at export time and never invalidate it. On a
  cache miss, analysis an earlier export left on the stick is reused.
- **Parallel analysis** on `--jobs` threads (default 3, like rbxport).
- **Incremental copy.** Unchanged audio and analysis are not rewritten.

Measured on a 4-core Linux VM, release build: decode + analysis of a
5-minute track takes about 0.5–0.65 s (WAV/FLAC/320 kbps MP3); 8 such MP3s
analyse in 5.1 s with `--jobs 1` and 2.8 s with `--jobs 3`. A re-export of
those 8 tracks with everything cached and unchanged takes about 1.7 s, most
of it rbl-export's fixed cost of building, checking and re-reading the two
databases.

## Roadmap

- Phase 1 (this): USB export and sync, reading and verifying exports.
- Next: a long-running `serve` mode (JSON-RPC over stdio) with mount
  watching, reading rekordbox's `master.db`, Pro DJ Link status and Link
  Export.

## License

GPL-2.0-or-later — see [LICENSE](LICENSE) and [NOTICE](NOTICE). rbx-cli is
built on rbxport (Copyright (c) 2026 Chris Le, GPL-2.0-or-later). Release
binaries also contain `mp3lame-encoder` (LGPL-3.0), so a distributed binary
is effectively under GPL-3.0 (using the "or later" option).
