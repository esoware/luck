use core::cell::RefCell;
use std::io::Write as _;
use std::path::PathBuf;

use flate2::Compression;
use flate2::write::GzEncoder;
use tempfile::TempDir;

type Responder = Box<dyn FnMut(&str) -> Vec<u8>>;

thread_local! {
    static CACHE_DIRECTORY: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static RESPONDER: RefCell<Option<Responder>> = const { RefCell::new(None) };
}

pub(crate) fn cache_directory_override() -> Option<PathBuf> {
    CACHE_DIRECTORY.with(|directory| directory.borrow().clone())
}

pub(crate) fn canned_response(url: &str) -> Option<Vec<u8>> {
    RESPONDER.with(|responder| responder.borrow_mut().as_mut().map(|respond| respond(url)))
}

/// Points this thread's cache at a temporary directory and answers its
/// fetches with `respond`, so a test reaches neither the real cache nor the
/// network. The directory lives as long as the returned guard.
pub(crate) fn isolated_cache<Respond>(respond: Respond) -> TempDir
where
    Respond: FnMut(&str) -> Vec<u8> + 'static,
{
    let directory = tempfile::tempdir().expect("create temporary cache");
    CACHE_DIRECTORY.with(|cache| *cache.borrow_mut() = Some(directory.path().to_path_buf()));
    RESPONDER.with(|responder| *responder.borrow_mut() = Some(Box::new(respond)));
    directory
}

pub(crate) fn tar_archive(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, contents) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(u64::try_from(contents.len()).expect("entry size fits in u64"));
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, path, contents.as_bytes()).expect("append tar entry");
    }
    builder.into_inner().expect("finish tar stream")
}

pub(crate) fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).expect("compress bytes");
    encoder.finish().expect("finish gzip stream")
}
