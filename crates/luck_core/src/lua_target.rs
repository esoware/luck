use core::fmt;
use core::str::FromStr;

use luck_token::{LuaVersion, StdlibEnvironment};

/// The user-facing Lua dialect.
///
/// [`Self::lua_version`] and [`Self::stdlib_environment`] project it onto the
/// syntax and stdlib axes the rest of the workspace keys off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LuaTarget {
    Lua51,
    Lua52,
    Lua53,
    Lua54,
    Lua55,
    Luau,
    /// Luau on Roblox: the same syntax as `Luau`, with the Roblox stdlib.
    LuauRoblox,
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

impl LuaTarget {
    #[must_use]
    pub const fn lua_version(self) -> LuaVersion {
        match self {
            Self::Lua51 => LuaVersion::Lua51,
            Self::Lua52 => LuaVersion::Lua52,
            Self::Lua53 => LuaVersion::Lua53,
            Self::Lua54 => LuaVersion::Lua54,
            Self::Lua55 => LuaVersion::Lua55,
            Self::Luau | Self::LuauRoblox => LuaVersion::Luau,
        }
    }

    #[must_use]
    pub const fn stdlib_environment(self) -> StdlibEnvironment {
        match self {
            Self::LuauRoblox => StdlibEnvironment::Roblox,
            Self::Lua51 | Self::Lua52 | Self::Lua53 | Self::Lua54 | Self::Lua55 | Self::Luau => {
                StdlibEnvironment::Standalone
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every target with every name it parses from, in lowercase.
    const ALIASES: [(LuaTarget, &[&str]); 7] = [
        (LuaTarget::Lua51, &["lua51", "5.1", "51"]),
        (LuaTarget::Lua52, &["lua52", "5.2", "52"]),
        (LuaTarget::Lua53, &["lua53", "5.3", "53"]),
        (LuaTarget::Lua54, &["lua54", "5.4", "54"]),
        (LuaTarget::Lua55, &["lua55", "5.5", "55"]),
        (LuaTarget::Luau, &["luau"]),
        (LuaTarget::LuauRoblox, &["roblox", "roblox-luau", "luau-roblox"]),
    ];

    #[test]
    fn parses_every_alias_in_lowercase_and_uppercase() {
        for (target, aliases) in ALIASES {
            for alias in aliases {
                assert_eq!(alias.parse::<LuaTarget>(), Ok(target), "{alias}");
                let uppercase = alias.to_uppercase();
                assert_eq!(uppercase.parse::<LuaTarget>(), Ok(target), "{uppercase}");
            }
        }
    }

    #[test]
    fn rejects_an_unknown_name() {
        let error = "invalid".parse::<LuaTarget>().unwrap_err();
        assert!(error.starts_with("invalid target `invalid`"), "{error}");
    }

    #[test]
    fn display_round_trips_through_from_str() {
        for (target, _) in ALIASES {
            let displayed = target.to_string();
            assert_eq!(displayed.parse::<LuaTarget>(), Ok(target), "{displayed}");
        }
    }

    #[test]
    fn luau_roblox_projects_onto_luau_syntax_and_roblox_stdlib() {
        assert_eq!(LuaTarget::LuauRoblox.lua_version(), LuaVersion::Luau);
        assert_eq!(LuaTarget::LuauRoblox.stdlib_environment(), StdlibEnvironment::Roblox);
        assert_eq!(LuaTarget::Luau.stdlib_environment(), StdlibEnvironment::Standalone);
    }
}
