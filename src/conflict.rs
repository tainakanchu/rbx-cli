// SPDX-License-Identifier: GPL-2.0-or-later
//! Machine-readable reasons for `conflict` errors.
//!
//! rbl-export reports every conflict as `ExportError::Conflict(String)`, an
//! English sentence with no structured kind. This module is the one place
//! that turns that text into a stable [`ConflictReason`] (plus the names
//! rbxport put in the sentence), so callers never parse messages. The
//! patterns follow rbl-export at the pinned `RBXPORT_REV`; the tests below
//! pin each one, and anything unrecognised is [`ConflictReason::Other`].

use schemars::JsonSchema;
use serde::Serialize;

/// Why the device's library conflicts with the request. Stable snake_case
/// strings; new reasons may be added, so treat unknown ones as `other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConflictReason {
    /// A track's analysis files on the device (cue lists or beat grid)
    /// changed since the last sync, e.g. cues saved on a player, and the
    /// request would replace them. rbxport does not say which of the two.
    CuesOrGridChangedOnDevice,
    /// `exportLibrary.db` holds cue records changed on the device since the
    /// last sync that this export would replace.
    OnelibraryCuesChangedOnDevice,
    /// A track's metadata changed on the device since the last sync.
    TrackChangedOnDevice,
    /// A playlist changed on the device since the last sync.
    PlaylistChangedOnDevice,
    /// My Tags (or a track's My Tags) changed on the device.
    MyTagsChangedOnDevice,
    /// Tracks or playlists were deleted on the device since the last sync.
    DeletedOnDevice,
    /// A track only the device has (not from an earlier sync) would be lost.
    DeviceOnlyTrack,
    /// A playlist only the device has would be lost.
    DeviceOnlyPlaylist,
    /// A track being removed is still referenced by the device's history.
    HistoryReferencesTrack,
    /// A track an earlier export put on the device has no readable source
    /// (missing, or it disappeared while being copied).
    SourceUnavailable,
    /// The device belongs to a different (or older, unverified) library.
    Ownership,
    /// rekordbox changed the device's track identities since the last sync.
    IdentitiesChanged,
    /// The device's two existing libraries (`export.pdb`,
    /// `exportLibrary.db`) disagree.
    LibrariesDisagree,
    /// Both `PIONEER` and `.PIONEER` hold a library.
    BothRoots,
    /// A device database cannot be read or fails its integrity check.
    UnreadableLibrary,
    /// `exportLibrary.db` has a schema version rbxport does not support.
    UnsupportedOnelibrary,
    /// Another program changed the device while the export was staging.
    DeviceChangedDuringSync,
    /// The staged export did not verify before publication.
    StagedVerificationFailed,
    /// A device library path is invalid or points outside the device.
    InvalidDevicePath,
    /// The device library is internally inconsistent (a playlist or the
    /// history references a track or folder it does not have).
    InconsistentDeviceLibrary,
    /// The tracks/playlists handed to rbl-export are inconsistent
    /// (duplicate ids or paths, a missing parent folder).
    InconsistentRequest,
    /// Anything not recognised above.
    Other,
}

/// Which of the device's databases a conflict is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Database {
    /// `export.pdb`.
    DeviceLibrary,
    /// `exportLibrary.db`.
    OneLibrary,
}

/// A request track a conflict concerns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConflictTrack {
    /// Zero-based index into the request's `tracks`.
    pub index: usize,
    /// The track's `ref` from the request, when it had one.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// The track's id on the device (from the last sync), when known.
    pub device_id: Option<u32>,
}

/// `details` of a `conflict` error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDetails {
    pub reason: ConflictReason,
    /// The database rbxport named, when it named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<Database>,
    /// The track title, playlist or My Tag name rbxport named, when it
    /// named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The device track id rbxport named, when it named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_track_id: Option<u32>,
    /// Request tracks concerned, when rbx-cli can tell (`usb export` only):
    /// for `source_unavailable`, every listed track the device holds whose
    /// source is not readable; for `cues_or_grid_changed_on_device`, every
    /// listed track whose analysis files on the device changed since the
    /// last sync; for `track_changed_on_device`, the listed tracks with the
    /// title rbxport named.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<ConflictTrack>,
}

impl ConflictDetails {
    #[must_use]
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// The text between the first `open` and the last `close` after it.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].rfind(close)? + start;
    Some(&text[start..end])
}

/// `Device Library: …` / `OneLibrary …` at the start of a message.
fn database_prefix(message: &str) -> Option<Database> {
    if message.starts_with("Device Library") {
        Some(Database::DeviceLibrary)
    } else if message.starts_with("OneLibrary") {
        Some(Database::OneLibrary)
    } else {
        None
    }
}

