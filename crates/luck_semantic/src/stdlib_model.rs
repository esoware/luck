//! Rich stdlib data model: overloaded signatures, typed argument
//! metadata, colon-method entries on named shapes, and deprecation at
//! the entry, parameter, and constant-value level.
//!
//! Each supported environment ships as one fully self-contained TOML
//! file under `stdlib_data/`: `lua51`..`lua55`, `luau` (standalone
//! Luau), and `luau_roblox` (the Roblox runtime). The files are
//! deliberately independent - no inheritance or tier layering - because
//! the environments diverge in both directions; shared entries are
//! duplicated and kept honest by drift-guard tests. A library is
//! selected by `(LuaVersion, StdlibEnvironment)`; the environment only
//! matters for Luau. Data is parsed once via `include_str!` +
//! `LazyLock`; extra generated sources (services, enums) splice into a
//! library's file list at load.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use compact_str::CompactString;
use luck_token::{LuaVersion, StdlibEnvironment};

use crate::stdlib_load::parse_library;

/// A single entry: function, constant, namespace, or property.
#[derive(Debug, Clone)]
pub enum StdlibEntry {
    Function(Box<StdlibFunction>),
    Constant(StdlibValue),
    Property(StdlibValue),
    Namespace(StdlibNamespace),
}

impl StdlibEntry {
    /// Entry-level deprecation, regardless of entry kind.
    #[must_use]
    pub fn deprecation(&self) -> Option<&StdlibDeprecation> {
        match self {
            StdlibEntry::Function(func) => func.deprecated.as_ref(),
            StdlibEntry::Constant(value) | StdlibEntry::Property(value) => {
                value.deprecated.as_ref()
            }
            StdlibEntry::Namespace(namespace) => namespace.deprecated.as_ref(),
        }
    }

    /// Whether rebinding this global is an error. Namespaces are always
    /// read-only at the identity level.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        match self {
            StdlibEntry::Function(func) => func.read_only,
            StdlibEntry::Constant(value) | StdlibEntry::Property(value) => value.read_only,
            StdlibEntry::Namespace(_) => true,
        }
    }
}

/// A stdlib function: one or more concrete signatures plus call-site
/// metadata. Boxed inside `StdlibEntry` to keep the enum tight.
#[derive(Debug, Clone)]
pub struct StdlibFunction {
    /// At least one; authoring order. The first is the primary form
    /// used where a single rendering is needed.
    pub signatures: Vec<StdlibSignature>,
    pub is_pure: bool,
    pub must_use: bool,
    pub deprecated: Option<StdlibDeprecation>,
    pub read_only: bool,
    /// Colon-callable on its owning shape; `self` is excluded from
    /// every signature's params.
    pub is_method: bool,
    /// Shape of the call's first return value, when it names a shape in
    /// the library's registry (e.g. `io.open` returns `file`).
    pub returns_shape: Option<CompactString>,
}

impl StdlibFunction {
    /// Minimum arity across all signatures.
    #[must_use]
    pub fn min_args(&self) -> usize {
        self.signatures
            .iter()
            .map(|sig| sig.min_args)
            .min()
            .unwrap_or(0)
    }

    /// Maximum arity across all signatures; `None` if any is unbounded.
    #[must_use]
    pub fn max_args(&self) -> Option<usize> {
        let mut max = 0;
        for sig in &self.signatures {
            match sig.max_args {
                Some(n) => max = max.max(n),
                None => return None,
            }
        }
        Some(max)
    }

    /// Whether any signature accepts a call with `count` arguments.
    #[must_use]
    pub fn accepts_arg_count(&self, count: usize) -> bool {
        self.signatures.iter().any(|sig| sig.accepts(count))
    }

    /// Signatures that accept a call with `count` arguments.
    pub fn matching_signatures(&self, count: usize) -> impl Iterator<Item = &StdlibSignature> {
        self.signatures.iter().filter(move |sig| sig.accepts(count))
    }

    /// The primary (first) signature.
    #[must_use]
    pub fn primary_signature(&self) -> &StdlibSignature {
        &self.signatures[0]
    }

    /// Index of the signature to highlight when the cursor sits on
    /// parameter `active_param`: the first signature with a slot at
    /// that position, else the last.
    #[must_use]
    pub fn signature_index_for_active_param(&self, active_param: usize) -> usize {
        self.signatures
            .iter()
            .position(|sig| {
                active_param < sig.params.len()
                    || sig
                        .params
                        .last()
                        .is_some_and(|param| matches!(param.kind, StdlibArgKind::Vararg))
            })
            .unwrap_or(self.signatures.len() - 1)
    }
}

