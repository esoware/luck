use luck_token::LuaVersion;
use std::fmt;
use std::str::FromStr;

/// Target Lua dialect for bundling and minification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LuaTarget {
    Lua51,
    Lua52,
    Lua53,
    Lua54,
    Lua55,
    Luau,
    /// Roblox-hosted Luau - same parser dialect as `Luau` but distinct target identity.
    LuauRoblox,
}

impl FromStr for LuaTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.to_lowercase();
        match normalized.as_str() {
            "lua51" | "5.1" | "51" => Ok(LuaTarget::Lua51),
            "lua52" | "5.2" | "52" => Ok(LuaTarget::Lua52),
            "lua53" | "5.3" | "53" => Ok(LuaTarget::Lua53),
            "lua54" | "5.4" | "54" => Ok(LuaTarget::Lua54),
            "lua55" | "5.5" | "55" => Ok(LuaTarget::Lua55),
            "luau" => Ok(LuaTarget::Luau),
            "roblox" | "roblox-luau" | "luau-roblox" => Ok(LuaTarget::LuauRoblox),
            _ => Err(format!(
                "invalid target \"{s}\": expected one of Lua51/5.1/51, Lua52/5.2/52, Lua53/5.3/53, Lua54/5.4/54, Lua55/5.5/55, Luau, or Roblox"
            )),
        }
    }
}

impl fmt::Display for LuaTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LuaTarget::Lua51 => write!(f, "Lua51"),
            LuaTarget::Lua52 => write!(f, "Lua52"),
            LuaTarget::Lua53 => write!(f, "Lua53"),
            LuaTarget::Lua54 => write!(f, "Lua54"),
            LuaTarget::Lua55 => write!(f, "Lua55"),
            LuaTarget::Luau => write!(f, "Luau"),
            LuaTarget::LuauRoblox => write!(f, "Roblox"),
        }
    }
}

impl LuaTarget {
    /// Returns the parser's [`LuaVersion`] corresponding to this target.
    pub fn lua_version(self) -> LuaVersion {
        match self {
            LuaTarget::Lua51 => LuaVersion::Lua51,
            LuaTarget::Lua52 => LuaVersion::Lua52,
            LuaTarget::Lua53 => LuaVersion::Lua53,
            LuaTarget::Lua54 => LuaVersion::Lua54,
            LuaTarget::Lua55 => LuaVersion::Lua55,
            LuaTarget::Luau => LuaVersion::Luau,
            LuaTarget::LuauRoblox => LuaVersion::Luau,
        }
    }

    /// Returns `true` if this target is any Luau dialect (standalone or Roblox).
    pub fn is_luau(self) -> bool {
        matches!(self, LuaTarget::Luau | LuaTarget::LuauRoblox)
    }

    /// Returns `true` if this target is the Roblox-hosted Luau dialect.
    pub fn is_roblox(self) -> bool {
        matches!(self, LuaTarget::LuauRoblox)
    }

    /// The stdlib environment this target runs in: `Roblox` for the Roblox
    /// Luau target, `Standalone` for standalone Luau and every vanilla Lua version.
    pub fn stdlib_environment(self) -> luck_token::StdlibEnvironment {
        match self {
            LuaTarget::LuauRoblox => luck_token::StdlibEnvironment::Roblox,
            LuaTarget::Lua51
            | LuaTarget::Lua52
            | LuaTarget::Lua53
            | LuaTarget::Lua54
            | LuaTarget::Lua55
            | LuaTarget::Luau => luck_token::StdlibEnvironment::Standalone,
        }
    }

    /// Returns the set of reserved keywords for this target.
    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            LuaTarget::Lua51 => LUA51_KEYWORDS,
            LuaTarget::Lua52 | LuaTarget::Lua53 | LuaTarget::Lua54 | LuaTarget::Lua55 => {
                LUA52_PLUS_KEYWORDS
            }
            LuaTarget::Luau | LuaTarget::LuauRoblox => LUAU_KEYWORDS,
        }
    }
}

