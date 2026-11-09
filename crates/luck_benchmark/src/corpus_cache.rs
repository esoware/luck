//! The gitignored `corpus/` cache that network-fetched bench inputs land in.

#![expect(clippy::panic, reason = "a bench has no way to report a failed corpus load")]

use core::time::Duration;
use std::path::{Path, PathBuf};
use std::{fs, thread};

use flate2::read::GzDecoder;
use tar::Archive;

#[cfg(test)]
use crate::fixtures::{cache_directory_override, canned_response};

pub(crate) fn cache_directory() -> PathBuf {
    #[cfg(test)]
    if let Some(directory) = cache_directory_override() {
        return directory;
    }
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
    #[cfg(test)]
    if let Some(bytes) = canned_response(url) {
        return bytes;
    }
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
/// miss. `base_url` ends in the pinned commit, which keys the cached copy.
///
/// # Panics
///
/// Panics when the file can't be fetched or isn't UTF-8.
pub(crate) fn fetch_corpus_file(base_url: &str, file_name: &str) -> String {
    let pinned_commit = base_url.rsplit('/').next().unwrap_or(base_url);
    let cache_path = cache_directory().join(format!("{pinned_commit}-{file_name}"));
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
    let extracted_name = format!("{repository}-{sha}");
    let extracted = cache_directory().join(&extracted_name);
    if extracted.is_dir() {
        return extracted;
    }
    let url = format!("https://codeload.github.com/{owner}/{repository}/tar.gz/{sha}");
    let bytes = fetch_bytes(&url);
    // Extracting aside and renaming keeps `extracted` absent until it's whole.
    // An interrupted run's leftovers go first.
    let partial = cache_directory().join(format!("{extracted_name}.partial"));
    let _stale_partial = fs::remove_dir_all(&partial);
    let mut archive = Archive::new(GzDecoder::new(bytes.as_slice()));
    archive.unpack(&partial).unwrap_or_else(|error| panic!("failed to extract {url}: {error}"));
    fs::rename(partial.join(&extracted_name), &extracted)
        .unwrap_or_else(|error| panic!("failed to move {url} into the cache: {error}"));
    let _empty_partial = fs::remove_dir_all(&partial);
    extracted
}

#[cfg(test)]
mod tests {
    use core::mem;
    use std::panic;

    use super::*;
    use crate::fixtures::{gzip, isolated_cache, tar_archive};

    #[test]
    fn fetch_corpus_file_refetches_when_the_pinned_commit_changes() {
        let _cache = isolated_cache(|url| {
            if url.contains("old-commit") { b"old pin".to_vec() } else { b"new pin".to_vec() }
        });

        assert_eq!(fetch_corpus_file("https://corpus.test/old-commit", "input.luau"), "old pin");
        assert_eq!(
            fetch_corpus_file("https://corpus.test/new-commit", "input.luau"),
            "new pin",
            "a file cached under one pinned commit must not satisfy a fetch of another"
        );
    }

    #[test]
    fn fetch_project_tarball_refetches_after_an_interrupted_extraction() {
        let complete_archive = tar_archive(&[
            ("project-0123/first.lua", "return 1\n"),
            ("project-0123/second.lua", "return 2\n"),
            ("project-0123/third.lua", "return 3\n"),
        ]);
        // Header and data of the first entry, then the second's header and
        // one byte of its data.
        let cut_off_archive = complete_archive.get(..1537).expect("cut point inside the archive");
        let mut response = gzip(cut_off_archive);
        let complete_response = gzip(&complete_archive);
        let _cache =
            isolated_cache(move |_url| mem::replace(&mut response, complete_response.clone()));

        let _interrupted =
            panic::catch_unwind(|| fetch_project_tarball("owner", "project", "0123"));
        let directory = fetch_project_tarball("owner", "project", "0123");

        assert!(
            directory.join("third.lua").is_file(),
            "a directory left by a failed extraction must not count as the finished project"
        );
    }
}
