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
        match self {
            Self::Lua51 => write!(f, "Lua51"),
            Self::Lua52 => write!(f, "Lua52"),
            Self::Lua53 => write!(f, "Lua53"),
            Self::Lua54 => write!(f, "Lua54"),
            Self::Lua55 => write!(f, "Lua55"),
            Self::Luau => write!(f, "Luau"),
            Self::LuauRoblox => write!(f, "Roblox"),
        }
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

    #[test]
    fn lua_target_from_str_case_insensitive() {
        assert_eq!("lua54".parse::<LuaTarget>().unwrap(), LuaTarget::Lua54);
        assert_eq!("LUA54".parse::<LuaTarget>().unwrap(), LuaTarget::Lua54);
        assert_eq!("Lua54".parse::<LuaTarget>().unwrap(), LuaTarget::Lua54);
        assert_eq!("luau".parse::<LuaTarget>().unwrap(), LuaTarget::Luau);
        assert_eq!("LUAU".parse::<LuaTarget>().unwrap(), LuaTarget::Luau);
        assert_eq!("5.4".parse::<LuaTarget>().unwrap(), LuaTarget::Lua54);
        assert_eq!("5.1".parse::<LuaTarget>().unwrap(), LuaTarget::Lua51);
        "invalid".parse::<LuaTarget>().unwrap_err();
    }

    #[test]
    fn lua_target_short_and_bare_number_aliases() {
        for alias in ["54", "5.4", "Lua54", "LUA54", "lua54"] {
            assert_eq!(
                alias.parse::<LuaTarget>().unwrap(),
                LuaTarget::Lua54,
                "alias {alias} should parse to Lua54"
            );
        }
        assert_eq!("51".parse::<LuaTarget>().unwrap(), LuaTarget::Lua51);
        assert_eq!("52".parse::<LuaTarget>().unwrap(), LuaTarget::Lua52);
        assert_eq!("53".parse::<LuaTarget>().unwrap(), LuaTarget::Lua53);
        assert_eq!("55".parse::<LuaTarget>().unwrap(), LuaTarget::Lua55);
        assert_eq!("roblox".parse::<LuaTarget>().unwrap(), LuaTarget::LuauRoblox);
        assert_eq!("Roblox".parse::<LuaTarget>().unwrap(), LuaTarget::LuauRoblox);
    }

    #[test]
    fn lua_target_display_roundtrip() {
        for target in [
            LuaTarget::Lua51,
            LuaTarget::Lua52,
            LuaTarget::Lua53,
            LuaTarget::Lua54,
            LuaTarget::Lua55,
            LuaTarget::Luau,
            LuaTarget::LuauRoblox,
        ] {
            let displayed = target.to_string();
            let parsed: LuaTarget = displayed.parse().unwrap();
            assert_eq!(parsed, target);
        }
    }

    #[test]
    fn luau_roblox_parses_and_projects_onto_luau_syntax_and_roblox_stdlib() {
        assert_eq!("roblox".parse::<LuaTarget>().unwrap(), LuaTarget::LuauRoblox);
        assert_eq!("roblox-luau".parse::<LuaTarget>().unwrap(), LuaTarget::LuauRoblox);
        assert_eq!("luau-roblox".parse::<LuaTarget>().unwrap(), LuaTarget::LuauRoblox);
        assert_eq!(LuaTarget::LuauRoblox.lua_version(), LuaVersion::Luau);
        assert_eq!(LuaTarget::LuauRoblox.stdlib_environment(), StdlibEnvironment::Roblox);
        assert_eq!(LuaTarget::Luau.stdlib_environment(), StdlibEnvironment::Standalone);
    }
}