/// Reserved keywords in Lua 5.1.
pub const LUA51_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local",
    "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Reserved keywords in Lua 5.2+.
pub const LUA52_PLUS_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Reserved keywords in Luau.
pub const LUAU_KEYWORDS: &[&str] = &[
    "and", "break", "continue", "do", "else", "elseif", "end", "false", "for", "function", "if",
    "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lua_target_from_str_case_insensitive() {
        assert_eq!("lua54".parse::<LuaTarget>().unwrap(), LuaTarget::Lua54);
        assert_eq!("LUA54".parse::<LuaTarget>().unwrap(), LuaTarget::Lua54);
        assert_eq!("Lua54".parse::<LuaTarget>().unwrap(), LuaTarget::Lua54);
        assert_eq!("luau".parse::<LuaTarget>().unwrap(), LuaTarget::Luau);
        assert_eq!("LUAU".parse::<LuaTarget>().unwrap(), LuaTarget::Luau);
        assert_eq!("5.4".parse::<LuaTarget>().unwrap(), LuaTarget::Lua54);
        assert_eq!("5.1".parse::<LuaTarget>().unwrap(), LuaTarget::Lua51);
        assert!("invalid".parse::<LuaTarget>().is_err());
    }

    #[test]
    fn test_lua_target_short_and_bare_number_aliases() {
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
        assert_eq!(
            "roblox".parse::<LuaTarget>().unwrap(),
            LuaTarget::LuauRoblox
        );
        assert_eq!(
            "Roblox".parse::<LuaTarget>().unwrap(),
            LuaTarget::LuauRoblox
        );
    }

    #[test]
    fn test_is_luau() {
        assert!(!LuaTarget::Lua51.is_luau());
        assert!(!LuaTarget::Lua54.is_luau());
        assert!(LuaTarget::Luau.is_luau());
    }

    #[test]
    fn test_keywords() {
        let kw51 = LuaTarget::Lua51.keywords();
        assert!(!kw51.contains(&"goto"));
        assert!(kw51.contains(&"while"));

        let kw52 = LuaTarget::Lua52.keywords();
        assert!(kw52.contains(&"goto"));

        let kwluau = LuaTarget::Luau.keywords();
        assert!(!kwluau.contains(&"goto"));
        assert!(kwluau.contains(&"continue"));
    }

    #[test]
    fn test_lua_target_display_roundtrip() {
        for target in [
            LuaTarget::Lua51,
            LuaTarget::Lua52,
            LuaTarget::Lua53,
            LuaTarget::Lua54,
            LuaTarget::Lua55,
            LuaTarget::Luau,
        ] {
            let displayed = target.to_string();
            let parsed: LuaTarget = displayed.parse().unwrap();
            assert_eq!(parsed, target);
        }
    }

    #[test]
    fn luau_roblox_parses_and_maps_to_luau_version() {
        assert_eq!(
            "roblox".parse::<LuaTarget>().unwrap(),
            LuaTarget::LuauRoblox
        );
        assert_eq!(
            "roblox-luau".parse::<LuaTarget>().unwrap(),
            LuaTarget::LuauRoblox
        );
        assert_eq!(
            "luau-roblox".parse::<LuaTarget>().unwrap(),
            LuaTarget::LuauRoblox
        );
        assert_eq!(LuaTarget::LuauRoblox.lua_version(), LuaVersion::Luau);
        assert!(LuaTarget::LuauRoblox.is_luau());
        assert!(LuaTarget::LuauRoblox.is_roblox());
        assert!(!LuaTarget::Luau.is_roblox());
        assert!(LuaTarget::LuauRoblox.keywords().contains(&"continue"));
    }

    #[test]
    fn luau_roblox_display_roundtrips() {
        assert_eq!(
            LuaTarget::LuauRoblox
                .to_string()
                .parse::<LuaTarget>()
                .unwrap(),
            LuaTarget::LuauRoblox
        );
    }
}
