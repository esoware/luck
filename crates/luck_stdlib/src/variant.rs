use std::fmt;
use std::str::FromStr;

/// The Luau a user names: standalone, or on Roblox, which decides the stdlib
/// a name resolves against. Both read the same syntax.
// `luck.json` spells it only as its lowercase name, as it spells every other
// enum, and `FromStr` takes those names in any case.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(
    feature = "schemars",
    derive(schemars::JsonSchema),
    schemars(description = "The Luau variant: standalone Luau, or Luau on Roblox.")
)]
pub enum Variant {
    #[default]
    Standalone,
    /// Luau on Roblox, with the Roblox stdlib.
    Roblox,
}

impl Variant {
    pub const ALL: [Self; 2] = [Self::Standalone, Self::Roblox];

    pub const fn is_roblox(self) -> bool {
        matches!(self, Self::Roblox)
    }
}

impl FromStr for Variant {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("standalone") {
            Ok(Self::Standalone)
        } else if s.eq_ignore_ascii_case("roblox") {
            Ok(Self::Roblox)
        } else {
            Err(format!("invalid variant `{s}`: expected `standalone` or `roblox`"))
        }
    }
}

impl fmt::Display for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Standalone => "Standalone",
            Self::Roblox => "Roblox",
        })
    }
}