/// One concrete signature of a function.
#[derive(Debug, Clone)]
pub struct StdlibSignature {
    pub params: Vec<StdlibParam>,
    pub min_args: usize,
    /// `None` = unbounded (trailing vararg).
    pub max_args: Option<usize>,
}

impl StdlibSignature {
    #[must_use]
    pub fn accepts(&self, count: usize) -> bool {
        count >= self.min_args && self.max_args.is_none_or(|max| count <= max)
    }
}

#[derive(Debug, Clone)]
pub struct StdlibParam {
    pub kind: StdlibArgKind,
    pub required: bool,
    pub accepts_nil: bool,
    /// Set when passing this parameter at all is deprecated (e.g. the
    /// `parent` argument of `Instance.new`).
    pub deprecated: Option<StdlibDeprecation>,
}

#[derive(Debug, Clone)]
pub enum StdlibArgKind {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    /// A display-typed argument, e.g. "file" or "buffer". Treated as
    /// any-shape at the lint level; the string is the display label.
    Display(CompactString),
    Nil,
    /// Restricted set of literal string constants the argument must
    /// take. Used by e.g. `collectgarbage("collect")`.
    Constant(Vec<StdlibConstant>),
    /// `...` - variadic; matches zero or more values of any type.
    Vararg,
}

/// One allowed value of a `Constant`-kind parameter. Individual values
/// can be deprecated (e.g. `collectgarbage("setpause")` in 5.4).
#[derive(Debug, Clone)]
pub struct StdlibConstant {
    pub value: CompactString,
    pub deprecated: Option<StdlibDeprecation>,
    /// Curated frequently-used value; completion ranks these first
    /// (e.g. `Players` among the 300+ Roblox service names).
    pub is_common: bool,
}

#[derive(Debug, Clone)]
pub struct StdlibDeprecation {
    pub message: CompactString,
    /// Replacement template. Placeholders `%1`, `%2`, ... expand to the
    /// stringified positional arguments of the call.
    pub replace_template: Option<CompactString>,
}

#[derive(Debug, Clone)]
pub struct StdlibValue {
    pub read_only: bool,
    pub deprecated: Option<StdlibDeprecation>,
    /// Shape of the value, when it names a shape in the library's
    /// registry (e.g. `game` is a `DataModel`, `io.stdout` a `file`).
    pub shape: Option<CompactString>,
}

/// A dotted-member container. `shape` is the namespace's own value
/// shape, for namespaces that are also first-class values with methods
/// (each `Enum.<Type>` is an `Enum` value carrying `GetEnumItems`).
#[derive(Debug, Clone)]
pub struct StdlibNamespace {
    pub members: BTreeMap<CompactString, StdlibEntry>,
    pub shape: Option<CompactString>,
    pub deprecated: Option<StdlibDeprecation>,
}

/// A named value shape: the member surface of a non-global stdlib value
/// (file handles, the string receiver, Roblox instances). Members with
/// `is_method` are colon-callable; the rest are dot accesses.
#[derive(Debug, Clone)]
pub struct StdlibShape {
    pub members: BTreeMap<CompactString, StdlibEntry>,
}

/// Top-level library: the complete global surface of one environment.
#[derive(Debug, Clone)]
pub struct StdlibLibrary {
    pub version: LuaVersion,
    pub environment: StdlibEnvironment,
    pub globals: BTreeMap<CompactString, StdlibEntry>,
    pub shapes: BTreeMap<CompactString, StdlibShape>,
}

impl StdlibLibrary {
    /// Resolve a dotted path like `["table", "concat"]` to its entry.
    /// Traversal follows namespace members and the shape of a property
    /// or constant, so `["game", "GetService"]` resolves once `game`
    /// declares a shape.
    #[must_use]
    pub fn lookup(&self, path: &[CompactString]) -> Option<&StdlibEntry> {
        self.lookup_segments(path.iter().map(CompactString::as_str))
    }

    /// Same as [`Self::lookup`] but accepts borrowed `&str` segments -
    /// preferred entry point for callers that have raw source slices.
    #[must_use]
    pub fn lookup_str(&self, path: &[&str]) -> Option<&StdlibEntry> {
        self.lookup_segments(path.iter().copied())
    }

