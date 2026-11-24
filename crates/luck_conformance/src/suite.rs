use std::path::{Path, PathBuf};
use std::{fs, io};

use luck_benchmark_files::{extract_once, fetch_bytes};
use luck_token::LuaTarget;

/// One dialect's official test suite, pinned to the release of the
/// interpreter mlua vendors for it.
#[derive(Debug)]
pub(crate) struct Suite {
    /// The name the snapshots carry, such as `lua54`, which is also the name
    /// of the feature that links its interpreter.
    pub name: &'static str,
    is_linked: bool,
    pub target: LuaTarget,
    /// The linked interpreter's `_VERSION`.
    interpreter: &'static str,
    url: &'static str,
    /// Where the tests sit in the tarball.
    tarball_directory: &'static str,
    /// The cache directory the tests unpack into, which the snapshots name
    /// each file under.
    directory: &'static str,
    extension: &'static str,
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
    pub source: Vec<u8>,
}

impl Suite {
    /// The suite of the interpreter this binary links. mlua builds only with
    /// exactly one dialect's feature.
    pub(crate) fn linked() -> io::Result<&'static Self> {
        SUITES
            .iter()
            .find(|suite| suite.is_linked)
            .ok_or_else(|| io::Error::other("no suite names the dialect feature that's on"))
    }

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
    pub(crate) fn fetch(&self) -> io::Result<PathBuf> {
        let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("cache");
        extract_once(&cache, self.directory, self.url, self.tarball_directory, fetch_bytes)
    }

    /// The suite's test files in `root`, or only those whose path contains
    /// `filter`, in path order, which every snapshot lists them in.
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
                source: fs::read(&path)?,
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }
}
