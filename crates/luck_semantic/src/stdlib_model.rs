//! Rich stdlib data model: typed argument metadata, per-entry version
//! gating, deprecation with `%n` replace templates, `must_use` markers,
//! `read_only` flags, and a Luau-flavor tier for the combined Luau
//! library.
//!
//! Data lives in `stdlib_data/<version>.toml` and is loaded once via
//! `include_str!` + `toml::from_str` into a `LazyLock<StdlibLibrary>`.
//!
//! The Luau library is a superset, and each entry carries a [`LuauTier`]
//! selecting which Luau flavor exposes it: `Core` (both standalone and
//! Roblox), `Standalone` (standalone Luau only, e.g. `loadstring`), and
//! `Roblox` (Roblox runtime only, e.g. `task`, `bit`, `warn`, `game`).
//! Entries Roblox and standalone both remove (`io`, `package`, `load`,
//! `dofile`, `loadfile`) are simply absent from `luau.toml`; `os` keeps
//! its sandboxed clock/date/difftime/time subset.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use compact_str::CompactString;
use luck_token::{LuaVersion, StdlibEnvironment};
use serde::Deserialize;

/// Bitset over the six supported Lua/Luau versions. Stored as a single
/// `u8` so the struct stays compact when embedded inside an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LuaVersionSet(u8);

impl LuaVersionSet {
    const LUA51: u8 = 1 << 0;
    const LUA52: u8 = 1 << 1;
    const LUA53: u8 = 1 << 2;
    const LUA54: u8 = 1 << 3;
    const LUA55: u8 = 1 << 4;
    const LUAU: u8 = 1 << 5;

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn all() -> Self {
        Self(Self::LUA51 | Self::LUA52 | Self::LUA53 | Self::LUA54 | Self::LUA55 | Self::LUAU)
    }

    #[must_use]
    pub const fn single(version: LuaVersion) -> Self {
        Self(Self::bit(version))
    }

    #[must_use]
    pub fn contains(self, version: LuaVersion) -> bool {
        self.0 & Self::bit(version) != 0
    }

    #[must_use]
    pub fn with(self, version: LuaVersion) -> Self {
        Self(self.0 | Self::bit(version))
    }

    #[must_use]
    pub fn without(self, version: LuaVersion) -> Self {
        Self(self.0 & !Self::bit(version))
    }

    const fn bit(version: LuaVersion) -> u8 {
        match version {
            LuaVersion::Lua51 => Self::LUA51,
            LuaVersion::Lua52 => Self::LUA52,
            LuaVersion::Lua53 => Self::LUA53,
            LuaVersion::Lua54 => Self::LUA54,
            LuaVersion::Lua55 => Self::LUA55,
            LuaVersion::Luau => Self::LUAU,
        }
    }
}

/// Which Luau flavor exposes an entry. Only meaningful for the Luau
/// library; numbered-Lua entries are always `Core`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LuauTier {
    #[default]
    Core,
    Standalone,
    Roblox,
}

/// A single entry: function, constant, namespace, or property.
#[derive(Debug, Clone)]
pub struct StdlibEntry {
    pub kind: EntryKind,
    pub lua_version: LuaVersionSet,
    /// Which Luau flavor exposes this entry. Inherited by namespace
    /// members from their namespace.
    pub luau_tier: LuauTier,
}

impl StdlibEntry {
    /// Reachable under a Luau target in the given stdlib environment.
    #[must_use]
    pub fn available_in_luau(&self, environment: StdlibEnvironment) -> bool {
        match self.luau_tier {
            LuauTier::Core => true,
            LuauTier::Standalone => environment == StdlibEnvironment::Standalone,
            LuauTier::Roblox => environment == StdlibEnvironment::Roblox,
        }
    }
}

#[derive(Debug, Clone)]
pub enum EntryKind {
    Function(Box<StdlibFunction>),
    Constant(StdlibValue),
    Namespace(BTreeMap<CompactString, StdlibEntry>),
    Property(StdlibValue),
}

