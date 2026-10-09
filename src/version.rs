// SPDX-License-Identifier: GPL-2.0-or-later
//! `rbx-cli version`: what this binary is and what it can do.

use schemars::JsonSchema;
use serde::Serialize;

use crate::protocol::PROTOCOL_VERSION;

/// The rbxport commit every `rbl-*` dependency is pinned to. Must equal the
/// `rev` in Cargo.toml (a test checks).
pub const RBXPORT_REV: &str = "a029c0c2b9d6c9732b4634ee8be42ff66bbfc56c";

/// Feature flags a caller can test for instead of comparing versions.
/// Only ever added to within one protocol version.
pub const CAPABILITIES: &[&str] = &[
    "usb.export",
    "usb.export.dryRun",
    "usb.export.analysis",
    "usb.export.analysisCache",
    "usb.export.deviceAnalysisReuse",
    "usb.export.beatGrid.beats",
    "usb.export.beatGrid.anchors",
    "usb.export.cues",
    "usb.export.artwork.embedded",
    "usb.export.artwork.file",
    "usb.export.convert",
    "usb.export.prune",
    "usb.export.deviceName",
    "usb.export.stdinCancel",
    "usb.export.stdinEofCancel",
    "usb.format.deviceLibrary",
    "usb.format.oneLibrary",
    "usb.inspect",
    "usb.verify",
    "devices.list",
    "devices.eject",
    "conflict-reasons",
];

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub name: &'static str,
    /// This CLI's semver version.
    pub version: &'static str,
    /// The wire protocol version (see docs/protocol.md).
    pub protocol: u32,
    /// The rbxport commit the `rbl-*` crates are built from.
    pub rbxport_rev: &'static str,
    pub rbxport_repository: &'static str,
    /// Supported features; see docs/protocol.md.
    pub capabilities: Vec<&'static str>,
    /// `<os>-<arch>` of this build.
    pub target: String,
}

#[must_use]
pub fn info() -> VersionInfo {
    VersionInfo {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        protocol: PROTOCOL_VERSION,
        rbxport_rev: RBXPORT_REV,
        rbxport_repository: "https://github.com/chrisle/rbxport",
        capabilities: CAPABILITIES.to_vec(),
        target: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
    }
}

#[must_use]
pub fn human(info: &VersionInfo) -> String {
    format!(
        "{} {} (protocol {}, rbxport {})",
        info.name,
        info.version,
        info.protocol,
        &info.rbxport_rev[..12.min(info.rbxport_rev.len())]
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_rev_matches_cargo_toml() {
        let manifest = include_str!("../Cargo.toml");
        let pins: Vec<&str> = manifest
            .lines()
            .filter(|l| l.starts_with("rbl-"))
            .map(|l| {
                l.split("rev = \"")
                    .nth(1)
                    .and_then(|r| r.split('"').next())
                    .expect("a rev")
            })
            .collect();
        assert!(!pins.is_empty());
        assert!(pins.iter().all(|rev| *rev == RBXPORT_REV), "{pins:?}");
    }
}
