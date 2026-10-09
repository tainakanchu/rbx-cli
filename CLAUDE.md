# rbx-cli — notes for agents

A GPL-2.0-or-later CLI over rbxport's `rbl-*` crates (git dependencies pinned
to one `rev`). Keep it generic: no consumer-specific names, no source-app
logic (no Traktor/Serato/… parsing); callers convert their data into the
request.

## Layout

- `src/lib.rs` — entry (`main()`), dispatch; `src/cli.rs` — clap definitions.
- `src/protocol.rs` — envelope types, error codes, exit codes, `PROTOCOL_VERSION`.
- `src/output.rs` — NDJSON / human output; `src/logging.rs` — tracing → stderr or `log` lines.
- `src/cancel.rs` — signals and stdin `cancel` lines.
- `src/request.rs` — `usb export` request types + validation.
- `src/export.rs` — `usb export` (parallel prepare → rbl-export → verify).
- `src/analyze.rs` (rbl-audio/rbl-analysis/rbl-anlz pipeline), `src/cache.rs`
  (analysis cache), `src/anlz.rs` (grid swap, cue lists), `src/grid.rs`
  (caller grids), `src/artwork.rs`, `src/device.rs` (reading a device).
- `src/usb.rs` (`inspect`, `verify`), `src/devices.rs`, `src/version.rs`.
- `src/schema.rs` → `schema/*.json`; `docs/protocol.md` is the contract.
- `tests/cli.rs` — end-to-end tests (WAVs generated in the test).

## Commands

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test                                  # slow first build (SQLCipher, symphonia)
RBX_CLI_UPDATE_SCHEMA=1 cargo test schema   # regenerate schema/ after changing types
```

## Rules

- Protocol versioning: additive changes (new optional request fields, new
  result fields/events/phases/error codes/capabilities) keep
  `PROTOCOL_VERSION`; anything breaking bumps it and is documented in
  `docs/protocol.md`. Update the schema files and the docs with every
  protocol change; add a capability string in `src/version.rs` for new
  features.
- `--json` stdout carries envelopes only, exactly one terminal
  `result`/`error`, last. Never `println!` elsewhere.
- Bump the rbxport `rev` for every `rbl-*` crate at once, and
  `RBXPORT_REV` in `src/version.rs` (a test checks); the toolchain follows
  rbxport's.
- Do not vendor or copy rbxport source; if an API is missing, say so.
- Every source file starts with `// SPDX-License-Identifier: GPL-2.0-or-later`.