/// Classifies the text of an `ExportError::Conflict` (without the
/// `USB sync conflict: ` prefix of its `Display`).
#[must_use]
#[allow(clippy::too_many_lines, reason = "one arm per rbl-export message")]
pub fn classify(message: &str) -> ConflictDetails {
    use ConflictReason as R;
    let mut details = ConflictDetails {
        reason: R::Other,
        database: None,
        name: None,
        device_track_id: None,
        tracks: Vec::new(),
    };
    let quoted = |open: &str, close: &str| between(message, open, close).map(str::to_owned);
    let has = |needle: &str| message.contains(needle);

    if has("USB cues or beat grids changed since the last sync") {
        details.reason = R::CuesOrGridChangedOnDevice;
    } else if message.starts_with("OneLibrary contains cue records") {
        details.reason = R::OnelibraryCuesChangedOnDevice;
        details.database = Some(Database::OneLibrary);
    } else if has(": playlist '") && has("' changed on the USB") {
        details.reason = R::PlaylistChangedOnDevice;
        details.database = database_prefix(message);
        details.name = quoted(": playlist '", "' changed on the USB");
    } else if has(": device-only track '") && has("' would be lost") {
        details.reason = R::DeviceOnlyTrack;
        details.database = database_prefix(message);
        details.name = quoted(": device-only track '", "' would be lost");
    } else if message.starts_with("Device-only playlist '") {
        details.reason = R::DeviceOnlyPlaylist;
        details.name = quoted("Device-only playlist '", "' would be lost");
    } else if message.starts_with("My Tag '") {
        details.reason = R::MyTagsChangedOnDevice;
        details.name = quoted("My Tag '", "' changed on the USB");
    } else if let Some(id) = message.strip_prefix("My Tags changed on USB track ") {
        details.reason = R::MyTagsChangedOnDevice;
        details.device_track_id = id.trim().parse().ok();
    } else if database_prefix(message).is_some() && has(": '") && has("' changed on the USB") {
        details.reason = R::TrackChangedOnDevice;
        details.database = database_prefix(message);
        details.name = quoted(": '", "' changed on the USB");
    } else if has("contains device-side deletions") {
        details.reason = R::DeletedOnDevice;
        details.database = database_prefix(message);
    } else if has("still referenced by USB history") {
        details.reason = R::HistoryReferencesTrack;
    } else if message.starts_with("Source unavailable for '") {
        details.reason = R::SourceUnavailable;
        details.name = quoted("Source unavailable for '", "'. Reconnect");
    } else if message.starts_with("Source disappeared while copying '") {
        details.reason = R::SourceUnavailable;
        details.name = quoted("Source disappeared while copying '", "': ");
    } else if has("belongs to a different or older unverified master library") {
        details.reason = R::Ownership;
    } else if has("changed the device track identities") {
        details.reason = R::IdentitiesChanged;
    } else if has("two existing device libraries disagree") {
        details.reason = R::LibrariesDisagree;
    } else if has("Both PIONEER and .PIONEER") {
        details.reason = R::BothRoots;
    } else if message.starts_with("Cannot read Device Library")
        || message.starts_with("Invalid Device Library")
    {
        details.reason = R::UnreadableLibrary;
        details.database = Some(Database::DeviceLibrary);
    } else if message.starts_with("OneLibrary integrity check failed") {
        details.reason = R::UnreadableLibrary;
        details.database = Some(Database::OneLibrary);
    } else if message.starts_with("Unsupported OneLibrary schema") {
        details.reason = R::UnsupportedOnelibrary;
        details.database = Some(Database::OneLibrary);
    } else if has("The device changed during sync") {
        details.reason = R::DeviceChangedDuringSync;
    } else if message.starts_with("Staged export did not verify") {
        details.reason = R::StagedVerificationFailed;
    } else if message.starts_with("Invalid device-relative path") || has("points outside the USB") {
        details.reason = R::InvalidDevicePath;
    } else if let Some(id) =
        message.strip_prefix("Device playlist/history references missing track ")
    {
        details.reason = R::InconsistentDeviceLibrary;
        details.device_track_id = id.trim().parse().ok();
    } else if [
        "Missing device playlist ancestor",
        "Unresolved device playlist track",
        "Missing USB track metadata",
        "Missing track in existing playlist",
    ]
    .iter()
    .any(|m| message.starts_with(m))
    {
        details.reason = R::InconsistentDeviceLibrary;
    } else if message.starts_with("Missing parent folder for '") {
        details.reason = R::InconsistentRequest;
        details.name = quoted("Missing parent folder for '", "'");
    } else if [
        "Duplicate source playlist ID",
        "Ambiguous playlist identity",
        "Duplicate tracks or invalid playlist membership",
        "Conversion would create duplicate audio path",
    ]
    .iter()
    .any(|m| message.starts_with(m))
    {
        details.reason = R::InconsistentRequest;
    }
    details
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{classify, ConflictReason as R, Database as D};

    /// Message, reason, database, name, device track id.
    type Case = (
        &'static str,
        R,
        Option<D>,
        Option<&'static str>,
        Option<u32>,
    );

    /// Every `ExportError::Conflict` text rbl-export produces at the pinned
    /// revision, as it formats it, with what it must classify as.
    #[test]
    fn rbl_export_messages_are_classified() {
        let cases: &[Case] = &[
            ("USB cues or beat grids changed since the last sync. Import the USB cues/grids before exporting.", R::CuesOrGridChangedOnDevice, None, None, None),
            ("OneLibrary contains cue records that this export would replace. Import the cues in rekordbox first.", R::OnelibraryCuesChangedOnDevice, Some(D::OneLibrary), None, None),
            ("Device Library: 'It's On' changed on the USB. Import its changes before syncing.", R::TrackChangedOnDevice, Some(D::DeviceLibrary), Some("It's On"), None),
            ("OneLibrary: 'Track' changed on the USB. Import its changes before syncing.", R::TrackChangedOnDevice, Some(D::OneLibrary), Some("Track"), None),
            ("OneLibrary: device-only track 'Solo' would be lost", R::DeviceOnlyTrack, Some(D::OneLibrary), Some("Solo"), None),
            ("Device Library: playlist 'Peak' changed on the USB. Import or reconcile it before syncing.", R::PlaylistChangedOnDevice, Some(D::DeviceLibrary), Some("Peak"), None),
            ("Device-only playlist 'Gig' would be lost", R::DeviceOnlyPlaylist, None, Some("Gig"), None),
            ("OneLibrary contains device-side deletions. Reconcile them before syncing.", R::DeletedOnDevice, Some(D::OneLibrary), None, None),
            ("My Tag 'Warm up' changed on the USB", R::MyTagsChangedOnDevice, None, Some("Warm up"), None),
            ("My Tags changed on USB track 12", R::MyTagsChangedOnDevice, None, None, Some(12)),
            ("A track being removed is still referenced by USB history. Import and clear that history in rekordbox first.", R::HistoryReferencesTrack, None, None, None),
            ("Source unavailable for 'A'. Reconnect or relocate it before syncing; the USB has not been changed.", R::SourceUnavailable, None, Some("A"), None),
            ("Source disappeared while copying 'B': No such file or directory (os error 2)", R::SourceUnavailable, None, Some("B"), None),
            ("This USB belongs to a different or older unverified master library. Import its contents in rekordbox before changing its ownership.", R::Ownership, None, None, None),
            ("rekordbox changed the device track identities. Reconcile this device before reusing its previous selection.", R::IdentitiesChanged, None, None, None),
            ("The two existing device libraries disagree. Reconcile them in rekordbox before their first sync here.", R::LibrariesDisagree, None, None, None),
            ("Both PIONEER and .PIONEER contain libraries. Reconcile them before syncing.", R::BothRoots, None, None, None),
            ("Cannot read Device Library: bad page", R::UnreadableLibrary, Some(D::DeviceLibrary), None, None),
            ("Invalid Device Library: bad page", R::UnreadableLibrary, Some(D::DeviceLibrary), None, None),
            ("OneLibrary integrity check failed: row 3 missing", R::UnreadableLibrary, Some(D::OneLibrary), None, None),
            ("Unsupported OneLibrary schema 9.9.9; the device was left unchanged.", R::UnsupportedOnelibrary, Some(D::OneLibrary), None, None),
            ("The device changed during sync. Close other writers and retry.", R::DeviceChangedDuringSync, None, None, None),
            ("Staged export did not verify: []; bad", R::StagedVerificationFailed, None, None, None),
            ("Invalid device-relative path", R::InvalidDevicePath, None, None, None),
            ("A device file points outside the USB", R::InvalidDevicePath, None, None, None),
            ("Device playlist/history references missing track 7", R::InconsistentDeviceLibrary, None, None, Some(7)),
            ("Missing device playlist ancestor", R::InconsistentDeviceLibrary, None, None, None),
            ("Unresolved device playlist track", R::InconsistentDeviceLibrary, None, None, None),
            ("Missing USB track metadata", R::InconsistentDeviceLibrary, None, None, None),
            ("Missing track in existing playlist", R::InconsistentDeviceLibrary, None, None, None),
            ("Duplicate source playlist ID", R::InconsistentRequest, None, None, None),
            ("Ambiguous playlist identity", R::InconsistentRequest, None, None, None),
            ("Missing parent folder for 'Crates'", R::InconsistentRequest, None, Some("Crates"), None),
            ("Duplicate tracks or invalid playlist membership", R::InconsistentRequest, None, None, None),
            ("Conversion would create duplicate audio path: /Contents/a.wav", R::InconsistentRequest, None, None, None),
            ("Missing OneLibrary output", R::Other, None, None, None),
            ("Analysis directory space exhausted", R::Other, None, None, None),
            ("something new", R::Other, None, None, None),
        ];
        for (message, reason, database, name, id) in cases {
            let details = classify(message);
            assert_eq!(details.reason, *reason, "{message}");
            assert_eq!(details.database, *database, "{message}");
            assert_eq!(details.name.as_deref(), *name, "{message}");
            assert_eq!(details.device_track_id, *id, "{message}");
        }
    }

    #[test]
    fn reasons_serialise_as_snake_case() {
        let value = classify("Both PIONEER and .PIONEER contain libraries.").to_value();
        assert_eq!(value, serde_json::json!({ "reason": "both_roots" }));
        let value = classify("Device Library: 'X' changed on the USB.").to_value();
        assert_eq!(
            value,
            serde_json::json!({ "reason": "track_changed_on_device", "database": "deviceLibrary", "name": "X" })
        );
    }
}
