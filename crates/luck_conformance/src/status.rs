//! What the runtime check expects less of, each with why, the way oxc's
//! runtime conformance lists the test262 cases it skips.

/// A program luck wrote for a suite file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rewrite {
    Formatted,
    Minified,
}

impl Rewrite {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Formatted => "Formatted",
            Self::Minified => "Minified",
        }
    }
}

/// A file the harness can't run.
#[derive(Debug)]
struct Skip {
    file: &'static str,
    reason: &'static str,
}

/// A file whose programs luck wrote can't end the way the original does.
#[derive(Debug)]
struct KnownDifference {
    file: &'static str,
    rewrites: &'static [Rewrite],
    reason: &'static str,
}

/// Why Luau's runner can run a file that this harness can't.
const HOST_FUNCTIONS: &str = "calls functions the runner registers in C++ for it alone";
const NATIVE_CODE: &str = "checks native code generation, which the harness runs without";

const NOT_RUNNABLE: &[Skip] = &[
    Skip {
        file: "coroutine.luau",
        reason: "expects to run as the interpreter's main thread, which mlua resumes only as a \
                 coroutine",
    },
    Skip { file: "coroutinehost.luau", reason: HOST_FUNCTIONS },
    Skip { file: "coverage.luau", reason: HOST_FUNCTIONS },
    Skip { file: "cyield.luau", reason: HOST_FUNCTIONS },
    Skip { file: "debugger.luau", reason: HOST_FUNCTIONS },
    Skip { file: "gc.luau", reason: HOST_FUNCTIONS },
    Skip { file: "iter.luau", reason: HOST_FUNCTIONS },
    Skip {
        file: "move.luau",
        reason: "crashes the interpreter without a fix the runner turns on for it alone",
    },
    Skip { file: "native.luau", reason: NATIVE_CODE },
    Skip { file: "native_types.luau", reason: NATIVE_CODE },
    Skip { file: "native_userdata.luau", reason: NATIVE_CODE },
    Skip { file: "ndebug_upvalues.luau", reason: HOST_FUNCTIONS },
    Skip { file: "pcall.luau", reason: HOST_FUNCTIONS },
    Skip {
        file: "tables.luau",
        reason: "expects the C stack limit of Luau's own build, which mlua's build raises",
    },
    Skip { file: "types.luau", reason: HOST_FUNCTIONS },
    Skip { file: "udata_direct.luau", reason: HOST_FUNCTIONS },
    Skip { file: "userdata.luau", reason: HOST_FUNCTIONS },
    Skip { file: "vector.luau", reason: HOST_FUNCTIONS },
];

/// Why a program luck wrote ends differently: the original reads something
/// that formatting or minifying changes.
const LINES: &str = "reads its own line numbers";
const NAMES: &str =
    "reads the names of its locals, upvalues, or functions, which minifying renames";

const MINIFIED: &[Rewrite] = &[Rewrite::Minified];
const BOTH: &[Rewrite] = &[Rewrite::Formatted, Rewrite::Minified];

const KNOWN_DIFFERENCES: &[KnownDifference] = &[
    KnownDifference { file: "basic.luau", rewrites: BOTH, reason: LINES },
    KnownDifference { file: "closure.luau", rewrites: BOTH, reason: LINES },
    KnownDifference { file: "debug.luau", rewrites: MINIFIED, reason: NAMES },
    KnownDifference { file: "tmerror.luau", rewrites: BOTH, reason: LINES },
];

/// Why the harness can't run `file`, if it can't.
pub(crate) fn not_runnable(file: &str) -> Option<&'static str> {
    NOT_RUNNABLE.iter().find(|skip| skip.file == file).map(|skip| skip.reason)
}

/// Why `file` can't end the same once `rewrite` rewrote it, if it can't.
pub(crate) fn known_difference(file: &str, rewrite: Rewrite) -> Option<&'static str> {
    KNOWN_DIFFERENCES
        .iter()
        .find(|difference| difference.file == file && difference.rewrites.contains(&rewrite))
        .map(|difference| difference.reason)
}
