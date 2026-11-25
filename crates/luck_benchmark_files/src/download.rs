use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{fs, io, thread};

use flate2::read::GzDecoder;
use tar::Archive;

pub(crate) fn fetch_project_tarball(owner: &str, repository: &str, sha: &str) -> PathBuf {
    let cache_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("cache");
    let url = format!("https://codeload.github.com/{owner}/{repository}/tar.gz/{sha}");
    let name = format!("{repository}-{sha}");
    extract_once(&cache_directory, &name, &url, &name, fetch_bytes)
        .unwrap_or_else(|e| panic!("{e}"))
}

/// The directory `extracted_name` in `cache_directory`, holding what the
/// tarball at `url` has in its directory `kept`. Only a cache that lacks it
/// has `fetch` download the tarball.
///
/// # Errors
///
/// Returns `fetch`'s error, or why the tarball couldn't be unpacked or moved
/// into the cache, which then holds no `extracted_name`.
pub fn extract_once(
    cache_directory: &Path,
    extracted_name: &str,
    url: &str,
    kept: &str,
    fetch: impl FnOnce(&str) -> io::Result<Vec<u8>>,
) -> io::Result<PathBuf> {
    let extracted = cache_directory.join(extracted_name);
    if extracted.is_dir() {
        return Ok(extracted);
    }
    let bytes = fetch(url)?;
    // Extracting aside and renaming keeps `extracted` absent until it's whole.
    // An interrupted run's leftovers go first.
    let partial = cache_directory.join(format!("{extracted_name}.partial"));
    let _ = fs::remove_dir_all(&partial);
    let unpack = || -> io::Result<()> {
        fs::create_dir_all(&partial)?;
        for read in Archive::new(GzDecoder::new(bytes.as_slice())).entries()? {
            let mut entry = read?;
            if entry.path()?.starts_with(kept) {
                entry.unpack_in(&partial)?;
            }
        }
        Ok(())
    };
    unpack().map_err(|e| io::Error::new(e.kind(), format!("failed to extract {url}: {e}")))?;
    fs::rename(partial.join(kept), &extracted).map_err(|e| {
        io::Error::new(e.kind(), format!("failed to move {url} into the cache: {e}"))
    })?;
    let _ = fs::remove_dir_all(&partial);
    Ok(extracted)
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

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::{Builder, Header};

    use super::*;

    const URL: &str = "https://codeload.github.com/owner/project/tar.gz/0123";

    fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        for (path, contents) in files {
            let mut header = Header::new_gnu();
            header.set_size(u64::try_from(contents.len()).expect("entry size fits in u64"));
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, contents.as_bytes()).expect("append tar entry");
        }
        builder.into_inner().expect("finish tar stream")
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("compress bytes");
        encoder.finish().expect("finish gzip stream")
    }

    #[test]
    fn extract_once_refetches_after_an_interrupted_extraction() {
        let complete_archive = tarball(&[
            ("project-0123/first.luau", "return 1\n"),
            ("project-0123/second.luau", "return 2\n"),
            ("project-0123/third.luau", "return 3\n"),
        ]);
        // Header and data of the first entry, then the second's header and
        // one byte of its data.
        let interrupted_response = gzip(&complete_archive[..1537]);
        let cache = tempfile::tempdir().expect("create temporary cache");
        let cache_directory = cache.path();
        let name = "project-0123";

        let interrupted =
            extract_once(cache_directory, name, URL, name, |_| Ok(interrupted_response));
        assert!(interrupted.is_err(), "a truncated tarball must fail to extract");
        assert!(!cache_directory.join(name).exists(), "a failed extraction must leave nothing");
        let directory =
            extract_once(cache_directory, name, URL, name, |_| Ok(gzip(&complete_archive)))
                .expect("a whole tarball extracts");

        assert!(
            directory.join("third.luau").is_file(),
            "a directory left by a failed extraction must not count as the finished project"
        );
    }

    #[test]
    fn extract_once_keeps_only_the_kept_directory() {
        let archive = tarball(&[
            ("project-0123/src/main.luau", "return 1\n"),
            ("project-0123/tests/conformance/basic.luau", "return 2\n"),
        ]);
        let cache = tempfile::tempdir().expect("create temporary cache");

        let directory =
            extract_once(cache.path(), "tests", URL, "project-0123/tests", |_| Ok(gzip(&archive)))
                .expect("the tarball extracts");

        assert_eq!(directory, cache.path().join("tests"));
        assert!(directory.join("conformance/basic.luau").is_file());
        let mut cached: Vec<_> = fs::read_dir(cache.path())
            .expect("read the cache")
            .map(|entry| entry.expect("cache entry").file_name())
            .collect();
        cached.sort();
        assert_eq!(cached, ["tests"], "nothing outside the kept directory stays in the cache");
    }
}
