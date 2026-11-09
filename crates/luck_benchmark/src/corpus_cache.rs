//! The gitignored `corpus/` cache that network-fetched bench inputs land in.

#![expect(clippy::panic, reason = "a bench has no way to report a failed corpus load")]

use core::time::Duration;
use std::path::{Path, PathBuf};
use std::{fs, thread};

use flate2::read::GzDecoder;
use tar::Archive;

pub(crate) fn cache_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus")
}

/// Fetches `url`, retrying with backoff.
///
/// CI runners see transient connection resets from GitHub's raw and
/// codeload hosts, and one failed fetch would fail the whole bench job.
///
/// # Panics
///
/// Panics when every attempt fails.
fn fetch_bytes(url: &str) -> Vec<u8> {
    let mut last_error = String::new();
    for delay in [Duration::ZERO, Duration::from_secs(1), Duration::from_secs(2)] {
        thread::sleep(delay);
        match ureq::get(url).call() {
            Ok(mut response) => match response.body_mut().read_to_vec() {
                Ok(bytes) => return bytes,
                Err(error) => last_error = error.to_string(),
            },
            Err(error) => last_error = error.to_string(),
        }
    }
    panic!("failed to fetch {url} after 3 attempts: {last_error}")
}

/// Reads `file_name` from the cache, fetching it from under `base_url` on a
/// miss.
///
/// # Panics
///
/// Panics when the file can't be fetched or isn't UTF-8.
pub(crate) fn fetch_corpus_file(base_url: &str, file_name: &str) -> String {
    let cache_path = cache_directory().join(file_name);
    if let Ok(text) = fs::read_to_string(&cache_path) {
        return text;
    }
    let url = format!("{base_url}/{file_name}");
    let bytes = fetch_bytes(&url);
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("failed to read {url} as UTF-8: {error}"));
    let _unused =
        fs::create_dir_all(cache_directory()).and_then(|()| fs::write(&cache_path, &text));
    text
}

/// Downloads and extracts a pinned GitHub tarball into the cache once, and
/// returns its top-level `<repository>-<sha>` directory.
///
/// # Panics
///
/// Panics when the tarball can't be fetched or extracted.
pub(crate) fn fetch_project_tarball(owner: &str, repository: &str, sha: &str) -> PathBuf {
    let extracted = cache_directory().join(format!("{repository}-{sha}"));
    if extracted.is_dir() {
        return extracted;
    }
    let url = format!("https://codeload.github.com/{owner}/{repository}/tar.gz/{sha}");
    let bytes = fetch_bytes(&url);
    let _unused = fs::create_dir_all(cache_directory());
    let mut archive = Archive::new(GzDecoder::new(bytes.as_slice()));
    archive
        .unpack(cache_directory())
        .unwrap_or_else(|error| panic!("failed to extract {url}: {error}"));
    extracted
}
