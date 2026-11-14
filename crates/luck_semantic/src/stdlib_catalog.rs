//! The built-in stdlib catalog, one static library per environment.
//!
//! Each submodule is generated from `stdlib_data/` by `just stdlib`, which
//! also checks the data, so nothing is parsed or validated at runtime.

use luck_token::{LuaVersion, StdlibEnvironment};

use crate::stdlib_model::{
    StdlibArgKind, StdlibConstant, StdlibEntry, StdlibFunction, StdlibLibrary, StdlibMap,
    StdlibNamespace, StdlibParam, StdlibSignature, StdlibValue,
};

#[rustfmt::skip]
mod lua51;
#[rustfmt::skip]
mod lua52;
#[rustfmt::skip]
mod lua53;
#[rustfmt::skip]
mod lua54;
#[rustfmt::skip]
mod lua55;
#[rustfmt::skip]
mod luau;
#[rustfmt::skip]
mod luau_roblox;

/// The environment only matters for Luau; each numbered Lua version has
/// one library.
#[must_use]
pub fn library_for(version: LuaVersion, environment: StdlibEnvironment) -> &'static StdlibLibrary {
    match version {
        LuaVersion::Lua51 => &lua51::LIBRARY,
        LuaVersion::Lua52 => &lua52::LIBRARY,
        LuaVersion::Lua53 => &lua53::LIBRARY,
        LuaVersion::Lua54 => &lua54::LIBRARY,
        LuaVersion::Lua55 => &lua55::LIBRARY,
        LuaVersion::Luau => match environment {
            StdlibEnvironment::Standalone => &luau::LIBRARY,
            StdlibEnvironment::Roblox => &luau_roblox::LIBRARY,
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

pub(crate) const NAMESPACE: StdlibNamespace =
    StdlibNamespace { members: StdlibMap::new(&[]), shape: None, deprecated: None };

const ENUM_ITEM: StdlibEntry =
    StdlibEntry::Constant(StdlibValue { shape: Some("EnumItem"), ..VALUE });

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
