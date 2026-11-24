use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{fs, thread};

use flate2::read::GzDecoder;
use tar::Archive;

#[cfg(test)]
use crate::fixtures::{cache_directory_override, canned_response};

fn cache_directory() -> PathBuf {
    #[cfg(test)]
    if let Some(directory) = cache_directory_override() {
        return directory;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("cache")
}

/// CI runners see transient connection resets from GitHub's codeload host,
/// and one failed fetch would fail the whole bench job.
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
    let _ = fs::remove_dir_all(&partial);
    let mut archive = Archive::new(GzDecoder::new(bytes.as_slice()));
    archive.unpack(&partial).unwrap_or_else(|e| panic!("failed to extract {url}: {e}"));
    fs::rename(partial.join(&extracted_name), &extracted)
        .unwrap_or_else(|e| panic!("failed to move {url} into the cache: {e}"));
    let _ = fs::remove_dir_all(&partial);
    extracted
}

#[cfg(test)]
mod tests {
    use std::mem;
    use std::panic;

    use super::*;
    use crate::fixtures::{gzip, isolated_cache, tar_archive};

    #[test]
    fn fetch_project_tarball_refetches_after_an_interrupted_extraction() {
        let complete_archive = tar_archive(&[
            ("project-0123/first.lua", "return 1\n"),
            ("project-0123/second.lua", "return 2\n"),
            ("project-0123/third.lua", "return 3\n"),
        ]);
        // Header and data of the first entry, then the second's header and
        // one byte of its data.
        let mut response = gzip(&complete_archive[..1537]);
        let complete_response = gzip(&complete_archive);
        let _cache =
            isolated_cache(move |_url| mem::replace(&mut response, complete_response.clone()));

        let _ = panic::catch_unwind(|| fetch_project_tarball("owner", "project", "0123"));
        let directory = fetch_project_tarball("owner", "project", "0123");

        assert!(
            directory.join("third.lua").is_file(),
            "a directory left by a failed extraction must not count as the finished project"
        );
    }
}
