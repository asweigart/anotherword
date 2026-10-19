//! Fetching and unpacking source archives. Downloads are cached: if the target
//! file already exists we skip the network, so rebuilds work offline.
//!
//! The URL is a named constant so it's easy to pin or mirror.

use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use tar::Archive;

pub const WORDNET_URL: &str = "https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz";

/// Download `url` to `dest` unless `dest` already exists.
pub fn fetch(url: &str, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    eprintln!("anotherword: downloading {url}");
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let resp = ureq::get(url)
        .set("User-Agent", "anotherword/0.1")
        .call()
        .with_context(|| format!("requesting {url}"))?;

    // Download to a temp file first, then rename, so an interrupted download
    // never leaves a truncated "cached" file behind.
    let tmp = dest.with_extension("part");
    {
        let mut reader = resp.into_reader();
        let mut file =
            File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        std::io::copy(&mut reader, &mut file).context("writing download")?;
    }
    std::fs::rename(&tmp, dest)
        .with_context(|| format!("finalizing download to {}", dest.display()))?;
    Ok(())
}

/// Download and extract a `.tar.gz` into `extract_dir`, returning that dir. The
/// archive is cached alongside it. Extraction is skipped if `sentinel` exists.
pub fn fetch_and_extract_targz(
    url: &str,
    archive_path: &Path,
    extract_dir: &Path,
    sentinel: &Path,
) -> Result<PathBuf> {
    fetch(url, archive_path)?;
    if sentinel.exists() {
        return Ok(extract_dir.to_path_buf());
    }
    std::fs::create_dir_all(extract_dir)?;
    let file = File::open(archive_path)
        .with_context(|| format!("opening archive {}", archive_path.display()))?;
    let mut archive = Archive::new(GzDecoder::new(file));
    archive
        .unpack(extract_dir)
        .with_context(|| format!("extracting {}", archive_path.display()))?;
    Ok(extract_dir.to_path_buf())
}