/// Concrete signature for a stdlib function. Boxed inside `EntryKind`
/// to keep the enum tight.
#[derive(Debug, Clone)]
pub struct StdlibFunction {
    pub params: Vec<StdlibParam>,
    pub min_args: usize,
    pub max_args: Option<usize>,
    pub is_pure: bool,
    pub must_use: bool,
    pub deprecated: Option<StdlibDeprecation>,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct StdlibParam {
    pub kind: StdlibArgKind,
    pub required: bool,
    pub accepts_nil: bool,
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
    Constant(Vec<CompactString>),
    /// `...` - variadic; matches zero or more values of any type.
    Vararg,
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
}

/// Top-level library: every global / namespace for a single Lua version.
#[derive(Debug, Clone)]
pub struct StdlibLibrary {
    pub version: LuaVersion,
    pub globals: BTreeMap<CompactString, StdlibEntry>,
}

impl StdlibLibrary {
    /// Resolve a dotted path like `["table", "concat"]` to its entry.
    #[must_use]
    pub fn lookup(&self, path: &[CompactString]) -> Option<&StdlibEntry> {
        if path.is_empty() {
            return None;
        }
        let mut current = self.globals.get(&path[0])?;
        for segment in &path[1..] {
            match &current.kind {
                EntryKind::Namespace(members) => {
                    current = members.get(segment)?;
                }
                _ => return None,
            }
        }
        Some(current)
    }

