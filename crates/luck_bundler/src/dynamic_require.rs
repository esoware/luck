use core::str::FromStr;

/// How the bundler treats a `require` whose argument isn't a string literal
/// and so can't be resolved at build time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DynamicRequire {
    /// Fail the bundle (E002).
    Error,
    /// Keep the call, resolving it at runtime, and warn (W007).
    #[default]
    Warn,
    /// Keep the call without a warning, for trees where runtime requires are
    /// normal, such as Roblox's `require(script.Parent.Child)`.
    Allow,
}

impl FromStr for DynamicRequire {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "allow" => Ok(Self::Allow),
            _ => Err(format!(
                "invalid dynamic require mode `{s}`: expected `error`, `warn`, or `allow`"
            )),
        }
    }
}
