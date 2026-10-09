// SPDX-License-Identifier: GPL-2.0-or-later
//! The analysis cache: generated ANLZ files keyed by what they were made
//! from, so unchanged audio is never decoded and analysed twice.
//!
//! Key: canonical source path, size, modification time (ns), a SHA-256 of
//! the first, middle and last 64 KiB, the pinned rbxport revision, the
//! analysis settings that change the output, and this cache's format
//! version. Value: the three files exactly as generated (empty cue lists,
//! the analysed grid) plus the analysis facts (BPM, key, length).
//!
//! Layout: `<dir>/v1/<2 hex>/<64 hex>/{meta.json,ANLZ0000.DAT,.EXT,.2EX}`.
//! Entries are written to a temporary directory and renamed into place,
//! so concurrent runs and crashes never leave half an entry.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analyze::{AnalysisMeta, Generated};

const FORMAT: &str = "v1";
const SAMPLE: u64 = 64 * 1024;

/// What a source file is, cheaply: enough to tell "unchanged" without
/// reading the whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStamp {
    pub canonical: PathBuf,
    pub size: u64,
    /// Nanoseconds since the Unix epoch, as rbl-export stamps sources.
    pub modified_ns: i64,
    /// SHA-256 over the size and the first, middle and last 64 KiB.
    pub sample: [u8; 32],
}

impl SourceStamp {
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let canonical = std::fs::canonicalize(path)?;
        let mut file = std::fs::File::open(&canonical)?;
        let meta = file.metadata()?;
        let size = meta.len();
        let modified_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_nanos()).ok())
            .unwrap_or(0);
        let mut hash = Sha256::new();
        hash.update(size.to_le_bytes());
        let mut buffer = vec![0_u8; usize::try_from(SAMPLE).unwrap_or(65_536)];
        let mut offsets = vec![0];
        if size > SAMPLE {
            offsets.push(size / 2 - SAMPLE / 2);
            offsets.push(size - SAMPLE);
        }
        for offset in offsets {
            file.seek(SeekFrom::Start(offset))?;
            let mut filled = 0;
            while filled < buffer.len() {
                let n = file.read(&mut buffer[filled..])?;
                if n == 0 {
                    break;
                }
                filled += n;
            }
            hash.update(&buffer[..filled]);
        }
        Ok(Self {
            canonical,
            size,
            modified_ns,
            sample: hash.finalize().into(),
        })
    }
}

/// The cache key for one source under one set of settings.
pub fn key(stamp: &SourceStamp, settings_fingerprint: &str) -> String {
    let mut hash = Sha256::new();
    for part in [
        FORMAT.as_bytes(),
        crate::version::RBXPORT_REV.as_bytes(),
        settings_fingerprint.as_bytes(),
        stamp.canonical.to_string_lossy().as_bytes(),
    ] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    hash.update(stamp.size.to_le_bytes());
    hash.update(stamp.modified_ns.to_le_bytes());
    hash.update(stamp.sample);
    hex(&hash.finalize())
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    meta: AnalysisMeta,
    extensions: Vec<String>,
    rbxport_rev: String,
}

/// An analysis cache directory, or none (`--no-cache`).
#[derive(Debug, Clone)]
pub struct Cache {
    root: Option<PathBuf>,
}

impl Cache {
    #[must_use]
    pub fn disabled() -> Self {
        Self { root: None }
    }

    #[must_use]
    pub fn at(dir: PathBuf) -> Self {
        Self { root: Some(dir) }
    }