    /// Same as [`Self::lookup`] but accepts borrowed `&str` segments -
    /// preferred entry point for callers that have raw source slices.
    #[must_use]
    pub fn lookup_str(&self, path: &[&str]) -> Option<&StdlibEntry> {
        if path.is_empty() {
            return None;
        }
        // BTreeMap::get accepts `Q: Ord + ?Sized` where K: Borrow<Q>.
        // CompactString implements Borrow<str>, so `&str` is a valid key.
        let mut current = self.globals.get(path[0])?;
        for segment in &path[1..] {
            match &current.kind {
                EntryKind::Namespace(members) => {
                    current = members.get(*segment)?;
                }
                _ => return None,
            }
        }
        Some(current)
    }
}

// TOML layout:
//   [globals.print]
//   kind = "function"
//   min_args = 0
//   max_args = -1            # -1 = unbounded
//   params = [{ kind = "vararg", required = false }]
//   must_use = false
//
//   [globals.string]
//   kind = "namespace"
//   [globals.string.members.byte]
//   kind = "function"
//   ...

#[derive(Debug, Deserialize)]
struct RawLibrary {
    #[serde(default)]
    globals: BTreeMap<String, RawEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RawEntry {
    Function(RawFunction),
    Constant(RawValue),
    Property(RawValue),
    Namespace(RawNamespace),
}

#[derive(Debug, Deserialize)]
struct RawFunction {
    #[serde(default)]
    params: Vec<RawParam>,
    min_args: usize,
    /// `-1` (or any negative) encodes "unbounded" - corresponds to
    /// `Option::None` in `StdlibFunction::max_args`. TOML has no native
    /// nullable so this sentinel keeps authoring ergonomic.
    max_args: i32,
    #[serde(default)]
    is_pure: bool,
    #[serde(default)]
    must_use: bool,
    #[serde(default)]
    deprecated: Option<RawDeprecation>,
    #[serde(default = "default_true")]
    read_only: bool,
    #[serde(default)]
    tier: RawLuauTier,
}

#[derive(Debug, Deserialize)]
struct RawParam {
    #[serde(flatten)]
    kind: RawParamKind,
    #[serde(default = "default_true")]
    required: bool,
    #[serde(default)]
    accepts_nil: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RawParamKind {
    Any,
    Bool,
    Number,
    String,
    Function,
    Table,
    Display { display: String },
    Nil,
    Constant { values: Vec<String> },
    Vararg,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum RawLuauTier {
    #[default]
    Core,
    Standalone,
    Roblox,
}

impl RawLuauTier {
    fn resolve(self) -> LuauTier {
        match self {
            RawLuauTier::Core => LuauTier::Core,
            RawLuauTier::Standalone => LuauTier::Standalone,
            RawLuauTier::Roblox => LuauTier::Roblox,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawValue {
    #[serde(default = "default_true")]
    read_only: bool,
    #[serde(default)]
    deprecated: Option<RawDeprecation>,
    #[serde(default)]
    tier: RawLuauTier,
}

#[derive(Debug, Deserialize)]
struct RawDeprecation {
    message: String,
    #[serde(default)]
    replace_template: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawNamespace {
    // Authored in TOML for future use (e.g. allowing `string = {}` in
    // sandboxed configs). Lookups currently treat all namespaces as
    // read-only at the identity level; we accept and discard the value.
    #[serde(default, rename = "read_only")]
    _read_only: bool,
    #[serde(default)]
    members: BTreeMap<String, RawEntry>,
    /// Members inherit this tier unless they declare their own.
    #[serde(default)]
    tier: RawLuauTier,
}

const fn default_true() -> bool {
    true
}

fn convert_param(raw: RawParam) -> StdlibParam {
    let kind = match raw.kind {
        RawParamKind::Any => StdlibArgKind::Any,
        RawParamKind::Bool => StdlibArgKind::Bool,
        RawParamKind::Number => StdlibArgKind::Number,
        RawParamKind::String => StdlibArgKind::String,
        RawParamKind::Function => StdlibArgKind::Function,
        RawParamKind::Table => StdlibArgKind::Table,
        RawParamKind::Display { display } => StdlibArgKind::Display(display.into()),
        RawParamKind::Nil => StdlibArgKind::Nil,
        RawParamKind::Constant { values } => {
            StdlibArgKind::Constant(values.into_iter().map(CompactString::from).collect())
        }
        RawParamKind::Vararg => StdlibArgKind::Vararg,
    };
    StdlibParam {
        kind,
        required: raw.required,
        accepts_nil: raw.accepts_nil,
    }
}

fn convert_deprecation(raw: RawDeprecation) -> StdlibDeprecation {
    StdlibDeprecation {
        message: raw.message.into(),
        replace_template: raw.replace_template.map(CompactString::from),
    }
}

fn convert_entry(raw: RawEntry, version: LuaVersion, parent_tier: LuauTier) -> StdlibEntry {
    let lua_version = LuaVersionSet::single(version);
    // The parent namespace's tier is a hard floor on visibility: a child
    // can only ever narrow, never widen past its parent. A `Core` parent
    // imposes no floor, so the child's own tier wins (it may narrow to
    // `Standalone` or `Roblox`). A non-`Core` parent (`Standalone` or
    // `Roblox`) gates everything beneath it, so every child inherits that
    // tier outright - a child can neither widen back to `Core` nor cross
    // into the other environment.
    let resolve = |own: LuauTier| match (parent_tier, own) {
        (LuauTier::Core, child) => child,
        (parent, _) => parent,
    };
    let (kind, luau_tier) = match raw {
        RawEntry::Function(func) => {
            let max_args = if func.max_args < 0 {
                None
            } else {
                Some(func.max_args as usize)
            };
            let tier = resolve(func.tier.resolve());
            (
                EntryKind::Function(Box::new(StdlibFunction {
                    params: func.params.into_iter().map(convert_param).collect(),
                    min_args: func.min_args,
                    max_args,
                    is_pure: func.is_pure,
                    must_use: func.must_use,
                    deprecated: func.deprecated.map(convert_deprecation),
                    read_only: func.read_only,
                })),
                tier,
            )
        }
        RawEntry::Constant(value) => {
            let tier = resolve(value.tier.resolve());
            (
                EntryKind::Constant(StdlibValue {
                    read_only: value.read_only,
                    deprecated: value.deprecated.map(convert_deprecation),
                }),
                tier,
            )
        }
        RawEntry::Property(value) => {
            let tier = resolve(value.tier.resolve());
            (
                EntryKind::Property(StdlibValue {
                    read_only: value.read_only,
                    deprecated: value.deprecated.map(convert_deprecation),
                }),
                tier,
            )
        }
        RawEntry::Namespace(namespace) => {
            let namespace_tier = resolve(namespace.tier.resolve());
            let mut members = BTreeMap::new();
            for (name, child) in namespace.members {
                members.insert(name.into(), convert_entry(child, version, namespace_tier));
            }
            (EntryKind::Namespace(members), namespace_tier)
        }
    };
    StdlibEntry {
        kind,
        lua_version,
        luau_tier,
    }
}

fn parse_library(toml_src: &str, version: LuaVersion) -> StdlibLibrary {
    // unwrap is acceptable here: these TOML files ship inside the
    // binary via include_str!. A parse failure means a bug in our
    // ship - not user input - so we'd rather fail loudly at first call.
    let raw: RawLibrary = toml::from_str(toml_src)
        .unwrap_or_else(|err| panic!("internal stdlib TOML parse error for {version:?}: {err}"));
    let mut globals = BTreeMap::new();
    for (name, entry) in raw.globals {
        globals.insert(name.into(), convert_entry(entry, version, LuauTier::Core));
    }
    StdlibLibrary { version, globals }
}

const LUA51_TOML: &str = include_str!("../stdlib_data/lua51.toml");
const LUA52_TOML: &str = include_str!("../stdlib_data/lua52.toml");
const LUA53_TOML: &str = include_str!("../stdlib_data/lua53.toml");
const LUA54_TOML: &str = include_str!("../stdlib_data/lua54.toml");
const LUA55_TOML: &str = include_str!("../stdlib_data/lua55.toml");
const LUAU_TOML: &str = include_str!("../stdlib_data/luau.toml");

static LIB_LUA51: LazyLock<StdlibLibrary> =
    LazyLock::new(|| parse_library(LUA51_TOML, LuaVersion::Lua51));
static LIB_LUA52: LazyLock<StdlibLibrary> =
    LazyLock::new(|| parse_library(LUA52_TOML, LuaVersion::Lua52));
static LIB_LUA53: LazyLock<StdlibLibrary> =
    LazyLock::new(|| parse_library(LUA53_TOML, LuaVersion::Lua53));
static LIB_LUA54: LazyLock<StdlibLibrary> =
    LazyLock::new(|| parse_library(LUA54_TOML, LuaVersion::Lua54));
static LIB_LUA55: LazyLock<StdlibLibrary> =
    LazyLock::new(|| parse_library(LUA55_TOML, LuaVersion::Lua55));
static LIB_LUAU: LazyLock<StdlibLibrary> =
    LazyLock::new(|| parse_library(LUAU_TOML, LuaVersion::Luau));

/// Reference to the parsed library for the given version. Cheap: data
/// is parsed once on first access and shared thereafter.
#[must_use]
pub fn library_for(version: LuaVersion) -> &'static StdlibLibrary {
    match version {
        LuaVersion::Lua51 => &LIB_LUA51,
        LuaVersion::Lua52 => &LIB_LUA52,
        LuaVersion::Lua53 => &LIB_LUA53,
        LuaVersion::Lua54 => &LIB_LUA54,
        LuaVersion::Lua55 => &LIB_LUA55,
        LuaVersion::Luau => &LIB_LUAU,
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
    fn version_set_membership() {
        let set = LuaVersionSet::single(LuaVersion::Lua52).with(LuaVersion::Lua53);
        assert!(set.contains(LuaVersion::Lua52));
        assert!(set.contains(LuaVersion::Lua53));
        assert!(!set.contains(LuaVersion::Lua51));
        assert!(!set.contains(LuaVersion::Luau));
    }

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

    #[test]
    fn luau_tier_availability() {
        use StdlibEnvironment::{Roblox, Standalone};
        assert!(entry_with(LuauTier::Core).available_in_luau(Standalone));
        assert!(entry_with(LuauTier::Core).available_in_luau(Roblox));
        assert!(entry_with(LuauTier::Standalone).available_in_luau(Standalone));
        assert!(!entry_with(LuauTier::Standalone).available_in_luau(Roblox));
        assert!(!entry_with(LuauTier::Roblox).available_in_luau(Standalone));
        assert!(entry_with(LuauTier::Roblox).available_in_luau(Roblox));
    }

    fn entry_with(tier: LuauTier) -> StdlibEntry {
        StdlibEntry {
            kind: EntryKind::Property(StdlibValue {
                read_only: true,
                deprecated: None,
            }),
            lua_version: LuaVersionSet::single(LuaVersion::Luau),
            luau_tier: tier,
        }
    }

    #[test]
    fn roblox_entries_only_in_roblox_flavor() {
        let lib = library_for(LuaVersion::Luau);
        for name in [
            "game",
            "workspace",
            "script",
            "warn",
            "task",
            "bit",
            "Vector3",
            "CFrame",
            "Instance",
        ] {
            let entry = lib
                .lookup_str(&[name])
                .unwrap_or_else(|| panic!("missing {name}"));
            assert!(
                !entry.available_in_luau(StdlibEnvironment::Standalone),
                "{name} must be hidden standalone"
            );
            assert!(
                entry.available_in_luau(StdlibEnvironment::Roblox),
                "{name} must be present roblox"
            );
        }
    }

    #[test]
    fn standalone_and_absent_entries() {
        let lib = library_for(LuaVersion::Luau);
        let ls = lib.lookup_str(&["loadstring"]).expect("loadstring present");
        assert!(ls.available_in_luau(StdlibEnvironment::Standalone)); // standalone CLI
        assert!(!ls.available_in_luau(StdlibEnvironment::Roblox)); // hidden on Roblox
        for absent in ["io", "package", "load", "dofile", "loadfile", "arg"] {
            assert!(
                lib.lookup_str(&[absent]).is_none(),
                "{absent} must be absent"
            );
        }
    }

    #[test]
    fn parent_tier_is_a_floor_no_widening() {
        // A Roblox namespace containing a child that tags itself
        // Standalone must not let that child escape the Roblox gate: the
        // parent tier is a floor, so the child resolves to Roblox.
        let namespace = RawEntry::Namespace(RawNamespace {
            _read_only: false,
            members: BTreeMap::from([(
                "child".to_string(),
                RawEntry::Property(RawValue {
                    read_only: true,
                    deprecated: None,
                    tier: RawLuauTier::Standalone,
                }),
            )]),
            tier: RawLuauTier::Roblox,
        });
        let entry = convert_entry(namespace, LuaVersion::Luau, LuauTier::Core);
        assert_eq!(entry.luau_tier, LuauTier::Roblox);
        let EntryKind::Namespace(members) = &entry.kind else {
            panic!("expected namespace");
        };
        let child = members.get("child").expect("child present");
        assert_eq!(
            child.luau_tier,
            LuauTier::Roblox,
            "child must inherit the Roblox floor, not widen to Standalone"
        );
        assert!(
            !child.available_in_luau(StdlibEnvironment::Standalone),
            "child must be hidden standalone"
        );
        assert!(
            child.available_in_luau(StdlibEnvironment::Roblox),
            "child must be visible on roblox"
        );
    }

    #[test]
    fn core_entries_in_both_flavors() {
        let lib = library_for(LuaVersion::Luau);
        for path in [
            &["bit32"][..],
            &["utf8"][..],
            &["buffer"][..],
            &["typeof"][..],
            &["gcinfo"][..],
            &["newproxy"][..],
            &["table", "create"][..],
            &["string", "split"][..],
        ] {
            let entry = lib
                .lookup_str(path)
                .unwrap_or_else(|| panic!("missing {path:?}"));
            assert!(
                entry.available_in_luau(StdlibEnvironment::Standalone)
                    && entry.available_in_luau(StdlibEnvironment::Roblox),
                "{path:?} should be core"
            );
        }
    }
}
