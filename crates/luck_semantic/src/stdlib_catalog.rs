//! The built-in stdlib catalog, one static library per environment.
//!
//! Each is an expression `just stdlib` generates from `stdlib_data/`, which
//! also checks the data, so nothing is parsed or validated at runtime. It
//! names what it uses from this module.

use luck_token::{LuaVersion, StdlibEnvironment};

use crate::stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibDeprecation, StdlibEntry, StdlibFunction, StdlibLibrary,
    StdlibNamespace, StdlibParam, StdlibShape, StdlibSignature, StdlibValue, stdlib_map,
};

static LUA51: StdlibLibrary = include!("stdlib_catalog/lua51.rs");
static LUA52: StdlibLibrary = include!("stdlib_catalog/lua52.rs");
static LUA53: StdlibLibrary = include!("stdlib_catalog/lua53.rs");
static LUA54: StdlibLibrary = include!("stdlib_catalog/lua54.rs");
static LUA55: StdlibLibrary = include!("stdlib_catalog/lua55.rs");
static LUAU: StdlibLibrary = include!("stdlib_catalog/luau.rs");
static LUAU_ROBLOX: StdlibLibrary = include!("stdlib_catalog/luau_roblox.rs");

/// The environment only matters for Luau; each numbered Lua version has
/// one library.
#[must_use]
pub fn library_for(version: LuaVersion, environment: StdlibEnvironment) -> &'static StdlibLibrary {
    match version {
        LuaVersion::Lua51 => &LUA51,
        LuaVersion::Lua52 => &LUA52,
        LuaVersion::Lua53 => &LUA53,
        LuaVersion::Lua54 => &LUA54,
        LuaVersion::Lua55 => &LUA55,
        LuaVersion::Luau => match environment {
            StdlibEnvironment::Standalone => &LUAU,
            StdlibEnvironment::Roblox => &LUAU_ROBLOX,
        },
    }
}

pub(crate) const FUNCTION: StdlibFunction = StdlibFunction {
    signatures: &[],
    is_pure: false,
    is_must_use: false,
    deprecated: None,
    is_read_only: true,
    is_method: false,
    returns_shape: None,
};

pub(crate) const VALUE: StdlibValue =
    StdlibValue { is_read_only: true, deprecated: None, shape: None };

const ENUM_ITEM: StdlibEntry =
    StdlibEntry::Constant(&StdlibValue { shape: Some("EnumItem"), ..VALUE });

pub(crate) const fn required(kind: StdlibArgKind) -> StdlibParam {
    StdlibParam { kind, is_required: true, deprecated: None }
}

pub(crate) const fn optional(kind: StdlibArgKind) -> StdlibParam {
    StdlibParam { kind, is_required: false, deprecated: None }
}

pub(crate) const fn signature(
    params: &'static [StdlibParam],
    min_args: usize,
    max_args: Option<usize>,
) -> StdlibSignature {
    StdlibSignature { params, min_args, max_args }
}

const fn constant(value: &'static str) -> StdlibConstant {
    StdlibConstant { value, deprecated: None, is_common: false }
}
