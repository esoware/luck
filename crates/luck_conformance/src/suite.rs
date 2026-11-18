use core::time::Duration;
use std::path::{Path, PathBuf};
use std::{fs, io, thread};

use flate2::read::GzDecoder;
use luck_core::LuaTarget;
use tar::Archive;

/// One dialect's official test suite, pinned to the release of the
/// interpreter mlua vendors for it.
#[derive(Debug)]
pub(crate) struct Suite {
    /// The name the snapshots carry, such as `lua54`, which is also the name
    /// of the feature that links its interpreter.
    pub name: &'static str,
    pub is_linked: bool,
    pub target: LuaTarget,
    /// The linked interpreter's `_VERSION`.
    pub interpreter: &'static str,
    pub url: &'static str,
    /// Where the tests sit in the tarball.
    pub tarball_directory: &'static str,
    /// The cache directory the tests unpack into, which the snapshots name
    /// each file under.
    pub directory: &'static str,
    pub extension: &'static str,
    pub runner: Runner,
    /// The Luau flags for what luck reads ahead of the pinned release:
    /// integer literals and their library, and exported values.
    pub fast_flags: &'static [&'static str],
}

/// The suite's own runner, whose setup every test expects.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Runner {
    /// PUC-Rio's `all.lua`, set for a user's run.
    PucRio,
    /// The `runConformance` of Luau's `Conformance.test.cpp`, without the
    /// host functions single tests register.
    Luau,
}

/// The suites, each where its project publishes it.
///
/// PUC-Rio publishes one suite per release at <https://www.lua.org/tests/>,
/// and its last 5.2 and 5.3 suites are older than the last 5.2 and 5.3
/// releases. Luau keeps its suite in its repository, at the commit of the
/// release tag.
const SUITES: [Suite; 6] = [
    Suite {
        name: "lua51",
        is_linked: cfg!(feature = "lua51"),
        target: LuaTarget::Lua51,
        interpreter: "Lua 5.1",
        url: "https://www.lua.org/tests/lua5.1-tests.tar.gz",
        tarball_directory: "lua5.1-tests",
        directory: "lua5.1-tests",
        extension: "lua",
        runner: Runner::PucRio,
        fast_flags: &[],
    },
    Suite {
        name: "lua52",
        is_linked: cfg!(feature = "lua52"),
        target: LuaTarget::Lua52,
        interpreter: "Lua 5.2",
        url: "https://www.lua.org/tests/lua-5.2.2-tests.tar.gz",
        tarball_directory: "lua-5.2.2-tests",
        directory: "lua-5.2.2-tests",
        extension: "lua",
        runner: Runner::PucRio,
        fast_flags: &[],
    },
    Suite {
        name: "lua53",
        is_linked: cfg!(feature = "lua53"),
        target: LuaTarget::Lua53,
        interpreter: "Lua 5.3",
        url: "https://www.lua.org/tests/lua-5.3.4-tests.tar.gz",
        tarball_directory: "lua-5.3.4-tests",
        directory: "lua-5.3.4-tests",
        extension: "lua",
        runner: Runner::PucRio,
        fast_flags: &[],
    },
    Suite {
        name: "lua54",
        is_linked: cfg!(feature = "lua54"),
        target: LuaTarget::Lua54,
        interpreter: "Lua 5.4",
        url: "https://www.lua.org/tests/lua-5.4.9-tests.tar.gz",
        tarball_directory: "lua-5.4.9-tests",
        directory: "lua-5.4.9-tests",
        extension: "lua",
        runner: Runner::PucRio,
        fast_flags: &[],
    },
    Suite {
        name: "lua55",
        is_linked: cfg!(feature = "lua55"),
        target: LuaTarget::Lua55,
        interpreter: "Lua 5.5",
        url: "https://www.lua.org/tests/lua-5.5.1-tests.tar.gz",
        tarball_directory: "lua-5.5.1-tests",
        directory: "lua-5.5.1-tests",
        extension: "lua",
        runner: Runner::PucRio,
        fast_flags: &[],
    },
    Suite {
        name: "luau",
        is_linked: cfg!(feature = "luau"),
        target: LuaTarget::Luau,
        interpreter: "Luau 0.740",
        // luau-lang/luau at the 0.740 tag
        url: "https://codeload.github.com/luau-lang/luau/tar.gz/c0e346edd89066b44dca174c9f54ce84c746a540",
        tarball_directory: "luau-c0e346edd89066b44dca174c9f54ce84c746a540/tests/conformance",
        directory: "luau-0.740",
        extension: "luau",
        runner: Runner::Luau,
        fast_flags: &["LuauIntegerType2", "LuauIntegerLibrary", "LuauExportValueSyntax"],
    },
];

