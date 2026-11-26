/// The security level Roblox runs a script at, as its API dump names it:
/// `None` for a `Script` or a `LocalScript`, `LocalUserSecurity`,
/// `PluginSecurity` for a plugin or Studio's command bar, or
/// `RobloxScriptSecurity` for a `CoreScript`. A level reaches the members of
/// Roblox's API that the levels before it reach, and more.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize)]
#[cfg_attr(
    feature = "schemars",
    derive(schemars::JsonSchema),
    schemars(description = "The security level Roblox runs a script at.")
)]
pub enum SecurityLevel {
    #[default]
    None,
    LocalUserSecurity,
    PluginSecurity,
    RobloxScriptSecurity,
}

impl SecurityLevel {
    pub const ALL: [Self; 4] =
        [Self::None, Self::LocalUserSecurity, Self::PluginSecurity, Self::RobloxScriptSecurity];

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "None" => Some(Self::None),
            "LocalUserSecurity" => Some(Self::LocalUserSecurity),
            "PluginSecurity" => Some(Self::PluginSecurity),
            "RobloxScriptSecurity" => Some(Self::RobloxScriptSecurity),
            _ => None,
        }
    }
}
