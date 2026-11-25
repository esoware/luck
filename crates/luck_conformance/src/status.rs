//! What the runtime check expects less of, each with why, the way oxc's
//! runtime conformance lists the test262 cases it skips.

use luck_token::LuaTarget::{self, Lua51, Lua52, Lua53, Lua54, Lua55, Luau};

/// A program luck wrote for a suite file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    Formatted,
    Minified,
}

impl Variant {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Formatted => "Formatted",
            Self::Minified => "Minified",
        }
    }
}

const PUC_RIO: &[LuaTarget] = &[Lua51, Lua52, Lua53, Lua54, Lua55];
const LUAU: &[LuaTarget] = &[Luau];

/// A file the harness can't run, in the suites of the dialects named.
#[derive(Debug)]
struct Skip {
    suites: &'static [LuaTarget],
    file: &'static str,
    reason: &'static str,
}

/// A file whose programs luck wrote can't end the way the original does, in
/// the suites of the dialects named.
#[derive(Debug)]
struct KnownDifference {
    suites: &'static [LuaTarget],
    file: &'static str,
    variants: &'static [Variant],
    reason: &'static str,
}

/// Why Luau's runner can run a file that this harness can't.
const HOST_FUNCTIONS: &str = "calls functions the runner registers in C++ for it alone";
const NATIVE_CODE: &str = "checks native code generation, which the harness runs without";

const NOT_RUNNABLE: &[Skip] = &[
    Skip { suites: PUC_RIO, file: "all.lua", reason: "runs every other test, main.lua among them" },
    Skip { suites: PUC_RIO, file: "main.lua", reason: "runs the standalone interpreter" },
    Skip {
        suites: &[Lua51],
        file: "big.lua",
        reason: "expects a string length overflow that only a 32-bit build reaches",
    },
    Skip {
        suites: &[Lua51],
        file: "files.lua",
        reason: "renames a file its default input still holds open, which Windows refuses",
    },
    Skip {
        suites: &[Lua55],
        file: "files.lua",
        reason: "writes to `/dev/full` whatever `_port` says, which only Linux has",
    },
    Skip {
        suites: &[Lua54],
        file: "gc.lua",
        reason: "counts, as memory a collection frees, the buffer metatable that its first long \
                 string makes, which all.lua's earlier tests make first",
    },
    Skip {
        suites: LUAU,
        file: "coroutine.luau",
        reason: "expects to run as the interpreter's main thread, which mlua resumes only as a \
                 coroutine",
    },
    Skip { suites: LUAU, file: "coroutinehost.luau", reason: HOST_FUNCTIONS },
    Skip { suites: LUAU, file: "coverage.luau", reason: HOST_FUNCTIONS },
    Skip { suites: LUAU, file: "cyield.luau", reason: HOST_FUNCTIONS },
    Skip { suites: LUAU, file: "debugger.luau", reason: HOST_FUNCTIONS },
    Skip { suites: LUAU, file: "gc.luau", reason: HOST_FUNCTIONS },
    Skip { suites: LUAU, file: "integers_regspill.luau", reason: NATIVE_CODE },
    Skip { suites: LUAU, file: "iter.luau", reason: HOST_FUNCTIONS },
    Skip {
        suites: LUAU,
        file: "move.luau",
        reason: "crashes the interpreter without a fix the runner turns on for it alone",
    },
    Skip { suites: LUAU, file: "native.luau", reason: NATIVE_CODE },
    Skip { suites: LUAU, file: "native_types.luau", reason: NATIVE_CODE },
    Skip { suites: LUAU, file: "native_userdata.luau", reason: NATIVE_CODE },
    Skip { suites: LUAU, file: "ndebug_upvalues.luau", reason: HOST_FUNCTIONS },
    Skip { suites: LUAU, file: "pcall.luau", reason: HOST_FUNCTIONS },
    Skip {
        suites: LUAU,
        file: "tables.luau",
        reason: "expects the C stack limit of Luau's own build, which mlua's build raises",
    },
    Skip { suites: LUAU, file: "types.luau", reason: HOST_FUNCTIONS },
    Skip { suites: LUAU, file: "udata_direct.luau", reason: HOST_FUNCTIONS },
    Skip { suites: LUAU, file: "userdata.luau", reason: HOST_FUNCTIONS },
    Skip { suites: LUAU, file: "vector.luau", reason: HOST_FUNCTIONS },
];

