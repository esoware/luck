use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{fs, io, thread};

use flate2::read::GzDecoder;
use tar::Archive;

pub(crate) fn fetch_project_tarball(owner: &str, repository: &str, sha: &str) -> PathBuf {
    let cache_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("cache");
    let url = format!("https://codeload.github.com/{owner}/{repository}/tar.gz/{sha}");
    extract_once(&cache_directory, &format!("{repository}-{sha}"), &url, |url| {
        fetch_bytes(url).unwrap_or_else(|e| panic!("{e}"))
    })
}

/// The directory `extracted_name` in `cache_directory`, the one directory
/// the tarball at `url` holds. Only a cache that lacks it has `fetch`
/// download the tarball.
fn extract_once(
    cache_directory: &Path,
    extracted_name: &str,
    url: &str,
    fetch: impl FnOnce(&str) -> Vec<u8>,
) -> PathBuf {
    let extracted = cache_directory.join(extracted_name);
    if extracted.is_dir() {
        return extracted;
    }
    let bytes = fetch(url);
    // Extracting aside and renaming keeps `extracted` absent until it's whole.
    // An interrupted run's leftovers go first.
    let partial = cache_directory.join(format!("{extracted_name}.partial"));
    let _ = fs::remove_dir_all(&partial);
    let mut archive = Archive::new(GzDecoder::new(bytes.as_slice()));
    archive.unpack(&partial).unwrap_or_else(|e| panic!("failed to extract {url}: {e}"));
    fs::rename(partial.join(extracted_name), &extracted)
        .unwrap_or_else(|e| panic!("failed to move {url} into the cache: {e}"));
    let _ = fs::remove_dir_all(&partial);
    extracted
}

/// The body at `url`, retried, because CI runners see transient connection
/// resets, such as from GitHub's codeload host, and one failed fetch would
/// fail the whole job.
///
/// # Errors
///
/// Returns the last attempt's error once three attempts have failed.
pub fn fetch_bytes(url: &str) -> io::Result<Vec<u8>> {
    let mut last_error = String::new();
    for delay in [Duration::ZERO, Duration::from_secs(1), Duration::from_secs(2)] {
        thread::sleep(delay);
        match ureq::get(url).call() {
            Ok(mut response) => match response.body_mut().read_to_vec() {
                Ok(bytes) => return Ok(bytes),
                Err(error) => last_error = error.to_string(),
            },
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(io::Error::other(format!("failed to fetch {url} after 3 attempts: {last_error}")))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::panic;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, Header};

    use super::*;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("compress bytes");
        encoder.finish().expect("finish gzip stream")
    }

    #[test]
    fn extract_once_refetches_after_an_interrupted_extraction() {
        let mut builder = Builder::new(Vec::new());
        for (path, contents) in [
            ("project-0123/first.lua", "return 1\n"),
            ("project-0123/second.lua", "return 2\n"),
            ("project-0123/third.lua", "return 3\n"),
        ] {
            let mut header = Header::new_gnu();
            header.set_size(u64::try_from(contents.len()).expect("entry size fits in u64"));
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, contents.as_bytes()).expect("append tar entry");
        }
        let complete_archive = builder.into_inner().expect("finish tar stream");
        // Header and data of the first entry, then the second's header and
        // one byte of its data.
        let interrupted_response = gzip(&complete_archive[..1537]);
        let cache = tempfile::tempdir().expect("create temporary cache");
        let cache_directory = cache.path();
        let url = "https://codeload.github.com/owner/project/tar.gz/0123";

        let interrupted = panic::catch_unwind(|| {
            extract_once(cache_directory, "project-0123", url, |_| interrupted_response)
        });
        assert!(interrupted.is_err(), "a truncated tarball must fail to extract");
        let directory =
            extract_once(cache_directory, "project-0123", url, |_| gzip(&complete_archive));

        assert!(
            directory.join("third.lua").is_file(),
            "a directory left by a failed extraction must not count as the finished project"
        );
    }
}
