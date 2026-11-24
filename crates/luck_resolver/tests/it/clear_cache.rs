use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use luck_resolver::{
    DirEntry, FileMetadata, FileSystem, FileSystemOs, ResolveOptions, ResolverGeneric,
};
use luck_token::LuaVersion;
use parking_lot::Mutex;
use tempfile::TempDir;

/// The OS's file system, except that its first listing of one directory
/// reads it, says so, and waits to be let go before it answers, so a test
/// can change the disk and clear the cache meanwhile.
struct PausingFileSystem {
    pause: Mutex<Option<Pause>>,
}

struct Pause {
    directory: PathBuf,
    listed: Sender<()>,
    resume: Receiver<()>,
}

impl FileSystem for PausingFileSystem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        FileSystemOs.read(path)
    }

    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        FileSystemOs.metadata(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let listing = FileSystemOs.read_dir(path);
        let pause = {
            let mut pause = self.pause.lock();
            if pause.as_ref().is_some_and(|paused| paused.directory == path) {
                pause.take()
            } else {
                None
            }
        };
        if let Some(Pause { listed, resume, .. }) = pause {
            listed.send(()).unwrap();
            resume.recv().unwrap();
        }
        listing
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        FileSystemOs.canonicalize(path)
    }
}

/// A resolver of `version` rooted at `root`, which pauses in its first
/// listing of `directory` while `change` runs and the cache clears, and
/// what it resolves `module` from `from_file` to then and afterwards.
fn resolved_around_a_clear(
    version: LuaVersion,
    root: &Path,
    directory: &Path,
    from_file: &Path,
    module: &str,
    change: impl FnOnce(),
) -> [Option<PathBuf>; 2] {
    let (listed_sender, listed) = channel();
    let (resume, resume_receiver) = channel();
    let pause = Pause {
        directory: directory.to_path_buf(),
        listed: listed_sender,
        resume: resume_receiver,
    };
    let resolver = Arc::new(ResolverGeneric::new_with_file_system(
        PausingFileSystem { pause: Mutex::new(Some(pause)) },
        ResolveOptions { version, search_paths: Vec::new(), root: root.to_path_buf() },
    ));
    let racing = {
        let (resolver, from_file, module) =
            (Arc::clone(&resolver), from_file.to_path_buf(), module.to_owned());
        thread::spawn(move || resolver.resolve(&from_file, &module).ok().map(|found| found.path))
    };
    listed.recv().unwrap();
    change();
    resolver.clear_cache();
    resume.send(()).unwrap();
    let during = racing.join().unwrap();
    let after = resolver.resolve(from_file, module).ok().map(|found| found.path);
    [during, after]
}

fn canonical_root() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = FileSystemOs.canonicalize(directory.path()).expect("canonicalize");
    (directory, root)
}

#[test]
fn a_lua_module_deleted_while_a_resolution_reads_is_gone_after_the_clear() {
    let (_directory, root) = canonical_root();
    let module = root.join("foo.lua");
    fs::write(&module, "return 1\n").expect("write foo.lua");
    let [during, after] = resolved_around_a_clear(
        LuaVersion::Lua54,
        &root,
        &root,
        &root.join("main.lua"),
        "foo",
        || {
            fs::remove_file(&module).expect("remove foo.lua");
        },
    );
    assert_eq!(during, Some(module), "the racing resolution answers from what it read");
    assert_eq!(after, None, "nothing it read outlives the clear");
}

#[test]
fn a_luau_module_created_while_a_resolution_reads_resolves_after_the_clear() {
    let (_directory, root) = canonical_root();
    let sub = root.join("sub");
    fs::create_dir_all(&sub).expect("mkdir sub");
    let init = sub.join("init.luau");
    let [during, after] = resolved_around_a_clear(
        LuaVersion::Luau,
        &root,
        &sub,
        &root.join("main.luau"),
        "./sub",
        || {
            fs::write(&init, "return 1\n").expect("write sub/init.luau");
        },
    );
    assert_eq!(during, None, "the racing resolution answers from what it read");
    assert_eq!(after, Some(init), "nothing it read outlives the clear");
}
