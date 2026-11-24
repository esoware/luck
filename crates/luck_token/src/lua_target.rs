use std::fmt;
use std::str::FromStr;

use crate::LuaVersion;

/// The dialect a user names: a [`LuaVersion`], and for Luau, whether it runs
/// on Roblox.
///
/// These are the only valid pairs of a syntax and a host, so whatever needs
/// both, such as the stdlib a name resolves against, takes one of these.
/// Whatever reads only the syntax takes its [`Self::version`].
// `luck.json` spells it only as its lowercase name, as it spells every other
// enum, and `FromStr` takes the shorter forms a command line wants too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize), serde(rename_all = "lowercase"))]
#[cfg_attr(
    feature = "schemars",
    derive(schemars::JsonSchema),
    schemars(description = "A Lua dialect: Lua 5.1 to 5.5, Luau, or Luau on Roblox.")
)]
pub enum LuaTarget {
    Lua51,
    Lua52,
    Lua53,
    Lua54,
    Lua55,
    Luau,
    /// Luau on Roblox: the same syntax as `Luau`, with the Roblox stdlib.
    #[cfg_attr(feature = "serde", serde(rename = "roblox"))]
    LuauRoblox,
}

impl LuaTarget {
    /// Every dialect, in release order, each version's standalone one first.
    pub const ALL: [Self; 7] = [
        Self::Lua51,
        Self::Lua52,
        Self::Lua53,
        Self::Lua54,
        Self::Lua55,
        Self::Luau,
        Self::LuauRoblox,
    ];

    pub const fn version(self) -> LuaVersion {
        match self {
            Self::Lua51 => LuaVersion::Lua51,
            Self::Lua52 => LuaVersion::Lua52,
            Self::Lua53 => LuaVersion::Lua53,
            Self::Lua54 => LuaVersion::Lua54,
            Self::Lua55 => LuaVersion::Lua55,
            Self::Luau | Self::LuauRoblox => LuaVersion::Luau,
        }
    }

    pub const fn is_roblox(self) -> bool {
        matches!(self, Self::LuauRoblox)
    }
}

/// Each version runs standalone, outside any host's own stdlib.
impl From<LuaVersion> for LuaTarget {
    fn from(version: LuaVersion) -> Self {
        match version {
            LuaVersion::Lua51 => Self::Lua51,
            LuaVersion::Lua52 => Self::Lua52,
            LuaVersion::Lua53 => Self::Lua53,
            LuaVersion::Lua54 => Self::Lua54,
            LuaVersion::Lua55 => Self::Lua55,
            LuaVersion::Luau => Self::Luau,
        }
    }
}

impl FromStr for LuaTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.to_lowercase();
        match normalized.as_str() {
            "lua51" | "5.1" | "51" => Ok(Self::Lua51),
            "lua52" | "5.2" | "52" => Ok(Self::Lua52),
            "lua53" | "5.3" | "53" => Ok(Self::Lua53),
            "lua54" | "5.4" | "54" => Ok(Self::Lua54),
            "lua55" | "5.5" | "55" => Ok(Self::Lua55),
            "luau" => Ok(Self::Luau),
            "roblox" | "roblox-luau" | "luau-roblox" => Ok(Self::LuauRoblox),
            _ => Err(format!(
                "invalid target `{s}`: expected one of `Lua51`/`5.1`/`51`, `Lua52`/`5.2`/`52`, `Lua53`/`5.3`/`53`, `Lua54`/`5.4`/`54`, `Lua55`/`5.5`/`55`, `Luau`, or `Roblox`"
            )),
        }
    }
}

impl fmt::Display for LuaTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Lua51 => "Lua51",
            Self::Lua52 => "Lua52",
            Self::Lua53 => "Lua53",
            Self::Lua54 => "Lua54",
            Self::Lua55 => "Lua55",
            Self::Luau => "Luau",
            Self::LuauRoblox => "Roblox",
        })
    }
}