/// A file of a suite, as bytes, since the PUC-Rio suites hold Latin-1 text.
#[derive(Debug)]
pub(crate) struct SuiteFile {
    /// The file's name in its suite's directory, such as `strings.lua`.
    pub name: String,
    /// The suite directory and the name, as the snapshots name the file.
    pub path: String,
    pub bytes: Vec<u8>,
}

impl Suite {
    /// The suite of the interpreter this binary links. mlua builds only with
    /// exactly one dialect's feature.
    ///
    /// # Errors
    ///
    /// Fails when no suite names the feature that's on.
    pub(crate) fn linked() -> io::Result<&'static Self> {
        SUITES
            .iter()
            .find(|suite| suite.is_linked)
            .ok_or_else(|| io::Error::other("no suite names the dialect feature that's on"))
    }

    /// # Errors
    ///
    /// Fails when the linked interpreter isn't the release the suite is
    /// pinned to, as when mlua's vendored Luau moves past the pinned tag.
    pub(crate) fn check_interpreter(&self, interpreter: &str) -> io::Result<()> {
        if interpreter == self.interpreter {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "the `{}` suite is pinned for `{}`, but mlua links `{interpreter}`; pin the release's tests in suite.rs",
                self.name, self.interpreter
            )))
        }
    }

    /// The suite's directory in the cache, downloaded into it on first use.
    ///
    /// # Errors
    ///
    /// Fails when the tarball can't be fetched or unpacked.
    pub(crate) fn fetch(&self) -> io::Result<PathBuf> {
        let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("cache");
        let root = cache.join(self.directory);
        if root.is_dir() {
            return Ok(root);
        }
        let bytes = fetch_bytes(self.url)?;
        // Unpacking aside and renaming keeps `root` absent until it's whole.
        // An interrupted run's leftovers go first.
        let partial = cache.join(format!("{}.partial", self.directory));
        let _stale_partial = fs::remove_dir_all(&partial);
        fs::create_dir_all(&partial)?;
        for read in Archive::new(GzDecoder::new(bytes.as_slice())).entries()? {
            let mut entry = read?;
            if entry.path()?.starts_with(self.tarball_directory) {
                entry.unpack_in(&partial)?;
            }
        }
        fs::rename(partial.join(self.tarball_directory), &root)?;
        fs::remove_dir_all(&partial)?;
        Ok(root)
    }

    /// The suite's test files in `root`, in name order, or only those whose
    /// path contains `filter`.
    ///
    /// # Errors
    ///
    /// Fails when the directory or a file can't be read.
    pub(crate) fn files(&self, root: &Path, filter: Option<&str>) -> io::Result<Vec<SuiteFile>> {
        let mut files = Vec::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != self.extension) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let suite_path = format!("{}/{name}", self.directory);
            if filter.is_some_and(|part| !suite_path.contains(part)) {
                continue;
            }
            files.push(SuiteFile {
                name: name.to_owned(),
                path: suite_path,
                bytes: fs::read(&path)?,
            });
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }
}

/// Retried, because CI runners see transient connection resets.
///
/// # Errors
///
/// Fails when every attempt does.
fn fetch_bytes(url: &str) -> io::Result<Vec<u8>> {
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