    /// `dirs::cache_dir()/rbx-cli/analysis`, when the platform has one.
    #[must_use]
    pub fn default_dir() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("rbx-cli").join("analysis"))
    }

    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    fn entry_dir(&self, key: &str) -> Option<PathBuf> {
        let root = self.root.as_ref()?;
        Some(
            root.join(FORMAT)
                .join(key.get(..2).unwrap_or("00"))
                .join(key),
        )
    }

    /// Whether an entry exists, without reading it (for `--dry-run`).
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.entry_dir(key)
            .is_some_and(|d| d.join("meta.json").is_file())
    }

    /// The entry for `key`, or `None` (also for a damaged entry, which is
    /// then treated as a miss and overwritten).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Generated> {
        let dir = self.entry_dir(key)?;
        let entry: Entry =
            serde_json::from_slice(&std::fs::read(dir.join("meta.json")).ok()?).ok()?;
        let mut files = Vec::with_capacity(entry.extensions.len());
        for extension in entry.extensions {
            let bytes = std::fs::read(dir.join(format!("ANLZ0000.{extension}"))).ok()?;
            rbl_anlz::parse(&bytes).ok()?;
            files.push((extension, bytes));
        }
        files.iter().any(|(e, _)| e == "DAT").then_some(Generated {
            files,
            meta: entry.meta,
        })
    }

    /// Stores an entry. Failure is reported, not fatal: the export goes on
    /// without caching.
    pub fn put(&self, key: &str, generated: &Generated) -> std::io::Result<()> {
        let Some(dir) = self.entry_dir(key) else {
            return Ok(());
        };
        let parent = dir.parent().unwrap_or(&dir);
        std::fs::create_dir_all(parent)?;
        let staging = tempfile::Builder::new()
            .prefix(".tmp-")
            .tempdir_in(parent)?;
        for (extension, bytes) in &generated.files {
            std::fs::write(staging.path().join(format!("ANLZ0000.{extension}")), bytes)?;
        }
        let entry = Entry {
            meta: generated.meta.clone(),
            extensions: generated.files.iter().map(|(e, _)| e.clone()).collect(),
            rbxport_rev: crate::version::RBXPORT_REV.to_owned(),
        };
        std::fs::write(
            staging.path().join("meta.json"),
            serde_json::to_vec_pretty(&entry).map_err(std::io::Error::other)?,
        )?;
        let staged = staging.keep();
        if let Err(e) = std::fs::rename(&staged, &dir) {
            let _ = std::fs::remove_dir_all(&staged);
            // Another run stored the same entry first; theirs is as good.
            if !dir.join("meta.json").is_file() {
                return Err(e);
            }
        }
        Ok(())
    }

    /// A directory for derived artwork beside the analysis entries.
    #[must_use]
    pub fn artwork_dir(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|r| r.join(FORMAT).join("artwork"))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_key_follows_content_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, vec![7_u8; 300_000]).unwrap();
        let a = SourceStamp::read(&path).unwrap();
        assert_eq!(a, SourceStamp::read(&path).unwrap());
        assert_eq!(key(&a, "s"), key(&a, "s"));
        assert_ne!(key(&a, "s"), key(&a, "t"));
        let mut middle = vec![7_u8; 300_000];
        middle[150_000] = 8;
        std::fs::write(&path, &middle).unwrap();
        let b = SourceStamp::read(&path).unwrap();
        assert_ne!(
            a.sample, b.sample,
            "a changed byte in the middle sample is noticed"
        );
    }

    #[test]
    fn entries_round_trip_and_damage_reads_as_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().to_owned());
        let dat = rbl_anlz::AnlzBuilder::new().path("/x.wav").finish();
        let generated = Generated {
            files: vec![("DAT".into(), dat)],
            meta: AnalysisMeta {
                bpm: 128.0,
                key: Some("Am".into()),
                duration_ms: 1000,
                full_length: true,
                beats: 2,
            },
        };
        assert!(cache.get("abcd").is_none());
        cache.put("abcd", &generated).unwrap();
        cache.put("abcd", &generated).unwrap();
        assert!(cache.contains("abcd"));
        let back = cache.get("abcd").unwrap();
        assert_eq!(back.files, generated.files);
        assert_eq!(back.meta, generated.meta);
        std::fs::write(dir.path().join("v1/ab/abcd/ANLZ0000.DAT"), b"junk").unwrap();
        assert!(cache.get("abcd").is_none());
        assert!(Cache::disabled().get("abcd").is_none());
    }
}