    fn lookup_segments<'lib, 'seg>(
        &'lib self,
        mut segments: impl Iterator<Item = &'seg str>,
    ) -> Option<&'lib StdlibEntry> {
        let mut current = self.globals.get(segments.next()?)?;
        for segment in segments {
            current = self.child(current, segment)?;
        }
        Some(current)
    }

    /// The member reachable from `entry` under `segment`: a namespace
    /// member, or a member of the entry's declared value shape.
    #[must_use]
    pub fn child<'lib>(
        &'lib self,
        entry: &'lib StdlibEntry,
        segment: &str,
    ) -> Option<&'lib StdlibEntry> {
        match entry {
            StdlibEntry::Namespace(namespace) => namespace.members.get(segment).or_else(|| {
                // A shaped namespace is also a value: `Enum.Material`
                // exposes its items AND the `Enum` shape's methods.
                self.shape_member(namespace.shape.as_ref()?, segment)
            }),
            StdlibEntry::Constant(value) | StdlibEntry::Property(value) => {
                self.shape_member(value.shape.as_ref()?, segment)
            }
            StdlibEntry::Function(_) => None,
        }
    }

    /// Member of a named shape.
    #[must_use]
    pub fn shape_member(&self, shape: &str, name: &str) -> Option<&StdlibEntry> {
        self.shapes.get(shape)?.members.get(name)
    }
}

const LUA51_TOML: &str = include_str!("../stdlib_data/lua51.toml");
const LUA52_TOML: &str = include_str!("../stdlib_data/lua52.toml");
const LUA53_TOML: &str = include_str!("../stdlib_data/lua53.toml");
const LUA54_TOML: &str = include_str!("../stdlib_data/lua54.toml");
const LUA55_TOML: &str = include_str!("../stdlib_data/lua55.toml");
const LUAU_TOML: &str = include_str!("../stdlib_data/luau.toml");
const LUAU_ROBLOX_TOML: &str = include_str!("../stdlib_data/luau_roblox.toml");
/// Generated from the Roblox API dump - see the file headers for the
/// regen command. Both splice into the Roblox library.
const ROBLOX_API_TOML: &str = include_str!("../stdlib_data/roblox_api.toml");
const ROBLOX_ENUMS_TOML: &str = include_str!("../stdlib_data/roblox_enums.toml");

static LIB_LUA51: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA51_TOML],
        LuaVersion::Lua51,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUA52: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA52_TOML],
        LuaVersion::Lua52,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUA53: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA53_TOML],
        LuaVersion::Lua53,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUA54: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA54_TOML],
        LuaVersion::Lua54,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUA55: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUA55_TOML],
        LuaVersion::Lua55,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUAU: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUAU_TOML],
        LuaVersion::Luau,
        StdlibEnvironment::Standalone,
    )
});
static LIB_LUAU_ROBLOX: LazyLock<StdlibLibrary> = LazyLock::new(|| {
    parse_library(
        &[LUAU_ROBLOX_TOML, ROBLOX_API_TOML, ROBLOX_ENUMS_TOML],
        LuaVersion::Luau,
        StdlibEnvironment::Roblox,
    )
});

/// Reference to the parsed library for the given version and
/// environment. The environment only matters for Luau (Standalone vs
/// Roblox); numbered Lua has a single environment. Cheap: data is
/// parsed once on first access and shared thereafter.
#[must_use]
pub fn library_for(version: LuaVersion, environment: StdlibEnvironment) -> &'static StdlibLibrary {
    match version {
        LuaVersion::Lua51 => &LIB_LUA51,
        LuaVersion::Lua52 => &LIB_LUA52,
        LuaVersion::Lua53 => &LIB_LUA53,
        LuaVersion::Lua54 => &LIB_LUA54,
        LuaVersion::Lua55 => &LIB_LUA55,
        LuaVersion::Luau => match environment {
            StdlibEnvironment::Standalone => &LIB_LUAU,
            StdlibEnvironment::Roblox => &LIB_LUAU_ROBLOX,
        },
    }
}

/// Expand a deprecation `replace_template` against positional argument
/// source slices. `%1`, `%2`, ..., `%9` refer to args[0..=8]; `%%` is a
/// literal percent. Missing arguments expand to an empty string - the
/// caller decides whether that constitutes a useful fix.
#[must_use]
pub fn expand_replace_template(template: &str, args: &[&str]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                output.push('%');
                chars.next();
            }
            Some(digit) if digit.is_ascii_digit() => {
                chars.next();
                let idx = (digit as u8 - b'0') as usize;
                if idx >= 1 && idx <= args.len() {
                    output.push_str(args[idx - 1]);
                }
            }
            _ => output.push('%'),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_template_basic() {
        assert_eq!(expand_replace_template("#%1", &["t"]), "#t");
        assert_eq!(expand_replace_template("%1 ^ %2", &["a", "b"]), "a ^ b");
        assert_eq!(expand_replace_template("load(%1)", &["x"]), "load(x)");
    }

    #[test]
    fn replace_template_escape() {
        assert_eq!(expand_replace_template("100%% %1", &["x"]), "100% x");
    }

    #[test]
    fn replace_template_missing_arg_empty() {
        assert_eq!(expand_replace_template("%1 + %2", &["a"]), "a + ");
    }
}
