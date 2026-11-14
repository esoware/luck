/// How the bundler treats a `require` whose argument isn't a string literal
/// and so can't be resolved at build time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DynamicRequire {
    /// Fail the bundle (E002).
    Error,
    /// Keep the call, resolving it at runtime, and warn (W007).
    Warn,
    /// Keep the call without a warning, for trees where runtime requires are
    /// normal, such as Roblox's `require(script.Parent.Child)`.
    Allow,
}

impl From<DynamicRequire> for luck_bundler::DynamicRequire {
    fn from(mode: DynamicRequire) -> Self {
        match mode {
            DynamicRequire::Error => Self::Error,
            DynamicRequire::Warn => Self::Warn,
            DynamicRequire::Allow => Self::Allow,
        }
    }
}
