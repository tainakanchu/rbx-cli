// SPDX-License-Identifier: GPL-2.0-or-later
//! Cover art for the device.
//!
//! rbl-export copies, per image, the library's two small sizes that sit
//! beside the image it is given (`artwork_s.jpg`, 80×80, and
//! `artwork_m.jpg`, 240×240). Images from outside a rekordbox library have
//! no such siblings, so they are made here the way rbxport makes them when
//! it imports a picture: the middle square, Lanczos3, JPEG quality 90.
//!
//! Derived images are content-addressed (SHA-256 of the source image) so
//! two tracks of one album share one image on the device, and kept in the
//! cache directory so a re-export does not resize again.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Where derived artwork goes for this run.
pub struct ArtworkStore {
    dir: PathBuf,
    /// Held so a no-cache run's images live until the export is done.
    _temp: Option<tempfile::TempDir>,
}

impl ArtworkStore {
    pub fn new(cache_dir: Option<PathBuf>) -> std::io::Result<Self> {
        match cache_dir {
            Some(dir) => {
                std::fs::create_dir_all(&dir)?;
                Ok(Self { dir, _temp: None })
            }
            None => {
                let temp = tempfile::Builder::new()
                    .prefix("rbx-cli-artwork-")
                    .tempdir()?;
                Ok(Self {
                    dir: temp.path().to_owned(),
                    _temp: Some(temp),
                })
            }
        }
    }

    /// The image to hand rbl-export for an explicit artwork file.
    pub fn for_file(&self, image: &Path) -> Result<PathBuf, String> {
        let bytes = std::fs::read(image).map_err(|e| format!("{}: {e}", image.display()))?;
        self.derive_from_bytes(&bytes)
    }

    /// The embedded cover of an audio file, if it has one. What a file
    /// holds is remembered by its stamp, so an unchanged file is not
    /// re-parsed on the next export.
    pub fn embedded(
        &self,
        audio: &Path,
        stamp: Option<&crate::cache::SourceStamp>,
    ) -> Result<Option<PathBuf>, String> {
        let memo = stamp.map(|s| {
            let mut hash = Sha256::new();
            hash.update(s.canonical.to_string_lossy().as_bytes());
            hash.update(s.size.to_le_bytes());
            hash.update(s.modified_ns.to_le_bytes());
            hash.update(s.sample);
            self.dir
                .join("embedded")
                .join(crate::cache::hex(&hash.finalize()))
        });
        if let Some(memo) = &memo {
            if let Ok(text) = std::fs::read_to_string(memo) {
                let text = text.trim();
                if text == "none" {
                    return Ok(None);
                }
                let image = self.dir.join(text).join("artwork.jpg");
                if image.is_file() {
                    return Ok(Some(image));
                }
            }
        }
        let bytes = rbl_db::import::read_artwork(audio).map_err(|e| e.to_string())?;
        let result = match bytes {
            Some(bytes) => Some(self.derive_from_bytes(&bytes)?),
            None => None,
        };
        if let Some(memo) = memo {
            let value = result
                .as_ref()
                .and_then(|p| {
                    p.parent()?
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "none".to_owned());
            if let Some(parent) = memo.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = write_atomic(&memo, value.as_bytes());
        }
        Ok(result)
    }

    /// An image already on a device (`a<id>.jpg` and its `_m` medium
    /// size), carried as it is.
    pub fn for_device(&self, small: &Path, medium: &Path) -> Result<PathBuf, String> {
        let small = std::fs::read(small).map_err(|e| format!("{}: {e}", small.display()))?;
        let medium = std::fs::read(medium).unwrap_or_else(|_| small.clone());
        let mut hash = Sha256::new();
        hash.update(b"device");
        hash.update((small.len() as u64).to_le_bytes());
        hash.update(&small);
        hash.update(&medium);
        let dir = self.dir.join(crate::cache::hex(&hash.finalize()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for (name, bytes) in [
            ("artwork_s.jpg", &small),
            ("artwork_m.jpg", &medium),
            ("artwork.jpg", &medium),
        ] {
            if std::fs::read(dir.join(name)).ok().as_ref() != Some(bytes) {
                write_atomic(&dir.join(name), bytes).map_err(|e| e.to_string())?;
            }
        }
        Ok(dir.join("artwork.jpg"))
    }

    fn derive_from_bytes(&self, bytes: &[u8]) -> Result<PathBuf, String> {
        let id = crate::cache::hex(&Sha256::digest(bytes));
        let dir = self.dir.join(&id);
        let main = dir.join("artwork.jpg");
        if main.is_file()
            && dir.join("artwork_s.jpg").is_file()
            && dir.join("artwork_m.jpg").is_file()
        {
            return Ok(main);
        }
        let decoded =
            image::load_from_memory(bytes).map_err(|e| format!("unreadable image: {e}"))?;
        let side = decoded.width().min(decoded.height());
        if side == 0 {
            return Err("empty image".into());
        }
        let square = decoded.crop_imm(
            (decoded.width() - side) / 2,
            (decoded.height() - side) / 2,
            side,
            side,
        );
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut medium = Vec::new();
        for (name, size) in [("artwork_m.jpg", 240_u32), ("artwork_s.jpg", 80_u32)] {
            let small = square
                .resize_exact(size, size, image::imageops::FilterType::Lanczos3)
                .to_rgb8();
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
                .encode_image(&small)
                .map_err(|e| e.to_string())?;
            write_atomic(&dir.join(name), &out).map_err(|e| e.to_string())?;
            if size == 240 {
                medium = out;
            }
        }
        // The file the track names; rbl-export reads the two siblings.
        write_atomic(&main, &medium).map_err(|e| e.to_string())?;
        Ok(main)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn an_image_becomes_two_square_sizes_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtworkStore::new(Some(dir.path().join("art"))).unwrap();
        let source = dir.path().join("cover.png");
        image::RgbImage::from_pixel(300, 200, image::Rgb([200, 10, 10]))
            .save(&source)
            .unwrap();
        let first = store.for_file(&source).unwrap();
        let second = store.for_file(&source).unwrap();
        assert_eq!(first, second, "content-addressed");
        let small = image::open(first.with_file_name("artwork_s.jpg")).unwrap();
        assert_eq!((small.width(), small.height()), (80, 80));
        let medium = image::open(first.with_file_name("artwork_m.jpg")).unwrap();
        assert_eq!((medium.width(), medium.height()), (240, 240));
        let junk = dir.path().join("junk.jpg");
        std::fs::write(&junk, b"not an image").unwrap();
        assert!(store.for_file(&junk).is_err());
    }
}