/// Why a program luck wrote ends differently: the original reads something
/// that formatting or minifying changes.
const LINES: &str = "reads its own line numbers";
const NAMES: &str =
    "reads the names of its locals, upvalues, or functions, which minifying renames";
const UPVALUES: &str = "reads its closures' upvalues, which minifying inlines or drops";
const GARBAGE: &str = "measures the garbage its unused locals make, which minifying drops";
const ADDRESSES: &str = "compares string addresses, which folding a concatenation changes";

const MINIFIED: &[Variant] = &[Variant::Minified];
const BOTH: &[Variant] = &[Variant::Formatted, Variant::Minified];

const KNOWN_DIFFERENCES: &[KnownDifference] = &[
    KnownDifference { suites: PUC_RIO, file: "calls.lua", variants: MINIFIED, reason: NAMES },
    KnownDifference {
        suites: &[Lua52, Lua53, Lua54, Lua55],
        file: "closure.lua",
        variants: MINIFIED,
        reason: UPVALUES,
    },
    KnownDifference { suites: &[Lua51], file: "closure.lua", variants: BOTH, reason: LINES },
    KnownDifference {
        suites: &[Lua54, Lua55],
        file: "constructs.lua",
        variants: MINIFIED,
        reason: NAMES,
    },
    KnownDifference {
        suites: &[Lua52, Lua53, Lua54, Lua55],
        file: "coroutine.lua",
        variants: BOTH,
        reason: LINES,
    },
    KnownDifference { suites: PUC_RIO, file: "db.lua", variants: BOTH, reason: LINES },
    KnownDifference {
        suites: &[Lua51, Lua53, Lua54, Lua55],
        file: "errors.lua",
        variants: BOTH,
        reason: LINES,
    },
    KnownDifference {
        suites: &[Lua51, Lua52, Lua53],
        file: "gc.lua",
        variants: MINIFIED,
        reason: GARBAGE,
    },
    KnownDifference {
        suites: &[Lua54, Lua55],
        file: "literals.lua",
        variants: MINIFIED,
        reason: ADDRESSES,
    },
    KnownDifference {
        suites: &[Lua54, Lua55],
        file: "locals.lua",
        variants: MINIFIED,
        reason: NAMES,
    },
    KnownDifference { suites: LUAU, file: "basic.luau", variants: BOTH, reason: LINES },
    KnownDifference { suites: LUAU, file: "closure.luau", variants: BOTH, reason: LINES },
    KnownDifference { suites: LUAU, file: "debug.luau", variants: MINIFIED, reason: NAMES },
    KnownDifference { suites: LUAU, file: "tmerror.luau", variants: BOTH, reason: LINES },
];

/// Why the harness can't run `file` of `target`'s suite, if it can't.
pub(crate) fn not_runnable(target: LuaTarget, file: &str) -> Option<&'static str> {
    NOT_RUNNABLE
        .iter()
        .find(|skip| skip.file == file && skip.suites.contains(&target))
        .map(|skip| skip.reason)
}

/// Why `file` of `target`'s suite can't end the same once `variant` rewrote
/// it, if it can't.
pub(crate) fn known_difference(
    target: LuaTarget,
    file: &str,
    variant: Variant,
) -> Option<&'static str> {
    KNOWN_DIFFERENCES
        .iter()
        .find(|difference| {
            difference.file == file
                && difference.suites.contains(&target)
                && difference.variants.contains(&variant)
        })
        .map(|difference| difference.reason)
}
